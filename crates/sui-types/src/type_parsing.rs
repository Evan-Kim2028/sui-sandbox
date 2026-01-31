//! Type string parsing utilities.
//!
//! Provides shared type parsing functions used across workspace crates.
//! This avoids duplicating type parsing logic in multiple places.

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};

/// Parse a Move type string into a TypeTag.
///
/// Supports:
/// - Primitive types: `bool`, `u8`, `u16`, `u32`, `u64`, `u128`, `u256`, `address`, `signer`
/// - Vector types: `vector<T>`
/// - Struct types: `0x2::module::Struct` or `0x2::module::Struct<T1, T2>`
///
/// # Examples
///
/// ```ignore
/// use sui_sandbox_types::parse_type_tag;
///
/// let tag = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
/// ```
pub fn parse_type_tag(type_str: &str) -> Option<TypeTag> {
    let type_str = type_str.trim();

    // Handle primitive types
    match type_str {
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
    if let Some(inner) = type_str
        .strip_prefix("vector<")
        .and_then(|s| s.strip_suffix('>'))
    {
        let inner_tag = parse_type_tag(inner)?;
        return Some(TypeTag::Vector(Box::new(inner_tag)));
    }

    // Handle struct types: 0x<address>::<module>::<name><type_args>
    let (base_type, type_args_str) = if let Some(angle_pos) = type_str.find('<') {
        let base = &type_str[..angle_pos];
        let args_str = &type_str[angle_pos..];
        (base, Some(args_str))
    } else {
        (type_str, None)
    };

    let parts: Vec<&str> = base_type.split("::").collect();
    if parts.len() != 3 {
        return None;
    }

    let address_str = parts[0];
    let module_name = parts[1];
    let struct_name = parts[2];

    let address = AccountAddress::from_hex_literal(address_str).ok()?;
    let module = Identifier::new(module_name).ok()?;
    let name = Identifier::new(struct_name).ok()?;

    // Parse type arguments if present
    let type_params = if let Some(args_str) = type_args_str {
        parse_type_args(args_str)?
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

/// Parse type arguments string like "<T1, T2, T3>".
fn parse_type_args(args_str: &str) -> Option<Vec<TypeTag>> {
    let inner = args_str.strip_prefix('<')?.strip_suffix('>')?;
    if inner.is_empty() {
        return Some(vec![]);
    }

    let mut args = vec![];
    let mut depth = 0;
    let mut current_start = 0;

    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                let arg = inner[current_start..i].trim();
                args.push(parse_type_tag(arg)?);
                current_start = i + 1;
            }
            _ => {}
        }
    }

    // Don't forget the last argument
    let last_arg = inner[current_start..].trim();
    if !last_arg.is_empty() {
        args.push(parse_type_tag(last_arg)?);
    }

    Some(args)
}

/// Split type parameters respecting nested angle brackets.
///
/// Given "A, B<C, D>, E", returns ["A", "B<C, D>", "E"] by tracking bracket depth.
pub fn split_type_params(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(s[start..].trim());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_primitives() {
        assert!(matches!(parse_type_tag("bool"), Some(TypeTag::Bool)));
        assert!(matches!(parse_type_tag("u64"), Some(TypeTag::U64)));
        assert!(matches!(parse_type_tag("address"), Some(TypeTag::Address)));
    }

    #[test]
    fn test_parse_vector() {
        let tag = parse_type_tag("vector<u8>").unwrap();
        assert!(matches!(tag, TypeTag::Vector(_)));
    }

    #[test]
    fn test_parse_struct() {
        let tag = parse_type_tag("0x2::coin::Coin<0x2::sui::SUI>").unwrap();
        if let TypeTag::Struct(s) = tag {
            assert_eq!(s.module.as_str(), "coin");
            assert_eq!(s.name.as_str(), "Coin");
            assert_eq!(s.type_params.len(), 1);
        } else {
            panic!("Expected struct type");
        }
    }

    #[test]
    fn test_split_type_params() {
        let params = split_type_params("u64, 0x2::coin::Coin<0x2::sui::SUI>, bool");
        assert_eq!(params.len(), 3);
        assert_eq!(params[0], "u64");
        assert_eq!(params[1], "0x2::coin::Coin<0x2::sui::SUI>");
        assert_eq!(params[2], "bool");
    }
}
