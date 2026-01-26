//! **DEPRECATED**: Use [`sui_state_fetcher::HistoricalStateProvider`] instead.
#![allow(deprecated)] // This module is deprecated but still functional for backwards compatibility
//!
//! This module is deprecated in favor of the `sui-state-fetcher` crate which provides:
//! - **Versioned caching**: Objects cached by `(id, version)` for historical replay
//! - **Unified API**: Single `fetch_replay_state()` call fetches everything needed
//! - **Proper linkage resolution**: Automatic package dependency resolution
//!
//! # Migration Guide
//!
//! Before (deprecated):
//! ```ignore
//! use sui_sandbox::data_fetcher::DataFetcher;
//!
//! let fetcher = DataFetcher::mainnet();
//! let obj = fetcher.fetch_object("0x...")?;
//! ```
//!
//! After (recommended):
//! ```ignore
//! use sui_state_fetcher::HistoricalStateProvider;
//!
//! let provider = HistoricalStateProvider::mainnet().await?;
//! let state = provider.fetch_replay_state("digest...").await?;
//! // state.objects, state.packages, state.transaction all included
//! ```
//!
//! # Why Deprecated?
//!
//! The cache in this module stores objects by ID only, not by version.
//! This is fundamentally incompatible with historical transaction replay,
//! which requires objects at their exact historical versions.
//!
//! The `sui_state_fetcher` crate provides a version-aware cache that correctly
//! handles historical state, making replay accurate and reliable.
//!
//! # Cache-First Strategy
//!
//! When a cache is configured, all package and object fetches check the cache first.
//! Network fetches are automatically written back to cache (write-through caching).
//!
//! ```no_run
//! use sui_sandbox::data_fetcher::DataFetcher;
//!
//! // Enable cache with write-through
//! let fetcher = DataFetcher::mainnet()
//!     .with_cache(".tx-cache").unwrap();
//!
//! // First fetch: cache miss → network → cache write
//! let pkg = fetcher.fetch_package("0x123");  // Source: GraphQL
//!
//! // Second fetch: cache hit
//! let pkg = fetcher.fetch_package("0x123");  // Source: Cache
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
//! ```no_run
//! use sui_sandbox::data_fetcher::DataFetcher;
//!
//! // Basic queries with cache
//! let fetcher = DataFetcher::mainnet()
//!     .with_cache(".tx-cache").unwrap();
//! let obj = fetcher.fetch_object("0x...");
//! let pkg = fetcher.fetch_package("0x2");
//! let tx = fetcher.fetch_transaction("digest...");
//!
//! // Real-time streaming requires async runtime - see examples/
//! ```
//!
//! See [`DATA_FETCHING.md`](../../docs/guides/DATA_FETCHING.md) for detailed tradeoffs.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

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

/// Unified data fetcher with cache-first strategy.
///
/// The DataFetcher provides a unified interface for fetching blockchain data with:
/// - **Cache**: Check local cache first (fastest, no network)
/// - **GraphQL**: Primary network backend for all queries
/// - **gRPC**: Optional streaming and batch operations
/// - **Write-through**: Automatically cache network fetches
#[deprecated(
    since = "0.9.0",
    note = "Use sui_state_fetcher::HistoricalStateProvider instead. DataFetcher's cache is not version-aware, making it unsuitable for historical transaction replay."
)]
pub struct DataFetcher {
    graphql: GraphQLClient,
    /// Optional gRPC client for streaming (requires provider endpoint)
    grpc: Option<GrpcClient>,
    /// Unified cache manager (replaces legacy tx_cache)
    /// Uses parking_lot::RwLock for better performance and simpler API (no poisoning)
    cache: Option<std::sync::Arc<parking_lot::RwLock<crate::cache::CacheManager>>>,
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
    /// Create a fetcher for mainnet with default settings.
    /// gRPC is not enabled by default (requires provider endpoint).
    /// Cache is not enabled by default - use `with_cache` to add it.
    #[must_use]
    pub fn mainnet() -> Self {
        Self {
            graphql: GraphQLClient::mainnet(),
            grpc: None,
            cache: None,
            write_through: true,
        }
    }

    /// Create a fetcher for testnet.
    /// gRPC is not enabled by default (requires provider endpoint).
    /// Cache is not enabled by default - use `with_cache` to add it.
    #[must_use]
    pub fn testnet() -> Self {
        Self {
            graphql: GraphQLClient::testnet(),
            grpc: None,
            cache: None,
            write_through: true,
        }
    }

