//! Common utilities for PTB replay examples.
//!
//! This module provides shared helper functions used across all replay examples:
//! - Type parsing (Sui type strings to Move TypeTags)
//! - Address normalization (consistent 66-char format)
//! - Bytecode dependency extraction
//! - Package ID extraction from type strings

use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use std::collections::{BTreeSet, HashSet};

/// Parse a Sui type string into a Move TypeTag.
///
/// Handles primitive types (u8, u64, etc.), vectors, and struct types with
/// nested type parameters. This is needed for the child object fetcher to
/// correctly deserialize objects by type.
///
/// # Examples
///
/// ```ignore
/// parse_type_tag_simple("u64")  // -> Some(TypeTag::U64)
/// parse_type_tag_simple("0x2::coin::Coin<0x2::sui::SUI>")  // -> Some(TypeTag::Struct(...))
/// ```
pub fn parse_type_tag_simple(type_str: &str) -> Option<TypeTag> {
    match type_str {
        "u8" => return Some(TypeTag::U8),
        "u64" => return Some(TypeTag::U64),
        "u128" => return Some(TypeTag::U128),
        "u256" => return Some(TypeTag::U256),
        "bool" => return Some(TypeTag::Bool),
        "address" => return Some(TypeTag::Address),
        _ => {}
    }

    if type_str.starts_with("vector<") && type_str.ends_with('>') {
        let inner = &type_str[7..type_str.len() - 1];
        return parse_type_tag_simple(inner).map(|t| TypeTag::Vector(Box::new(t)));
    }

    let (base_type, type_params_str) = if let Some(idx) = type_str.find('<') {
        (
            &type_str[..idx],
            Some(&type_str[idx + 1..type_str.len() - 1]),
        )
    } else {
        (type_str, None)
    };

    let parts: Vec<&str> = base_type.split("::").collect();
    if parts.len() != 3 {
        return None;
    }

    let address = AccountAddress::from_hex_literal(parts[0]).ok()?;
    let module = Identifier::new(parts[1]).ok()?;
    let name = Identifier::new(parts[2]).ok()?;

    let type_params = type_params_str
        .map(|s| {
            split_type_params(s)
                .iter()
                .filter_map(|t| parse_type_tag_simple(t.trim()))
                .collect()
        })
        .unwrap_or_default();

    Some(TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    })))
}

/// Split type parameters respecting nested angle brackets.
///
/// Given "A, B<C, D>, E", returns ["A", " B<C, D>", " E"] by tracking bracket depth.
/// Used by `parse_type_tag_simple` to correctly handle generic types.
pub fn split_type_params(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

/// Extract package IDs from a type string.
///
/// Scans a type string like "0xabc::module::Struct<0xdef::m::T, 0xghi::n::U>"
/// and returns all package addresses found (["0xabc", "0xdef", "0xghi"]).
/// Framework packages (0x1, 0x2, 0x3) are automatically excluded.
///
/// This is used to discover additional packages that need to be fetched based
/// on the types of objects involved in the transaction.
pub fn extract_package_ids_from_type(type_str: &str) -> Vec<String> {
    let mut package_ids = HashSet::new();

    // Framework packages to skip
    let framework_prefixes = [
        "0x1::",
        "0x2::",
        "0x3::",
        "0x0000000000000000000000000000000000000000000000000000000000000001::",
        "0x0000000000000000000000000000000000000000000000000000000000000002::",
        "0x0000000000000000000000000000000000000000000000000000000000000003::",
    ];

    // Find all package addresses in the type string
    // Pattern: 0x followed by hex chars, then ::
    let mut i = 0;
    let chars: Vec<char> = type_str.chars().collect();

    while i < chars.len() {
        if i + 2 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
            let start = i;
            i += 2;
            // Consume hex chars
            while i < chars.len() && (chars[i].is_ascii_hexdigit()) {
                i += 1;
            }
            // Check if followed by ::
            if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':' {
                let pkg_id: String = chars[start..i].iter().collect();
                // Skip framework packages
                let full_prefix = format!("{}::", pkg_id);
                if !framework_prefixes.iter().any(|p| full_prefix == *p) {
                    package_ids.insert(pkg_id);
                }
            }
        } else {
            i += 1;
        }
    }

    package_ids.into_iter().collect()
}

/// Normalize a Sui address to a consistent 66-character format (0x + 64 hex chars).
///
/// Sui addresses can appear in shortened form (0x2) or full form
/// (0x0000...0002). This function ensures consistent formatting for
/// HashMap key lookups and address comparisons.
///
/// # Examples
///
/// ```ignore
/// normalize_address("0x2")      // -> "0x0000...0002" (64 hex chars)
/// normalize_address("0x3637")   // -> "0x0000...3637" (64 hex chars)
/// ```
pub fn normalize_address(addr: &str) -> String {
    let addr = addr.strip_prefix("0x").unwrap_or(addr);
    // Pad to 64 hex characters
    format!("0x{:0>64}", addr)
}

/// Extract package addresses that a module depends on from its bytecode.
///
/// Parses the compiled Move bytecode to find all module handles (references to
/// other modules), and returns the package addresses of non-framework dependencies.
/// This enables transitive dependency resolution - fetching all packages needed
/// to execute a transaction.
///
/// Framework packages (0x1, 0x2, 0x3) are excluded since they are bundled
/// with the VM and don't need to be fetched.
pub fn extract_dependencies_from_bytecode(bytecode: &[u8]) -> Vec<String> {
    // Framework addresses to skip
    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    let mut deps = Vec::new();

    if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
        for handle in &module.module_handles {
            let addr = *module.address_identifier_at(handle.address);
            // Skip framework modules and self
            if !framework_addrs.contains(&addr) {
                deps.push(addr.to_hex_literal());
            }
        }
    }

    deps
}

/// Check if a package ID is a framework package (0x1, 0x2, 0x3).
pub fn is_framework_package(pkg_id: &str) -> bool {
    matches!(
        pkg_id,
        "0x0000000000000000000000000000000000000000000000000000000000000001"
            | "0x0000000000000000000000000000000000000000000000000000000000000002"
            | "0x0000000000000000000000000000000000000000000000000000000000000003"
            | "0x1"
            | "0x2"
            | "0x3"
    )
}
