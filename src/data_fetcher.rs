//! Unified Data Fetcher for Sui Network
//!
//! Provides a unified interface for fetching blockchain data with:
//! - **Cache**: Local transaction cache (fastest, no network)
//! - **gRPC**: Real-time streaming (push-based, ~30-40 tx/sec)
//! - **GraphQL**: Rich queries with complete data
//! - **JSON-RPC**: Legacy fallback
//!
//! # Choosing a Backend
//!
//! | Use Case | Recommended | Why |
//! |----------|-------------|-----|
//! | Real-time monitoring | gRPC streaming | Push-based, no gaps, ~30-40 tx/sec |
//! | Transaction replay | GraphQL | Complete effects (created/mutated/deleted) |
//! | Package/object queries | Cache → GraphQL | Cache-first for speed, GraphQL fallback |
//! | Historical lookups | GraphQL or gRPC archive | Both support point queries |
//!
//! # Cache-First Strategy
//!
//! When a cache is configured, all package and object fetches check the cache first.
//! Network fetches are automatically written back to cache (write-through caching).
//!
//! ```ignore
//! // Enable cache with write-through
//! let fetcher = DataFetcher::mainnet()
//!     .with_cache(".tx-cache")?;
//!
//! // First fetch: cache miss → network → cache write
//! let pkg = fetcher.fetch_package("0x123")?;  // Source: GraphQL
//!
//! // Second fetch: cache hit
//! let pkg = fetcher.fetch_package("0x123")?;  // Source: Cache
//! ```
//!
//! # Data Completeness
//!
//! **Critical for replay/simulation:**
//!
//! | Field | Cache | gRPC | GraphQL |
//! |-------|-------|------|---------|
//! | inputs[], commands[] | ✅ | ✅ | ✅ |
//! | effects.status | ✅ | ✅ | ✅ |
//! | effects.created/mutated/deleted | ✅ | ❌ | ✅ |
//! | version/type metadata | ✅ | ❌ | ✅ |
//!
//! # Public Endpoints
//!
//! | Endpoint | gRPC Streaming | Queries |
//! |----------|----------------|---------|
//! | `fullnode.mainnet.sui.io:443` | ✅ | Recent only |
//! | `archive.mainnet.sui.io:443` | ❌ | Full history |
//! | `graphql.mainnet.sui.io` | N/A | Full history |
//!
//! # Usage
//!
//! ```ignore
//! // Basic queries with cache (GraphQL primary, JSON-RPC fallback)
//! let fetcher = DataFetcher::mainnet()
//!     .with_cache(".tx-cache")?;
//! let obj = fetcher.fetch_object("0x...")?;
//! let pkg = fetcher.fetch_package("0x2")?;
//! let tx = fetcher.fetch_transaction("digest...")?;
//!
//! // Real-time streaming (gRPC)
//! let fetcher = DataFetcher::mainnet()
//!     .with_grpc_endpoint("https://fullnode.mainnet.sui.io:443")
//!     .await?;
//!
//! let mut stream = fetcher.subscribe_checkpoints().await?;
//! while let Some(result) = stream.next().await {
//!     let checkpoint = result?;
//!     for tx in checkpoint.transactions {
//!         if tx.is_ptb() {
//!             println!("{}: {} commands", tx.digest, tx.commands.len());
//!         }
//!     }
//! }
//! ```
//!
//! See [`DATA_FETCHING.md`](../../docs/guides/DATA_FETCHING.md) for detailed tradeoffs.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::benchmark::tx_replay::TransactionFetcher;
use crate::graphql::{GraphQLClient, GraphQLObject, GraphQLTransaction, ObjectOwner};
use crate::grpc::{CheckpointStream, GrpcCheckpoint, GrpcClient, GrpcTransaction, ServiceInfo};

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert a GraphQL object to our unified FetchedObjectData.
/// This avoids duplicating the conversion logic across multiple methods.
fn graphql_object_to_fetched(obj: GraphQLObject) -> Result<FetchedObjectData> {
    use base64::Engine;

    let bcs_bytes = obj
        .bcs_base64
        .as_ref()
        .map(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| anyhow!("Failed to decode BCS: {}", e))
        })
        .transpose()?;

    let (is_shared, is_immutable) = match &obj.owner {
        ObjectOwner::Shared { .. } => (true, false),
        ObjectOwner::Immutable => (false, true),
        _ => (false, false),
    };

    Ok(FetchedObjectData {
        address: obj.address,
        version: obj.version,
        type_string: obj.type_string,
        bcs_bytes,
        is_shared,
        is_immutable,
        source: DataSource::GraphQL,
    })
}

