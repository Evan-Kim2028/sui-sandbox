//! # Type Utilities
//!
//! This module provides canonical type handling utilities for the benchmark system.
//! All type string parsing and formatting should go through these functions to ensure
//! consistency across the codebase.
//!
//! ## Key Functions
//!
//! - [`format_type_tag`] - Convert a TypeTag to its canonical string representation
//! - [`parse_type_string`] - Parse a type string into a TypeTag (returns `Option`)
//! - [`parse_type_tag`] - Parse a type string into a TypeTag (returns `Result`, with caching)
//! - [`parse_type_args`] - Parse comma-separated type arguments
//! - [`normalize_address`] - Normalize an address to canonical form (0x-prefixed, lowercase)
//!
//! ## Caching
//!
//! The [`parse_type_tag`] function uses a thread-local cache to avoid re-parsing
//! the same type strings repeatedly. This provides significant speedup for batch
//! operations and PTB execution where the same types are referenced many times.
//!
//! Cache management functions:
//! - [`clear_type_cache`] - Clear the cache (useful for testing or memory pressure)
//! - [`type_cache_size`] - Get the current cache size
//! - [`type_cache_stats`] - Get cache hit/miss statistics

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use std::cell::RefCell;
use std::collections::HashMap;

// =============================================================================
// Type Parsing Cache
// =============================================================================

// Thread-local cache for parsed TypeTags.
// This significantly speeds up repeated parsing of the same type strings,
// which is common during PTB execution and transaction replay.
thread_local! {
    static TYPE_TAG_CACHE: RefCell<TypeCache> = RefCell::new(TypeCache::new());
}

/// Maximum cache size before clearing (to prevent unbounded memory growth)
const TYPE_TAG_CACHE_MAX_SIZE: usize = 2048;

/// Cache statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct TypeCacheStats {
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Current cache size
    pub size: usize,
}

impl TypeCacheStats {
    /// Get the cache hit rate (0.0 to 1.0)
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Internal cache structure
struct TypeCache {
    cache: HashMap<String, TypeTag>,
    hits: u64,
    misses: u64,
}

impl TypeCache {
    fn new() -> Self {
        Self {
            cache: HashMap::with_capacity(256),
            hits: 0,
            misses: 0,
        }
    }

    fn get(&mut self, key: &str) -> Option<TypeTag> {
        if let Some(tag) = self.cache.get(key) {
            self.hits += 1;
            Some(tag.clone())
        } else {
            self.misses += 1;
            None
        }
    }

    fn insert(&mut self, key: String, value: TypeTag) {
        // Clear cache if it's too large
        if self.cache.len() >= TYPE_TAG_CACHE_MAX_SIZE {
            self.cache.clear();
        }
        self.cache.insert(key, value);
    }

    fn clear(&mut self) {
        self.cache.clear();
        self.hits = 0;
        self.misses = 0;
    }

