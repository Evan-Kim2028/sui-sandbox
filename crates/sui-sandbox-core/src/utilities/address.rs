//! Address normalization utilities.
//!
//! This module re-exports address utilities from `sui_resolver`.
//! All address normalization should use these canonical functions.

// Re-export canonical address utilities from sui-resolver
pub use sui_resolver::{
    address_to_string, is_framework_account_address, is_framework_address, normalize_address,
    normalize_address_checked, normalize_address_short, parse_address, FRAMEWORK_ADDRESSES,
};

/// Check if a package ID is a framework package (0x1, 0x2, 0x3).
///
/// This is an alias for [`is_framework_address`] for backward compatibility.
///
/// Framework packages are bundled with the VM and don't need to be fetched
/// from the network. This function handles both short and full address formats.
///
/// # Examples
///
/// ```
/// use sui_sandbox_core::utilities::is_framework_package;
///
/// assert!(is_framework_package("0x1"));
/// assert!(is_framework_package("0x2"));
/// assert!(is_framework_package("0x0000000000000000000000000000000000000000000000000000000000000002"));
/// assert!(!is_framework_package("0x1234"));
/// ```
pub fn is_framework_package(pkg_id: &str) -> bool {
    is_framework_address(pkg_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_address_short() {
        assert_eq!(
            normalize_address("0x2"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_address_medium() {
        assert_eq!(
            normalize_address("0x3637"),
            "0x0000000000000000000000000000000000000000000000000000000000003637"
        );
    }

    #[test]
    fn test_normalize_address_full() {
        let full = "0x0000000000000000000000000000000000000000000000000000000000000002";
        assert_eq!(normalize_address(full), full);
    }

    #[test]
    fn test_normalize_address_no_prefix() {
        assert_eq!(
            normalize_address("2"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_is_framework_package() {
        assert!(is_framework_package("0x1"));
        assert!(is_framework_package("0x2"));
        assert!(is_framework_package("0x3"));
        assert!(is_framework_package(
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        ));
        assert!(!is_framework_package("0x4"));
        assert!(!is_framework_package("0x1234abcd"));
    }
}
