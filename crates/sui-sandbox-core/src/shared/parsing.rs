//! Shared argument and value parsing utilities.
//!
//! This module provides unified parsing functions for converting user input
//! (from CLI args or JSON) into Move values and PTB inputs.
//!
//! # Example
//!
//! ```
//! use sui_sandbox_core::shared::parsing::{parse_pure_value, parse_typed_value};
//!
//! // Parse a simple number
//! let bytes = parse_pure_value("42").unwrap();
//!
//! // Parse with explicit type
//! let bytes = parse_typed_value("u8", "255").unwrap();
//! ```

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde_json::Value;

use crate::ptb::InputValue;

/// Parse a pure value from a string representation.
///
/// Supports:
/// - Booleans: `true`, `false`
/// - Addresses: `0x123...`
/// - Strings: `"hello"` or `'hello'`
/// - Byte vectors: `b"hello"` or `x"deadbeef"`
/// - Numbers: `42`, `1000000`
/// - Typed values: `u8:255`, `u128:123456789`
/// - Address vectors: `[@0x1, @0x2]`
pub fn parse_pure_value(arg: &str) -> Result<Vec<u8>> {
    let arg = arg.trim();

    // Boolean
    if arg == "true" {
        return Ok(bcs::to_bytes(&true)?);
    }
    if arg == "false" {
        return Ok(bcs::to_bytes(&false)?);
    }

    // Address (0x prefixed)
    if arg.starts_with("0x") || arg.starts_with("0X") {
        if let Ok(addr) = AccountAddress::from_hex_literal(arg) {
            return Ok(bcs::to_bytes(&addr)?);
        }
    }

    // String (quoted)
    if (arg.starts_with('"') && arg.ends_with('"'))
        || (arg.starts_with('\'') && arg.ends_with('\''))
    {
        let s = &arg[1..arg.len() - 1];
        return Ok(bcs::to_bytes(&s.as_bytes().to_vec())?);
    }

    // Byte vector (b"..." or x"...")
    if arg.starts_with("b\"") && arg.ends_with('"') {
        let s = &arg[2..arg.len() - 1];
        return Ok(bcs::to_bytes(&s.as_bytes().to_vec())?);
    }
    if arg.starts_with("x\"") && arg.ends_with('"') {
        let hex_str = &arg[2..arg.len() - 1];
        let bytes = hex::decode(hex_str).context("Invalid hex in x\"...\"")?;
        return Ok(bcs::to_bytes(&bytes)?);
    }

    // Vector of addresses ([@0x1, @0x2])
    if arg.starts_with("[@") && arg.ends_with(']') {
        let inner = &arg[1..arg.len() - 1];
        let addrs: Result<Vec<AccountAddress>> = inner
            .split(',')
            .map(|s| {
                let s = s.trim().trim_start_matches('@');
                AccountAddress::from_hex_literal(s).context("Invalid address in vector")
            })
            .collect();
        return Ok(bcs::to_bytes(&addrs?)?);
    }

    // Try as u64
    if let Ok(n) = arg.parse::<u64>() {
        return Ok(bcs::to_bytes(&n)?);
    }

    // Try as u128 (for large numbers)
    if let Ok(n) = arg.parse::<u128>() {
        return Ok(bcs::to_bytes(&n)?);
    }

    // Try as i64 (negative numbers)
    if let Ok(n) = arg.parse::<i64>() {
        let u = n as u64;
        return Ok(bcs::to_bytes(&u)?);
    }

    // Explicit type annotations: u8:42, u16:1000, etc.
    if let Some((type_prefix, value)) = arg.split_once(':') {
        return parse_typed_value(type_prefix, value);
    }

    Err(anyhow!(
        "Could not parse '{}'. Supported: numbers, true/false, \"string\", 0xADDRESS, u8:N, etc.",
        arg
    ))
}