// Re-export GraphQL types for convenience
pub use crate::graphql::{
    GraphQLArgument,
    GraphQLCommand,
    GraphQLEffects,
    GraphQLObjectChange,
    GraphQLTransactionInput,
    // Pagination support
    PageInfo,
    PaginationDirection,
    Paginator,
};

// Re-export gRPC types for streaming
pub use crate::grpc::{
    CheckpointStream as StreamingCheckpointStream, GrpcArgument as StreamingArgument,
    GrpcCheckpoint as StreamingCheckpoint, GrpcCommand as StreamingCommand,
    GrpcInput as StreamingInput, GrpcTransaction as StreamingTransaction,
    ServiceInfo as StreamingServiceInfo,
};

/// Unified data fetcher with cache-first strategy and fallback support.
///
/// The DataFetcher provides a unified interface for fetching blockchain data with:
/// - **Cache**: Check local cache first (fastest, no network)
/// - **Network**: Fall back to GraphQL or JSON-RPC
/// - **Write-through**: Automatically cache network fetches
pub struct DataFetcher {
    json_rpc: TransactionFetcher,
    graphql: GraphQLClient,
    /// Optional gRPC client for streaming (requires provider endpoint)
    grpc: Option<GrpcClient>,
    /// Unified cache manager (replaces legacy tx_cache)
    cache: Option<std::sync::Arc<std::sync::RwLock<crate::cache::CacheManager>>>,
    /// Whether to use the other source as fallback when primary fails
    use_fallback: bool,
    /// Whether to prefer GraphQL over JSON-RPC (default: true)
    prefer_graphql: bool,
    /// Whether to write network fetches back to cache (default: true)
    write_through: bool,
}

/// Unified object representation from either source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedObjectData {
    pub address: String,
    pub version: u64,
    pub type_string: Option<String>,
    pub bcs_bytes: Option<Vec<u8>>,
    pub is_shared: bool,
    pub is_immutable: bool,
    /// Which source provided this data
    pub source: DataSource,
}

/// Which data source provided the data.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataSource {
    /// Local transaction cache (fastest, no network)
    Cache,
    /// JSON-RPC endpoint (legacy, deprecated April 2026)
    JsonRpc,
    /// GraphQL endpoint (recommended for queries)
    GraphQL,
    /// gRPC endpoint (recommended for streaming)
    Grpc,
}

/// Package data fetched from any source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedPackageData {
    pub address: String,
    /// Package version (for upgraded packages)
    pub version: u64,
    pub modules: Vec<FetchedModuleData>,
    pub source: DataSource,
}

/// Module data within a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedModuleData {
    pub name: String,
    pub bytecode: Vec<u8>,
}

impl DataFetcher {
    /// Create a fetcher for mainnet with default settings (prefers GraphQL).
    /// gRPC is not enabled by default (requires provider endpoint).
    /// Cache is not enabled by default - use `with_cache` to add it.
    pub fn mainnet() -> Self {
        Self {
            json_rpc: TransactionFetcher::mainnet(),
            graphql: GraphQLClient::mainnet(),
            grpc: None,
            cache: None,
            use_fallback: true,
            prefer_graphql: true,
            write_through: true,
        }
    }