    /// Create with a custom GraphQL endpoint.
    /// gRPC and cache are not enabled by default.
    #[must_use]
    pub fn new(graphql_endpoint: &str) -> Self {
        Self {
            graphql: GraphQLClient::new(graphql_endpoint),
            grpc: None,
            cache: None,
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
    /// ```no_run
    /// use sui_sandbox::data_fetcher::DataFetcher;
    ///
    /// let fetcher = DataFetcher::mainnet()
    ///     .with_cache(".tx-cache").unwrap();
    ///
    /// // First fetch: cache miss → network → cache write
    /// let pkg = fetcher.fetch_package("0x123");  // Source: GraphQL
    ///
    /// // Second fetch: cache hit
    /// let pkg = fetcher.fetch_package("0x123");  // Source: Cache
    /// ```
    pub fn with_cache<P: AsRef<std::path::Path>>(mut self, cache_dir: P) -> Result<Self> {
        use std::sync::Arc;
        let manager = crate::cache::CacheManager::new(cache_dir)?;
        self.cache = Some(Arc::new(parking_lot::RwLock::new(manager)));
        Ok(self)
    }

    /// Enable local transaction cache, ignoring errors if cache doesn't exist.
    ///
    /// This is useful for optional caching where you want to use it if available
    /// but don't want to fail if the cache directory doesn't exist.
    #[must_use]
    pub fn with_cache_optional<P: AsRef<std::path::Path>>(mut self, cache_dir: P) -> Self {
        use std::sync::Arc;
        if let Ok(manager) = crate::cache::CacheManager::new(cache_dir) {
            if !manager.is_empty() {
                self.cache = Some(Arc::new(parking_lot::RwLock::new(manager)));
            }
        }
        self
    }

    /// Enable or disable write-through caching.
    ///
    /// When enabled (default), network fetches are automatically written to cache.
    /// Disable for read-only cache access.
    #[must_use]
    pub fn with_write_through(mut self, enabled: bool) -> Self {
        self.write_through = enabled;
        self
    }

    /// Check if cache is enabled and has data.
    pub fn has_cache(&self) -> bool {
        self.cache
            .as_ref()
            .map(|c| !c.read().is_empty())
            .unwrap_or(false)
    }

    /// Get cache statistics (packages, objects, transactions, disk size).
    pub fn cache_stats(&self) -> Option<crate::cache::CacheStats> {
        self.cache.as_ref().map(|c| c.read().stats())
    }

    /// Get basic cache counts (packages and objects indexed).
    /// For full stats, use `cache_stats()`.
    pub fn cache_counts(&self) -> Option<(usize, usize)> {
        self.cache.as_ref().map(|c| {
            let cache = c.read();
            (cache.package_count(), cache.object_count())
        })
    }

    /// Add gRPC client for real-time streaming capabilities.
    ///
    /// Note: Sui's public fullnodes now support gRPC:
    /// - `https://fullnode.mainnet.sui.io:443`
    /// - `https://archive.mainnet.sui.io:443` (historical queries, no streaming)
    ///
    /// Once added, you can use `subscribe_checkpoints()` for real-time data.
    pub async fn with_grpc_endpoint(mut self, endpoint: &str) -> Result<Self> {
        self.grpc = Some(GrpcClient::new(endpoint).await?);
        Ok(self)
    }

    /// Add a pre-configured gRPC client.
    #[must_use]
    pub fn with_grpc_client(mut self, client: GrpcClient) -> Self {
        self.grpc = Some(client);
        self
    }

    /// Check if gRPC streaming is available.
    pub fn has_grpc(&self) -> bool {
        self.grpc.is_some()
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
    ///
    /// See `examples/` directory for async streaming examples.
    /// Requires a tokio runtime and gRPC endpoint configured via `with_grpc_endpoint()`.
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

    // ========== Object Fetching ==========

    /// Fetch an object by address, with cache-first strategy.
    ///
    /// The lookup order is:
    /// 1. Check cache (fastest, no network)
    /// 2. Fetch from GraphQL
    ///
    /// If write-through is enabled (default), network fetches are cached for future use.
    pub fn fetch_object(&self, address: &str) -> Result<FetchedObjectData> {
        // Try cache first if enabled
        if let Some(ref cache_lock) = self.cache {
            let cache = cache_lock.read();
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

        // Fetch from GraphQL
        let obj = self.graphql.fetch_object(address)?;
        let result = graphql_object_to_fetched(obj)?;

        // Write-through: cache the network result
        if self.write_through {
            if let Some(ref cache_lock) = self.cache {
                let mut cache = cache_lock.write();
                if let Some(ref bytes) = result.bcs_bytes {
                    if let Err(e) = cache.put_object(
                        &result.address,
                        result.version,
                        result.type_string.clone(),
                        bytes.clone(),
                    ) {
                        eprintln!("warning: failed to cache object {}: {}", result.address, e);
                    }
                }
            }
        }

        Ok(result)
    }

    /// Fetch object at a specific version.
    pub fn fetch_object_at_version(
        &self,
        address: &str,
        version: u64,
    ) -> Result<FetchedObjectData> {
        let obj = self.graphql.fetch_object_at_version(address, version)?;
        graphql_object_to_fetched(obj)
    }

    /// Fetch a package by address, with cache-first strategy.
    ///
    /// The lookup order is:
    /// 1. Check cache (fastest, no network)
    /// 2. Fetch from GraphQL
    ///
    /// If write-through is enabled (default), network fetches are cached for future use.
    /// Returns module names and their bytecode.
    pub fn fetch_package(&self, address: &str) -> Result<FetchedPackageData> {
        // Try cache first if enabled
        if let Some(ref cache_lock) = self.cache {
            let cache = cache_lock.read();
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

        // Fetch from GraphQL
        let pkg = self.graphql.fetch_package(address)?;

        use base64::Engine;
        let mut modules = Vec::new();
        let mut decode_errors = Vec::new();

        for m in pkg.modules {
            match m.bytecode_base64 {
                Some(b64) => match base64::engine::general_purpose::STANDARD.decode(&b64) {
                    Ok(bytecode) => modules.push(FetchedModuleData {
                        name: m.name,
                        bytecode,
                    }),
                    Err(e) => decode_errors.push(format!("{}: {}", m.name, e)),
                },
                None => decode_errors.push(format!("{}: missing bytecode_base64", m.name)),
            }
        }

        if !decode_errors.is_empty() {
            eprintln!(
                "warning: {} module(s) failed to decode in package {}: {}",
                decode_errors.len(),
                address,
                decode_errors.join(", ")
            );
        }

        if modules.is_empty() {
            return Err(anyhow!(
                "No modules with bytecode found in package {}",
                address
            ));
        }

        let result = FetchedPackageData {
            address: pkg.address,
            version: pkg.version,
            modules,
            source: DataSource::GraphQL,
        };

        // Write-through: cache the network result
        if self.write_through {
            if let Some(ref cache_lock) = self.cache {
                let mut cache = cache_lock.write();
                let modules: Vec<(String, Vec<u8>)> = result
                    .modules
                    .iter()
                    .map(|m| (m.name.clone(), m.bytecode.clone()))
                    .collect();
                if let Err(e) = cache.put_package(&result.address, result.version, modules) {
                    eprintln!("warning: failed to cache package {}: {}", result.address, e);
                }
            }
        }

        Ok(result)
    }

    /// Search for objects by type (GraphQL only).
    pub fn search_objects_by_type(
        &self,
        type_filter: &str,
        limit: usize,
    ) -> Result<Vec<FetchedObjectData>> {
        let objects = self.graphql.search_objects_by_type(type_filter, limit)?;
        objects.into_iter().map(graphql_object_to_fetched).collect()
    }

    // ========== Transaction Fetching ==========

    /// Fetch a transaction by digest with full PTB details.
    ///
    /// GraphQL provides complete transaction data including effects with
    /// created/mutated/deleted arrays.
    pub fn fetch_transaction(&self, digest: &str) -> Result<GraphQLTransaction> {
        self.graphql.fetch_transaction(digest)
    }

    /// Fetch recent transaction digests.
    ///
    /// For complete data, prefer [`fetch_recent_transactions_full`] which
    /// fetches transactions with full PTB data in a single query.
    pub fn fetch_recent_transactions(&self, limit: usize) -> Result<Vec<String>> {
        self.graphql.fetch_recent_transactions(limit)
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

    /// Get access to the underlying GraphQL client.
    pub fn graphql(&self) -> &GraphQLClient {
        &self.graphql
    }

    /// Extract all package IDs referenced in a transaction's MoveCall commands.
    ///
    /// This is useful for determining which packages need to be fetched
    /// before replaying a transaction.
    pub fn extract_package_ids(tx: &GraphQLTransaction) -> Vec<String> {
        use crate::graphql::GraphQLCommand;
        use std::collections::BTreeSet;
        let mut packages = BTreeSet::new();
        for cmd in &tx.commands {
            if let GraphQLCommand::MoveCall {
                package,
                type_arguments,
                ..
            } = cmd
            {
                // Add the called package
                packages.insert(package.clone());

                // Also extract packages from type arguments (e.g., "0xabc::module::Type<0xdef::mod::T>")
                for type_arg in type_arguments {
                    Self::extract_packages_from_type_string(type_arg, &mut packages);
                }
            }
        }
        packages.into_iter().collect()
    }

    /// Extract package addresses from a type string like "0xabc::module::Type<0xdef::mod::T>".
    fn extract_packages_from_type_string(
        type_str: &str,
        packages: &mut std::collections::BTreeSet<String>,
    ) {
        // Find all 0x... patterns that look like package addresses
        // A package address is followed by ::module_name
        let mut chars = type_str.chars().peekable();
        let mut current_addr = String::new();
        let mut in_addr = false;

        while let Some(ch) = chars.next() {
            if ch == '0' && chars.peek() == Some(&'x') {
                // Start of a potential address
                in_addr = true;
                current_addr.clear();
                current_addr.push(ch);
            } else if in_addr {
                if ch.is_ascii_hexdigit() || ch == 'x' {
                    current_addr.push(ch);
                } else if ch == ':' && chars.peek() == Some(&':') {
                    // This looks like a package address (0x...::)
                    if current_addr.len() > 2 {
                        // Normalize to full 64-char hex address
                        let addr = current_addr.strip_prefix("0x").unwrap_or(&current_addr);
                        let normalized = format!("0x{:0>64}", addr);
                        packages.insert(normalized);
                    }
                    in_addr = false;
                    current_addr.clear();
                } else {
                    in_addr = false;
                    current_addr.clear();
                }
            }
        }
    }

    /// Fetch all input objects for a transaction.
    ///
    /// Returns a map of object_id -> BCS bytes for all OwnedObject and SharedObject
    /// inputs in the transaction. Pure inputs are skipped as they don't require fetching.
    ///
    /// For SharedObject inputs, fetches at their initial_shared_version.
    /// For OwnedObject inputs, fetches at their specified version.
    pub fn fetch_transaction_inputs(
        &self,
        tx: &GraphQLTransaction,
    ) -> Result<std::collections::HashMap<String, Vec<u8>>> {
        use crate::graphql::GraphQLTransactionInput;
        use std::collections::HashMap;
        let mut objects = HashMap::new();

        for input in &tx.inputs {
            match input {
                GraphQLTransactionInput::OwnedObject {
                    address, version, ..
                } => {
                    let obj = self.fetch_object_at_version(address, *version)?;
                    if let Some(bcs) = obj.bcs_bytes {
                        objects.insert(address.clone(), bcs);
                    }
                }
                GraphQLTransactionInput::SharedObject {
                    address,
                    initial_shared_version,
                    ..
                } => {
                    let obj = self.fetch_object_at_version(address, *initial_shared_version)?;
                    if let Some(bcs) = obj.bcs_bytes {
                        objects.insert(address.clone(), bcs);
                    }
                }
                GraphQLTransactionInput::Receiving {
                    address, version, ..
                } => {
                    let obj = self.fetch_object_at_version(address, *version)?;
                    if let Some(bcs) = obj.bcs_bytes {
                        objects.insert(address.clone(), bcs);
                    }
                }
                GraphQLTransactionInput::Pure { .. } => {
                    // Pure inputs don't need fetching
                }
            }
        }

        Ok(objects)
    }

    /// Fetch all packages referenced in a transaction.
    ///
    /// Returns a map of package_id -> Vec<(module_name, module_bytecode)>.
    pub fn fetch_transaction_packages(
        &self,
        tx: &GraphQLTransaction,
    ) -> Result<std::collections::HashMap<String, Vec<(String, Vec<u8>)>>> {
        use std::collections::HashMap;
        let package_ids = Self::extract_package_ids(tx);
        let mut packages = HashMap::new();

        for pkg_id in package_ids {
            let pkg = self.fetch_package(&pkg_id)?;
            let modules: Vec<(String, Vec<u8>)> = pkg
                .modules
                .into_iter()
                .map(|m| (m.name, m.bytecode))
                .collect();
            packages.insert(pkg_id, modules);
        }

        Ok(packages)
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

// Tests removed:
// - test_fetcher_creation: Had zero assertions, only verified no panic on construction
// - test_unified_package_fetch: Was #[ignore], required network, used println instead of assertions
