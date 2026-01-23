//! Address normalization utilities.
//!
//! Sui addresses can appear in different formats:
//! - Short form: `0x2`, `0x3637`
//! - Full form: `0x0000000000000000000000000000000000000000000000000000000000000002`
//!
//! This inconsistency causes issues with HashMap lookups and address comparisons.
//! These utilities normalize addresses to a consistent format.

/// Normalize a Sui address to a consistent 66-character format (0x + 64 hex chars).
///
/// Sui addresses can appear in shortened form (0x2) or full form
/// (0x0000...0002). This function ensures consistent formatting for
/// HashMap key lookups and address comparisons.
///
/// # Examples
///
/// ```
/// use sui_sandbox_core::utilities::normalize_address;
///
/// assert_eq!(
///     normalize_address("0x2"),
///     "0x0000000000000000000000000000000000000000000000000000000000000002"
/// );
/// assert_eq!(
///     normalize_address("0x3637"),
///     "0x0000000000000000000000000000000000000000000000000000000000003637"
/// );
/// ```
pub fn normalize_address(addr: &str) -> String {
    let addr = addr.strip_prefix("0x").unwrap_or(addr);
    format!("0x{:0>64}", addr)
}

/// Check if a package ID is a framework package (0x1, 0x2, 0x3).
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
    matches!(
        pkg_id,
        "0x0000000000000000000000000000000000000000000000000000000000000001"
            | "0x0000000000000000000000000000000000000000000000000000000000000002"
            | "0x0000000000000000000000000000000000000000000000000000000000000003"
            | "0x1"
            | "0x2"
            | "0x3"
    )
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
