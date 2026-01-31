//! Shared address parsing utilities.
//!
//! This module provides unified address parsing that combines normalization
//! and parsing into a single step, reducing boilerplate across the codebase.

use anyhow::{Context, Result};
use move_core_types::account_address::AccountAddress;

// Re-export the underlying normalize function for convenience
pub use sui_resolver::normalize_address;

/// Parse an address string into an AccountAddress.
///
/// This combines normalization (padding short addresses) and parsing in one step.
/// Handles addresses with or without "0x" prefix, short addresses like "0x2",
/// and full 64-character addresses.
///
/// # Example
/// ```
/// use sui_sandbox_core::shared::address::parse_address;
///
/// let addr = parse_address("0x2").unwrap();
/// assert_eq!(addr, move_core_types::account_address::AccountAddress::from_hex_literal("0x2").unwrap());
///
/// let addr = parse_address("2").unwrap();
/// assert_eq!(addr, move_core_types::account_address::AccountAddress::from_hex_literal("0x2").unwrap());
/// ```
pub fn parse_address(s: &str) -> Result<AccountAddress> {
    let normalized = normalize_address(s);
    AccountAddress::from_hex_literal(&normalized).context("Invalid address format")
}

/// Parse an address string, returning a default on failure.
///
/// Useful for optional address fields where a default value is acceptable.
pub fn parse_address_or(s: &str, default: AccountAddress) -> AccountAddress {
    parse_address(s).unwrap_or(default)
}

/// Parse an address string, returning ZERO address on failure.
pub fn parse_address_or_zero(s: &str) -> AccountAddress {
    parse_address_or(s, AccountAddress::ZERO)
}

/// Try to parse an address, returning None on failure.
pub fn try_parse_address(s: &str) -> Option<AccountAddress> {
    parse_address(s).ok()
}

/// Parse multiple addresses from an iterator of strings.
///
/// Returns an error if any address fails to parse.
pub fn parse_addresses<'a, I>(addrs: I) -> Result<Vec<AccountAddress>>
where
    I: IntoIterator<Item = &'a str>,
{
    addrs
        .into_iter()
        .map(parse_address)
        .collect::<Result<Vec<_>>>()
}

/// Format an AccountAddress to its canonical hex string representation.
///
/// Always includes the 0x prefix and full 64-character padding.
pub fn format_address(addr: &AccountAddress) -> String {
    addr.to_hex_literal()
}

/// Format an address in short form (removes leading zeros after 0x).
///
/// Uses the standard Sui display format.
pub fn format_address_short(addr: &AccountAddress) -> String {
    let full = addr.to_hex_literal();
    let without_prefix = full.strip_prefix("0x").unwrap_or(&full);
    let trimmed = without_prefix.trim_start_matches('0');
    if trimmed.is_empty() {
        "0x0".to_string()
    } else {
        format!("0x{}", trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_address_short() {
        let addr = parse_address("0x2").unwrap();
        assert_eq!(addr, AccountAddress::from_hex_literal("0x2").unwrap());
    }

    #[test]
    fn test_parse_address_no_prefix() {
        let addr = parse_address("2").unwrap();
        assert_eq!(addr, AccountAddress::from_hex_literal("0x2").unwrap());
    }

    #[test]
    fn test_parse_address_full() {
        let full = "0x0000000000000000000000000000000000000000000000000000000000000002";
        let addr = parse_address(full).unwrap();
        assert_eq!(addr, AccountAddress::from_hex_literal("0x2").unwrap());
    }

    #[test]
    fn test_parse_address_or_zero() {
        let addr = parse_address_or_zero("invalid");
        assert_eq!(addr, AccountAddress::ZERO);

        let addr = parse_address_or_zero("0x123");
        assert_ne!(addr, AccountAddress::ZERO);
    }

    #[test]
    fn test_try_parse_address() {
        assert!(try_parse_address("0x2").is_some());
        assert!(try_parse_address("invalid").is_none());
    }

    #[test]
    fn test_parse_addresses() {
        let addrs = parse_addresses(["0x1", "0x2", "0x3"]).unwrap();
        assert_eq!(addrs.len(), 3);
    }

    #[test]
    fn test_format_address_short() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert_eq!(format_address_short(&addr), "0x2");

        let addr = AccountAddress::ZERO;
        assert_eq!(format_address_short(&addr), "0x0");

        let addr = AccountAddress::from_hex_literal("0x123456").unwrap();
        assert_eq!(format_address_short(&addr), "0x123456");
    }
}
