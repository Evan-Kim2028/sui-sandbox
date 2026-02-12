//! Walrus-First Data Fetching Strategy
//!
//! This module provides a unified data fetching approach that prioritizes Walrus
//! checkpoints (free, unauthenticated) over gRPC/GraphQL (authenticated, rate-limited).
//!
//! # Data Source Priority
//!
//! 1. **Walrus checkpoints**: Free historical data including transactions and input objects
//! 2. **Local disk cache**: Persisted objects/packages from previous fetches
//! 3. **gRPC archive**: For packages and objects not in Walrus (authenticated)
//! 4. **GraphQL**: For dynamic field enumeration and fallback
//!
//! # Example Usage
//!
//! ```ignore
//! use examples::common::walrus_first::WalrusFirstFetcher;
//!
//! let fetcher = WalrusFirstFetcher::new(grpc, graphql, cache_dir)?;
//!
//! // Fetch transaction with Walrus-first priority
//! let tx_data = fetcher.fetch_transaction(&digest, checkpoint).await?;
//!
//! // Access objects from Walrus JSON, fallback to gRPC
//! let obj = fetcher.get_object(&id, version).await?;
//! ```

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;

use sui_transport::walrus::WalrusClient;
use sui_transport::grpc::GrpcClient;
use sui_transport::graphql::GraphQLClient;
use sui_historical_cache::{
    CachedPackage, FsObjectStore, FsPackageStore, ObjectMeta, ObjectVersionStore, PackageStore,
};

// ============================================================================
// Core Types
// ============================================================================

/// Cached object with type and version info.
#[derive(Clone, Debug)]
pub struct CachedObject {
    pub bytes: Vec<u8>,
    pub type_tag: TypeTag,
    pub version: u64,
}

/// Source where an object was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// Object from Walrus checkpoint transaction JSON
    WalrusCheckpoint,
    /// Object from local disk cache
    DiskCache,
    /// Object fetched from gRPC
    GrpcFetch,
    /// Object fetched from GraphQL
    GraphQLFetch,
    /// System object synthesized (Clock, Random)
    Synthesized,
}

/// Result of a fetch operation with source tracking.
#[derive(Debug, Clone)]
pub struct FetchResult<T> {
    pub data: T,
    pub source: DataSource,
}

/// Statistics for data fetching operations.
#[derive(Debug, Default, Clone)]
pub struct FetchStats {
    pub walrus_hits: usize,
    pub cache_hits: usize,
    pub grpc_fetches: usize,
    pub graphql_fetches: usize,
    pub misses: usize,
}

// ============================================================================
// Memory Cache
// ============================================================================

/// Thread-safe object cache with version support.
#[derive(Default)]
pub struct ObjectCache {
    by_id_version: HashMap<(AccountAddress, u64), CachedObject>,
    by_id_latest: HashMap<AccountAddress, CachedObject>,
}

impl ObjectCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: AccountAddress, version: u64, entry: CachedObject) {
        self.by_id_version.insert((id, version), entry.clone());
        let replace = match self.by_id_latest.get(&id) {
            Some(existing) => version >= existing.version,
            None => true,
        };
        if replace {
            self.by_id_latest.insert(id, entry);
        }
    }

    pub fn get(&self, id: AccountAddress, version: u64) -> Option<&CachedObject> {
        self.by_id_version.get(&(id, version))
    }

    pub fn get_any(&self, id: AccountAddress) -> Option<&CachedObject> {
        self.by_id_latest.get(&id)
    }
}

// ============================================================================
// Walrus-First Fetcher
// ============================================================================

/// Unified data fetcher with Walrus-first priority.
///
/// This fetcher tries data sources in order of cost/availability:
/// 1. Walrus checkpoints (free, public)
/// 2. Local disk cache (fast, persistent)
/// 3. gRPC archive (authenticated)
/// 4. GraphQL (authenticated, for dynamic fields)
pub struct WalrusFirstFetcher {
    /// Walrus client for checkpoint data (free)
    walrus: WalrusClient,
    /// gRPC client for package/object fetching (authenticated)
    grpc: Arc<GrpcClient>,
    /// GraphQL client for dynamic field enumeration
    graphql: GraphQLClient,
    /// In-memory object cache
    objects: Arc<RwLock<ObjectCache>>,
    /// Optional disk-based object cache
    disk_objects: Option<Arc<FsObjectStore>>,
    /// Optional disk-based package cache
    disk_packages: Option<Arc<FsPackageStore>>,
    /// Fetch statistics
    stats: Arc<RwLock<FetchStats>>,
}

