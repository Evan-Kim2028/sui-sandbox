//! Core types for historical state fetching.
//!
//! These types represent the data needed to replay a Sui transaction locally.

use std::collections::HashMap;

use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use sui_sandbox_types::FetchedTransaction;

/// Object ID type (32-byte address).
pub type ObjectID = AccountAddress;

/// Everything needed to replay a single transaction.
///
/// This is the main output of [`HistoricalStateProvider::fetch_replay_state`].
#[derive(Debug, Clone)]
pub struct ReplayState {
    /// The transaction to replay (commands, inputs, sender, gas).
    pub transaction: FetchedTransaction,

    /// Objects at their input versions (before the tx modified them).
    /// Keyed by object ID for fast lookup.
    pub objects: HashMap<ObjectID, VersionedObject>,

    /// Packages with their modules and linkage tables.
    /// Keyed by package address.
    pub packages: HashMap<AccountAddress, PackageData>,

    /// Protocol version for the epoch this transaction executed in.
    pub protocol_version: u64,

    /// Epoch number.
    pub epoch: u64,

    /// Checkpoint that included this transaction.
    pub checkpoint: Option<u64>,
}

/// Object data with version information for cache keying.
///
/// The key insight: for replay, we need objects at their *exact* historical versions,
/// not their current state. The cache must key by `(object_id, version)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedObject {
    /// Object ID (32-byte address).
    pub id: ObjectID,

    /// Object version (sequence number).
    pub version: u64,

    /// Object digest (for verification).
    pub digest: Option<String>,

    /// Move type tag (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    pub type_tag: Option<String>,

    /// BCS-serialized object contents.
    pub bcs_bytes: Vec<u8>,

    /// Whether this object is shared.
    pub is_shared: bool,

    /// Whether this object is immutable.
    pub is_immutable: bool,
}

impl VersionedObject {
    /// Create a cache key for this object: (id, version).
    pub fn cache_key(&self) -> (ObjectID, u64) {
        (self.id, self.version)
    }
}

/// Package data with modules and linkage table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageData {
    /// Package address (storage ID for upgraded packages).
    pub address: AccountAddress,

    /// Package version.
    pub version: u64,

    /// Module bytecode: (module_name, bytecode).
    pub modules: Vec<(String, Vec<u8>)>,

    /// Linkage table for package upgrades.
    /// Maps runtime_id -> storage_id for dependencies.
    pub linkage: HashMap<AccountAddress, AccountAddress>,

    /// Original package ID (for upgraded packages, this is the runtime_id).
    /// If None, this package has never been upgraded.
    pub original_id: Option<AccountAddress>,
}

impl PackageData {
    /// Get the runtime ID for this package (used in type tags).
    pub fn runtime_id(&self) -> AccountAddress {
        self.original_id.unwrap_or(self.address)
    }
}

/// Statistics about a fetch operation.
#[derive(Debug, Clone, Default)]
pub struct FetchStats {
    /// Number of objects requested.
    pub objects_requested: usize,

    /// Number of objects successfully fetched.
    pub objects_fetched: usize,

    /// Number of objects found in cache.
    pub objects_cached: usize,

    /// Number of packages requested.
    pub packages_requested: usize,

    /// Number of packages successfully fetched.
    pub packages_fetched: usize,

    /// Number of packages found in cache.
    pub packages_cached: usize,

    /// Total fetch time in milliseconds.
    pub fetch_time_ms: u64,

    /// Errors encountered (object_id/package_id, error message).
    pub errors: Vec<(String, String)>,
}

impl FetchStats {
    /// Check if all requested data was successfully fetched.
    pub fn is_complete(&self) -> bool {
        self.objects_fetched + self.objects_cached == self.objects_requested
            && self.packages_fetched + self.packages_cached == self.packages_requested
    }

    /// Get cache hit rate for objects (0.0 to 1.0).
    pub fn object_cache_hit_rate(&self) -> f64 {
        if self.objects_requested == 0 {
            0.0
        } else {
            self.objects_cached as f64 / self.objects_requested as f64
        }
    }
}
