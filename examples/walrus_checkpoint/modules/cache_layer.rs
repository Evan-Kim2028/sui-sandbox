//! Trait-based storage abstraction for objects and packages.
//!
//! Provides a clean interface for multi-tiered caching:
//! - Memory cache (fastest, limited capacity)
//! - Disk cache (persistent, larger capacity)
//! - Network fallback (gRPC/Walrus)
//! - System object synthesis (Clock, Random)
//!
//! Note: These traits are defined for future migration of the replay engine
//! to a fully modular architecture. Some types may appear unused until
//! integration is complete.

#![allow(dead_code)]

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

// ============================================================================
// Core Data Types
// ============================================================================

/// Entry for a cached object with type and version info.
#[derive(Clone, Debug)]
pub struct ObjectEntry {
    pub bytes: Vec<u8>,
    pub type_tag: TypeTag,
    pub version: u64,
}

/// Source indicator for cache hit reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSource {
    /// Object found in Walrus transaction JSON
    WalrusJson,
    /// Object found in memory cache
    MemoryCache,
    /// Object found in disk cache
    DiskCache,
    /// Object fetched from gRPC
    GrpcFetch,
    /// System object synthesized (Clock, Random)
    Synthesized,
}

/// Result of a cache lookup including the source.
#[derive(Debug, Clone)]
pub struct CacheLookupResult {
    pub entry: ObjectEntry,
    pub source: CacheSource,
}

/// Object data with full metadata for PTB construction.
#[derive(Debug, Clone)]
pub struct ObjectData {
    pub id: AccountAddress,
    pub bcs_bytes: Vec<u8>,
    pub type_tag: TypeTag,
    pub version: u64,
    pub is_immutable: bool,
    pub is_shared: bool,
}

impl From<ObjectData> for ObjectEntry {
    fn from(data: ObjectData) -> Self {
        Self {
            bytes: data.bcs_bytes,
            type_tag: data.type_tag,
            version: data.version,
        }
    }
}

// ============================================================================
// Object Store Traits
// ============================================================================

/// Primary trait for object caching with version support.
///
/// Implementations should provide versioned object storage with both
/// exact version lookup and "latest known version" fallback.
pub trait ObjectStore: Send + Sync {
    /// Get object at specific version.
    fn get(&self, id: AccountAddress, version: u64) -> Option<ObjectEntry>;

    /// Get latest known version of an object.
    fn get_latest(&self, id: AccountAddress) -> Option<ObjectEntry>;

    /// Insert object at specific version.
    fn insert(&mut self, id: AccountAddress, version: u64, entry: ObjectEntry);

    /// Remove all versions of an object (for conflict eviction).
    fn remove_all(&mut self, id: AccountAddress);

    /// Check if object exists at version.
    fn has(&self, id: AccountAddress, version: u64) -> bool {
        self.get(id, version).is_some()
    }

    /// Check if any version of object exists.
    fn has_any(&self, id: AccountAddress) -> bool {
        self.get_latest(id).is_some()
    }
}

/// Tiered object store that checks multiple levels in order.
///
/// Typically: Memory -> Disk -> Network
pub trait TieredObjectStore: Send + Sync {
    /// Lookup with automatic tier fallback and source tracking.
    fn lookup(&self, id: AccountAddress, version: Option<u64>)
        -> Result<Option<CacheLookupResult>>;

    /// Store in appropriate tier(s). If write_through is enabled,
    /// stores in all tiers up to and including the specified level.
    fn store(&self, id: AccountAddress, version: u64, entry: ObjectEntry) -> Result<()>;

    /// Store with explicit tier targeting.
    fn store_at_tier(
        &self,
        id: AccountAddress,
        version: u64,
        entry: ObjectEntry,
        tier: CacheTier,
    ) -> Result<()>;
}

/// Cache tier for explicit storage targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CacheTier {
    Memory,
    Disk,
}

// ============================================================================
// Package Store Traits
// ============================================================================

/// Package module cache trait.
///
/// Manages compiled Move modules and handles package versioning
/// including runtime-to-storage address aliasing for upgrades.
pub trait PackageStore: Send + Sync {
    /// Get compiled modules for a package.
    fn get_modules(&self, pkg: AccountAddress) -> Option<Vec<(String, Vec<u8>)>>;

    /// Get version for a package.
    fn get_version(&self, pkg: AccountAddress) -> Option<u64>;

