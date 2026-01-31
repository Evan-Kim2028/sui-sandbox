//! Encoding utilities for hex and base64.
//!
//! Provides shared encoding/decoding functions used across workspace crates.
//! These eliminate repetitive error handling patterns.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;

// =============================================================================
// Hex Address Parsing
// =============================================================================

/// Parse a hex string to an AccountAddress with context-aware error message.
///
/// # Arguments
/// * `hex_str` - Hex string (with or without 0x prefix)
/// * `context` - Description for error messages (e.g., "object ID", "package address")
///
/// # Examples
///
/// ```ignore
/// use sui_sandbox_types::encoding::parse_address;
///
/// let addr = parse_address("0x2", "package")?;
/// let addr = parse_address("0xabc123", "object ID")?;
/// ```
pub fn parse_address(hex_str: &str, context: &str) -> Result<AccountAddress> {
    AccountAddress::from_hex_literal(hex_str)
        .map_err(|e| anyhow!("Invalid {} '{}': {}", context, hex_str, e))
}

/// Parse a hex string to an AccountAddress, returning None on failure.
///
/// Use this when the address is optional or when you want to handle
/// the error yourself.
pub fn try_parse_address(hex_str: &str) -> Option<AccountAddress> {
    AccountAddress::from_hex_literal(hex_str).ok()
}

/// Parse a hex string to raw bytes.
///
/// # Arguments
/// * `hex_str` - Hex string (with or without 0x prefix)
/// * `context` - Description for error messages
pub fn parse_hex_bytes(hex_str: &str, context: &str) -> Result<Vec<u8>> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    hex::decode(hex_str).map_err(|e| anyhow!("Invalid {} hex '{}': {}", context, hex_str, e))
}

// =============================================================================
// Base64 Encoding/Decoding
// =============================================================================

/// Encode bytes to base64 string.
pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decode base64 string to bytes with context-aware error message.
///
/// # Arguments
/// * `b64` - Base64 encoded string
/// * `context` - Description for error messages (e.g., "module bytecode", "BCS data")
pub fn base64_decode(b64: &str, context: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| anyhow!("Failed to decode {} from base64: {}", context, e))
}

/// Decode base64 string to bytes, returning None on failure.
pub fn try_base64_decode(b64: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

// =============================================================================
// Address Formatting & Normalization
// =============================================================================

/// Normalize an address string to lowercase with 0x prefix and full 64 hex characters.
///
/// This is the canonical address format for internal comparisons and storage.
/// Handles addresses with or without 0x prefix, short or full form.
///
/// # Examples
///
/// ```
/// use sui_sandbox_types::encoding::normalize_address;
///
/// assert_eq!(
///     normalize_address("0x2"),
///     "0x0000000000000000000000000000000000000000000000000000000000000002"
/// );
/// assert_eq!(
///     normalize_address("ABC"),
///     "0x0000000000000000000000000000000000000000000000000000000000000abc"
/// );
/// ```
pub fn normalize_address(addr: &str) -> String {
    let addr = addr.trim();
    let hex = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr)
        .to_lowercase();
    if hex.len() < 64 {
        format!("0x{:0>64}", hex)
    } else {
        format!("0x{}", &hex[..64])
    }
}

/// Normalize an address string, returning None if it's not valid hex.
///
/// Validates by attempting to parse as an AccountAddress.
pub fn normalize_address_checked(addr: &str) -> Option<String> {
    let normalized = normalize_address(addr);
    AccountAddress::from_hex_literal(&normalized).ok()?;
    Some(normalized)
}

/// Normalize an address string to short form (minimal hex digits).
///
/// Useful for display purposes.
///
/// # Examples
///
/// ```
/// use sui_sandbox_types::encoding::normalize_address_short;
///
/// assert_eq!(normalize_address_short("0x0000000000000000000000000000000000000000000000000000000000000002"), "0x2");
/// ```
pub fn normalize_address_short(addr: &str) -> String {
    let normalized = normalize_address(addr);
    let hex = normalized.strip_prefix("0x").unwrap_or(&normalized);
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0x0".to_string()
    } else {
        format!("0x{}", trimmed)
    }
}

/// Format an AccountAddress as a full 66-character hex string (0x + 64 hex chars).
pub fn format_address_full(addr: &AccountAddress) -> String {
    format!("0x{}", hex::encode(addr.as_ref()))
}

/// Format an AccountAddress in short form (strips leading zeros).
///
/// Example: 0x0000...0002 -> 0x2
pub fn format_address_short(addr: &AccountAddress) -> String {
    let bytes = addr.as_ref();
    // Find first non-zero byte
    let first_nonzero = bytes.iter().position(|&b| b != 0).unwrap_or(31);
    if first_nonzero == 31 && bytes[31] == 0 {
        return "0x0".to_string();
    }
    format!("0x{}", hex::encode(&bytes[first_nonzero..]))
}

/// Convert an AccountAddress to its normalized full-form string.
pub fn address_to_string(addr: &AccountAddress) -> String {
    format_address_full(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_address() {
        let addr = parse_address("0x2", "test").unwrap();
        assert_eq!(addr, AccountAddress::from_hex_literal("0x2").unwrap());

        let result = parse_address("invalid", "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid test"));
    }

    #[test]
    fn test_try_parse_address() {
        assert!(try_parse_address("0x2").is_some());
        assert!(try_parse_address("invalid").is_none());
    }

    #[test]
    fn test_base64_roundtrip() {
        let original = b"hello world";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded, "test").unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_base64_decode_error() {
        let result = base64_decode("not-valid-base64!!!", "test data");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("test data"));
    }

    #[test]
    fn test_format_address_short() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert_eq!(format_address_short(&addr), "0x02");

        let addr = AccountAddress::from_hex_literal("0x0").unwrap();
        assert_eq!(format_address_short(&addr), "0x0");
    }

    #[test]
    fn test_format_address_full() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        let full = format_address_full(&addr);
        assert_eq!(full.len(), 66); // 0x + 64 hex chars
        assert!(full.starts_with("0x"));
        assert!(full.ends_with("02"));
    }

    #[test]
    fn test_normalize_address() {
        assert_eq!(
            normalize_address("0xABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("ABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("  0x2  "),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_address_short() {
        assert_eq!(normalize_address_short("0x2"), "0x2");
        assert_eq!(
            normalize_address_short(
                "0x0000000000000000000000000000000000000000000000000000000000000002"
            ),
            "0x2"
        );
        assert_eq!(normalize_address_short("0x0"), "0x0");
    }
}
