//! # Unified BCS Encoding/Decoding
//!
//! This module provides a single source of truth for BCS serialization in the sandbox.
//! All handlers should use these functions instead of implementing their own encoding logic.
//!
//! ## Supported Types
//!
//! | Type | Encode | Decode | Notes |
//! |------|--------|--------|-------|
//! | `u8`, `u16`, `u32`, `u64`, `u128` | ✓ | ✓ | Little-endian |
//! | `u256` | ✓ | ✓ | 32 bytes, little-endian |
//! | `bool` | ✓ | ✓ | 1 byte |
//! | `address` | ✓ | ✓ | 32 bytes |
//! | `vector<u8>` | ✓ | ✓ | ULEB128 length prefix |
//! | `0x1::string::String` | ✓ | ✓ | Same as vector<u8> |
//! | `0x2::object::UID` | - | ✓ | 32 bytes (object ID) |
//! | `0x2::object::ID` | - | ✓ | 32 bytes |
//! | `0x1::option::Option<T>` | - | ✓ | 0 for None, 1+value for Some |

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;

// =============================================================================
// Type Definitions for Codec
// =============================================================================

/// Metadata about a primitive type for encoding/decoding.
#[derive(Debug, Clone, Copy)]
pub struct PrimitiveTypeInfo {
    /// Size in bytes (0 for variable-length types)
    pub size: usize,
    /// Whether this type uses ULEB128 length prefix
    pub is_variable_length: bool,
}

/// Get type info for a primitive type string.
pub fn get_primitive_type_info(type_str: &str) -> Option<PrimitiveTypeInfo> {
    match type_str {
        "bool" | "u8" => Some(PrimitiveTypeInfo {
            size: 1,
            is_variable_length: false,
        }),
        "u16" => Some(PrimitiveTypeInfo {
            size: 2,
            is_variable_length: false,
        }),
        "u32" => Some(PrimitiveTypeInfo {
            size: 4,
            is_variable_length: false,
        }),
        "u64" => Some(PrimitiveTypeInfo {
            size: 8,
            is_variable_length: false,
        }),
        "u128" => Some(PrimitiveTypeInfo {
            size: 16,
            is_variable_length: false,
        }),
        "u256" | "address" | "0x2::object::UID" | "UID" | "0x2::object::ID" | "ID" => {
            Some(PrimitiveTypeInfo {
                size: 32,
                is_variable_length: false,
            })
        }
        "vector<u8>" | "0x1::string::String" | "String" => Some(PrimitiveTypeInfo {
            size: 0,
            is_variable_length: true,
        }),
        _ => None,
    }
}

// =============================================================================
// Encoding Functions
// =============================================================================

/// Encode a JSON value to BCS bytes based on the type string.
///
/// This is the canonical encoding function for the sandbox.
pub fn encode_value(value: &serde_json::Value, type_str: &str) -> Result<Vec<u8>> {
    match type_str {
        "bool" => {
            let v: bool = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u8" => {
            let v: u8 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u16" => {
            let v: u16 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u32" => {
            let v: u32 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u64" => {
            let v: u64 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u128" => {
            let v: u128 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u256" => {
            // u256 as hex string or large number string
            let s: String = serde_json::from_value(value.clone())?;
            let bytes = parse_u256_to_bytes(&s)?;
            Ok(bytes)
        }
        "address" => {
            let s: String = serde_json::from_value(value.clone())?;
            let addr = AccountAddress::from_hex_literal(&s)?;
            Ok(bcs::to_bytes(&addr)?)
        }
        "vector<u8>" => encode_vector_u8(value),
        "0x1::string::String" | "String" => {
            let s: String = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&s)?)
        }
        _ if type_str.starts_with("vector<") => {
            // For other vector types, we need more context
            Err(anyhow!(
                "Cannot encode complex vector type: {}. Use encode_vector for arrays.",
                type_str
            ))
        }
        _ => Err(anyhow!("Cannot encode type: {}", type_str)),
    }
}

