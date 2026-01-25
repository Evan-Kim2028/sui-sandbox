//! Address normalization utilities.
//!
//! Sui addresses are 32-byte values, but they're often represented in different formats:
//! - Short form: "0x2"
//! - Full form: "0x0000000000000000000000000000000000000000000000000000000000000002"
//! - Without prefix: "2"
//!
//! This module provides utilities to normalize addresses to a consistent format.

/// Normalize an address to lowercase with 0x prefix and full 64 hex characters.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::normalize_address;
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
    let hex = addr.strip_prefix("0x").unwrap_or(addr).to_lowercase();
    if hex.len() < 64 {
        format!("0x{:0>64}", hex)
    } else {
        format!("0x{}", hex)
    }
}

/// Check if an address is a framework address (0x1, 0x2, 0x3).
///
/// Framework packages are always available and don't need to be fetched.
pub fn is_framework_address(addr: &str) -> bool {
    let normalized = normalize_address(addr);
    normalized == "0x0000000000000000000000000000000000000000000000000000000000000001"
        || normalized == "0x0000000000000000000000000000000000000000000000000000000000000002"
        || normalized == "0x0000000000000000000000000000000000000000000000000000000000000003"
        || addr == "0x1"
        || addr == "0x2"
        || addr == "0x3"
}

#[cfg(test)]
mod tests {
    use super::*;

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
            normalize_address("0x0000000000000000000000000000000000000000000000000000000000000002"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_is_framework_address() {
        assert!(is_framework_address("0x1"));
        assert!(is_framework_address("0x2"));
        assert!(is_framework_address("0x3"));
        assert!(is_framework_address(
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        ));
        assert!(!is_framework_address("0x4"));
        assert!(!is_framework_address("0xabc"));
    }
}
