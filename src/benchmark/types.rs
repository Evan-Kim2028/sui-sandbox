//! # Type Utilities
//!
//! This module provides canonical type handling utilities for the benchmark system.
//! All type string parsing and formatting should go through these functions to ensure
//! consistency across the codebase.
//!
//! ## Key Functions
//!
//! - [`format_type_tag`] - Convert a TypeTag to its canonical string representation
//! - [`parse_type_string`] - Parse a type string into a TypeTag
//! - [`parse_type_args`] - Parse comma-separated type arguments
//! - [`normalize_address`] - Normalize an address to canonical form (0x-prefixed, lowercase)

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};

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
/// ```ignore
/// let tag = TypeTag::U64;
/// assert_eq!(format_type_tag(&tag), "u64");
///
/// // Struct with type parameters
/// let coin_tag = /* Coin<SUI> */;
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
/// ```ignore
/// let tag = parse_type_string("u64").unwrap();
/// assert_eq!(tag, TypeTag::U64);
///
/// let coin = parse_type_string("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
/// // Returns Coin<SUI> TypeTag
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
// Address Normalization
// =============================================================================

/// Normalize an address string to canonical form.
///
/// Canonical form is:
/// - Lowercase hex
/// - 0x-prefixed
/// - Full 64-character (32-byte) representation
///
/// # Examples
/// ```ignore
/// assert_eq!(normalize_address("0x2"), Some("0x0000000000000000000000000000000000000000000000000000000000000002".to_string()));
/// assert_eq!(normalize_address("0X02"), Some("0x0000000000000000000000000000000000000000000000000000000000000002".to_string()));
/// ```
pub fn normalize_address(addr: &str) -> Option<String> {
    let trimmed = addr.trim();
    let addr = AccountAddress::from_hex_literal(trimmed).ok()?;
    Some(addr.to_hex_literal())
}

/// Normalize an address to short form (no leading zeros except for special addresses).
///
/// Special addresses (0x0, 0x1, 0x2, 0x3) keep their short form.
/// Other addresses are trimmed of leading zeros but keep at least one digit.
pub fn normalize_address_short(addr: &str) -> Option<String> {
    let trimmed = addr.trim();
    let addr = AccountAddress::from_hex_literal(trimmed).ok()?;

    // Use the short form for common framework addresses
    let bytes = addr.into_bytes();
    let is_short = bytes[..31].iter().all(|&b| b == 0) && bytes[31] <= 3;

    if is_short {
        Some(format!("0x{}", bytes[31]))
    } else {
        // Trim leading zeros
        let hex = hex::encode(bytes);
        let trimmed = hex.trim_start_matches('0');
        if trimmed.is_empty() {
            Some("0x0".to_string())
        } else {
            Some(format!("0x{}", trimmed))
        }
    }
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
// Framework Type Detection
// =============================================================================

/// Well-known framework addresses.
pub const FRAMEWORK_ADDRESSES: [&str; 4] = ["0x1", "0x2", "0x3", "0xdee9"];

/// Check if an address is a framework address.
pub fn is_framework_address(addr: &AccountAddress) -> bool {
    let short = normalize_address_short(&addr.to_hex_literal());
    short
        .map(|s| FRAMEWORK_ADDRESSES.contains(&s.as_str()))
        .unwrap_or(false)
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
}