/// Parse a value with an explicit type prefix.
///
/// Supported types: u8, u16, u32, u64, u128, u256, bool, address, string, hex
pub fn parse_typed_value(type_prefix: &str, value: &str) -> Result<Vec<u8>> {
    match type_prefix {
        "u8" => {
            let n: u8 = value.parse().context("Invalid u8 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u16" => {
            let n: u16 = value.parse().context("Invalid u16 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u32" => {
            let n: u32 = value.parse().context("Invalid u32 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u64" => {
            let n: u64 = value.parse().context("Invalid u64 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u128" => {
            let n: u128 = value.parse().context("Invalid u128 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u256" => {
            let n = move_core_types::u256::U256::from_str_radix(value, 10)
                .context("Invalid u256 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "bool" => {
            let b: bool = value.parse().context("Invalid bool value")?;
            Ok(bcs::to_bytes(&b)?)
        }
        "address" => {
            let addr = AccountAddress::from_hex_literal(value).context("Invalid address value")?;
            Ok(bcs::to_bytes(&addr)?)
        }
        "string" | "utf8" => Ok(bcs::to_bytes(&value.as_bytes().to_vec())?),
        "hex" => {
            let bytes = hex::decode(value).context("Invalid hex value")?;
            Ok(bcs::to_bytes(&bytes)?)
        }
        _ => Err(anyhow!(
            "Unknown type '{}'. Supported: u8, u16, u32, u64, u128, u256, bool, address, string, hex",
            type_prefix
        )),
    }
}

/// Parse a pure value from a JSON Value.
///
/// Handles JSON types and optional explicit type hints.
pub fn parse_pure_from_json(value: &Value, type_hint: Option<&str>) -> Result<Vec<u8>> {
    // If we have both value and type hint, use typed encoding
    if let Some(type_str) = type_hint {
        return encode_json_with_type(value, type_str);
    }

    // Infer type from JSON value
    match value {
        Value::Bool(b) => Ok(bcs::to_bytes(b)?),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(bcs::to_bytes(&u)?)
            } else if let Some(i) = n.as_i64() {
                Ok(bcs::to_bytes(&(i as u64))?)
            } else {
                Err(anyhow!("Number too large for u64"))
            }
        }
        Value::String(s) => {
            // Try to parse as a value string first
            if let Ok(bytes) = parse_pure_value(s) {
                return Ok(bytes);
            }
            // Otherwise treat as string bytes
            Ok(bcs::to_bytes(&s.as_bytes().to_vec())?)
        }
        Value::Array(arr) => {
            // Try to encode as vector of addresses if all elements look like addresses
            let maybe_addrs: Result<Vec<AccountAddress>> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| anyhow!("Array element not a string"))
                        .and_then(|s| {
                            AccountAddress::from_hex_literal(s)
                                .context("Invalid address in array")
                        })
                })
                .collect();

            if let Ok(addrs) = maybe_addrs {
                return Ok(bcs::to_bytes(&addrs)?);
            }

            // Try as vector of u64
            let maybe_nums: Result<Vec<u64>> = arr
                .iter()
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| anyhow!("Array element not a number"))
                })
                .collect();

            if let Ok(nums) = maybe_nums {
                return Ok(bcs::to_bytes(&nums)?);
            }

            Err(anyhow!("Could not encode array - use explicit type hint"))
        }
        Value::Null => Err(anyhow!("Cannot encode null value")),
        Value::Object(_) => Err(anyhow!("Cannot encode object - use explicit type hint")),
    }
}

/// Encode a JSON value with an explicit type string.
fn encode_json_with_type(value: &Value, type_str: &str) -> Result<Vec<u8>> {
    match type_str {
        "bool" => {
            let b = value.as_bool().ok_or_else(|| anyhow!("Expected bool"))?;
            Ok(bcs::to_bytes(&b)?)
        }
        "u8" => {
            let n = json_to_u64(value)? as u8;
            Ok(bcs::to_bytes(&n)?)
        }
        "u16" => {
            let n = json_to_u64(value)? as u16;
            Ok(bcs::to_bytes(&n)?)
        }
        "u32" => {
            let n = json_to_u64(value)? as u32;
            Ok(bcs::to_bytes(&n)?)
        }
        "u64" => {
            let n = json_to_u64(value)?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u128" => {
            let n = json_to_u128(value)?;
            Ok(bcs::to_bytes(&n)?)
        }
        "u256" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("u256 must be a string"))?;
            let n =
                move_core_types::u256::U256::from_str_radix(s, 10).context("Invalid u256 value")?;
            Ok(bcs::to_bytes(&n)?)
        }
        "address" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("address must be a string"))?;
            let addr = AccountAddress::from_hex_literal(s)?;
            Ok(bcs::to_bytes(&addr)?)
        }
        "string" | "0x1::string::String" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("string must be a string"))?;
            Ok(bcs::to_bytes(&s.as_bytes().to_vec())?)
        }
        "ascii" | "0x1::ascii::String" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("ascii must be a string"))?;
            Ok(bcs::to_bytes(&s.as_bytes().to_vec())?)
        }
        t if t.starts_with("vector<") => {
            let inner = &t[7..t.len() - 1];
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector type requires array value"))?;
            encode_vector(arr, inner)
        }
        _ => Err(anyhow!("Unsupported type for JSON encoding: {}", type_str)),
    }
}

