//! Shared encoding utilities.
//!
//! This module provides unified base64 encoding/decoding helpers that are used
//! across the codebase. Using these helpers ensures consistent behavior and
//! reduces boilerplate.

use anyhow::{Context, Result};
use base64::Engine;

/// Encode bytes to a base64 string using standard encoding.
///
/// # Example
/// ```
/// use sui_sandbox_core::shared::encoding::encode_b64;
///
/// let encoded = encode_b64(b"hello world");
/// assert_eq!(encoded, "aGVsbG8gd29ybGQ=");
/// ```
pub fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decode a base64 string to bytes using standard encoding.
///
/// # Example
/// ```
/// use sui_sandbox_core::shared::encoding::decode_b64;
///
/// let bytes = decode_b64("aGVsbG8gd29ybGQ=").unwrap();
/// assert_eq!(bytes, b"hello world");
/// ```
pub fn decode_b64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .context("Invalid base64 string")
}

/// Decode a base64 string, returning None on failure instead of an error.
///
/// Useful for optional fields where invalid base64 should be silently ignored.
pub fn decode_b64_opt(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// Decode a base64 string without padding (STANDARD_NO_PAD).
pub fn decode_b64_no_pad(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s)
        .context("Invalid base64 string (no padding)")
}

/// Decode a base64 string, returning None on failure (no padding variant).
pub fn decode_b64_no_pad_opt(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s)
        .ok()
}

/// Encode bytes to a URL-safe base64 string (no padding).
pub fn encode_b64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode a URL-safe base64 string.
pub fn decode_b64_url(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .context("Invalid URL-safe base64 string")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_b64() {
        assert_eq!(encode_b64(b"hello"), "aGVsbG8=");
        assert_eq!(encode_b64(b""), "");
        assert_eq!(encode_b64(&[0, 1, 2, 255]), "AAEC/w==");
    }

    #[test]
    fn test_decode_b64() {
        assert_eq!(decode_b64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_b64("").unwrap(), b"");
        assert_eq!(decode_b64("AAEC/w==").unwrap(), vec![0, 1, 2, 255]);
    }

    #[test]
    fn test_decode_b64_invalid() {
        assert!(decode_b64("not-valid-base64!!!").is_err());
    }

    #[test]
    fn test_decode_b64_opt() {
        assert_eq!(decode_b64_opt("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode_b64_opt("invalid!!!"), None);
    }

    #[test]
    fn test_url_safe_encoding() {
        // URL-safe encoding uses - and _ instead of + and /
        let bytes = vec![251, 255, 254]; // Would have + and / in standard
        let encoded = encode_b64_url(&bytes);
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert_eq!(decode_b64_url(&encoded).unwrap(), bytes);
    }
}