    /// Create a fetcher for testnet (prefers GraphQL).
    /// gRPC is not enabled by default (requires provider endpoint).
    /// Cache is not enabled by default - use `with_cache` to add it.
    pub fn testnet() -> Self {
        Self {
            json_rpc: TransactionFetcher::testnet(),
            graphql: GraphQLClient::testnet(),
            grpc: None,
            cache: None,
            use_fallback: true,
            prefer_graphql: true,
            write_through: true,
        }
    }

    /// Create with custom endpoints (prefers GraphQL by default).
    /// gRPC and cache are not enabled by default.
    pub fn new(json_rpc_endpoint: &str, graphql_endpoint: &str) -> Self {
        Self {
            json_rpc: TransactionFetcher::new(json_rpc_endpoint),
            graphql: GraphQLClient::new(graphql_endpoint),
            grpc: None,
            cache: None,
            use_fallback: true,
            prefer_graphql: true,
            write_through: true,
        }
    }

    /// Enable local transaction cache for cache-first lookups with write-through.
    ///
    /// When enabled:
    /// - Package and object fetches check cache first (faster, no network)
    /// - Network fetches are automatically written back to cache
    /// - Cache stores version and type metadata for accurate results
    ///
    /// # Example
    /// ```ignore
    /// let fetcher = DataFetcher::mainnet()
    ///     .with_cache(".tx-cache")?;
    ///
    /// // First fetch: cache miss → network → cache write
    /// let pkg = fetcher.fetch_package("0x123")?;  // Source: GraphQL
    ///
    /// // Second fetch: cache hit
    /// let pkg = fetcher.fetch_package("0x123")?;  // Source: Cache
    /// ```
    pub fn with_cache<P: AsRef<std::path::Path>>(mut self, cache_dir: P) -> Result<Self> {
        use std::sync::{Arc, RwLock};
        let manager = crate::cache::CacheManager::new(cache_dir)?;
        self.cache = Some(Arc::new(RwLock::new(manager)));
        Ok(self)
    }

    /// Enable local transaction cache, ignoring errors if cache doesn't exist.
    ///
    /// This is useful for optional caching where you want to use it if available
    /// but don't want to fail if the cache directory doesn't exist.
    pub fn with_cache_optional<P: AsRef<std::path::Path>>(mut self, cache_dir: P) -> Self {
        use std::sync::{Arc, RwLock};
        if let Ok(manager) = crate::cache::CacheManager::new(cache_dir) {
            if !manager.is_empty() {
                self.cache = Some(Arc::new(RwLock::new(manager)));
            }
        }
        self
    }

    /// Enable or disable write-through caching.
    ///
    /// When enabled (default), network fetches are automatically written to cache.
    /// Disable for read-only cache access.
    pub fn with_write_through(mut self, enabled: bool) -> Self {
        self.write_through = enabled;
        self
    }

    /// Check if cache is enabled and has data.
    pub fn has_cache(&self) -> bool {
        self.cache
            .as_ref()
            .and_then(|c| c.read().ok())
            .map(|c| !c.is_empty())
            .unwrap_or(false)
    }

    /// Get cache statistics (packages, objects, transactions, disk size).
    pub fn cache_stats(&self) -> Option<crate::cache::CacheStats> {
        self.cache
            .as_ref()
            .and_then(|c| c.read().ok())
            .map(|c| c.stats())
    }

    /// Get basic cache counts (packages and objects indexed).
    /// For full stats, use `cache_stats()`.
    pub fn cache_counts(&self) -> Option<(usize, usize)> {
        self.cache
            .as_ref()
            .and_then(|c| c.read().ok())
            .map(|c| (c.package_count(), c.object_count()))
    }

    /// Add gRPC client for real-time streaming capabilities.
    ///
    /// Note: Sui's public fullnodes don't expose gRPC. You need a provider like:
    /// - QuickNode: `https://your-endpoint.sui-mainnet.quiknode.pro:9000`
    /// - Dwellir: Contact for access
    ///
    /// Once added, you can use `subscribe_checkpoints()` for real-time data.
    pub async fn with_grpc_endpoint(mut self, endpoint: &str) -> Result<Self> {
        self.grpc = Some(GrpcClient::new(endpoint).await?);
        Ok(self)
    }