fn encode_vector(arr: &[Value], inner_type: &str) -> Result<Vec<u8>> {
    match inner_type {
        "u8" => {
            let v: Result<Vec<u8>> = arr.iter().map(|v| Ok(json_to_u64(v)? as u8)).collect();
            Ok(bcs::to_bytes(&v?)?)
        }
        "u64" => {
            let v: Result<Vec<u64>> = arr.iter().map(json_to_u64).collect();
            Ok(bcs::to_bytes(&v?)?)
        }
        "address" => {
            let v: Result<Vec<AccountAddress>> = arr
                .iter()
                .map(|v| {
                    let s = v.as_str().ok_or_else(|| anyhow!("Expected string"))?;
                    AccountAddress::from_hex_literal(s).context("Invalid address")
                })
                .collect();
            Ok(bcs::to_bytes(&v?)?)
        }
        _ => Err(anyhow!("Unsupported vector element type: {}", inner_type)),
    }
}

fn json_to_u64(value: &Value) -> Result<u64> {
    if let Some(n) = value.as_u64() {
        return Ok(n);
    }
    if let Some(s) = value.as_str() {
        return s.parse().context("Invalid number string");
    }
    Err(anyhow!("Expected number or numeric string"))
}

fn json_to_u128(value: &Value) -> Result<u128> {
    if let Some(n) = value.as_u64() {
        return Ok(n as u128);
    }
    if let Some(s) = value.as_str() {
        return s.parse().context("Invalid number string");
    }
    Err(anyhow!("Expected number or numeric string"))
}

/// Parse a type tag from a string.
pub fn parse_type_tag_string(s: &str) -> Result<TypeTag> {
    crate::types::parse_type_tag(s).map_err(|e| anyhow!("Invalid type tag '{}': {}", s, e))
}

/// Convert a pure value string into an InputValue for PTB execution.
pub fn string_to_input_value(arg: &str) -> Result<InputValue> {
    let bytes = parse_pure_value(arg)?;
    Ok(InputValue::Pure(bytes))
}

/// Convert a JSON value into an InputValue for PTB execution.
pub fn json_to_input_value(value: &Value, type_hint: Option<&str>) -> Result<InputValue> {
    let bytes = parse_pure_from_json(value, type_hint)?;
    Ok(InputValue::Pure(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bool() {
        let bytes = parse_pure_value("true").unwrap();
        let val: bool = bcs::from_bytes(&bytes).unwrap();
        assert!(val);

        let bytes = parse_pure_value("false").unwrap();
        let val: bool = bcs::from_bytes(&bytes).unwrap();
        assert!(!val);
    }

    #[test]
    fn test_parse_number() {
        let bytes = parse_pure_value("42").unwrap();
        let val: u64 = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(val, 42);
    }

    #[test]
    fn test_parse_address() {
        let bytes = parse_pure_value("0x123").unwrap();
        let val: AccountAddress = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(val, AccountAddress::from_hex_literal("0x123").unwrap());
    }

    #[test]
    fn test_parse_string() {
        let bytes = parse_pure_value("\"hello\"").unwrap();
        let val: Vec<u8> = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(val, b"hello".to_vec());
    }

    #[test]
    fn test_parse_typed_u8() {
        let bytes = parse_typed_value("u8", "255").unwrap();
        let val: u8 = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(val, 255);
    }

    #[test]
    fn test_parse_json_number() {
        let value = serde_json::json!(42);
        let bytes = parse_pure_from_json(&value, None).unwrap();
        let val: u64 = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(val, 42);
    }

    #[test]
    fn test_parse_json_bool() {
        let value = serde_json::json!(true);
        let bytes = parse_pure_from_json(&value, None).unwrap();
        let val: bool = bcs::from_bytes(&bytes).unwrap();
        assert!(val);
    }

    #[test]
    fn test_parse_json_with_type() {
        let value = serde_json::json!(255);
        let bytes = parse_pure_from_json(&value, Some("u8")).unwrap();
        let val: u8 = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(val, 255);
    }

    #[test]
    fn test_parse_type_tag() {
        let tag = parse_type_tag_string("0x2::sui::SUI").unwrap();
        assert!(tag.to_string().contains("SUI"));
    }
}
