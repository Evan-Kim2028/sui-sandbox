//! Encoding and address utilities used across the workspace.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use sui_resolver::address::parse_address as parse_address_checked;

// =============================================================================
// Hex Parsing
// =============================================================================

/// Parse a hex string to an `AccountAddress` with context-aware error message.
///
/// # Arguments
/// * `hex_str` - Hex string (with or without 0x prefix)
/// * `context` - Description for error messages (e.g., "object ID", "package address")
pub fn parse_address(hex_str: &str, context: &str) -> Result<AccountAddress> {
    parse_address_checked(hex_str).ok_or_else(|| anyhow!("Invalid {} '{}'", context, hex_str))
}

/// Parse a hex string to an `AccountAddress`, returning `None` on failure.
///
/// Use this when the address is optional or when you want to handle the error
/// separately.
pub fn try_parse_address(hex_str: &str) -> Option<AccountAddress> {
    parse_address_checked(hex_str)
}

/// Parse a hex string to raw bytes.
///
/// # Arguments
/// * `hex_str` - Hex string (with or without `0x` prefix)
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

/// Decode base64 string to bytes, returning `None` on failure.
pub fn try_base64_decode(b64: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

// =============================================================================
// Address Formatting & Normalization
// =============================================================================

/// Normalize an address string to lowercase with 0x prefix and full 64 hex characters.
///
/// Canonical internal format for internal comparisons and storage.
pub use sui_resolver::address::normalize_address;

/// Normalize an address string, returning `None` if it's not valid hex.
pub use sui_resolver::address::normalize_address_checked;

/// Normalize an address string to short form (minimal hex digits).
pub use sui_resolver::address::normalize_address_short;

/// Format an address to a full-length `0x` + 64-hex representation.
pub use sui_resolver::address::address_to_string;

/// Format an `AccountAddress` as a full 66-character hex string (`0x` + 64 hex chars).
#[inline]
pub fn format_address_full(addr: &AccountAddress) -> String {
    address_to_string(addr)
}

/// Format an `AccountAddress` as a short hex string (`0x2` instead of
/// `0x0000...0002`).
#[inline]
pub fn format_address_short(addr: &AccountAddress) -> String {
    normalize_address_short(&address_to_string(addr))
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
        assert_eq!(format_address_short(&addr), "0x2");

        let addr = AccountAddress::from_hex_literal("0x0").unwrap();
        assert_eq!(format_address_short(&addr), "0x0");
    }

    #[test]
    fn test_format_address_full() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        let full = format_address_full(&addr);
        assert_eq!(full.len(), 66);
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