/// Encode a vector<u8> from either a hex string or an array of u8.
fn encode_vector_u8(value: &serde_json::Value) -> Result<Vec<u8>> {
    if let Some(s) = value.as_str() {
        // Hex string input
        let bytes = hex::decode(s.trim_start_matches("0x"))?;
        Ok(bcs::to_bytes(&bytes)?)
    } else {
        // Array of u8 input
        let v: Vec<u8> = serde_json::from_value(value.clone())?;
        Ok(bcs::to_bytes(&v)?)
    }
}

/// Parse a u256 string (hex or decimal) to 32 bytes.
fn parse_u256_to_bytes(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    let bytes = if s.starts_with("0x") || s.starts_with("0X") {
        hex::decode(s.trim_start_matches("0x").trim_start_matches("0X"))?
    } else {
        // Decimal - convert via string parsing
        // For simplicity, we'll require hex for u256
        return Err(anyhow!("u256 must be hex-encoded (0x prefix). Got: {}", s));
    };

    // Pad to 32 bytes (little-endian)
    let mut result = vec![0u8; 32];
    let start = 32usize.saturating_sub(bytes.len());
    result[start..].copy_from_slice(&bytes);
    // Reverse for little-endian
    result.reverse();
    Ok(result)
}

/// Encode a ULEB128 length prefix.
pub fn encode_uleb128(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            bytes.push(byte);
            break;
        } else {
            bytes.push(byte | 0x80);
        }
    }
    bytes
}

/// Encode a vector of values with ULEB128 length prefix.
pub fn encode_vector(element_type: &str, values: &[serde_json::Value]) -> Result<Vec<u8>> {
    let mut bytes = encode_uleb128(values.len());

    for (i, value) in values.iter().enumerate() {
        match encode_value(value, element_type) {
            Ok(element_bytes) => bytes.extend(element_bytes),
            Err(e) => {
                return Err(anyhow!(
                    "Failed to encode element {} of type {}: {}",
                    i,
                    element_type,
                    e
                ))
            }
        }
    }

    Ok(bytes)
}

// =============================================================================
// Decoding Functions
// =============================================================================

/// Decode BCS bytes to a JSON value based on the type string.
///
/// Returns (decoded_value, bytes_consumed).
pub fn decode_value(bytes: &[u8], type_str: &str) -> (serde_json::Value, usize) {
    if bytes.is_empty() {
        return (serde_json::json!(null), 0);
    }

    match type_str {
        "bool" => decode_bool(bytes),
        "u8" => decode_u8(bytes),
        "u16" => decode_u16(bytes),
        "u32" => decode_u32(bytes),
        "u64" => decode_u64(bytes),
        "u128" => decode_u128(bytes),
        "u256" => decode_u256(bytes),
        "address" | "0x2::object::UID" | "UID" | "0x2::object::ID" | "ID" => decode_address(bytes),
        "vector<u8>" | "0x1::string::String" | "String" => decode_vector_u8(bytes),
        t if t.starts_with("vector<") => decode_generic_vector(bytes, t),
        t if t.starts_with("0x1::option::Option<") => decode_option(bytes, t),
        _ => (
            serde_json::json!({
                "raw_hex": hex::encode(bytes),
                "type": type_str,
            }),
            bytes.len(),
        ),
    }
}

fn decode_bool(bytes: &[u8]) -> (serde_json::Value, usize) {
    if bytes.is_empty() {
        (serde_json::json!(null), 0)
    } else {
        (serde_json::json!(bytes[0] != 0), 1)
    }
}

fn decode_u8(bytes: &[u8]) -> (serde_json::Value, usize) {
    if bytes.is_empty() {
        (serde_json::json!(null), 0)
    } else {
        (serde_json::json!(bytes[0]), 1)
    }
}