    /// Add a pre-configured gRPC client.
    pub fn with_grpc_client(mut self, client: GrpcClient) -> Self {
        self.grpc = Some(client);
        self
    }

    /// Check if gRPC streaming is available.
    pub fn has_grpc(&self) -> bool {
        self.grpc.is_some()
    }

    /// Enable or disable fallback to alternate source.
    pub fn with_fallback(mut self, enabled: bool) -> Self {
        self.use_fallback = enabled;
        self
    }

    /// Set whether to prefer GraphQL over JSON-RPC.
    /// Default is true (prefer GraphQL for reliability).
    pub fn with_prefer_graphql(mut self, prefer: bool) -> Self {
        self.prefer_graphql = prefer;
        self
    }

    // ========== gRPC Streaming Methods ==========

    /// Get gRPC service info (chain, epoch, checkpoint height).
    /// Returns error if gRPC is not configured.
    pub async fn get_service_info(&self) -> Result<ServiceInfo> {
        let grpc = self.grpc.as_ref().ok_or_else(|| {
            anyhow!("gRPC not configured. Use with_grpc_endpoint() to enable streaming.")
        })?;
        grpc.get_service_info().await
    }

    /// Subscribe to real-time checkpoint stream.
    ///
    /// This is the recommended way to monitor transactions in real-time.
    /// Each checkpoint contains all transactions finalized in that checkpoint.
    ///
    /// Returns error if gRPC is not configured.
    ///
    /// ## Example
    /// ```ignore
    /// let fetcher = DataFetcher::mainnet()
    ///     .with_grpc_endpoint("https://your-provider:9000")
    ///     .await?;
    ///
    /// let mut stream = fetcher.subscribe_checkpoints().await?;
    /// while let Some(result) = stream.next().await {
    ///     let checkpoint = result?;
    ///     for tx in &checkpoint.transactions {
    ///         if tx.is_ptb() {
    ///             println!("{}: {} commands", tx.digest, tx.commands.len());
    ///         }
    ///     }
    /// }
    /// ```
    pub async fn subscribe_checkpoints(&self) -> Result<CheckpointStream> {
        let grpc = self.grpc.as_ref().ok_or_else(|| {
            anyhow!("gRPC not configured. Use with_grpc_endpoint() to enable streaming.")
        })?;
        grpc.subscribe_checkpoints().await
    }

    /// Get the latest checkpoint via gRPC (faster than GraphQL for this).
    /// Returns error if gRPC is not configured or checkpoint not found.
    pub async fn get_latest_checkpoint_grpc(&self) -> Result<GrpcCheckpoint> {
        let grpc = self.grpc.as_ref().ok_or_else(|| {
            anyhow!("gRPC not configured. Use with_grpc_endpoint() to enable streaming.")
        })?;
        grpc.get_latest_checkpoint()
            .await?
            .ok_or_else(|| anyhow!("No checkpoint found"))
    }

    /// Fetch a specific checkpoint by sequence number via gRPC.
    /// Returns error if gRPC is not configured or checkpoint not found.
    pub async fn get_checkpoint_grpc(&self, sequence_number: u64) -> Result<GrpcCheckpoint> {
        let grpc = self.grpc.as_ref().ok_or_else(|| {
            anyhow!("gRPC not configured. Use with_grpc_endpoint() to enable streaming.")
        })?;
        grpc.get_checkpoint(sequence_number)
            .await?
            .ok_or_else(|| anyhow!("Checkpoint {} not found", sequence_number))
    }

    /// Batch fetch transactions by digest via gRPC.
    /// More efficient than individual fetches for multiple transactions.
    /// Returns error if gRPC is not configured.
    /// Note: Returns only transactions that were found (filters out None results).
    pub async fn batch_get_transactions_grpc(
        &self,
        digests: &[&str],
    ) -> Result<Vec<GrpcTransaction>> {
        let grpc = self.grpc.as_ref().ok_or_else(|| {
            anyhow!("gRPC not configured. Use with_grpc_endpoint() to enable streaming.")
        })?;
        let results = grpc.batch_get_transactions(digests).await?;
        Ok(results.into_iter().flatten().collect())
    }

