//! Data fetching coordination for Walrus, gRPC, and GraphQL sources.
//!
//! Provides unified interfaces for:
//! - Object fetching (versioned, with fallback strategies)
//! - Package fetching (with linkage info)
//! - Batch pre-scanning for efficient prefetch
//! - Transaction object ingestion from Walrus JSON
//!
//! Note: Some types may appear unused until full migration is complete.

#![allow(dead_code)]

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;

use super::cache_layer::ObjectEntry;

// ============================================================================
// Version Map Types
// ============================================================================

/// Version information for a transaction's input objects.
#[derive(Debug, Clone, Default)]
pub struct TxVersionMap {
    /// Object ID (hex) -> Version
    pub versions: HashMap<String, u64>,
    /// Source of version information
    pub source: VersionMapSource,
}

impl TxVersionMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get version for an object ID.
    pub fn get(&self, id: &str) -> Option<u64> {
        self.versions.get(id).copied()
    }

    /// Get version for an AccountAddress.
    pub fn get_addr(&self, id: &AccountAddress) -> Option<u64> {
        self.versions.get(&id.to_hex_literal()).copied()
    }

    /// Insert a version.
    pub fn insert(&mut self, id: impl Into<String>, version: u64) {
        self.versions.insert(id.into(), version);
    }

    /// Merge another version map, preferring the other's values.
    pub fn merge(&mut self, other: &TxVersionMap) {
        for (k, v) in &other.versions {
            self.versions.insert(k.clone(), *v);
        }
        if other.source != VersionMapSource::WalrusJson {
            self.source = VersionMapSource::Combined;
        }
    }
}

/// Source of version information.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VersionMapSource {
    /// Versions extracted from Walrus JSON input_objects
    #[default]
    WalrusJson,
    /// Versions extracted from gRPC transaction effects
    GrpcTransaction,
    /// Combined from multiple sources
    Combined,
}

// ============================================================================
// Batch Prefetch Results
// ============================================================================

/// Results from batch pre-scan operation.
#[derive(Debug, Default)]
pub struct BatchPrefetchResult {
    /// Transaction digest -> version map
    pub tx_versions: HashMap<String, TxVersionMap>,
    /// Number of unique objects prefetched
    pub prefetched_objects: usize,
    /// Number of transactions scanned
    pub txs_scanned: usize,
    /// Number of transactions that had versions prefetched
    pub txs_prefetched: usize,
    /// Notes/warnings from the scan
    pub notes: Vec<String>,
}

impl BatchPrefetchResult {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a note.
    pub fn add_note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// Get version map for a transaction digest.
    pub fn get_versions(&self, digest: &str) -> Option<&TxVersionMap> {
        self.tx_versions.get(digest)
    }
}

// ============================================================================
// Fetched Package
// ============================================================================

/// Fetched package with modules and linkage info.
#[derive(Debug, Clone)]
pub struct FetchedPackage {
    /// Storage address of the package
    pub address: AccountAddress,
    /// Compiled module bytecode: (module_name, bytes)
    pub modules: Vec<(String, Vec<u8>)>,
    /// Package version
    pub version: u64,
    /// Original package ID (before upgrades)
    pub original_id: Option<AccountAddress>,
    /// Linkage information for dependencies
    pub linkage: Vec<LinkageEntry>,
}

/// Linkage entry describing a package dependency.
#[derive(Debug, Clone)]
pub struct LinkageEntry {
    /// Original/runtime package ID
    pub original_id: AccountAddress,
    /// Storage address of the upgraded package
    pub upgraded_id: AccountAddress,
    /// Version of the upgraded package
    pub upgraded_version: u64,
}

// ============================================================================
// Object Fetcher Trait
// ============================================================================

/// Trait for fetching historical object data.
#[async_trait::async_trait]
pub trait ObjectFetcher: Send + Sync {
    /// Fetch object at specific version from remote source.
    async fn fetch_object_at_version(
        &self,
        id: &str,
        version: Option<u64>,
    ) -> Result<Option<ObjectEntry>>;