fn decode_u16(bytes: &[u8]) -> (serde_json::Value, usize) {
    if let Some(arr) = bytes.get(0..2).and_then(|s| <[u8; 2]>::try_from(s).ok()) {
        (serde_json::json!(u16::from_le_bytes(arr)), 2)
    } else {
        (serde_json::json!(null), bytes.len())
    }
}

fn decode_u32(bytes: &[u8]) -> (serde_json::Value, usize) {
    if let Some(arr) = bytes.get(0..4).and_then(|s| <[u8; 4]>::try_from(s).ok()) {
        (serde_json::json!(u32::from_le_bytes(arr)), 4)
    } else {
        (serde_json::json!(null), bytes.len())
    }
}

fn decode_u64(bytes: &[u8]) -> (serde_json::Value, usize) {
    if let Some(arr) = bytes.get(0..8).and_then(|s| <[u8; 8]>::try_from(s).ok()) {
        (serde_json::json!(u64::from_le_bytes(arr)), 8)
    } else {
        (serde_json::json!(null), bytes.len())
    }
}

fn decode_u128(bytes: &[u8]) -> (serde_json::Value, usize) {
    if let Some(arr) = bytes.get(0..16).and_then(|s| <[u8; 16]>::try_from(s).ok()) {
        // Return as string to avoid JSON number precision issues
        (serde_json::json!(u128::from_le_bytes(arr).to_string()), 16)
    } else {
        (serde_json::json!(null), bytes.len())
    }
}

