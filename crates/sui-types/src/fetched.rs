//! Unified types for fetched blockchain data.
//!
//! This module provides canonical definitions for data fetched from the Sui network.
//! These types are used across multiple crates to avoid duplication:
//! - `sui-prefetch`: Data prefetching strategies
//! - `sui-sandbox-core`: VM execution and simulation
//! - `sui-state-fetcher`: Historical state provider
//!
//! ## Design Principles
//!
//! 1. **String IDs for JSON compatibility**: Object and package IDs use `String` rather than
//!    `AccountAddress` to simplify JSON serialization and avoid hex parsing at boundaries.
//!
//! 2. **Optional fields for flexibility**: Fields like `digest` and `original_id` are optional
//!    since not all fetch paths provide them.
//!
//! 3. **BCS bytes are canonical**: The `bcs_bytes` field contains the authoritative object data.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use move_core_types::account_address::AccountAddress;

use crate::encoding::normalize_address;

/// Object ID type (32-byte address).
///
/// This is the canonical ObjectID type for the workspace. Other crates should
/// re-export this rather than defining their own.
pub type ObjectID = AccountAddress;

/// Fetched object data from the Sui network.
///
/// This is the unified type for object data, combining fields from:
/// - `sui_sandbox_types::FetchedObject` (transaction.rs)
/// - `sui_prefetch::FetchedObject` (eager_prefetch.rs)
/// - `sui_sandbox_core::FetchedObjectData` (fetcher.rs)
/// - `sui_state_fetcher::VersionedObject` (types.rs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedObject {
    /// Object ID (hex string with 0x prefix).
    pub object_id: String,

    /// Object version (sequence number / lamport timestamp).
    pub version: u64,

    /// BCS-serialized object contents.
    ///
    /// For Move structs, this is the BCS encoding of the struct fields.
    /// For packages, this contains the package metadata (not module bytecode).
    #[serde(with = "crate::transaction::base64_bytes")]
    pub bcs_bytes: Vec<u8>,

    /// Move type tag (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    ///
    /// None for packages or when type information is unavailable.
    pub type_string: Option<String>,

    /// Whether this object is shared.
    #[serde(default)]
    pub is_shared: bool,

    /// Whether this object is immutable.
    #[serde(default)]
    pub is_immutable: bool,

    /// Object digest (base58 encoded, for verification).
    ///
    /// Optional because not all fetch paths provide digests.
    #[serde(default)]
    pub digest: Option<String>,
}

impl FetchedObject {
    /// Create a new FetchedObject with minimal required fields.
    pub fn new(object_id: String, version: u64, bcs_bytes: Vec<u8>) -> Self {
        Self {
            object_id,
            version,
            bcs_bytes,
            type_string: None,
            is_shared: false,
            is_immutable: false,
            digest: None,
        }
    }

    /// Builder: set type string.
    pub fn with_type(mut self, type_string: impl Into<String>) -> Self {
        self.type_string = Some(type_string.into());
        self
    }

    /// Builder: mark as shared.
    pub fn shared(mut self) -> Self {
        self.is_shared = true;
        self
    }

    /// Builder: mark as immutable.
    pub fn immutable(mut self) -> Self {
        self.is_immutable = true;
        self
    }

    /// Builder: set digest.
    pub fn with_digest(mut self, digest: impl Into<String>) -> Self {
        self.digest = Some(digest.into());
        self
    }

    /// Parse object ID as AccountAddress.
    ///
    /// Returns None if the object_id is not a valid hex address.
    pub fn object_id_as_address(&self) -> Option<AccountAddress> {
        AccountAddress::from_hex_literal(&self.object_id).ok()
    }

    /// Create a cache key for this object: (normalized_id, version).
    pub fn cache_key(&self) -> (String, u64) {
        (normalize_address(&self.object_id), self.version)
    }
}

/// Fetched package data from the Sui network.
///
/// This is the unified type for package data, combining fields from:
/// - `sui_prefetch::FetchedPackage` (eager_prefetch.rs)
/// - `sui_state_fetcher::PackageData` (types.rs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedPackage {
    /// Package ID / storage address (hex string with 0x prefix).
    ///
    /// For upgraded packages, this is the storage_id where bytecode lives.
    pub package_id: String,

    /// Package version.
    pub version: u64,

    /// Module bytecode: (module_name, bytecode).
    ///
    /// Each tuple contains the module name and its compiled Move bytecode.
    #[serde(default)]
    pub modules: Vec<(String, Vec<u8>)>,

    /// Linkage table for package upgrades.
    ///
    /// Maps original_package_id -> upgraded_package_id for dependencies.
    /// Uses String keys for JSON compatibility.
    #[serde(default)]
    pub linkage: HashMap<String, String>,

    /// Original package ID (runtime_id) for upgraded packages.
    ///
    /// If Some, this package is an upgrade and types should reference
    /// the original_id, not the storage package_id.
    /// If None, this package has never been upgraded (original == storage).
    #[serde(default)]
    pub original_id: Option<String>,
}