    fn stats(&self) -> TypeCacheStats {
        TypeCacheStats {
            hits: self.hits,
            misses: self.misses,
            size: self.cache.len(),
        }
    }
}

/// Clear the type tag cache.
///
/// Useful for testing or when memory pressure is high.
pub fn clear_type_cache() {
    TYPE_TAG_CACHE.with(|cache| cache.borrow_mut().clear());
}

/// Get the current cache size.
pub fn type_cache_size() -> usize {
    TYPE_TAG_CACHE.with(|cache| cache.borrow().cache.len())
}

/// Get cache statistics for monitoring.
pub fn type_cache_stats() -> TypeCacheStats {
    TYPE_TAG_CACHE.with(|cache| cache.borrow().stats())
}

// =============================================================================
// Type Formatting
// =============================================================================

/// Convert a TypeTag to its canonical string representation.
///
/// This is the canonical way to format TypeTags throughout the codebase.
/// The output format matches Sui's conventions:
/// - Primitives: `bool`, `u8`, `u64`, `address`, etc.
/// - Vectors: `vector<inner_type>`
/// - Structs: `0xADDR::module::Name` or `0xADDR::module::Name<T1, T2>`
///
/// # Examples
/// ```
/// use sui_sandbox_core::types::{format_type_tag, coin_sui_type};
/// use move_core_types::language_storage::TypeTag;
///
/// let tag = TypeTag::U64;
/// assert_eq!(format_type_tag(&tag), "u64");
///
/// // Struct with type parameters
/// let coin_tag = coin_sui_type();
/// assert_eq!(format_type_tag(&coin_tag), "0x2::coin::Coin<0x2::sui::SUI>");
/// ```
pub fn format_type_tag(type_tag: &TypeTag) -> String {
    match type_tag {
        TypeTag::Bool => "bool".to_string(),
        TypeTag::U8 => "u8".to_string(),
        TypeTag::U16 => "u16".to_string(),
        TypeTag::U32 => "u32".to_string(),
        TypeTag::U64 => "u64".to_string(),
        TypeTag::U128 => "u128".to_string(),
        TypeTag::U256 => "u256".to_string(),
        TypeTag::Address => "address".to_string(),
        TypeTag::Signer => "signer".to_string(),
        TypeTag::Vector(inner) => format!("vector<{}>", format_type_tag(inner)),
        TypeTag::Struct(s) => format_struct_tag(s),
    }
}

/// Format a StructTag to its canonical string representation.
pub fn format_struct_tag(s: &StructTag) -> String {
    let mut result = format!("{}::{}::{}", s.address.to_hex_literal(), s.module, s.name);
    if !s.type_params.is_empty() {
        let params: Vec<String> = s.type_params.iter().map(format_type_tag).collect();
        result.push_str(&format!("<{}>", params.join(", ")));
    }
    result
}

// =============================================================================
// Well-Known Type Strings
// =============================================================================
// Canonical type strings for commonly used Sui types.
// All code should reference these constants rather than hardcoding type strings.

/// The canonical SUI coin type string.
pub const SUI_TYPE_STR: &str = "0x2::sui::SUI";

/// The canonical Coin<SUI> type string.
pub const COIN_SUI_TYPE_STR: &str = "0x2::coin::Coin<0x2::sui::SUI>";

/// The Clock type string.
pub const CLOCK_TYPE_STR: &str = "0x2::clock::Clock";

/// The Random type string.
pub const RANDOM_TYPE_STR: &str = "0x2::random::Random";

// =============================================================================
// Well-Known Object IDs
// =============================================================================
// System object IDs that are constant across all Sui networks.

/// Clock object ID (0x6) - the shared Clock object for timestamp access.
pub const CLOCK_OBJECT_ID: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000006";

/// Random object ID (0x8) - the shared Random object for on-chain randomness.
pub const RANDOM_OBJECT_ID: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000008";

// =============================================================================
// Common Type Constructors
// =============================================================================

/// Create a TypeTag for the SUI type (0x2::sui::SUI).
///
/// This is a commonly used type throughout the codebase. Using this function
/// ensures consistency and avoids duplicate StructTag construction.
///
/// # Example
/// ```
/// use sui_sandbox_core::types::{sui_type, format_type_tag};
///
/// let sui = sui_type();
/// assert_eq!(format_type_tag(&sui), "0x2::sui::SUI");
/// ```
pub fn sui_type() -> TypeTag {
    TypeTag::Struct(Box::new(StructTag {
        address: AccountAddress::from_hex_literal("0x2").unwrap(),
        module: Identifier::new("sui").unwrap(),
        name: Identifier::new("SUI").unwrap(),
        type_params: vec![],
    }))
}

/// Create a TypeTag for Coin<T> with a given inner type.
///
/// # Arguments
/// * `inner` - The inner type for the Coin (e.g., SUI type for Coin<SUI>)
///
/// # Example
/// ```
/// use sui_sandbox_core::types::{coin_type, sui_type, format_type_tag};
///
/// let coin_sui = coin_type(sui_type());
/// assert_eq!(format_type_tag(&coin_sui), "0x2::coin::Coin<0x2::sui::SUI>");
/// ```
pub fn coin_type(inner: TypeTag) -> TypeTag {
    TypeTag::Struct(Box::new(StructTag {
        address: AccountAddress::from_hex_literal("0x2").unwrap(),
        module: Identifier::new("coin").unwrap(),
        name: Identifier::new("Coin").unwrap(),
        type_params: vec![inner],
    }))
}

/// Create a TypeTag for Coin<SUI> (0x2::coin::Coin<0x2::sui::SUI>).
///
/// This is a convenience function that combines `coin_type` and `sui_type`.
/// It's the most common coin type used throughout the codebase.
///
/// # Example
/// ```
/// use sui_sandbox_core::types::{coin_sui_type, format_type_tag};
///
/// let coin_sui = coin_sui_type();
/// assert_eq!(format_type_tag(&coin_sui), "0x2::coin::Coin<0x2::sui::SUI>");
/// ```
pub fn coin_sui_type() -> TypeTag {
    coin_type(sui_type())
}

/// Create a StructTag for the SUI type (0x2::sui::SUI).
///
/// Use this when you need a StructTag directly rather than a TypeTag.
pub fn sui_struct_tag() -> StructTag {
    StructTag {
        address: AccountAddress::from_hex_literal("0x2").unwrap(),
        module: Identifier::new("sui").unwrap(),
        name: Identifier::new("SUI").unwrap(),
        type_params: vec![],
    }
}

/// Create a StructTag for Coin<T> with a given inner type.
///
/// # Arguments
/// * `inner` - The inner type for the Coin as a TypeTag
pub fn coin_struct_tag(inner: TypeTag) -> StructTag {
    StructTag {
        address: AccountAddress::from_hex_literal("0x2").unwrap(),
        module: Identifier::new("coin").unwrap(),
        name: Identifier::new("Coin").unwrap(),
        type_params: vec![inner],
    }
}

// =============================================================================
// Type Parsing
// =============================================================================

/// Parse a type string into a TypeTag.
///
/// Supports:
/// - Primitives: `bool`, `u8`, `u16`, `u32`, `u64`, `u128`, `u256`, `address`, `signer`
/// - Vectors: `vector<inner_type>`
/// - Structs: `0xADDR::module::Name` or `0xADDR::module::Name<T1, T2>`
///
/// # Examples
/// ```
/// use sui_sandbox_core::types::{parse_type_string, coin_sui_type};
/// use move_core_types::language_storage::TypeTag;
///
/// let tag = parse_type_string("u64").unwrap();
/// assert_eq!(tag, TypeTag::U64);
///
/// let coin = parse_type_string("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
/// assert_eq!(coin, coin_sui_type());
/// ```
///
/// # Returns
/// - `Some(TypeTag)` if parsing succeeds
/// - `None` if the type string is invalid
pub fn parse_type_string(type_str: &str) -> Option<TypeTag> {
    let trimmed = type_str.trim();

    // Handle primitives
    match trimmed {
        "bool" => return Some(TypeTag::Bool),
        "u8" => return Some(TypeTag::U8),
        "u16" => return Some(TypeTag::U16),
        "u32" => return Some(TypeTag::U32),
        "u64" => return Some(TypeTag::U64),
        "u128" => return Some(TypeTag::U128),
        "u256" => return Some(TypeTag::U256),
        "address" => return Some(TypeTag::Address),
        "signer" => return Some(TypeTag::Signer),
        _ => {}
    }

    // Handle vector types
    if trimmed.starts_with("vector<") && trimmed.ends_with('>') {
        let inner = &trimmed[7..trimmed.len() - 1];
        return parse_type_string(inner).map(|t| TypeTag::Vector(Box::new(t)));
    }

    // Handle struct types: 0xADDR::module::Name or 0xADDR::module::Name<T1, T2>
    parse_struct_type_string(trimmed)
}

/// Parse a struct type string into a TypeTag.
///
/// Handles both simple structs (`0x2::sui::SUI`) and generic structs
/// (`0x2::coin::Coin<0x2::sui::SUI>`).
fn parse_struct_type_string(type_str: &str) -> Option<TypeTag> {
    let (base, type_args_str) = if let Some(angle_pos) = type_str.find('<') {
        if !type_str.ends_with('>') {
            return None;
        }
        let base = &type_str[..angle_pos];
        let args = &type_str[angle_pos + 1..type_str.len() - 1];
        (base, Some(args))
    } else {
        (type_str, None)
    };

    let parts: Vec<&str> = base.split("::").collect();
    if parts.len() != 3 {
        return None;
    }

    let address = AccountAddress::from_hex_literal(parts[0]).ok()?;
    let module = Identifier::new(parts[1]).ok()?;
    let name = Identifier::new(parts[2]).ok()?;

    let type_params = if let Some(args_str) = type_args_str {
        parse_type_args(args_str)
    } else {
        vec![]
    };

    Some(TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    })))
}