    /// Batch fetch objects at versions.
    async fn batch_fetch_objects(
        &self,
        requests: &[(String, u64)],
    ) -> HashMap<String, Result<Option<ObjectEntry>>>;

    /// Extract version map from transaction JSON.
    fn extract_versions_from_tx(&self, tx_json: &serde_json::Value) -> TxVersionMap;
}

// ============================================================================
// Package Fetcher Trait
// ============================================================================

/// Trait for fetching package modules.
#[async_trait::async_trait]
pub trait PackageFetcher: Send + Sync {
    /// Fetch package modules at optional checkpoint.
    async fn fetch_package(
        &self,
        pkg: AccountAddress,
        checkpoint: Option<u64>,
    ) -> Result<Option<FetchedPackage>>;

    /// Get package upgrades history.
    async fn get_upgrade_history(&self, pkg: &str) -> Result<Vec<(String, u64)>>;
}

// ============================================================================
// Data Fetcher Coordinator Trait
// ============================================================================

/// Composite data fetcher coordinating multiple sources.
pub trait DataFetcherCoordinator: Send + Sync {
    /// Ingest objects from Walrus transaction JSON into cache.
    ///
    /// Returns the number of objects ingested.
    fn ingest_walrus_objects(
        &self,
        tx_json: &serde_json::Value,
        section: &str, // "input_objects" or "output_objects"
    ) -> Result<Vec<IngestedObject>>;

    /// Pre-scan batch of transactions to build version maps.
    fn pre_scan_batch(
        &self,
        txs: &[&serde_json::Value],
        max_ptbs: Option<usize>,
    ) -> BatchPrefetchResult;

    /// Ensure required packages are loaded.
    ///
    /// Returns list of newly loaded package addresses.
    fn ensure_packages_loaded(
        &self,
        package_ids: &[AccountAddress],
        checkpoint: Option<u64>,
    ) -> Result<Vec<AccountAddress>>;
}

/// Result of ingesting an object from Walrus JSON.
#[derive(Debug, Clone)]
pub struct IngestedObject {
    pub id: AccountAddress,
    pub version: u64,
    pub entry: ObjectEntry,
}

// ============================================================================
// Walrus JSON Helpers
// ============================================================================

/// Extract object ID from Walrus object JSON.
pub fn extract_object_id(obj: &serde_json::Value) -> Option<AccountAddress> {
    let id_str = obj.pointer("/data/Move/id")?.as_str()?;
    AccountAddress::from_hex_literal(id_str).ok()
}

/// Extract object version from Walrus object JSON.
pub fn extract_object_version(obj: &serde_json::Value) -> Option<u64> {
    obj.pointer("/data/Move/version")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
}

/// Extract BCS contents from Walrus object JSON.
pub fn extract_bcs_contents(obj: &serde_json::Value) -> Option<Vec<u8>> {
    let b64 = obj.pointer("/data/Move/contents")?.as_str()?;
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()
}