    /// Get direct access to the gRPC client (for advanced use).
    pub fn grpc(&self) -> Option<&GrpcClient> {
        self.grpc.as_ref()
    }

    // ========== Internal Helpers ==========

    /// Try primary source, then fallback if enabled.
    /// Returns the result from whichever source succeeded.
    fn try_with_fallback<T, F1, F2>(&self, primary: F1, fallback: F2) -> Result<T>
    where
        F1: FnOnce() -> Result<T>,
        F2: FnOnce() -> Result<T>,
    {
        match primary() {
            Ok(result) => Ok(result),
            Err(_) if self.use_fallback => fallback(),
            Err(e) => Err(e),
        }
    }

    // ========== Object Fetching ==========

    /// Fetch an object by address, with cache-first strategy and automatic fallback.
    ///
    /// The lookup order is:
    /// 1. Check cache (fastest, no network)
    /// 2. Try primary network source (GraphQL by default)
    /// 3. Fall back to secondary source (JSON-RPC)
    ///
    /// If write-through is enabled (default), network fetches are cached for future use.
    pub fn fetch_object(&self, address: &str) -> Result<FetchedObjectData> {
        // Try cache first if enabled
        if let Some(ref cache_lock) = self.cache {
            if let Ok(cache) = cache_lock.read() {
                if let Ok(Some(obj)) = cache.get_object(address) {
                    return Ok(FetchedObjectData {
                        address: obj.address,
                        version: obj.version,
                        type_string: obj.type_tag,
                        bcs_bytes: Some(obj.bcs_bytes),
                        is_shared: obj.is_shared,
                        is_immutable: obj.is_immutable,
                        source: DataSource::Cache,
                    });
                }
            }
        }

        // Fall back to network
        let result = if self.prefer_graphql {
            self.try_with_fallback(
                || self.fetch_object_graphql(address),
                || self.fetch_object_json_rpc(address),
            )
        } else {
            self.try_with_fallback(
                || self.fetch_object_json_rpc(address),
                || self.fetch_object_graphql(address),
            )
        }?;

        // Write-through: cache the network result
        if self.write_through {
            if let Some(ref cache_lock) = self.cache {
                if let Ok(mut cache) = cache_lock.write() {
                    if let Some(ref bytes) = result.bcs_bytes {
                        let _ = cache.put_object(
                            &result.address,
                            result.version,
                            result.type_string.clone(),
                            bytes.clone(),
                        );
                    }
                }
            }
        }

        Ok(result)
    }

    /// Fetch object using JSON-RPC only.
    fn fetch_object_json_rpc(&self, address: &str) -> Result<FetchedObjectData> {
        let fetched = self.json_rpc.fetch_object_full(address)?;

        Ok(FetchedObjectData {
            address: address.to_string(),
            version: fetched.version,
            type_string: fetched.type_string,
            bcs_bytes: Some(fetched.bcs_bytes),
            is_shared: fetched.is_shared,
            is_immutable: fetched.is_immutable,
            source: DataSource::JsonRpc,
        })
    }

    /// Fetch object using GraphQL only.
    fn fetch_object_graphql(&self, address: &str) -> Result<FetchedObjectData> {
        let obj = self.graphql.fetch_object(address)?;
        graphql_object_to_fetched(obj)
    }

    /// Fetch object at a specific version.
    pub fn fetch_object_at_version(
        &self,
        address: &str,
        version: u64,
    ) -> Result<FetchedObjectData> {
        if self.prefer_graphql {
            self.try_with_fallback(
                || self.fetch_object_at_version_graphql(address, version),
                || self.fetch_object_at_version_json_rpc(address, version),
            )
        } else {
            self.try_with_fallback(
                || self.fetch_object_at_version_json_rpc(address, version),
                || self.fetch_object_at_version_graphql(address, version),
            )
        }
    }

