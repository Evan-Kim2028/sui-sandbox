//! Type parsing and package extraction utilities.
//!
//! This module provides utilities for working with Move type strings and bytecode:
//!
//! - [`parse_type_tag`]: Parse Sui type strings into Move TypeTags
//! - [`extract_package_ids_from_type`]: Extract package addresses from type strings
//! - [`extract_dependencies_from_bytecode`]: Find package dependencies in compiled bytecode
//!
//! These utilities are useful for:
//! - Deserializing objects by their type
//! - Discovering packages needed for transaction execution
//! - Resolving transitive dependencies

use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use std::collections::{BTreeSet, HashSet};

/// Parse a Sui type string into a Move TypeTag.
///
/// Handles primitive types (u8, u64, etc.), vectors, and struct types with
/// nested type parameters. This is needed for correctly deserializing objects
/// by their type.
///
/// # Examples
///
/// ```
/// use sui_sandbox_core::utilities::parse_type_tag;
///
/// // Primitive types
/// assert!(matches!(parse_type_tag("u64"), Some(move_core_types::language_storage::TypeTag::U64)));
/// assert!(matches!(parse_type_tag("bool"), Some(move_core_types::language_storage::TypeTag::Bool)));
///
/// // Struct types
/// let coin_type = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>");
/// assert!(coin_type.is_some());
/// ```
pub fn parse_type_tag(type_str: &str) -> Option<TypeTag> {
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
        return parse_type_tag(inner).map(|t| TypeTag::Vector(Box::new(t)));
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
                .filter_map(|t| parse_type_tag(t.trim()))
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
/// Used by `parse_type_tag` to correctly handle generic types.
///
/// # Example
///
/// ```
/// use sui_sandbox_core::utilities::split_type_params;
///
/// let params = split_type_params("u64, 0x2::coin::Coin<0x2::sui::SUI>");
/// assert_eq!(params.len(), 2);
/// ```
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
/// Scans a type string like "0xabc::module::Struct<0xdef::m::T, 0x123::n::U>"
/// and returns all package addresses found (["0xabc", "0xdef", "0x123"]).
/// Framework packages (0x1, 0x2, 0x3) are automatically excluded.
///
/// This is used to discover additional packages that need to be fetched based
/// on the types of objects involved in the transaction.
///
/// # Example
///
/// ```
/// use sui_sandbox_core::utilities::extract_package_ids_from_type;
///
/// let ids = extract_package_ids_from_type("0xabc::mod::Type<0xdef::m::T>");
/// assert!(ids.contains(&"0xabc".to_string()));
/// assert!(ids.contains(&"0xdef".to_string()));
/// ```
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

/// Extract package addresses that a module depends on from its bytecode.
///
/// Parses the compiled Move bytecode to find all module handles (references to
/// other modules), and returns the package addresses of non-framework dependencies.
/// This enables transitive dependency resolution - fetching all packages needed
/// to execute a transaction.
///
/// Framework packages (0x1, 0x2, 0x3) are excluded since they are bundled
/// with the VM and don't need to be fetched.
///
/// # Example
///
/// ```ignore
/// use sui_sandbox_core::utilities::extract_dependencies_from_bytecode;
///
/// let deps = extract_dependencies_from_bytecode(&module_bytecode);
/// for pkg_addr in deps {
///     println!("Depends on package: {}", pkg_addr);
/// }
/// ```
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_type_tag_primitives() {
        assert!(matches!(parse_type_tag("u8"), Some(TypeTag::U8)));
        assert!(matches!(parse_type_tag("u64"), Some(TypeTag::U64)));
        assert!(matches!(parse_type_tag("u128"), Some(TypeTag::U128)));
        assert!(matches!(parse_type_tag("u256"), Some(TypeTag::U256)));
        assert!(matches!(parse_type_tag("bool"), Some(TypeTag::Bool)));
        assert!(matches!(parse_type_tag("address"), Some(TypeTag::Address)));
    }

    #[test]
    fn test_parse_type_tag_vector() {
        let result = parse_type_tag("vector<u64>");
        assert!(matches!(result, Some(TypeTag::Vector(_))));
    }

    #[test]
    fn test_parse_type_tag_struct() {
        let result = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>");
        assert!(result.is_some());
        if let Some(TypeTag::Struct(s)) = result {
            assert_eq!(s.module.as_str(), "coin");
            assert_eq!(s.name.as_str(), "Coin");
            assert_eq!(s.type_params.len(), 1);
        } else {
            panic!("Expected struct type");
        }
    }

    #[test]
    fn test_parse_type_tag_invalid() {
        assert!(parse_type_tag("invalid").is_none());
        assert!(parse_type_tag("0x2::coin").is_none()); // Missing name
    }

    #[test]
    fn test_split_type_params_simple() {
        let result = split_type_params("u64, u128");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "u64");
        assert_eq!(result[1].trim(), "u128");
    }

    #[test]
    fn test_split_type_params_nested() {
        let result = split_type_params("A, B<C, D>, E");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "A");
        assert!(result[1].contains("B<C, D>"));
        assert_eq!(result[2].trim(), "E");
    }

    #[test]
    fn test_extract_package_ids_simple() {
        let ids = extract_package_ids_from_type("0xabc::mod::Type");
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"0xabc".to_string()));
    }

    #[test]
    fn test_extract_package_ids_nested() {
        let ids = extract_package_ids_from_type("0xabc::mod::Type<0xdef::m::T, 0x123::n::U>");
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"0xabc".to_string()));
        assert!(ids.contains(&"0xdef".to_string()));
        assert!(ids.contains(&"0x123".to_string()));
    }

    #[test]
    fn test_extract_package_ids_excludes_framework() {
        let ids = extract_package_ids_from_type("0x2::coin::Coin<0xabc::token::TOKEN>");
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"0xabc".to_string()));
        assert!(!ids.iter().any(|id| id == "0x2"));
    }

    #[test]
    fn test_extract_dependencies_empty_bytecode() {
        let deps = extract_dependencies_from_bytecode(&[]);
        assert!(deps.is_empty());
    }
}
