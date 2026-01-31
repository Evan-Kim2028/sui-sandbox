//! Address normalization utilities.
//!
//! This module is the canonical source for address normalization in the workspace.
//! Other crates should import from here rather than defining their own logic.
//!
//! Sui addresses are 32-byte values, but they're often represented in different formats:
//! - Short form: "0x2"
//! - Full form: "0x0000000000000000000000000000000000000000000000000000000000000002"
//! - Without prefix: "2"
//!
//! This module provides utilities to normalize addresses to a consistent format.

use move_core_types::account_address::AccountAddress;

/// Framework package addresses (move-stdlib, sui-framework, sui-system).
pub const FRAMEWORK_ADDRESSES: [&str; 3] = [
    "0x0000000000000000000000000000000000000000000000000000000000000001",
    "0x0000000000000000000000000000000000000000000000000000000000000002",
    "0x0000000000000000000000000000000000000000000000000000000000000003",
];

/// Normalize an address to lowercase with 0x prefix and full 64 hex characters.
///
/// This is the canonical address format for internal use and comparisons.
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

/// Normalize an address, returning None if it's not a valid hex address.
///
/// This validates the address by parsing it as an AccountAddress.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::normalize_address_checked;
///
/// assert_eq!(
///     normalize_address_checked("0x2"),
///     Some("0x0000000000000000000000000000000000000000000000000000000000000002".to_string())
/// );
/// assert_eq!(normalize_address_checked("not-hex"), None);
/// ```
pub fn normalize_address_checked(addr: &str) -> Option<String> {
    let normalized = normalize_address(addr);
    // Validate by attempting to parse as AccountAddress
    AccountAddress::from_hex_literal(&normalized).ok()?;
    Some(normalized)
}

/// Normalize an address to short form (minimal hex digits).
///
/// Framework addresses (0x1, 0x2, 0x3) are kept in their short form.
/// Other addresses have leading zeros trimmed.
///
/// This is useful for display purposes.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::normalize_address_short;
///
/// assert_eq!(normalize_address_short("0x0000000000000000000000000000000000000000000000000000000000000002"), "0x2");
/// assert_eq!(normalize_address_short("0x00abc"), "0xabc");
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

/// Parse a string address into an AccountAddress.
///
/// Handles both short ("0x2") and full forms.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::parse_address;
///
/// let addr = parse_address("0x2").unwrap();
/// assert_eq!(addr.to_hex_literal(), "0x2");
/// ```
pub fn parse_address(addr: &str) -> Option<AccountAddress> {
    let normalized = normalize_address(addr);
    AccountAddress::from_hex_literal(&normalized).ok()
}

/// Convert an AccountAddress to its normalized full-form string.
///
/// # Examples
///
/// ```
/// use move_core_types::account_address::AccountAddress;
/// use sui_resolver::address::address_to_string;
///
/// let addr = AccountAddress::from_hex_literal("0x2").unwrap();
/// assert_eq!(
///     address_to_string(&addr),
///     "0x0000000000000000000000000000000000000000000000000000000000000002"
/// );
/// ```
pub fn address_to_string(addr: &AccountAddress) -> String {
    format!("0x{}", hex::encode(addr.as_ref()))
}

/// Check if an address is a framework address (0x1, 0x2, 0x3).
///
/// Framework packages are always available and don't need to be fetched.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::is_framework_address;
///
/// assert!(is_framework_address("0x1"));
/// assert!(is_framework_address("0x2"));
/// assert!(is_framework_address("0x3"));
/// assert!(!is_framework_address("0x4"));
/// ```
pub fn is_framework_address(addr: &str) -> bool {
    let normalized = normalize_address(addr);
    FRAMEWORK_ADDRESSES.contains(&normalized.as_str())
}

/// Check if an AccountAddress is a framework address (0x1, 0x2, 0x3).
///
/// # Examples
///
/// ```
/// use move_core_types::account_address::AccountAddress;
/// use sui_resolver::address::is_framework_account_address;
///
/// let addr = AccountAddress::from_hex_literal("0x2").unwrap();
/// assert!(is_framework_account_address(&addr));
/// ```
pub fn is_framework_account_address(addr: &AccountAddress) -> bool {
    let normalized = address_to_string(addr);
    FRAMEWORK_ADDRESSES.contains(&normalized.as_str())
}

// =============================================================================
// ID Normalization Aliases
// =============================================================================
// These are aliases for address normalization functions, provided for semantic
// clarity when working with object/package IDs rather than wallet addresses.
// Functionally, IDs and addresses are the same 32-byte hex values.

/// Normalize an object/package ID to canonical format (64 hex chars with 0x prefix).
///
/// This is an alias for [`normalize_address`] for semantic clarity when working
/// with object or package IDs.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::normalize_id;
///
/// assert_eq!(
///     normalize_id("0x2"),
///     "0x0000000000000000000000000000000000000000000000000000000000000002"
/// );
/// ```
#[inline]
pub fn normalize_id(id: &str) -> String {
    normalize_address(id)
}

/// Normalize an object/package ID to short form (minimal hex digits).
///
/// This is an alias for [`normalize_address_short`] for semantic clarity.
///
/// # Examples
///
/// ```
/// use sui_resolver::address::normalize_id_short;
///
/// assert_eq!(normalize_id_short("0x0000000000000000000000000000000000000000000000000000000000000002"), "0x2");
/// ```
#[inline]
pub fn normalize_id_short(id: &str) -> String {
    normalize_address_short(id)
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
        // Test whitespace trimming
        assert_eq!(
            normalize_address("  0x2  "),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        // Test uppercase 0X prefix
        assert_eq!(
            normalize_address("0XABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
    }

    #[test]
    fn test_normalize_address_checked() {
        assert!(normalize_address_checked("0x2").is_some());
        assert!(normalize_address_checked("not-hex").is_none());
        assert!(normalize_address_checked("0xGGG").is_none());
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
        assert_eq!(normalize_address_short("0x00abc"), "0xabc");
        assert_eq!(normalize_address_short("0x0"), "0x0");
        assert_eq!(
            normalize_address_short(
                "0x0000000000000000000000000000000000000000000000000000000000000000"
            ),
            "0x0"
        );
    }

    #[test]
    fn test_parse_address() {
        let addr = parse_address("0x2").unwrap();
        assert_eq!(addr.to_hex_literal(), "0x2");

        let addr = parse_address("0xabc").unwrap();
        assert_eq!(addr.to_hex_literal(), "0xabc");

        assert!(parse_address("not-hex").is_none());
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

    #[test]
    fn test_is_framework_account_address() {
        let addr1 = AccountAddress::from_hex_literal("0x1").unwrap();
        let addr2 = AccountAddress::from_hex_literal("0x2").unwrap();
        let addr3 = AccountAddress::from_hex_literal("0x3").unwrap();
        let addr4 = AccountAddress::from_hex_literal("0x4").unwrap();

        assert!(is_framework_account_address(&addr1));
        assert!(is_framework_account_address(&addr2));
        assert!(is_framework_account_address(&addr3));
        assert!(!is_framework_account_address(&addr4));
    }

    #[test]
    fn test_address_to_string() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert_eq!(
            address_to_string(&addr),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }
}