    /// Store package modules.
    fn insert_package(
        &mut self,
        storage_addr: AccountAddress,
        modules: Vec<(String, Vec<u8>)>,
        version: u64,
    );

    /// Get runtime-to-storage address mapping (for package upgrades).
    fn get_storage_addr(&self, runtime: AccountAddress) -> Option<AccountAddress>;

    /// Add runtime-to-storage mapping.
    fn add_alias(&mut self, runtime: AccountAddress, storage: AccountAddress);

    /// Check if package is already loaded into the environment.
    fn is_loaded(&self, addr: AccountAddress) -> bool;

    /// Mark package as loaded.
    fn mark_loaded(&mut self, addr: AccountAddress);
}

// ============================================================================
// Object Resolver Trait (for PTB Parser decoupling)
// ============================================================================

/// Trait for resolving object data during PTB parsing.
///
/// This decouples the parser from the caching infrastructure,
/// allowing different resolution strategies to be plugged in.
pub trait ObjectResolver: Send + Sync {
    /// Resolve object data by ID with optional version hint.
    fn resolve(&self, id: AccountAddress, version_hint: Option<u64>) -> Result<Option<ObjectData>>;

    /// Batch resolve objects for efficiency.
    fn resolve_batch(
        &self,
        requests: &[(AccountAddress, Option<u64>)],
    ) -> Vec<Result<Option<ObjectData>>> {
        // Default implementation: sequential resolution
        requests
            .iter()
            .map(|(id, version)| self.resolve(*id, *version))
            .collect()
    }
}

/// Context for object resolution containing version hints and other metadata.
#[derive(Debug, Clone, Default)]
pub struct ResolveContext {
    /// Known object versions from transaction metadata
    pub version_hints: HashMap<AccountAddress, u64>,
    /// Timestamp for system object synthesis
    pub timestamp_ms: Option<u64>,
    /// Checkpoint for historical queries
    pub checkpoint: Option<u64>,
}

// ============================================================================
// Cache Metrics Trait
// ============================================================================

/// Cache metrics recorder for observability.
pub trait CacheMetricsRecorder: Send + Sync {
    fn record_walrus_hit(&self);
    fn record_memory_hit(&self);
    fn record_disk_hit(&self);
    fn record_grpc_fetch(&self);
    fn record_package_disk_hit(&self);
    fn record_package_grpc_fetch(&self);
    fn record_dynamic_field_disk_hit(&self);
    fn record_dynamic_field_grpc_fetch(&self);
}

// ============================================================================
// In-Memory Implementations
// ============================================================================

/// Simple in-memory object cache with version support.
#[derive(Default, Clone)]
pub struct MemoryObjectStore {
    by_id_version: HashMap<(AccountAddress, u64), ObjectEntry>,
    by_id_latest: HashMap<AccountAddress, ObjectEntry>,
}

impl MemoryObjectStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get number of versioned entries.
    pub fn len(&self) -> usize {
        self.by_id_version.len()
    }

    /// Check if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.by_id_version.is_empty()
    }
}

impl ObjectStore for MemoryObjectStore {
    fn get(&self, id: AccountAddress, version: u64) -> Option<ObjectEntry> {
        self.by_id_version.get(&(id, version)).cloned()
    }

    fn get_latest(&self, id: AccountAddress) -> Option<ObjectEntry> {
        self.by_id_latest.get(&id).cloned()
    }

    fn insert(&mut self, id: AccountAddress, version: u64, entry: ObjectEntry) {
        self.by_id_version.insert((id, version), entry.clone());
        let replace = match self.by_id_latest.get(&id) {
            Some(existing) => version >= existing.version,
            None => true,
        };
        if replace {
            self.by_id_latest.insert(id, entry);
        }
    }

    fn remove_all(&mut self, id: AccountAddress) {
        self.by_id_latest.remove(&id);
        self.by_id_version.retain(|(obj_id, _), _| obj_id != &id);
    }
}

/// Simple in-memory package cache.
#[derive(Default)]
pub struct MemoryPackageStore {
    modules_by_package: HashMap<AccountAddress, Vec<(String, Vec<u8>)>>,
    versions_by_package: HashMap<AccountAddress, u64>,
    runtime_to_storage: HashMap<AccountAddress, AccountAddress>,
    loaded_packages: std::collections::HashSet<AccountAddress>,
}

