//! Sui Resolver
//!
//! Package upgrade resolution and address normalization for Sui.
//!
//! This crate provides:
//! - [`address`]: Address normalization utilities
//! - [`linkage`]: Linkage table handling for package upgrades
//! - [`package_upgrades`]: Bidirectional mapping between original and upgraded package addresses
//!
//! # Package Upgrade Resolution
//!
//! In Sui, packages can be upgraded. When this happens:
//! - The **original_id** (runtime_id) stays stable - this is how types reference the package
//! - The **storage_id** changes - this is where the actual bytecode lives
//!
//! This crate provides utilities to:
//! - Map storage_id → original_id (for normalizing addresses)
//! - Map original_id → storage_id (for fetching bytecode)
//! - Normalize StructTag addresses for dynamic field matching

pub mod address;
pub mod linkage;
pub mod package_upgrades;

// Re-export address utilities
pub use address::{
    address_to_string, is_framework_account_address, is_framework_address, normalize_address,
    normalize_address_checked, normalize_address_short, normalize_id, normalize_id_short,
    parse_address, FRAMEWORK_ADDRESSES,
};
pub use linkage::{extract_linkage_map, extract_linkage_with_versions};
pub use package_upgrades::PackageUpgradeResolver;