    fn fetch_object_at_version_json_rpc(
        &self,
        address: &str,
        version: u64,
    ) -> Result<FetchedObjectData> {
        let bcs_bytes = self.json_rpc.fetch_object_at_version(address, version)?;

        Ok(FetchedObjectData {
            address: address.to_string(),
            version,
            type_string: None, // JSON-RPC tryGetPastObject doesn't always return type
            bcs_bytes: Some(bcs_bytes),
            is_shared: false,
            is_immutable: false,
            source: DataSource::JsonRpc,
        })
    }

    fn fetch_object_at_version_graphql(
        &self,
        address: &str,
        version: u64,
    ) -> Result<FetchedObjectData> {
        let obj = self.graphql.fetch_object_at_version(address, version)?;
        graphql_object_to_fetched(obj)
    }

    /// Fetch a package by address, with cache-first strategy and automatic fallback.
    ///
    /// The lookup order is:
    /// 1. Check cache (fastest, no network)
    /// 2. Try primary network source (GraphQL by default)
    /// 3. Fall back to secondary source (JSON-RPC)
    ///
    /// If write-through is enabled (default), network fetches are cached for future use.
    /// Returns module names and their bytecode.
    pub fn fetch_package(&self, address: &str) -> Result<FetchedPackageData> {
        // Try cache first if enabled
        if let Some(ref cache_lock) = self.cache {
            if let Ok(cache) = cache_lock.read() {
                if let Ok(Some(pkg)) = cache.get_package(address) {
                    return Ok(FetchedPackageData {
                        address: pkg.address,
                        version: pkg.version,
                        modules: pkg
                            .modules
                            .into_iter()
                            .map(|(name, bytecode)| FetchedModuleData { name, bytecode })
                            .collect(),
                        source: DataSource::Cache,
                    });
                }
            }
        }

        // Fall back to network
        let result = if self.prefer_graphql {
            self.try_with_fallback(
                || self.fetch_package_graphql(address),
                || self.fetch_package_json_rpc(address),
            )
        } else {
            self.try_with_fallback(
                || self.fetch_package_json_rpc(address),
                || self.fetch_package_graphql(address),
            )
        }?;

        // Write-through: cache the network result
        if self.write_through {
            if let Some(ref cache_lock) = self.cache {
                if let Ok(mut cache) = cache_lock.write() {
                    let modules: Vec<(String, Vec<u8>)> = result
                        .modules
                        .iter()
                        .map(|m| (m.name.clone(), m.bytecode.clone()))
                        .collect();
                    let _ = cache.put_package(&result.address, result.version, modules);
                }
            }
        }

        Ok(result)
    }

    fn fetch_package_json_rpc(&self, address: &str) -> Result<FetchedPackageData> {
        let modules = self.json_rpc.fetch_package_modules(address)?;

        if modules.is_empty() {
            return Err(anyhow!("No modules found in package {}", address));
        }

        Ok(FetchedPackageData {
            address: address.to_string(),
            version: 1, // JSON-RPC doesn't provide version, assume 1
            modules: modules
                .into_iter()
                .map(|(name, bytes)| FetchedModuleData {
                    name,
                    bytecode: bytes,
                })
                .collect(),
            source: DataSource::JsonRpc,
        })
    }

    fn fetch_package_graphql(&self, address: &str) -> Result<FetchedPackageData> {
        let pkg = self.graphql.fetch_package(address)?;

        use base64::Engine;
        let modules = pkg
            .modules
            .into_iter()
            .filter_map(|m| {
                let bytecode = m
                    .bytecode_base64
                    .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(&b64).ok())?;
                Some(FetchedModuleData {
                    name: m.name,
                    bytecode,
                })
            })
            .collect::<Vec<_>>();

        if modules.is_empty() {
            return Err(anyhow!(
                "No modules with bytecode found in package {}",
                address
            ));
        }