impl MemoryPackageStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PackageStore for MemoryPackageStore {
    fn get_modules(&self, pkg: AccountAddress) -> Option<Vec<(String, Vec<u8>)>> {
        self.modules_by_package.get(&pkg).cloned()
    }

    fn get_version(&self, pkg: AccountAddress) -> Option<u64> {
        self.versions_by_package.get(&pkg).copied()
    }

    fn insert_package(
        &mut self,
        storage_addr: AccountAddress,
        modules: Vec<(String, Vec<u8>)>,
        version: u64,
    ) {
        self.modules_by_package.insert(storage_addr, modules);
        self.versions_by_package.insert(storage_addr, version);
    }

    fn get_storage_addr(&self, runtime: AccountAddress) -> Option<AccountAddress> {
        self.runtime_to_storage.get(&runtime).copied()
    }

    fn add_alias(&mut self, runtime: AccountAddress, storage: AccountAddress) {
        self.runtime_to_storage.insert(runtime, storage);
    }

    fn is_loaded(&self, addr: AccountAddress) -> bool {
        self.loaded_packages.contains(&addr)
    }

    fn mark_loaded(&mut self, addr: AccountAddress) {
        self.loaded_packages.insert(addr);
    }
}

// ============================================================================
// Builder for Tiered Object Store
// ============================================================================

/// Builder for creating tiered object stores.
pub struct TieredObjectStoreBuilder {
    memory: Option<Arc<parking_lot::RwLock<MemoryObjectStore>>>,
    disk: Option<Arc<dyn DiskObjectStore>>,
    metrics: Option<Arc<dyn CacheMetricsRecorder>>,
}

/// Trait for disk-based object storage (wraps existing FsObjectStore).
pub trait DiskObjectStore: Send + Sync {
    fn get(&self, id: &str, version: u64) -> Result<Option<ObjectEntry>>;
    fn put(&self, id: &str, version: u64, entry: &ObjectEntry) -> Result<()>;
}

impl TieredObjectStoreBuilder {
    pub fn new() -> Self {
        Self {
            memory: None,
            disk: None,
            metrics: None,
        }
    }

    pub fn with_memory(mut self, store: Arc<parking_lot::RwLock<MemoryObjectStore>>) -> Self {
        self.memory = Some(store);
        self
    }

    pub fn with_disk(mut self, store: Arc<dyn DiskObjectStore>) -> Self {
        self.disk = Some(store);
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<dyn CacheMetricsRecorder>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn build(self) -> DefaultTieredObjectStore {
        DefaultTieredObjectStore {
            memory: self
                .memory
                .unwrap_or_else(|| Arc::new(parking_lot::RwLock::new(MemoryObjectStore::new()))),
            disk: self.disk,
            metrics: self.metrics,
        }
    }
}

impl Default for TieredObjectStoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Default implementation of TieredObjectStore.
pub struct DefaultTieredObjectStore {
    memory: Arc<parking_lot::RwLock<MemoryObjectStore>>,
    disk: Option<Arc<dyn DiskObjectStore>>,
    metrics: Option<Arc<dyn CacheMetricsRecorder>>,
}

impl TieredObjectStore for DefaultTieredObjectStore {
    fn lookup(
        &self,
        id: AccountAddress,
        version: Option<u64>,
    ) -> Result<Option<CacheLookupResult>> {
        // Check memory first
        let memory_guard = self.memory.read();
        let memory_result = if let Some(v) = version {
            memory_guard.get(id, v)
        } else {
            memory_guard.get_latest(id)
        };

        if let Some(entry) = memory_result {
            if let Some(ref metrics) = self.metrics {
                metrics.record_memory_hit();
            }
            return Ok(Some(CacheLookupResult {
                entry,
                source: CacheSource::MemoryCache,
            }));
        }
        drop(memory_guard);

        // Check disk if available
        if let (Some(ref disk), Some(v)) = (&self.disk, version) {
            if let Some(entry) = disk.get(&id.to_hex_literal(), v)? {
                if let Some(ref metrics) = self.metrics {
                    metrics.record_disk_hit();
                }
                // Populate memory cache
                self.memory.write().insert(id, v, entry.clone());
                return Ok(Some(CacheLookupResult {
                    entry,
                    source: CacheSource::DiskCache,
                }));
            }
        }

        Ok(None)
    }