/// Parse comma-separated type arguments, handling nested generics.
///
/// This correctly handles nested angle brackets, e.g.:
/// - `"u64, bool"` -> `[TypeTag::U64, TypeTag::Bool]`
/// - `"0x2::coin::Coin<0x2::sui::SUI>, u64"` -> `[Coin<SUI>, U64]`
///
/// # Returns
/// A vector of parsed TypeTags. Invalid entries are skipped.
pub fn parse_type_args(args_str: &str) -> Vec<TypeTag> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for ch in args_str.chars() {
        match ch {
            '<' => {
                depth += 1;
                current.push(ch);
            }
            '>' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if let Some(tag) = parse_type_string(current.trim()) {
                    args.push(tag);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Don't forget the last argument
    if !current.is_empty() {
        if let Some(tag) = parse_type_string(current.trim()) {
            args.push(tag);
        }
    }

    args
}

// =============================================================================
// Cached Type Parsing (Result-returning)
// =============================================================================

/// Parse a type string into a TypeTag with caching and error reporting.
///
/// This is the **preferred function** for parsing type strings in production code.
/// It uses a thread-local cache to avoid re-parsing the same type strings,
/// providing significant speedup for batch operations.
///
/// Unlike [`parse_type_string`] which returns `Option`, this function returns
/// `Result` with detailed error messages for invalid input.
///
/// # Performance
///
/// - Primitives (u8, u64, bool, etc.) are handled immediately without cache lookup
/// - Complex types (structs, vectors, generics) are cached after first parse
/// - Cache auto-clears at 2048 entries to prevent unbounded memory growth
///
/// # Examples
///
/// ```
/// use sui_sandbox_core::types::{parse_type_tag, coin_sui_type};
/// use move_core_types::language_storage::TypeTag;
///
/// let tag = parse_type_tag("u64").unwrap();
/// assert_eq!(tag, TypeTag::U64);
///
/// let coin = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
/// // Second call hits cache:
/// let coin2 = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
/// assert_eq!(coin, coin2);
/// ```
///
/// # Errors
///
/// Returns an error if:
/// - The type string is malformed (e.g., mismatched angle brackets)
/// - The address is invalid (not valid hex)
/// - The module or struct name is invalid
pub fn parse_type_tag(s: &str) -> Result<TypeTag> {
    let s = s.trim();

    // Fast path for primitives - no caching needed
    match s {
        "u8" => return Ok(TypeTag::U8),
        "u16" => return Ok(TypeTag::U16),
        "u32" => return Ok(TypeTag::U32),
        "u64" => return Ok(TypeTag::U64),
        "u128" => return Ok(TypeTag::U128),
        "u256" => return Ok(TypeTag::U256),
        "bool" => return Ok(TypeTag::Bool),
        "address" => return Ok(TypeTag::Address),
        "signer" => return Ok(TypeTag::Signer),
        _ => {}
    }

    // Check cache for complex types
    let cached = TYPE_TAG_CACHE.with(|cache| cache.borrow_mut().get(s));
    if let Some(type_tag) = cached {
        return Ok(type_tag);
    }

    // Parse the type
    let type_tag = parse_type_tag_uncached(s)?;

    // Cache the result
    TYPE_TAG_CACHE.with(|cache| {
        cache.borrow_mut().insert(s.to_string(), type_tag.clone());
    });

    Ok(type_tag)
}

/// Internal uncached implementation of type tag parsing with error reporting.
fn parse_type_tag_uncached(s: &str) -> Result<TypeTag> {
    // Handle vector<T>
    if s.starts_with("vector<") && s.ends_with('>') {
        let inner = &s[7..s.len() - 1];
        let inner_type = parse_type_tag(inner)?;
        return Ok(TypeTag::Vector(Box::new(inner_type)));
    }

    // Handle struct types: address::module::name or address::module::name<type_args>
    let (base, type_args_str) = if let Some(idx) = s.find('<') {
        if !s.ends_with('>') {
            return Err(anyhow!("Malformed type string (unmatched '<'): {}", s));
        }
        (&s[..idx], Some(&s[idx + 1..s.len() - 1]))
    } else {
        (s, None)
    };

    // Parse base: address::module::name
    let parts: Vec<&str> = base.split("::").collect();
    if parts.len() != 3 {
        return Err(anyhow!(
            "Invalid type format '{}': expected ADDRESS::MODULE::NAME",
            s
        ));
    }

    let address = AccountAddress::from_hex_literal(parts[0])
        .map_err(|e| anyhow!("Invalid address '{}': {}", parts[0], e))?;
    let module = Identifier::new(parts[1].to_string())
        .map_err(|e| anyhow!("Invalid module name '{}': {:?}", parts[1], e))?;
    let name = Identifier::new(parts[2].to_string())
        .map_err(|e| anyhow!("Invalid struct name '{}': {:?}", parts[2], e))?;

    // Parse type arguments if present
    let type_params = if let Some(args_str) = type_args_str {
        parse_type_args_result(args_str)?
    } else {
        vec![]
    };

    Ok(TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    })))
}