impl FetchedPackage {
    /// Create a new FetchedPackage with minimal required fields.
    pub fn new(package_id: String, version: u64) -> Self {
        Self {
            package_id,
            version,
            modules: Vec::new(),
            linkage: HashMap::new(),
            original_id: None,
        }
    }

    /// Builder: add modules.
    pub fn with_modules(mut self, modules: Vec<(String, Vec<u8>)>) -> Self {
        self.modules = modules;
        self
    }

    /// Builder: add a single module.
    pub fn add_module(mut self, name: impl Into<String>, bytecode: Vec<u8>) -> Self {
        self.modules.push((name.into(), bytecode));
        self
    }

    /// Builder: set linkage table.
    pub fn with_linkage(mut self, linkage: HashMap<String, String>) -> Self {
        self.linkage = linkage;
        self
    }

    /// Builder: set original ID.
    pub fn with_original_id(mut self, original_id: impl Into<String>) -> Self {
        self.original_id = Some(original_id.into());
        self
    }

    /// Get the runtime ID for this package (used in type tags).
    ///
    /// For upgraded packages, returns the original_id.
    /// For non-upgraded packages, returns the package_id.
    pub fn runtime_id(&self) -> &str {
        self.original_id.as_deref().unwrap_or(&self.package_id)
    }

    /// Parse package ID as AccountAddress.
    pub fn package_id_as_address(&self) -> Option<AccountAddress> {
        AccountAddress::from_hex_literal(&self.package_id).ok()
    }

    /// Parse original ID as AccountAddress (if present).
    pub fn original_id_as_address(&self) -> Option<AccountAddress> {
        self.original_id
            .as_ref()
            .and_then(|id| AccountAddress::from_hex_literal(id).ok())
    }

    /// Get linkage as AccountAddress map (for module resolver).
    pub fn linkage_as_addresses(&self) -> HashMap<AccountAddress, AccountAddress> {
        self.linkage
            .iter()
            .filter_map(|(k, v)| {
                let k_addr = AccountAddress::from_hex_literal(k).ok()?;
                let v_addr = AccountAddress::from_hex_literal(v).ok()?;
                Some((k_addr, v_addr))
            })
            .collect()
    }
}

// normalize_address is defined in crate::encoding

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetched_object_builder() {
        let obj = FetchedObject::new("0x123".to_string(), 42, vec![1, 2, 3])
            .with_type("0x2::coin::Coin<0x2::sui::SUI>")
            .shared()
            .with_digest("abc123");

        assert_eq!(obj.object_id, "0x123");
        assert_eq!(obj.version, 42);
        assert_eq!(obj.bcs_bytes, vec![1, 2, 3]);
        assert_eq!(
            obj.type_string,
            Some("0x2::coin::Coin<0x2::sui::SUI>".to_string())
        );
        assert!(obj.is_shared);
        assert!(!obj.is_immutable);
        assert_eq!(obj.digest, Some("abc123".to_string()));
    }

    #[test]
    fn test_fetched_package_builder() {
        let pkg = FetchedPackage::new("0xabc".to_string(), 1)
            .add_module("coin", vec![0x01, 0x02])
            .add_module("sui", vec![0x03, 0x04])
            .with_original_id("0xdef");

        assert_eq!(pkg.package_id, "0xabc");
        assert_eq!(pkg.version, 1);
        assert_eq!(pkg.modules.len(), 2);
        assert_eq!(pkg.runtime_id(), "0xdef");
    }

    #[test]
    fn test_fetched_package_runtime_id() {
        // Non-upgraded package
        let pkg1 = FetchedPackage::new("0x2".to_string(), 1);
        assert_eq!(pkg1.runtime_id(), "0x2");

        // Upgraded package
        let pkg2 = FetchedPackage::new("0xabc".to_string(), 2).with_original_id("0x2");
        assert_eq!(pkg2.runtime_id(), "0x2");
    }

    #[test]
    fn test_normalize_address() {
        // normalize_address now returns full 64-char padded form
        assert_eq!(
            normalize_address("0xABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("0XABC"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("abc"),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
        assert_eq!(
            normalize_address("  0xABC  "),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
    }
}