    fn store(&self, id: AccountAddress, version: u64, entry: ObjectEntry) -> Result<()> {
        // Always store in memory
        self.memory.write().insert(id, version, entry.clone());

        // Store in disk if available (write-through)
        if let Some(ref disk) = self.disk {
            disk.put(&id.to_hex_literal(), version, &entry)?;
        }

        Ok(())
    }

    fn store_at_tier(
        &self,
        id: AccountAddress,
        version: u64,
        entry: ObjectEntry,
        tier: CacheTier,
    ) -> Result<()> {
        match tier {
            CacheTier::Memory => {
                self.memory.write().insert(id, version, entry);
            }
            CacheTier::Disk => {
                if let Some(ref disk) = self.disk {
                    disk.put(&id.to_hex_literal(), version, &entry)?;
                }
            }
        }
        Ok(())
    }
}

// ============================================================================
// Unified Object Resolver
// ============================================================================

/// A unified object resolver that wraps multiple data sources.
///
/// This decouples the PTB parser from caching infrastructure by providing
/// a single interface for object resolution.
pub struct UnifiedObjectResolver<'a> {
    /// Walrus input objects (keyed by ID hex string)
    pub walrus_objects: HashMap<String, WalrusObjectData>,
    /// Memory cache
    pub memory_cache: Option<&'a dyn ObjectStore>,
    /// Disk cache
    pub disk_cache: Option<Arc<dyn DiskObjectStore>>,
    /// Version hints from batch pre-scan
    pub version_hints: HashMap<String, u64>,
    /// Timestamp for system object synthesis
    pub timestamp_ms: Option<u64>,
    /// Metrics recorder
    pub metrics: Option<Arc<dyn CacheMetricsRecorder>>,
}

/// Walrus object data extracted from JSON.
#[derive(Debug, Clone)]
pub struct WalrusObjectData {
    pub id: AccountAddress,
    pub bcs_bytes: Vec<u8>,
    pub type_tag: TypeTag,
    pub version: u64,
    pub is_shared: bool,
}

impl<'a> UnifiedObjectResolver<'a> {
    pub fn new() -> Self {
        Self {
            walrus_objects: HashMap::new(),
            memory_cache: None,
            disk_cache: None,
            version_hints: HashMap::new(),
            timestamp_ms: None,
            metrics: None,
        }
    }

    pub fn with_walrus_objects(mut self, objects: HashMap<String, WalrusObjectData>) -> Self {
        self.walrus_objects = objects;
        self
    }

    pub fn with_memory_cache(mut self, cache: &'a dyn ObjectStore) -> Self {
        self.memory_cache = Some(cache);
        self
    }

    pub fn with_disk_cache(mut self, cache: Arc<dyn DiskObjectStore>) -> Self {
        self.disk_cache = Some(cache);
        self
    }

    pub fn with_version_hints(mut self, hints: HashMap<String, u64>) -> Self {
        self.version_hints = hints;
        self
    }

    pub fn with_timestamp(mut self, timestamp_ms: u64) -> Self {
        self.timestamp_ms = Some(timestamp_ms);
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<dyn CacheMetricsRecorder>) -> Self {
        self.metrics = Some(metrics);
        self
    }
}

