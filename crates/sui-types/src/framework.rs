//! Sui framework constants and well-known addresses.
//!
//! This module provides compile-time constants for framework addresses and
//! commonly used object IDs, eliminating the need for repeated
//! `AccountAddress::from_hex_literal("0x1").unwrap()` calls throughout the codebase.
//!
//! # Example
//!
//! ```
//! use sui_sandbox_types::framework::{MOVE_STDLIB, SUI_FRAMEWORK, SUI_SYSTEM};
//!
//! // Instead of: AccountAddress::from_hex_literal("0x1").unwrap()
//! let stdlib = MOVE_STDLIB;
//!
//! // Check if an address is a framework package
//! use sui_sandbox_types::framework::is_framework_address;
//! assert!(is_framework_address(&MOVE_STDLIB));
//! ```

use move_core_types::account_address::AccountAddress;

// ============================================================================
// Framework Package Addresses
// ============================================================================

/// Move standard library address (0x1)
pub const MOVE_STDLIB: AccountAddress = AccountAddress::ONE;

/// Sui framework address (0x2)
pub const SUI_FRAMEWORK: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[31] = 2;
    AccountAddress::new(bytes)
};

/// Sui system address (0x3)
pub const SUI_SYSTEM: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[31] = 3;
    AccountAddress::new(bytes)
};

/// Deepbook address (0xdee9)
pub const DEEPBOOK: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[30] = 0xde;
    bytes[31] = 0xe9;
    AccountAddress::new(bytes)
};

/// Sui bridge address (0xb)
pub const SUI_BRIDGE: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[31] = 0xb;
    AccountAddress::new(bytes)
};

// ============================================================================
// Well-Known Object IDs
// ============================================================================

/// Clock object ID (0x6)
pub const CLOCK_OBJECT_ID: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[31] = 6;
    AccountAddress::new(bytes)
};

/// Random object ID (0x8)
pub const RANDOM_OBJECT_ID: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[31] = 8;
    AccountAddress::new(bytes)
};

/// Deny list object ID (0x403)
pub const DENY_LIST_OBJECT_ID: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[30] = 0x04;
    bytes[31] = 0x03;
    AccountAddress::new(bytes)
};

/// System state object ID (0x5)
pub const SYSTEM_STATE_OBJECT_ID: AccountAddress = {
    let mut bytes = [0u8; 32];
    bytes[31] = 5;
    AccountAddress::new(bytes)
};

// ============================================================================
// Framework Address Utilities
// ============================================================================

/// All framework package addresses (0x1, 0x2, 0x3)
pub const FRAMEWORK_ADDRESSES: [AccountAddress; 3] = [MOVE_STDLIB, SUI_FRAMEWORK, SUI_SYSTEM];

/// Check if an address is a framework package (0x1, 0x2, or 0x3)
#[inline]
pub fn is_framework_address(addr: &AccountAddress) -> bool {
    *addr == MOVE_STDLIB || *addr == SUI_FRAMEWORK || *addr == SUI_SYSTEM
}

/// Check if an address is a system object (clock, random, etc.)
#[inline]
pub fn is_system_object(addr: &AccountAddress) -> bool {
    *addr == CLOCK_OBJECT_ID
        || *addr == RANDOM_OBJECT_ID
        || *addr == DENY_LIST_OBJECT_ID
        || *addr == SYSTEM_STATE_OBJECT_ID
}

// ============================================================================
// String Constants (for parsing/display)
// ============================================================================

/// Clock object ID as hex string
pub const CLOCK_OBJECT_ID_STR: &str = "0x6";

/// Random object ID as hex string
pub const RANDOM_OBJECT_ID_STR: &str = "0x8";

/// Deny list object ID as hex string
pub const DENY_LIST_OBJECT_ID_STR: &str = "0x403";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framework_addresses() {
        assert_eq!(
            MOVE_STDLIB,
            AccountAddress::from_hex_literal("0x1").unwrap()
        );
        assert_eq!(
            SUI_FRAMEWORK,
            AccountAddress::from_hex_literal("0x2").unwrap()
        );
        assert_eq!(
            SUI_SYSTEM,
            AccountAddress::from_hex_literal("0x3").unwrap()
        );
    }

    #[test]
    fn test_well_known_objects() {
        assert_eq!(
            CLOCK_OBJECT_ID,
            AccountAddress::from_hex_literal("0x6").unwrap()
        );
        assert_eq!(
            RANDOM_OBJECT_ID,
            AccountAddress::from_hex_literal("0x8").unwrap()
        );
        assert_eq!(
            DENY_LIST_OBJECT_ID,
            AccountAddress::from_hex_literal("0x403").unwrap()
        );
    }

    #[test]
    fn test_is_framework_address() {
        assert!(is_framework_address(&MOVE_STDLIB));
        assert!(is_framework_address(&SUI_FRAMEWORK));
        assert!(is_framework_address(&SUI_SYSTEM));
        assert!(!is_framework_address(&CLOCK_OBJECT_ID));
        assert!(!is_framework_address(&AccountAddress::ZERO));
    }

    #[test]
    fn test_is_system_object() {
        assert!(is_system_object(&CLOCK_OBJECT_ID));
        assert!(is_system_object(&RANDOM_OBJECT_ID));
        assert!(!is_system_object(&MOVE_STDLIB));
        assert!(!is_system_object(&AccountAddress::ZERO));
    }

    #[test]
    fn test_deepbook_address() {
        assert_eq!(
            DEEPBOOK,
            AccountAddress::from_hex_literal("0xdee9").unwrap()
        );
    }
}