impl WalrusFirstFetcher {
    /// Create a new fetcher with optional disk cache.
    pub fn new(
        grpc: GrpcClient,
        graphql: GraphQLClient,
        cache_dir: Option<&str>,
    ) -> Result<Self> {
        let disk_objects = cache_dir
            .map(FsObjectStore::new)
            .transpose()
            .context("Failed to initialize object cache")?;

        let disk_packages = cache_dir
            .map(FsPackageStore::new)
            .transpose()
            .context("Failed to initialize package cache")?;

        Ok(Self {
            walrus: WalrusClient::mainnet(),
            grpc: Arc::new(grpc),
            graphql,
            objects: Arc::new(RwLock::new(ObjectCache::new())),
            disk_objects: disk_objects.map(Arc::new),
            disk_packages: disk_packages.map(Arc::new),
            stats: Arc::new(RwLock::new(FetchStats::default())),
        })
    }

    /// Create with existing clients (useful for sharing).
    pub fn with_clients(
        walrus: WalrusClient,
        grpc: Arc<GrpcClient>,
        graphql: GraphQLClient,
        disk_objects: Option<Arc<FsObjectStore>>,
        disk_packages: Option<Arc<FsPackageStore>>,
    ) -> Self {
        Self {
            walrus,
            grpc,
            graphql,
            objects: Arc::new(RwLock::new(ObjectCache::new())),
            disk_objects,
            disk_packages,
            stats: Arc::new(RwLock::new(FetchStats::default())),
        }
    }

    /// Get the Walrus client reference.
    pub fn walrus(&self) -> &WalrusClient {
        &self.walrus
    }

    /// Get the gRPC client reference.
    pub fn grpc(&self) -> &GrpcClient {
        &self.grpc
    }

    /// Get the GraphQL client reference.
    pub fn graphql(&self) -> &GraphQLClient {
        &self.graphql
    }

    /// Get current fetch statistics.
    pub fn stats(&self) -> FetchStats {
        self.stats.read().clone()
    }