impl<'a> Default for UnifiedObjectResolver<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ObjectResolver for UnifiedObjectResolver<'a> {
    fn resolve(&self, id: AccountAddress, version_hint: Option<u64>) -> Result<Option<ObjectData>> {
        let id_hex = id.to_hex_literal();

        // 1. Check Walrus objects first
        if let Some(walrus_obj) = self.walrus_objects.get(&id_hex) {
            if let Some(ref metrics) = self.metrics {
                metrics.record_walrus_hit();
            }
            return Ok(Some(ObjectData {
                id: walrus_obj.id,
                bcs_bytes: walrus_obj.bcs_bytes.clone(),
                type_tag: walrus_obj.type_tag.clone(),
                version: walrus_obj.version,
                is_immutable: false,
                is_shared: walrus_obj.is_shared,
            }));
        }

        // 2. Determine version to look up
        let version = version_hint.or_else(|| self.version_hints.get(&id_hex).copied());

        // 3. Check memory cache
        if let Some(cache) = self.memory_cache {
            let entry = if let Some(v) = version {
                cache.get(id, v)
            } else {
                cache.get_latest(id)
            };

            if let Some(e) = entry {
                if let Some(ref metrics) = self.metrics {
                    metrics.record_memory_hit();
                }
                return Ok(Some(ObjectData {
                    id,
                    bcs_bytes: e.bytes.clone(),
                    type_tag: e.type_tag.clone(),
                    version: e.version,
                    is_immutable: false,
                    is_shared: false,
                }));
            }
        }

        // 4. Check disk cache
        if let (Some(ref disk), Some(v)) = (&self.disk_cache, version) {
            if let Some(entry) = disk.get(&id_hex, v)? {
                if let Some(ref metrics) = self.metrics {
                    metrics.record_disk_hit();
                }
                return Ok(Some(ObjectData {
                    id,
                    bcs_bytes: entry.bytes.clone(),
                    type_tag: entry.type_tag.clone(),
                    version: entry.version,
                    is_immutable: false,
                    is_shared: false,
                }));
            }
        }

        // 5. Check for system objects (Clock 0x6, Random 0x8)
        if let Some(system_obj) = self.synthesize_system_object(id) {
            return Ok(Some(system_obj));
        }

        // Not found
        Ok(None)
    }
}

impl<'a> UnifiedObjectResolver<'a> {
    /// Synthesize system objects (Clock, Random).
    fn synthesize_system_object(&self, id: AccountAddress) -> Option<ObjectData> {
        let clock_id = AccountAddress::from_hex_literal("0x6").ok()?;
        let random_id = AccountAddress::from_hex_literal("0x8").ok()?;

        if id == clock_id {
            // Clock object: ID (32 bytes) + timestamp_ms (8 bytes)
            let timestamp = self.timestamp_ms.unwrap_or(0);
            let mut bytes = Vec::with_capacity(40);
            bytes.extend_from_slice(&id.to_vec());
            bytes.extend_from_slice(&timestamp.to_le_bytes());

            return Some(ObjectData {
                id,
                bcs_bytes: bytes,
                type_tag: TypeTag::Struct(Box::new(
                    move_core_types::language_storage::StructTag::from_str("0x2::clock::Clock")
                        .ok()?,
                )),
                version: 1,
                is_immutable: true,
                is_shared: true,
            });
        }

        if id == random_id {
            // Random object: UID (32 bytes) + inner (32 bytes)
            let mut bytes = Vec::with_capacity(64);
            bytes.extend_from_slice(&id.to_vec());
            bytes.extend_from_slice(&[0u8; 32]); // Placeholder inner

            return Some(ObjectData {
                id,
                bcs_bytes: bytes,
                type_tag: TypeTag::Struct(Box::new(
                    move_core_types::language_storage::StructTag::from_str("0x2::random::Random")
                        .ok()?,
                )),
                version: 1,
                is_immutable: false,
                is_shared: true,
            });
        }

        None
    }
}

// ============================================================================
// FsObjectStore Adapter
// ============================================================================

/// Adapter to make FsObjectStore compatible with DiskObjectStore trait.
///
/// Note: This requires a TypeTag parser since FsObjectStore stores type as string.
pub struct FsObjectStoreAdapter {
    inner: Arc<sui_historical_cache::FsObjectStore>,
}

impl FsObjectStoreAdapter {
    pub fn new(store: Arc<sui_historical_cache::FsObjectStore>) -> Self {
        Self { inner: store }
    }
}

impl DiskObjectStore for FsObjectStoreAdapter {
    fn get(&self, id: &str, version: u64) -> Result<Option<ObjectEntry>> {
        use sui_historical_cache::ObjectVersionStore;

        let addr = AccountAddress::from_hex_literal(id)
            .map_err(|e| anyhow::anyhow!("Invalid address {}: {}", id, e))?;

        match self.inner.get(addr, version)? {
            Some(cached) => {
                // Parse type tag from string - for now use a placeholder
                // A full implementation would parse the type string
                let type_tag =
                    parse_type_tag_string(&cached.meta.type_tag).unwrap_or(TypeTag::Address);

                Ok(Some(ObjectEntry {
                    bytes: cached.bcs_bytes,
                    type_tag,
                    version,
                }))
            }
            None => Ok(None),
        }
    }

