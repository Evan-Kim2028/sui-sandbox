//! Address normalization utilities.
//!
//! Provides consistent address formatting across all cache operations.
//! All Sui addresses are normalized to:
//! - 64 hex characters (32 bytes)
//! - Lowercase
//! - With 0x prefix
//! - Left-padded with zeros

/// Normalize a Sui address to canonical format.
///
/// This ensures consistent lookups regardless of how addresses are formatted:
/// - "0x2" -> "0x0000000000000000000000000000000000000000000000000000000000000002"
/// - "2" -> "0x0000000000000000000000000000000000000000000000000000000000000002"
/// - "0X02" -> "0x0000000000000000000000000000000000000000000000000000000000000002"
///
/// # Examples
///
/// ```
/// use sui_sandbox::cache::normalize_address;
///
/// assert_eq!(
///     normalize_address("0x2"),
///     "0x0000000000000000000000000000000000000000000000000000000000000002"
/// );
/// ```
pub fn normalize_address(addr: &str) -> String {
    let addr = addr.trim().to_lowercase();
    let addr = addr.strip_prefix("0x").unwrap_or(&addr);

    // Pad to 64 characters (32 bytes)
    let padded = format!("{:0>64}", addr);
    format!("0x{}", padded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_short_address() {
        assert_eq!(
            normalize_address("0x2"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_no_prefix() {
        assert_eq!(
            normalize_address("2"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_full_length() {
        let full = "0x0000000000000000000000000000000000000000000000000000000000000002";
        assert_eq!(normalize_address(full), full);
    }

    #[test]
    fn test_normalize_uppercase() {
        assert_eq!(
            normalize_address("0X02"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_whitespace() {
        assert_eq!(
            normalize_address("  0x2  "),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_mixed_case() {
        assert_eq!(
            normalize_address("0xDeEb7A4662eec9F2f3def03Fb937a663dddAa2e215b8078A284d026b7946c270"),
            "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270"
        );
    }
}