        Ok(FetchedPackageData {
            address: pkg.address,
            version: pkg.version,
            modules,
            source: DataSource::GraphQL,
        })
    }

    /// Search for objects by type (GraphQL only - not available in JSON-RPC).
    pub fn search_objects_by_type(
        &self,
        type_filter: &str,
        limit: usize,
    ) -> Result<Vec<FetchedObjectData>> {
        let objects = self.graphql.search_objects_by_type(type_filter, limit)?;
        objects.into_iter().map(graphql_object_to_fetched).collect()
    }

    // ========== Transaction Fetching ==========

    /// Fetch a transaction by digest with full PTB details (GraphQL only).
    ///
    /// GraphQL provides complete transaction data including effects with
    /// created/mutated/deleted arrays. JSON-RPC uses a different format
    /// and is not supported for this method.
    pub fn fetch_transaction(&self, digest: &str) -> Result<GraphQLTransaction> {
        self.graphql.fetch_transaction(digest)
    }

    /// Fetch recent transaction digests.
    ///
    /// For complete data, prefer [`fetch_recent_transactions_full`] which
    /// fetches transactions with full PTB data in a single query.
    pub fn fetch_recent_transactions(&self, limit: usize) -> Result<Vec<String>> {
        self.try_with_fallback(
            || self.graphql.fetch_recent_transactions(limit),
            || {
                let digests = self.json_rpc.fetch_recent_transactions(limit)?;
                Ok(digests.into_iter().map(|d| d.0).collect())
            },
        )
    }

    /// Fetch recent transactions with full PTB data in a single GraphQL query.
    /// This is the recommended method as it avoids consistency issues when fetching
    /// digests and then individual transactions separately.
    ///
    /// Note: This includes ALL transaction types (including system transactions like
    /// epoch changes, randomness updates). Use `fetch_recent_ptb_transactions` to
    /// get only programmable transactions with actual user content.
    ///
    /// Currently limited to 50 transactions due to GraphQL query complexity.
    pub fn fetch_recent_transactions_full(&self, limit: usize) -> Result<Vec<GraphQLTransaction>> {
        self.graphql.fetch_recent_transactions_full(limit)
    }

    /// Fetch recent programmable transactions only (filters out system transactions).
    ///
    /// This is the recommended method for getting user transactions for analysis,
    /// as it excludes system transactions (epoch changes, randomness, etc.) that
    /// have no sender, zero gas budget, and no commands.
    pub fn fetch_recent_ptb_transactions(&self, limit: usize) -> Result<Vec<GraphQLTransaction>> {
        self.graphql.fetch_recent_ptb_transactions(limit)
    }

    /// Get access to the underlying JSON-RPC fetcher (for legacy compatibility).
    pub fn json_rpc(&self) -> &TransactionFetcher {
        &self.json_rpc
    }

    /// Get access to the underlying GraphQL client.
    pub fn graphql(&self) -> &GraphQLClient {
        &self.graphql
    }

    // ========== Dynamic Field Fetching ==========

    /// Fetch dynamic fields (children) of an object.
    ///
    /// This is used to enumerate child objects stored via dynamic_field::add.
    /// Returns info about each child including name/key and value.
    pub fn fetch_dynamic_fields(
        &self,
        parent_address: &str,
        limit: usize,
    ) -> Result<Vec<crate::graphql::DynamicFieldInfo>> {
        self.graphql.fetch_dynamic_fields(parent_address, limit)
    }

    /// Recursively fetch all dynamic field children of an object and its nested children.
    ///
    /// This is essential for replaying transactions that use complex data structures like
    /// Tables, Bags, LinkedTables, and skip_lists. These structures store their data as
    /// dynamic field children, and nested structures may have multiple levels.
    ///
    /// # Arguments
    /// * `root_address` - The parent object's address
    /// * `max_depth` - Maximum recursion depth (0 = just direct children)
    /// * `max_total` - Maximum total children to fetch across all levels
    ///
    /// # Returns
    /// A vector of (parent_address, child_object_id, object_data) tuples
    pub fn fetch_dynamic_fields_recursive(
        &self,
        root_address: &str,
        max_depth: usize,
        max_total: usize,
    ) -> Result<Vec<FetchedDynamicChild>> {
        let mut all_children = Vec::new();
        let mut queue = vec![(root_address.to_string(), 0usize)];
        let mut visited = std::collections::HashSet::new();
        visited.insert(root_address.to_string());

        while let Some((parent_addr, depth)) = queue.pop() {
            if all_children.len() >= max_total {
                break;
            }

            // Fetch children of this parent
            let remaining = max_total.saturating_sub(all_children.len());
            let fields = self.fetch_dynamic_fields(&parent_addr, remaining.min(50))?;

            for field in fields {
                if all_children.len() >= max_total {
                    break;
                }

                // Only process if it's an actual object (not just a MoveValue)
                if let Some(ref child_id) = field.object_id {
                    // Add to results
                    all_children.push(FetchedDynamicChild {
                        parent_address: parent_addr.clone(),
                        child_address: child_id.clone(),
                        name_type: field.name_type.clone(),
                        name_bcs: field.decode_name_bcs(),
                        value_type: field.value_type.clone(),
                        value_bcs: field.decode_value_bcs(),
                        version: field.version,
                    });

                    // If we haven't reached max depth, queue this child for recursive fetching
                    if depth < max_depth && !visited.contains(child_id) {
                        visited.insert(child_id.clone());
                        queue.push((child_id.clone(), depth + 1));
                    }
                }
            }
        }

        Ok(all_children)
    }
}