/// Parse comma-separated type arguments with error reporting.
///
/// Like [`parse_type_args`] but returns `Result` instead of silently skipping invalid entries.
pub fn parse_type_args_result(args_str: &str) -> Result<Vec<TypeTag>> {
    let trimmed = args_str.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }

    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in trimmed.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                let arg = trimmed[start..i].trim();
                if !arg.is_empty() {
                    result.push(parse_type_tag(arg)?);
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    // Don't forget the last argument
    let last = trimmed[start..].trim();
    if !last.is_empty() {
        result.push(parse_type_tag(last)?);
    }

    Ok(result)
}

// =============================================================================
// Address Normalization
// =============================================================================

/// Normalize an address string to canonical form.
///
/// Canonical form is:
/// - Lowercase hex
/// - 0x-prefixed
/// - Format depends on `move-core-types` implementation (may be short or long form)
///
/// NOTE: For the full 64-character normalized form, use
/// [`sui_resolver::normalize_address`] instead.
///
/// # Examples
/// ```
/// use sui_sandbox_core::types::normalize_address;
///
/// let normalized = normalize_address("0x2").unwrap();
/// assert!(normalized.starts_with("0x"));
///
/// // Invalid addresses return None
/// assert!(normalize_address("not_an_address").is_none());
/// ```
pub fn normalize_address(addr: &str) -> Option<String> {
    // Use sui_resolver's checked variant for validation
    sui_resolver::normalize_address_checked(addr).map(|_| {
        // But return move-core-types short form for backward compatibility
        let trimmed = addr.trim();
        let addr = AccountAddress::from_hex_literal(trimmed).ok().unwrap();
        addr.to_hex_literal()
    })
}