    fn put(&self, id: &str, version: u64, entry: &ObjectEntry) -> Result<()> {
        use sui_historical_cache::{ObjectMeta, ObjectVersionStore};

        let addr = AccountAddress::from_hex_literal(id)
            .map_err(|e| anyhow::anyhow!("Invalid address {}: {}", id, e))?;

        let meta = ObjectMeta {
            type_tag: format!("{}", entry.type_tag),
            owner_kind: None,
            source_checkpoint: None,
        };

        self.inner.put(addr, version, &entry.bytes, &meta)
    }
}

/// Parse a type tag from its string representation.
/// This is a simplified parser - a full implementation would handle all type variants.
fn parse_type_tag_string(s: &str) -> Option<TypeTag> {
    use std::str::FromStr;

    // Handle primitive types
    match s {
        "bool" => return Some(TypeTag::Bool),
        "u8" => return Some(TypeTag::U8),
        "u16" => return Some(TypeTag::U16),
        "u32" => return Some(TypeTag::U32),
        "u64" => return Some(TypeTag::U64),
        "u128" => return Some(TypeTag::U128),
        "u256" => return Some(TypeTag::U256),
        "address" => return Some(TypeTag::Address),
        "signer" => return Some(TypeTag::Signer),
        _ => {}
    }

    // Try to parse as StructTag
    move_core_types::language_storage::StructTag::from_str(s)
        .ok()
        .map(|st| TypeTag::Struct(Box::new(st)))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_object_store_insert_and_get() {
        let mut store = MemoryObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x1").unwrap();
        let entry = ObjectEntry {
            bytes: vec![1, 2, 3],
            type_tag: TypeTag::U64,
            version: 5,
        };

        store.insert(id, 5, entry.clone());

        // Exact version lookup
        let result = store.get(id, 5);
        assert!(result.is_some());
        assert_eq!(result.unwrap().version, 5);

        // Latest lookup
        let latest = store.get_latest(id);
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().version, 5);

        // Non-existent version
        assert!(store.get(id, 10).is_none());
    }

    #[test]
    fn test_memory_object_store_version_priority() {
        let mut store = MemoryObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x1").unwrap();

        // Insert version 5
        store.insert(
            id,
            5,
            ObjectEntry {
                bytes: vec![1],
                type_tag: TypeTag::U64,
                version: 5,
            },
        );

        // Insert version 10 (should become latest)
        store.insert(
            id,
            10,
            ObjectEntry {
                bytes: vec![2],
                type_tag: TypeTag::U64,
                version: 10,
            },
        );

        // Latest should be version 10
        assert_eq!(store.get_latest(id).unwrap().version, 10);

        // Insert version 7 (should NOT become latest)
        store.insert(
            id,
            7,
            ObjectEntry {
                bytes: vec![3],
                type_tag: TypeTag::U64,
                version: 7,
            },
        );

        // Latest should still be version 10
        assert_eq!(store.get_latest(id).unwrap().version, 10);

        // But version 7 should be retrievable
        assert_eq!(store.get(id, 7).unwrap().version, 7);
    }

    #[test]
    fn test_memory_object_store_remove_all() {
        let mut store = MemoryObjectStore::new();
        let id = AccountAddress::from_hex_literal("0x1").unwrap();

        store.insert(
            id,
            5,
            ObjectEntry {
                bytes: vec![1],
                type_tag: TypeTag::U64,
                version: 5,
            },
        );
        store.insert(
            id,
            10,
            ObjectEntry {
                bytes: vec![2],
                type_tag: TypeTag::U64,
                version: 10,
            },
        );

        assert!(store.has(id, 5));
        assert!(store.has(id, 10));

        store.remove_all(id);

        assert!(!store.has(id, 5));
        assert!(!store.has(id, 10));
        assert!(!store.has_any(id));
    }

    #[test]
    fn test_memory_package_store() {
        let mut store = MemoryPackageStore::new();
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        let runtime = AccountAddress::from_hex_literal("0x3").unwrap();

        store.insert_package(addr, vec![("module".to_string(), vec![1, 2, 3])], 1);
        store.add_alias(runtime, addr);
        store.mark_loaded(addr);

        assert!(store.get_modules(addr).is_some());
        assert_eq!(store.get_version(addr), Some(1));
        assert_eq!(store.get_storage_addr(runtime), Some(addr));
        assert!(store.is_loaded(addr));
        assert!(!store.is_loaded(runtime));
    }
}