fn decode_u256(bytes: &[u8]) -> (serde_json::Value, usize) {
    if let Some(slice) = bytes.get(0..32) {
        (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
    } else {
        (serde_json::json!(null), bytes.len())
    }
}

fn decode_address(bytes: &[u8]) -> (serde_json::Value, usize) {
    if let Some(slice) = bytes.get(0..32) {
        (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
    } else {
        (serde_json::json!(null), bytes.len())
    }
}

/// Decode a vector<u8> or String from BCS bytes (ULEB128 length prefix).
pub fn decode_vector_u8(bytes: &[u8]) -> (serde_json::Value, usize) {
    let (len, header_size) = decode_uleb128(bytes);
    if header_size == 0 {
        return (serde_json::json!(null), 0);
    }

    let total_size = header_size + len;
    if total_size > bytes.len() {
        return (
            serde_json::json!({
                "error": "Invalid vector: length exceeds available bytes",
                "raw_hex": hex::encode(bytes),
            }),
            bytes.len(),
        );
    }

    let data = &bytes[header_size..total_size];

    // Try to interpret as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        (serde_json::json!(s), total_size)
    } else {
        (
            serde_json::json!(format!("0x{}", hex::encode(data))),
            total_size,
        )
    }
}

/// Decode ULEB128 encoded length. Returns (value, bytes_consumed).
pub fn decode_uleb128(bytes: &[u8]) -> (usize, usize) {
    let mut result: usize = 0;
    let mut shift = 0;
    let mut offset = 0;

    while offset < bytes.len() {
        let byte = bytes[offset];
        result |= ((byte & 0x7f) as usize) << shift;
        offset += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }

    (result, offset)
}

fn decode_generic_vector(bytes: &[u8], type_str: &str) -> (serde_json::Value, usize) {
    // For complex vectors, return raw hex for now
    (
        serde_json::json!({
            "raw_hex": hex::encode(bytes),
            "note": format!("Cannot fully decode {}", type_str)
        }),
        bytes.len(),
    )
}

fn decode_option(bytes: &[u8], type_str: &str) -> (serde_json::Value, usize) {
    if bytes.is_empty() {
        return (serde_json::json!(null), 0);
    }

    if bytes[0] == 0 {
        // None
        (serde_json::json!(null), 1)
    } else {
        // Some - extract inner type and decode
        let inner = type_str
            .strip_prefix("0x1::option::Option<")
            .and_then(|s| s.strip_suffix(">"))
            .unwrap_or("unknown");
        let (inner_val, consumed) = decode_value(&bytes[1..], inner);
        (inner_val, 1 + consumed)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_u64() {
        let value = serde_json::json!(12345u64);
        let encoded = encode_value(&value, "u64").unwrap();
        assert_eq!(encoded, bcs::to_bytes(&12345u64).unwrap());

        let (decoded, consumed) = decode_value(&encoded, "u64");
        assert_eq!(consumed, 8);
        assert_eq!(decoded, serde_json::json!(12345u64));
    }

    #[test]
    fn test_encode_decode_bool() {
        let value = serde_json::json!(true);
        let encoded = encode_value(&value, "bool").unwrap();

        let (decoded, consumed) = decode_value(&encoded, "bool");
        assert_eq!(consumed, 1);
        assert_eq!(decoded, serde_json::json!(true));
    }

    #[test]
    fn test_encode_decode_address() {
        let value = serde_json::json!("0x2");
        let encoded = encode_value(&value, "address").unwrap();
        assert_eq!(encoded.len(), 32);

        let (decoded, consumed) = decode_value(&encoded, "address");
        assert_eq!(consumed, 32);
        // Address will be in full form
        assert!(decoded.as_str().unwrap().starts_with("0x"));
    }

    #[test]
    fn test_encode_decode_vector_u8() {
        let value = serde_json::json!("deadbeef");
        let encoded = encode_value(&value, "vector<u8>").unwrap();

        let (decoded, consumed) = decode_vector_u8(&encoded);
        assert!(consumed > 0);
        // Should decode to hex string
        assert_eq!(decoded, serde_json::json!("0xdeadbeef"));
    }

    #[test]
    fn test_encode_decode_string() {
        let value = serde_json::json!("hello world");
        let encoded = encode_value(&value, "0x1::string::String").unwrap();

        let (decoded, consumed) = decode_vector_u8(&encoded);
        assert!(consumed > 0);
        assert_eq!(decoded, serde_json::json!("hello world"));
    }

    #[test]
    fn test_uleb128() {
        // Test small value
        let encoded = encode_uleb128(127);
        assert_eq!(encoded, vec![127]);
        let (decoded, consumed) = decode_uleb128(&encoded);
        assert_eq!(decoded, 127);
        assert_eq!(consumed, 1);

        // Test larger value
        let encoded = encode_uleb128(300);
        let (decoded, consumed) = decode_uleb128(&encoded);
        assert_eq!(decoded, 300);
        assert_eq!(consumed, 2);
    }

    #[test]
    fn test_encode_vector() {
        let values = vec![serde_json::json!(1u64), serde_json::json!(2u64)];
        let encoded = encode_vector("u64", &values).unwrap();

        // Should have ULEB128 length (1 byte for len=2) + 2*8 bytes
        assert_eq!(encoded.len(), 1 + 16);
    }

    #[test]
    fn test_decode_option_none() {
        let bytes = vec![0u8];
        let (decoded, consumed) = decode_value(&bytes, "0x1::option::Option<u64>");
        assert_eq!(consumed, 1);
        assert_eq!(decoded, serde_json::json!(null));
    }

    #[test]
    fn test_decode_option_some() {
        let mut bytes = vec![1u8]; // Some marker
        bytes.extend(42u64.to_le_bytes()); // Value

        let (decoded, consumed) = decode_value(&bytes, "0x1::option::Option<u64>");
        assert_eq!(consumed, 9); // 1 + 8
        assert_eq!(decoded, serde_json::json!(42u64));
    }

    #[test]
    fn test_get_primitive_type_info() {
        assert_eq!(get_primitive_type_info("u64").unwrap().size, 8);
        assert_eq!(get_primitive_type_info("address").unwrap().size, 32);
        assert!(
            get_primitive_type_info("vector<u8>")
                .unwrap()
                .is_variable_length
        );
        assert!(get_primitive_type_info("unknown_type").is_none());
    }
}