/// Normalize an address to short form (no leading zeros except for special addresses).
///
/// Special addresses (0x0, 0x1, 0x2, 0x3) keep their short form.
/// Other addresses are trimmed of leading zeros but keep at least one digit.
///
/// NOTE: For consistent short-form normalization, prefer
/// [`sui_resolver::normalize_address_short`] which doesn't validate.
pub fn normalize_address_short(addr: &str) -> Option<String> {
    // Use sui_resolver's checked variant for validation
    sui_resolver::normalize_address_checked(addr)?;
    Some(sui_resolver::normalize_address_short(addr))
}

// =============================================================================
// Type Comparison
// =============================================================================

/// Check if two TypeTags are structurally equal, ignoring address normalization differences.
///
/// This compares types semantically - addresses are normalized before comparison.
pub fn type_tags_equal(a: &TypeTag, b: &TypeTag) -> bool {
    match (a, b) {
        (TypeTag::Bool, TypeTag::Bool) => true,
        (TypeTag::U8, TypeTag::U8) => true,
        (TypeTag::U16, TypeTag::U16) => true,
        (TypeTag::U32, TypeTag::U32) => true,
        (TypeTag::U64, TypeTag::U64) => true,
        (TypeTag::U128, TypeTag::U128) => true,
        (TypeTag::U256, TypeTag::U256) => true,
        (TypeTag::Address, TypeTag::Address) => true,
        (TypeTag::Signer, TypeTag::Signer) => true,
        (TypeTag::Vector(inner_a), TypeTag::Vector(inner_b)) => type_tags_equal(inner_a, inner_b),
        (TypeTag::Struct(sa), TypeTag::Struct(sb)) => struct_tags_equal(sa, sb),
        _ => false,
    }
}

/// Check if two StructTags are equal.
pub fn struct_tags_equal(a: &StructTag, b: &StructTag) -> bool {
    a.address == b.address
        && a.module == b.module
        && a.name == b.name
        && a.type_params.len() == b.type_params.len()
        && a.type_params
            .iter()
            .zip(b.type_params.iter())
            .all(|(ta, tb)| type_tags_equal(ta, tb))
}

/// Check if a TypeTag is a primitive type.
pub fn is_primitive(tag: &TypeTag) -> bool {
    matches!(
        tag,
        TypeTag::Bool
            | TypeTag::U8
            | TypeTag::U16
            | TypeTag::U32
            | TypeTag::U64
            | TypeTag::U128
            | TypeTag::U256
            | TypeTag::Address
            | TypeTag::Signer
    )
}

/// Check if a TypeTag is a vector of primitives.
pub fn is_primitive_vector(tag: &TypeTag) -> bool {
    matches!(tag, TypeTag::Vector(inner) if is_primitive(inner))
}

// =============================================================================
// Type Tag Address Normalization
// =============================================================================