    /// Ingest objects from a Walrus transaction JSON into cache.
    ///
    /// This extracts input_objects and output_objects from the checkpoint
    /// transaction data and caches them for replay.
    pub fn ingest_walrus_tx(&self, tx_json: &serde_json::Value) -> Result<usize> {
        let mut count = 0;
        for key in ["input_objects", "output_objects"] {
            let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
                continue;
            };
            for obj_json in arr {
                if let Ok(obj) = self.parse_walrus_object(obj_json) {
                    let id = extract_object_id(&obj.bytes)?;
                    self.objects.write().insert(id, obj.version, obj.clone());

                    // Persist to disk cache if available
                    if let Some(ref disk) = self.disk_objects {
                        let meta = ObjectMeta {
                            type_tag: format!("{}", obj.type_tag),
                            owner_kind: None,
                            source_checkpoint: None,
                        };
                        let _ = disk.put(id, obj.version, &obj.bytes, &meta);
                    }
                    count += 1;
                }
            }
        }
        self.stats.write().walrus_hits += count;
        Ok(count)
    }

    /// Get object, trying sources in priority order.
    ///
    /// Priority: memory cache → Walrus-ingested → disk cache → gRPC
    pub async fn get_object(
        &self,
        id: AccountAddress,
        version: Option<u64>,
    ) -> Result<Option<FetchResult<CachedObject>>> {
        // 1. Check memory cache
        {
            let cache = self.objects.read();
            if let Some(v) = version {
                if let Some(obj) = cache.get(id, v) {
                    return Ok(Some(FetchResult {
                        data: obj.clone(),
                        source: DataSource::WalrusCheckpoint, // Memory cache from Walrus
                    }));
                }
            } else if let Some(obj) = cache.get_any(id) {
                return Ok(Some(FetchResult {
                    data: obj.clone(),
                    source: DataSource::WalrusCheckpoint,
                }));
            }
        }

        // 2. Check disk cache
        if let Some(ref disk) = self.disk_objects {
            if let Some(v) = version {
                if let Ok(Some(cached)) = disk.get(id, v) {
                    if let Some(type_tag) = parse_type_tag_str(&cached.meta.type_tag) {
                        let obj = CachedObject {
                            bytes: cached.bcs_bytes,
                            type_tag,
                            version: v,
                        };
                        self.objects.write().insert(id, v, obj.clone());
                        self.stats.write().cache_hits += 1;
                        return Ok(Some(FetchResult {
                            data: obj,
                            source: DataSource::DiskCache,
                        }));
                    }
                }
            }
        }

        // 3. Try gRPC
        let id_str = id.to_hex_literal();
        let result = self.grpc.get_object_at_version(&id_str, version).await;

        if let Ok(Some(grpc_obj)) = result {
            if let (Some(type_str), Some(bcs)) = (&grpc_obj.type_string, &grpc_obj.bcs) {
                if let Some(type_tag) = parse_type_tag_str(type_str) {
                    let obj = CachedObject {
                        bytes: bcs.clone(),
                        type_tag,
                        version: grpc_obj.version,
                    };
                    self.objects.write().insert(id, grpc_obj.version, obj.clone());

                    // Persist to disk cache
                    if let Some(ref disk) = self.disk_objects {
                        let meta = ObjectMeta {
                            type_tag: type_str.clone(),
                            owner_kind: None,
                            source_checkpoint: None,
                        };
                        let _ = disk.put(id, grpc_obj.version, bcs, &meta);
                    }

                    self.stats.write().grpc_fetches += 1;
                    return Ok(Some(FetchResult {
                        data: obj,
                        source: DataSource::GrpcFetch,
                    }));
                }
            }
        }

        self.stats.write().misses += 1;
        Ok(None)
    }

    /// Get package modules, trying sources in priority order.
    ///
    /// Returns decoded bytecode modules: Vec<(module_name, bytecode)>
    pub async fn get_package(
        &self,
        pkg_id: AccountAddress,
    ) -> Result<Option<Vec<(String, Vec<u8>)>>> {
        // 1. Check disk cache
        if let Some(ref disk) = self.disk_packages {
            if let Ok(Some(cached)) = disk.get(pkg_id) {
                // Decode base64 modules from cache
                let decoded: Vec<(String, Vec<u8>)> = cached
                    .modules
                    .iter()
                    .filter_map(|(name, b64)| {
                        base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .ok()
                            .map(|bytes| (name.clone(), bytes))
                    })
                    .collect();
                return Ok(Some(decoded));
            }
        }

        // 2. Fetch from gRPC
        let pkg_str = pkg_id.to_hex_literal();
        let result = self.grpc.get_object(&pkg_str).await;
        if let Ok(Some(pkg)) = result {
            if let Some(ref modules) = pkg.package_modules {
                // Cache to disk (need to encode as base64)
                if let Some(ref disk) = self.disk_packages {
                    let cached_pkg = sui_historical_cache::CachedPackage {
                        version: pkg.version,
                        modules: modules
                            .iter()
                            .map(|(name, bytes)| {
                                (
                                    name.clone(),
                                    base64::engine::general_purpose::STANDARD.encode(bytes),
                                )
                            })
                            .collect(),
                        original_id: pkg.package_original_id.clone(),
                        linkage: None, // Could populate from pkg.package_linkage if needed
                    };
                    let _ = disk.put(pkg_id, &cached_pkg);
                }

                return Ok(Some(modules.clone()));
            }
        }

        Ok(None)
    }

    /// Extract version map from transaction effects (for historical replay).
    pub fn extract_versions_from_effects(&self, tx_json: &serde_json::Value) -> HashMap<String, u64> {
        let mut versions = HashMap::new();

        // changed_objects: [[id, {input_state: {Exist: [[version, digest], owner]}}]]
        if let Some(changed) = tx_json.pointer("/effects/V2/changed_objects") {
            if let Some(arr) = changed.as_array() {
                for item in arr {
                    if let (Some(id), Some(input_state)) = (
                        item.get(0).and_then(|v| v.as_str()),
                        item.get(1).and_then(|v| v.get("input_state")),
                    ) {
                        if let Some(exist) = input_state.get("Exist") {
                            if let Some(version_digest) = exist.get(0) {
                                if let Some(version) = version_digest.get(0).and_then(|v| v.as_str()) {
                                    if let Ok(v) = version.parse::<u64>() {
                                        versions.insert(id.to_string(), v);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // unchanged_shared_objects: [[id, {ReadOnlyRoot: [version, digest]}]]
        if let Some(unchanged) = tx_json.pointer("/effects/V2/unchanged_shared_objects") {
            if let Some(arr) = unchanged.as_array() {
                for item in arr {
                    if let (Some(id), Some(kind)) = (item.get(0).and_then(|v| v.as_str()), item.get(1)) {
                        if let Some(read_only) = kind.get("ReadOnlyRoot") {
                            if let Some(version) = read_only.get(0).and_then(|v| v.as_str()) {
                                if let Ok(v) = version.parse::<u64>() {
                                    versions.insert(id.to_string(), v);
                                }
                            }
                        }
                    }
                }
            }
        }

        versions
    }

    /// Parse a Move object from Walrus JSON format.
    fn parse_walrus_object(&self, obj_json: &serde_json::Value) -> Result<CachedObject> {
        let move_obj = obj_json
            .get("data")
            .and_then(|d| d.get("Move"))
            .ok_or_else(|| anyhow!("Not a Move object"))?;

        let contents_b64 = move_obj
            .get("contents")
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow!("Missing contents"))?;

        let bcs_bytes = base64::engine::general_purpose::STANDARD
            .decode(contents_b64)
            .context("base64 decode failed")?;

        let version = move_obj
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let type_json = move_obj
            .get("type_")
            .ok_or_else(|| anyhow!("missing type_"))?;

        let type_tag = parse_type_tag_json(type_json)?;

        Ok(CachedObject {
            bytes: bcs_bytes,
            type_tag,
            version,
        })
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract object ID from BCS bytes (first 32 bytes).
fn extract_object_id(bytes: &[u8]) -> Result<AccountAddress> {
    if bytes.len() < 32 {
        return Err(anyhow!("BCS bytes too short for object ID"));
    }
    let mut id_bytes = [0u8; 32];
    id_bytes.copy_from_slice(&bytes[0..32]);
    Ok(AccountAddress::new(id_bytes))
}

/// Parse TypeTag from a type string.
fn parse_type_tag_str(type_str: &str) -> Option<TypeTag> {
    sui_sandbox_core::utilities::parse_type_tag(type_str)
}

/// Parse TypeTag from Walrus JSON format.
fn parse_type_tag_json(type_json: &serde_json::Value) -> Result<TypeTag> {
    // Walrus uses a structured format: {"Struct": {"address": "0x2", "module": "coin", "name": "Coin", ...}}
    if let Some(struct_obj) = type_json.get("Struct") {
        let address = struct_obj
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing Struct.address"))?;
        let module = struct_obj
            .get("module")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing Struct.module"))?;
        let name = struct_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing Struct.name"))?;

        // Parse type arguments recursively
        let type_args: Vec<TypeTag> = struct_obj
            .get("type_args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| parse_type_tag_json(t).ok())
                    .collect()
            })
            .unwrap_or_default();

        let addr = AccountAddress::from_hex_literal(address)
            .map_err(|e| anyhow!("invalid address: {}", e))?;

        Ok(TypeTag::Struct(Box::new(
            move_core_types::language_storage::StructTag {
                address: addr,
                module: move_core_types::identifier::Identifier::new(module)
                    .map_err(|e| anyhow!("invalid module: {}", e))?,
                name: move_core_types::identifier::Identifier::new(name)
                    .map_err(|e| anyhow!("invalid name: {}", e))?,
                type_params: type_args,
            },
        )))
    } else {
        // Primitive types
        let type_str = type_json
            .as_str()
            .ok_or_else(|| anyhow!("expected type string or Struct"))?;

        match type_str {
            "Bool" => Ok(TypeTag::Bool),
            "U8" => Ok(TypeTag::U8),
            "U16" => Ok(TypeTag::U16),
            "U32" => Ok(TypeTag::U32),
            "U64" => Ok(TypeTag::U64),
            "U128" => Ok(TypeTag::U128),
            "U256" => Ok(TypeTag::U256),
            "Address" => Ok(TypeTag::Address),
            "Signer" => Ok(TypeTag::Signer),
            _ => Err(anyhow!("unknown primitive type: {}", type_str)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_type_tag_json_struct() {
        let json = serde_json::json!({
            "Struct": {
                "address": "0x2",
                "module": "coin",
                "name": "Coin",
                "type_args": [
                    {
                        "Struct": {
                            "address": "0x2",
                            "module": "sui",
                            "name": "SUI",
                            "type_args": []
                        }
                    }
                ]
            }
        });

        let result = parse_type_tag_json(&json).unwrap();
        let type_str = format!("{}", result);
        assert!(type_str.contains("Coin"));
        assert!(type_str.contains("SUI"));
    }

    #[test]
    fn test_parse_type_tag_json_primitive() {
        assert_eq!(parse_type_tag_json(&serde_json::json!("U64")).unwrap(), TypeTag::U64);
        assert_eq!(parse_type_tag_json(&serde_json::json!("Bool")).unwrap(), TypeTag::Bool);
    }

    #[test]
    fn test_extract_object_id() {
        let mut bytes = vec![0u8; 64];
        bytes[0] = 0x42;
        bytes[31] = 0x01;

        let id = extract_object_id(&bytes).unwrap();
        assert_eq!(id.to_vec()[0], 0x42);
        assert_eq!(id.to_vec()[31], 0x01);
    }
}