/// A dynamic field child fetched from mainnet.
///
/// This contains all the information needed to pre-load the child into
/// the VM's ObjectRuntime for replay.
#[derive(Debug, Clone)]
pub struct FetchedDynamicChild {
    /// Address of the parent object (the one that owns this child)
    pub parent_address: String,
    /// Object ID of this child (the dynamic field wrapper object)
    pub child_address: String,
    /// Type of the key/name used to store this child
    pub name_type: String,
    /// BCS-encoded key/name bytes
    pub name_bcs: Option<Vec<u8>>,
    /// Type of the stored value
    pub value_type: Option<String>,
    /// BCS-encoded value bytes
    pub value_bcs: Option<Vec<u8>>,
    /// Version of the child object
    pub version: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetcher_creation() {
        // Just test that we can create fetchers without panicking
        let _mainnet = DataFetcher::mainnet();
        let _testnet = DataFetcher::testnet();
        let _custom = DataFetcher::new(
            "https://fullnode.mainnet.sui.io:443",
            "https://graphql.mainnet.sui.io/graphql",
        );
    }

    /// Test unified package fetching with automatic fallback
    /// Run with: cargo test test_unified_package_fetch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_unified_package_fetch() {
        let fetcher = DataFetcher::mainnet();

        println!("=== Testing Unified Package Fetching ===\n");

        // These packages failed with JSON-RPC but work with GraphQL
        let test_packages = [
            (
                "Artipedia",
                "0x13fe3a7422946badff042be0e6dbbb0686fbff3fabc0c86cedc2d7a029486ece",
            ),
            (
                "Campaign",
                "0x9f6de0f9c1333cecfafed4fd51ecf445d237a6295bd6ae88754821c8f8189789",
            ),
            ("Sui Framework", "0x2"), // Should work with JSON-RPC
        ];

        for (name, addr) in test_packages {
            print!("{}: ", name);
            match fetcher.fetch_package(addr) {
                Ok(pkg) => {
                    let total_bytes: usize = pkg.modules.iter().map(|m| m.bytecode.len()).sum();
                    println!(
                        "SUCCESS via {:?} ({} modules, {} bytes total)",
                        pkg.source,
                        pkg.modules.len(),
                        total_bytes
                    );
                }
                Err(e) => println!("FAILED: {}", e),
            }
        }
    }
}