/// Extract type tag from Walrus object JSON.
pub fn extract_type_string(obj: &serde_json::Value) -> Option<String> {
    obj.pointer("/data/Move/type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Check if transaction is a PTB (ProgrammableTransaction).
pub fn is_ptb_transaction(tx_json: &serde_json::Value) -> bool {
    tx_json
        .pointer("/transaction/V1/txn_data/V1/kind")
        .map(|k| k.get("ProgrammableTransaction").is_some())
        .unwrap_or(false)
}

/// Extract transaction digest from JSON.
pub fn extract_tx_digest(tx_json: &serde_json::Value) -> Option<String> {
    tx_json
        .pointer("/digest")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract sender address from transaction JSON.
pub fn extract_sender(tx_json: &serde_json::Value) -> Option<AccountAddress> {
    let sender_str = tx_json
        .pointer("/transaction/V1/txn_data/V1/sender")
        .and_then(|v| v.as_str())?;
    AccountAddress::from_hex_literal(sender_str).ok()
}

/// Extract timestamp from transaction JSON.
pub fn extract_timestamp_ms(tx_json: &serde_json::Value) -> Option<u64> {
    tx_json
        .pointer("/timestamp_ms")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
}

// ============================================================================
// Version Extraction from Effects
// ============================================================================

/// Extract object versions from transaction effects.
pub fn extract_versions_from_effects(tx_json: &serde_json::Value) -> TxVersionMap {
    let mut map = TxVersionMap::new();
    map.source = VersionMapSource::WalrusJson;

    // Try changed_objects first
    if let Some(changed) = tx_json.pointer("/effects/V2/changed_objects") {
        if let Some(arr) = changed.as_array() {
            for item in arr {
                if let (Some(id), Some(input_state)) = (
                    item.get(0).and_then(|v| v.as_str()),
                    item.get(1).and_then(|v| v.get("input_state")),
                ) {
                    // input_state can be {"Exist": [[version, digest], owner]}
                    if let Some(exist) = input_state.get("Exist") {
                        if let Some(version_digest) = exist.get(0) {
                            if let Some(version) = version_digest.get(0).and_then(|v| v.as_str()) {
                                if let Ok(v) = version.parse::<u64>() {
                                    map.insert(id, v);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Also check unchanged_shared_objects
    if let Some(unchanged) = tx_json.pointer("/effects/V2/unchanged_shared_objects") {
        if let Some(arr) = unchanged.as_array() {
            for item in arr {
                if let (Some(id), Some(kind)) = (item.get(0).and_then(|v| v.as_str()), item.get(1))
                {
                    if let Some(read_only) = kind.get("ReadOnlyRoot") {
                        if let Some(version) = read_only.get(0).and_then(|v| v.as_str()) {
                            if let Ok(v) = version.parse::<u64>() {
                                map.insert(id, v);
                            }
                        }
                    }
                }
            }
        }
    }

    map
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tx_version_map() {
        let mut map = TxVersionMap::new();
        map.insert("0x1", 5);
        map.insert("0x2", 10);

        assert_eq!(map.get("0x1"), Some(5));
        assert_eq!(map.get("0x2"), Some(10));
        assert_eq!(map.get("0x3"), None);
    }

    #[test]
    fn test_tx_version_map_merge() {
        let mut map1 = TxVersionMap::new();
        map1.insert("0x1", 5);
        map1.insert("0x2", 10);

        let mut map2 = TxVersionMap::new();
        map2.insert("0x2", 15); // Override
        map2.insert("0x3", 20); // New
        map2.source = VersionMapSource::GrpcTransaction;

        map1.merge(&map2);

        assert_eq!(map1.get("0x1"), Some(5));
        assert_eq!(map1.get("0x2"), Some(15)); // Overridden
        assert_eq!(map1.get("0x3"), Some(20)); // Added
        assert_eq!(map1.source, VersionMapSource::Combined);
    }

    #[test]
    fn test_batch_prefetch_result() {
        let mut result = BatchPrefetchResult::new();
        result.txs_scanned = 10;
        result.prefetched_objects = 50;
        result.add_note("Test note");

        assert_eq!(result.txs_scanned, 10);
        assert_eq!(result.prefetched_objects, 50);
        assert_eq!(result.notes.len(), 1);
    }

    #[test]
    fn test_is_ptb_transaction() {
        let ptb_json = serde_json::json!({
            "transaction": {
                "V1": {
                    "txn_data": {
                        "V1": {
                            "kind": {
                                "ProgrammableTransaction": {}
                            }
                        }
                    }
                }
            }
        });
        assert!(is_ptb_transaction(&ptb_json));

        let non_ptb_json = serde_json::json!({
            "transaction": {
                "V1": {
                    "txn_data": {
                        "V1": {
                            "kind": {
                                "ChangeEpoch": {}
                            }
                        }
                    }
                }
            }
        });
        assert!(!is_ptb_transaction(&non_ptb_json));
    }

    #[test]
    fn test_extract_tx_digest() {
        let json = serde_json::json!({
            "digest": "ABC123"
        });
        assert_eq!(extract_tx_digest(&json), Some("ABC123".to_string()));

        let empty = serde_json::json!({});
        assert_eq!(extract_tx_digest(&empty), None);
    }
}