/// Normalize a TypeTag by replacing addresses using an alias map.
///
/// This is useful when type tags may contain storage/deployment addresses
/// that need to be converted to bytecode addresses for proper comparison.
///
/// The alias map should contain: storage_addr -> bytecode_addr mappings.
pub fn normalize_type_tag_with_aliases(
    tag: &TypeTag,
    aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> TypeTag {
    match tag {
        TypeTag::Struct(st) => {
            TypeTag::Struct(Box::new(normalize_struct_tag_with_aliases(st, aliases)))
        }
        TypeTag::Vector(inner) => {
            TypeTag::Vector(Box::new(normalize_type_tag_with_aliases(inner, aliases)))
        }
        // Primitives don't have addresses
        other => other.clone(),
    }
}

/// Normalize a StructTag by replacing addresses using an alias map.
pub fn normalize_struct_tag_with_aliases(
    tag: &StructTag,
    aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> StructTag {
    // Check if this address has an alias
    let normalized_addr = aliases.get(&tag.address).copied().unwrap_or(tag.address);

    StructTag {
        address: normalized_addr,
        module: tag.module.clone(),
        name: tag.name.clone(),
        type_params: tag
            .type_params
            .iter()
            .map(|tp| normalize_type_tag_with_aliases(tp, aliases))
            .collect(),
    }
}

/// Normalize a type string by replacing addresses using an alias map.
///
/// Scans through the string and replaces any addresses that have aliases.
/// This is useful when parsing type strings from external sources (like GraphQL)
/// that may use storage addresses instead of bytecode addresses.
pub fn normalize_type_string_with_aliases(
    type_str: &str,
    aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> String {
    let mut result = String::with_capacity(type_str.len());
    let chars: Vec<char> = type_str.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Look for 0x prefix indicating an address
        if i + 1 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
            // Extract the full hex address
            let mut end = i + 2;
            while end < chars.len() && chars[end].is_ascii_hexdigit() {
                end += 1;
            }

            let addr_str: String = chars[i..end].iter().collect();
            // Try to parse and normalize
            if let Ok(addr) = AccountAddress::from_hex_literal(&addr_str) {
                let normalized = aliases.get(&addr).copied().unwrap_or(addr);
                result.push_str(&normalized.to_hex_literal());
            } else {
                result.push_str(&addr_str);
            }
            i = end;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Parse a type string and normalize addresses using an alias map.
///
/// Combines parsing with address normalization in one step.
pub fn parse_type_tag_with_aliases(
    type_str: &str,
    aliases: &std::collections::HashMap<AccountAddress, AccountAddress>,
) -> Result<TypeTag> {
    // First normalize the string, then parse
    let normalized_str = normalize_type_string_with_aliases(type_str, aliases);
    parse_type_tag(&normalized_str)
}

// =============================================================================
// Framework Type Detection
// =============================================================================

/// Well-known system package addresses (includes DeepBook).
///
/// This includes the standard framework addresses plus DeepBook (0xdee9).
/// For just the standard framework addresses (0x1, 0x2, 0x3), use
/// [`sui_resolver::FRAMEWORK_ADDRESSES`] instead.
pub const SYSTEM_PACKAGE_ADDRESSES: [&str; 4] = ["0x1", "0x2", "0x3", "0xdee9"];

/// DeepBook package address (0xdee9).
pub const DEEPBOOK_ADDRESS: &str = "0xdee9";

/// Check if an address is a system package address (framework + DeepBook).
///
/// This returns true for 0x1, 0x2, 0x3 (standard framework) and 0xdee9 (DeepBook).
/// For checking just the standard framework addresses (0x1, 0x2, 0x3),
/// use [`sui_resolver::is_framework_account_address`] instead.
pub fn is_system_package_address(addr: &AccountAddress) -> bool {
    // Check standard framework addresses via sui_resolver
    if sui_resolver::is_framework_account_address(addr) {
        return true;
    }
    // Also check DeepBook
    let short = sui_resolver::normalize_address_short(&addr.to_hex_literal());
    short == DEEPBOOK_ADDRESS
}

/// Check if a TypeTag is a Sui Coin type.
pub fn is_coin_type(tag: &TypeTag) -> bool {
    if let TypeTag::Struct(s) = tag {
        s.module.as_str() == "coin" && s.name.as_str() == "Coin"
    } else {
        false
    }
}

/// Extract the inner type from a Coin<T>, if this is a Coin type.
pub fn extract_coin_inner_type(tag: &TypeTag) -> Option<&TypeTag> {
    if let TypeTag::Struct(s) = tag {
        if s.module.as_str() == "coin" && s.name.as_str() == "Coin" && s.type_params.len() == 1 {
            return Some(&s.type_params[0]);
        }
    }
    None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_type_tag_primitives() {
        assert_eq!(format_type_tag(&TypeTag::Bool), "bool");
        assert_eq!(format_type_tag(&TypeTag::U8), "u8");
        assert_eq!(format_type_tag(&TypeTag::U64), "u64");
        assert_eq!(format_type_tag(&TypeTag::U128), "u128");
        assert_eq!(format_type_tag(&TypeTag::U256), "u256");
        assert_eq!(format_type_tag(&TypeTag::Address), "address");
        assert_eq!(format_type_tag(&TypeTag::Signer), "signer");
    }

    #[test]
    fn test_format_type_tag_vector() {
        let vec_u8 = TypeTag::Vector(Box::new(TypeTag::U8));
        assert_eq!(format_type_tag(&vec_u8), "vector<u8>");

        let nested = TypeTag::Vector(Box::new(TypeTag::Vector(Box::new(TypeTag::U64))));
        assert_eq!(format_type_tag(&nested), "vector<vector<u64>>");
    }

    #[test]
    fn test_format_type_tag_struct() {
        let sui = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("sui").unwrap(),
            name: Identifier::new("SUI").unwrap(),
            type_params: vec![],
        }));
        let formatted = format_type_tag(&sui);
        // The address can be short or long form depending on AccountAddress::to_hex_literal
        assert!(formatted.contains("::sui::SUI"));
        assert!(formatted.starts_with("0x"));
    }

    #[test]
    fn test_format_type_tag_generic_struct() {
        let sui = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("sui").unwrap(),
            name: Identifier::new("SUI").unwrap(),
            type_params: vec![],
        }));
        let coin = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("coin").unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![sui],
        }));
        let formatted = format_type_tag(&coin);
        assert!(formatted.contains("coin::Coin<"));
        assert!(formatted.contains("sui::SUI"));
    }

    #[test]
    fn test_parse_type_string_primitives() {
        assert_eq!(parse_type_string("bool"), Some(TypeTag::Bool));
        assert_eq!(parse_type_string("u8"), Some(TypeTag::U8));
        assert_eq!(parse_type_string("u64"), Some(TypeTag::U64));
        assert_eq!(parse_type_string("address"), Some(TypeTag::Address));
        assert_eq!(parse_type_string("  u64  "), Some(TypeTag::U64)); // Trimming
    }

    #[test]
    fn test_parse_type_string_vector() {
        let parsed = parse_type_string("vector<u8>").unwrap();
        assert_eq!(parsed, TypeTag::Vector(Box::new(TypeTag::U8)));

        let nested = parse_type_string("vector<vector<u64>>").unwrap();
        assert_eq!(
            nested,
            TypeTag::Vector(Box::new(TypeTag::Vector(Box::new(TypeTag::U64))))
        );
    }

    #[test]
    fn test_parse_type_string_struct() {
        let parsed = parse_type_string("0x2::sui::SUI").unwrap();
        if let TypeTag::Struct(s) = parsed {
            assert_eq!(s.module.as_str(), "sui");
            assert_eq!(s.name.as_str(), "SUI");
        } else {
            panic!("Expected struct");
        }
    }

    #[test]
    fn test_parse_type_string_generic_struct() {
        let parsed = parse_type_string("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
        if let TypeTag::Struct(s) = parsed {
            assert_eq!(s.module.as_str(), "coin");
            assert_eq!(s.name.as_str(), "Coin");
            assert_eq!(s.type_params.len(), 1);
        } else {
            panic!("Expected struct");
        }
    }

    #[test]
    fn test_parse_type_args() {
        let args = parse_type_args("u64, bool");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], TypeTag::U64);
        assert_eq!(args[1], TypeTag::Bool);
    }

    #[test]
    fn test_parse_type_args_nested() {
        let args = parse_type_args("0x2::coin::Coin<0x2::sui::SUI>, u64");
        assert_eq!(args.len(), 2);
        if let TypeTag::Struct(s) = &args[0] {
            assert_eq!(s.name.as_str(), "Coin");
        } else {
            panic!("Expected struct");
        }
        assert_eq!(args[1], TypeTag::U64);
    }

    #[test]
    fn test_normalize_address() {
        let normalized = normalize_address("0x2").unwrap();
        assert!(normalized.starts_with("0x"));
        // Address format depends on move-core-types implementation
        // Could be short "0x2" or long "0x000...002"
        assert!(normalized.len() >= 3); // At minimum "0x2"
    }

    #[test]
    fn test_normalize_address_short() {
        assert_eq!(normalize_address_short("0x2"), Some("0x2".to_string()));
        assert_eq!(
            normalize_address_short(
                "0x0000000000000000000000000000000000000000000000000000000000000002"
            ),
            Some("0x2".to_string())
        );
    }

    #[test]
    fn test_type_tags_equal() {
        assert!(type_tags_equal(&TypeTag::U64, &TypeTag::U64));
        assert!(!type_tags_equal(&TypeTag::U64, &TypeTag::U8));

        let vec_a = TypeTag::Vector(Box::new(TypeTag::U8));
        let vec_b = TypeTag::Vector(Box::new(TypeTag::U8));
        assert!(type_tags_equal(&vec_a, &vec_b));
    }

    #[test]
    fn test_is_primitive() {
        assert!(is_primitive(&TypeTag::Bool));
        assert!(is_primitive(&TypeTag::U64));
        assert!(is_primitive(&TypeTag::Address));
        assert!(!is_primitive(&TypeTag::Vector(Box::new(TypeTag::U8))));
    }

    #[test]
    fn test_is_primitive_vector() {
        assert!(is_primitive_vector(&TypeTag::Vector(Box::new(TypeTag::U8))));
        assert!(!is_primitive_vector(&TypeTag::U64));
        assert!(!is_primitive_vector(&TypeTag::Vector(Box::new(
            TypeTag::Vector(Box::new(TypeTag::U8))
        ))));
    }

    #[test]
    fn test_is_coin_type() {
        let coin = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("coin").unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![TypeTag::U64],
        }));
        assert!(is_coin_type(&coin));
        assert!(!is_coin_type(&TypeTag::U64));
    }

    #[test]
    fn test_extract_coin_inner_type() {
        let inner = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("sui").unwrap(),
            name: Identifier::new("SUI").unwrap(),
            type_params: vec![],
        }));
        let coin = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("coin").unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![inner.clone()],
        }));

        let extracted = extract_coin_inner_type(&coin).unwrap();
        assert!(type_tags_equal(extracted, &inner));
    }

    #[test]
    fn test_roundtrip_format_parse() {
        // Test that formatting then parsing gives the same result
        let original = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("coin").unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![TypeTag::Struct(Box::new(StructTag {
                address: AccountAddress::from_hex_literal("0x2").unwrap(),
                module: Identifier::new("sui").unwrap(),
                name: Identifier::new("SUI").unwrap(),
                type_params: vec![],
            }))],
        }));

        let formatted = format_type_tag(&original);
        let parsed = parse_type_string(&formatted).unwrap();
        assert!(type_tags_equal(&original, &parsed));
    }

    // =========================================================================
    // Tests for Cached parse_type_tag
    // =========================================================================

    #[test]
    fn test_parse_type_tag_primitives() {
        clear_type_cache();
        assert_eq!(parse_type_tag("u8").unwrap(), TypeTag::U8);
        assert_eq!(parse_type_tag("u64").unwrap(), TypeTag::U64);
        assert_eq!(parse_type_tag("bool").unwrap(), TypeTag::Bool);
        assert_eq!(parse_type_tag("address").unwrap(), TypeTag::Address);
        assert_eq!(parse_type_tag("  u64  ").unwrap(), TypeTag::U64);
        // Primitives don't use cache
        assert_eq!(type_cache_size(), 0);
    }

    #[test]
    fn test_parse_type_tag_struct() {
        clear_type_cache();
        let parsed = parse_type_tag("0x2::sui::SUI").unwrap();
        if let TypeTag::Struct(s) = parsed {
            assert_eq!(s.module.as_str(), "sui");
            assert_eq!(s.name.as_str(), "SUI");
        } else {
            panic!("Expected struct");
        }
        // Struct should be cached
        assert_eq!(type_cache_size(), 1);
    }

    #[test]
    fn test_parse_type_tag_caching() {
        clear_type_cache();

        // First parse - cache miss
        let _ = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
        let stats1 = type_cache_stats();
        assert!(stats1.misses > 0);

        // Second parse - cache hit
        let _ = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
        let stats2 = type_cache_stats();
        assert!(stats2.hits > stats1.hits);
    }

    #[test]
    fn test_parse_type_tag_cache_hit_rate() {
        clear_type_cache();

        // Parse same type multiple times
        for _ in 0..10 {
            let _ = parse_type_tag("0x2::sui::SUI").unwrap();
        }

        let stats = type_cache_stats();
        // First call is a miss, rest are hits
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 9);
        assert!(stats.hit_rate() > 0.8);
    }

    #[test]
    fn test_parse_type_tag_error_messages() {
        let result = parse_type_tag("invalid");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ADDRESS::MODULE::NAME"));

        let result = parse_type_tag("0x2::coin::Coin<");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unmatched"));
    }

    #[test]
    fn test_parse_type_args_result() {
        let args = parse_type_args_result("u64, bool").unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], TypeTag::U64);
        assert_eq!(args[1], TypeTag::Bool);

        let args = parse_type_args_result("").unwrap();
        assert!(args.is_empty());

        let result = parse_type_args_result("u64, invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_clear_type_cache() {
        // Populate cache
        let _ = parse_type_tag("0x2::sui::SUI").unwrap();
        assert!(type_cache_size() > 0);

        // Clear
        clear_type_cache();
        assert_eq!(type_cache_size(), 0);

        // Stats should also be reset
        let stats = type_cache_stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }
}
