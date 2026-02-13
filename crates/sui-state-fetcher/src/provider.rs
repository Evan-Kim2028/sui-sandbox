//! HistoricalStateProvider - Unified historical state fetching.
//!
//! This is the main entry point for fetching all state needed to replay a transaction.
//! It unifies gRPC and GraphQL access behind a single interface with versioned caching.
//!
//! # Example
//!
//! ```ignore
//! use sui_state_fetcher::HistoricalStateProvider;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let provider = HistoricalStateProvider::mainnet().await?;
//!
//!     // Fetch everything needed to replay a transaction
//!     let state = provider.fetch_replay_state("8JTTa...").await?;
//!
//!     // state.transaction - the PTB commands
//!     // state.objects - all objects at their input versions
//!     // state.packages - all packages with linkage resolved
//!     Ok(())
//! }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde_json::Value;
use sui_sandbox_types::env_var_or;
use tokio::sync::{Mutex, Notify};
use tracing::debug;

use sui_prefetch::grpc_to_fetched_transaction;
use sui_resolver::address::normalize_address;
use sui_transport::graphql::{GraphQLClient, GraphQLPackage};
use sui_transport::grpc::GrpcClient;
use sui_transport::walrus::WalrusClient;
use sui_types::move_package::MovePackage;

use sui_historical_cache::{
    DynamicFieldEntry, FsDynamicFieldCache, FsObjectIndex, FsObjectStore, FsPackageIndex,
    FsTxDigestIndex, ObjectMeta, ObjectVersionStore,
};

use crate::cache::VersionedCache;
use crate::types::{ObjectID, PackageData, ReplayState, VersionedObject};

/// Unified provider for historical state fetching.
///
/// Combines gRPC (for transactions and versioned objects) and GraphQL
/// (for packages and dynamic field discovery) behind a single interface.
///
/// This is a purely async API - use within a tokio runtime context.
pub struct HistoricalStateProvider {
    /// gRPC client for transactions and versioned object fetching.
    grpc: GrpcClient,

    /// GraphQL client for packages and supplemental queries.
    graphql: GraphQLClient,

    /// Versioned cache for objects and packages.
    cache: Arc<VersionedCache>,

    /// gRPC endpoint URL for creating new clients (needed for on-demand fetcher).
    grpc_endpoint: String,

    /// Optional Walrus checkpoint source (HTTP aggregator + cache).
    walrus: Option<WalrusClient>,

    /// Optional local filesystem object store (Walrus-backed).
    local_object_store: Option<Arc<FsObjectStore>>,

    /// Optional local object index (object_id + version -> checkpoint).
    local_object_index: Option<Arc<FsObjectIndex>>,

    /// Optional local tx digest index (digest -> checkpoint).
    local_tx_index: Option<Arc<FsTxDigestIndex>>,

    /// Optional local dynamic field cache (parent -> children).
    local_dynamic_fields: Option<Arc<FsDynamicFieldCache>>,

    /// Optional local package index (package_id -> checkpoint).
    local_package_index: Option<Arc<FsPackageIndex>>,

    /// Walrus checkpoint fetch pool for deduped, concurrent fetches.
    walrus_pool: Arc<WalrusCheckpointPool>,
}

/// Default mainnet gRPC endpoint
const MAINNET_GRPC: &str = "https://fullnode.mainnet.sui.io:443";
/// Default testnet gRPC endpoint
const TESTNET_GRPC: &str = "https://fullnode.testnet.sui.io:443";
/// Clock object ID (0x6) - well-known system object
const CLOCK_OBJECT_ID: &str = "0x0000000000000000000000000000000000000000000000000000000000000006";
/// Random object ID (0x8) - well-known system object
const RANDOM_OBJECT_ID: &str = "0x0000000000000000000000000000000000000000000000000000000000000008";
/// Clock type string
const CLOCK_TYPE: &str = "0x2::clock::Clock";
/// Random type string
const RANDOM_TYPE: &str = "0x2::random::Random";
/// Default Clock timestamp base (2024-01-01 00:00:00 UTC)
const DEFAULT_CLOCK_BASE_MS: u64 = 1_704_067_200_000;

const WALRUS_MAINNET_CACHE_URL: &str = "https://walrus-sui-archival.mainnet.walrus.space";
const WALRUS_MAINNET_AGGREGATOR_URL: &str = "https://aggregator.walrus-mainnet.walrus.space";
const WALRUS_TESTNET_CACHE_URL: &str = "https://walrus-sui-archival.testnet.walrus.space";
const WALRUS_TESTNET_AGGREGATOR_URL: &str = "https://aggregator.walrus-testnet.walrus.space";

struct WalrusCheckpointPool {
    cache: Mutex<HashMap<u64, Arc<Value>>>,
    inflight: Mutex<HashMap<u64, Arc<Notify>>>,
}

impl WalrusCheckpointPool {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
        }
    }

    async fn get(&self, walrus: &WalrusClient, checkpoint: u64) -> Option<Arc<Value>> {
        {
            let cache = self.cache.lock().await;
            if let Some(val) = cache.get(&checkpoint) {
                return Some(val.clone());
            }
        }

        let (notify, is_leader) = {
            let mut inflight = self.inflight.lock().await;
            if let Some(existing) = inflight.get(&checkpoint) {
                (existing.clone(), false)
            } else {
                let notify = Arc::new(Notify::new());
                inflight.insert(checkpoint, notify.clone());
                (notify, true)
            }
        };

        if !is_leader {
            notify.notified().await;
            let cache = self.cache.lock().await;
            return cache.get(&checkpoint).cloned();
        }

        let walrus = walrus.clone();
        let fetched =
            tokio::task::spawn_blocking(move || walrus.get_checkpoint_json(checkpoint)).await;

        let value = match fetched {
            Ok(Ok(val)) => val,
            _ => {
                let mut inflight = self.inflight.lock().await;
                if let Some(waiters) = inflight.remove(&checkpoint) {
                    waiters.notify_waiters();
                }
                return None;
            }
        };

        let arc = Arc::new(value);
        {
            let mut cache = self.cache.lock().await;
            cache.insert(checkpoint, arc.clone());
        }
        {
            let mut inflight = self.inflight.lock().await;
            if let Some(waiters) = inflight.remove(&checkpoint) {
                waiters.notify_waiters();
            }
        }
        Some(arc)
    }
}

fn linkage_debug_enabled() -> bool {
    matches!(
        std::env::var("SUI_DEBUG_LINKAGE")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn timing_enabled() -> bool {
    matches!(
        std::env::var("SUI_DEBUG_TIMING")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn checkpoint_lookup_debug_enabled() -> bool {
    matches!(
        std::env::var("SUI_DEBUG_CHECKPOINT_LOOKUP")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn data_gap_debug_enabled() -> bool {
    matches!(
        std::env::var("SUI_DEBUG_DATA_GAPS")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn walrus_store_enabled() -> bool {
    matches!(
        std::env::var("SUI_WALRUS_LOCAL_STORE")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn walrus_store_full_ingest_enabled() -> bool {
    !matches!(
        std::env::var("SUI_WALRUS_FULL_CHECKPOINT_INGEST")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

fn sandbox_home_dir() -> PathBuf {
    std::env::var("SUI_SANDBOX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sui-sandbox")
        })
}

fn walrus_store_path_from_env() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("SUI_WALRUS_STORE_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    if walrus_store_enabled() {
        let network = std::env::var("SUI_WALRUS_NETWORK")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "mainnet".to_string());
        return Some(sandbox_home_dir().join("walrus-store").join(network));
    }
    None
}

fn walrus_store_from_env() -> Option<Arc<FsObjectStore>> {
    let dir = walrus_store_path_from_env()?;
    match FsObjectStore::new(&dir) {
        Ok(store) => Some(Arc::new(store)),
        Err(e) => {
            eprintln!(
                "[walrus_store] failed to initialize store at {}: {}",
                dir.display(),
                e
            );
            None
        }
    }
}

fn walrus_index_from_env() -> Option<Arc<FsObjectIndex>> {
    let dir = walrus_store_path_from_env()?;
    match FsObjectIndex::new(&dir) {
        Ok(index) => Some(Arc::new(index)),
        Err(e) => {
            eprintln!(
                "[walrus_index] failed to initialize index at {}: {}",
                dir.display(),
                e
            );
            None
        }
    }
}

fn walrus_tx_index_from_env() -> Option<Arc<FsTxDigestIndex>> {
    let dir = walrus_store_path_from_env()?;
    match FsTxDigestIndex::new(&dir) {
        Ok(index) => Some(Arc::new(index)),
        Err(e) => {
            eprintln!(
                "[walrus_tx_index] failed to initialize index at {}: {}",
                dir.display(),
                e
            );
            None
        }
    }
}

fn walrus_dynamic_fields_from_env() -> Option<Arc<FsDynamicFieldCache>> {
    let dir = walrus_store_path_from_env()?;
    match FsDynamicFieldCache::new(&dir) {
        Ok(cache) => Some(Arc::new(cache)),
        Err(e) => {
            eprintln!(
                "[walrus_dynamic_fields] failed to initialize cache at {}: {}",
                dir.display(),
                e
            );
            None
        }
    }
}

fn walrus_package_index_from_env() -> Option<Arc<FsPackageIndex>> {
    let dir = walrus_store_path_from_env()?;
    match FsPackageIndex::new(&dir) {
        Ok(index) => Some(Arc::new(index)),
        Err(e) => {
            eprintln!(
                "[walrus_package_index] failed to initialize index at {}: {}",
                dir.display(),
                e
            );
            None
        }
    }
}

fn walrus_recursive_enabled() -> bool {
    match std::env::var("SUI_WALRUS_RECURSIVE_LOOKUP")
        .ok()
        .as_deref()
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        Some(_) => true,
        None => walrus_store_enabled(),
    }
}

fn walrus_recursive_max_checkpoints() -> usize {
    std::env::var("SUI_WALRUS_RECURSIVE_MAX_CHECKPOINTS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(5)
}

fn walrus_recursive_max_tx_steps() -> usize {
    std::env::var("SUI_WALRUS_RECURSIVE_MAX_TX_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3)
}

fn log_package_linkage(
    pkg: &PackageData,
    source: &str,
    version_hint: Option<u64>,
    used_cache: bool,
) {
    if !linkage_debug_enabled() {
        return;
    }
    let storage_id = pkg.address.to_hex_literal();
    let original_id = pkg.original_id.map(|id| id.to_hex_literal());
    let mut linkage_pairs: Vec<String> = Vec::new();
    for (runtime, storage) in &pkg.linkage {
        linkage_pairs.push(format!(
            "{}=>{}",
            runtime.to_hex_literal(),
            storage.to_hex_literal()
        ));
    }
    eprintln!(
        "[linkage] source={} cache={} storage_id={} original_id={} version={} version_hint={:?} linkage_count={} [{}]",
        source,
        used_cache,
        storage_id,
        original_id.as_deref().unwrap_or("none"),
        pkg.version,
        version_hint,
        linkage_pairs.len(),
        linkage_pairs.join(", ")
    );
}

fn walrus_enabled() -> bool {
    matches!(
        std::env::var("SUI_WALRUS_ENABLED")
            .ok()
            .as_deref()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn walrus_default_urls(network: &str) -> (&'static str, &'static str) {
    match network {
        "testnet" => (WALRUS_TESTNET_CACHE_URL, WALRUS_TESTNET_AGGREGATOR_URL),
        _ => (WALRUS_MAINNET_CACHE_URL, WALRUS_MAINNET_AGGREGATOR_URL),
    }
}

fn walrus_from_env() -> Option<WalrusClient> {
    let cache_env = std::env::var("SUI_WALRUS_CACHE_URL").ok();
    let agg_env = std::env::var("SUI_WALRUS_AGGREGATOR_URL").ok();
    let enabled = walrus_enabled() || cache_env.is_some() || agg_env.is_some();
    if !enabled {
        return None;
    }

    let network = std::env::var("SUI_WALRUS_NETWORK")
        .ok()
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_else(|| "mainnet".to_string());
    let (default_cache, default_agg) = walrus_default_urls(&network);
    let cache_url = cache_env.unwrap_or_else(|| default_cache.to_string());
    let agg_url = agg_env.unwrap_or_else(|| default_agg.to_string());

    Some(WalrusClient::new(cache_url, agg_url))
}

impl HistoricalStateProvider {
    /// Create a provider for Sui mainnet using environment variables.
    ///
    /// Reads configuration from environment:
    /// - `SUI_GRPC_ENDPOINT` - gRPC endpoint (default: public mainnet endpoint)
    /// - `SUI_GRPC_API_KEY` - API key for authentication (optional, depends on provider)
    pub async fn mainnet() -> Result<Self> {
        let endpoint =
            std::env::var("SUI_GRPC_ENDPOINT").unwrap_or_else(|_| MAINNET_GRPC.to_string());
        let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

        let grpc = GrpcClient::with_api_key(&endpoint, api_key).await?;
        let graphql = GraphQLClient::mainnet();

        Ok(Self {
            grpc,
            graphql,
            cache: Arc::new(VersionedCache::new()),
            grpc_endpoint: endpoint,
            walrus: None,
            local_object_store: None,
            local_object_index: None,
            local_tx_index: None,
            local_dynamic_fields: None,
            local_package_index: None,
            walrus_pool: Arc::new(WalrusCheckpointPool::new()),
        })
    }

    /// Create a provider for Sui testnet.
    pub async fn testnet() -> Result<Self> {
        let grpc = GrpcClient::testnet().await?;
        let graphql = GraphQLClient::testnet();

        Ok(Self {
            grpc,
            graphql,
            cache: Arc::new(VersionedCache::new()),
            grpc_endpoint: TESTNET_GRPC.to_string(),
            walrus: None,
            local_object_store: None,
            local_object_index: None,
            local_tx_index: None,
            local_dynamic_fields: None,
            local_package_index: None,
            walrus_pool: Arc::new(WalrusCheckpointPool::new()),
        })
    }

    /// Build replay state using a configurable builder.
    pub fn replay_state_builder(&self) -> crate::replay_builder::ReplayStateBuilder<'_> {
        crate::replay_builder::ReplayStateBuilder::new(self)
    }

    /// Create a provider with custom endpoints.
    pub async fn new(grpc_endpoint: &str, graphql_endpoint: &str) -> Result<Self> {
        let grpc = GrpcClient::new(grpc_endpoint).await?;
        let graphql = GraphQLClient::new(graphql_endpoint);

        Ok(Self {
            grpc,
            graphql,
            cache: Arc::new(VersionedCache::new()),
            grpc_endpoint: grpc_endpoint.to_string(),
            walrus: None,
            local_object_store: None,
            local_object_index: None,
            local_tx_index: None,
            local_dynamic_fields: None,
            local_package_index: None,
            walrus_pool: Arc::new(WalrusCheckpointPool::new()),
        })
    }

    /// Create a provider with existing clients.
    ///
    /// Note: The gRPC endpoint is extracted from the client for on-demand fetching.
    pub fn with_clients(grpc: GrpcClient, graphql: GraphQLClient) -> Self {
        let grpc_endpoint = grpc.endpoint().to_string();

        Self {
            grpc,
            graphql,
            cache: Arc::new(VersionedCache::new()),
            grpc_endpoint,
            walrus: None,
            local_object_store: None,
            local_object_index: None,
            local_tx_index: None,
            local_dynamic_fields: None,
            local_package_index: None,
            walrus_pool: Arc::new(WalrusCheckpointPool::new()),
        }
    }

    /// Enable disk caching at the specified directory.
    pub fn with_cache_dir(mut self, cache_dir: impl AsRef<Path>) -> Result<Self> {
        self.cache = Arc::new(VersionedCache::with_storage(cache_dir)?);
        Ok(self)
    }

    /// Use an existing cache instance.
    pub fn with_cache(mut self, cache: Arc<VersionedCache>) -> Self {
        self.cache = cache;
        self
    }

    /// Enable Walrus checkpoint fetching with a custom client.
    pub fn with_walrus(mut self, walrus: WalrusClient) -> Self {
        self.walrus = Some(walrus);
        self
    }

    /// Enable Walrus checkpoint fetching from environment configuration.
    ///
    /// Uses:
    /// - `SUI_WALRUS_ENABLED` (optional, true/false)
    /// - `SUI_WALRUS_CACHE_URL` (optional)
    /// - `SUI_WALRUS_AGGREGATOR_URL` (optional)
    /// - `SUI_WALRUS_NETWORK` (optional: mainnet|testnet)
    pub fn with_walrus_from_env(mut self) -> Self {
        if let Some(client) = walrus_from_env() {
            self.walrus = Some(client);
        }
        self
    }

    /// Enable local filesystem object store for Walrus checkpoint ingestion.
    pub fn with_local_object_store(mut self, store: FsObjectStore) -> Self {
        let cache_root = store.cache_root().to_path_buf();
        self.local_object_store = Some(Arc::new(store));
        if self.local_object_index.is_none() {
            if let Ok(index) = FsObjectIndex::new(&cache_root) {
                self.local_object_index = Some(Arc::new(index));
            }
        }
        if self.local_tx_index.is_none() {
            if let Ok(index) = FsTxDigestIndex::new(&cache_root) {
                self.local_tx_index = Some(Arc::new(index));
            }
        }
        if self.local_dynamic_fields.is_none() {
            if let Ok(cache) = FsDynamicFieldCache::new(&cache_root) {
                self.local_dynamic_fields = Some(Arc::new(cache));
            }
        }
        if self.local_package_index.is_none() {
            if let Ok(index) = FsPackageIndex::new(&cache_root) {
                self.local_package_index = Some(Arc::new(index));
            }
        }
        self
    }

    /// Enable local object store from environment configuration.
    ///
    /// Uses:
    /// - `SUI_WALRUS_LOCAL_STORE` (true/false)
    /// - `SUI_WALRUS_STORE_DIR` (optional override)
    /// - `SUI_WALRUS_NETWORK` (mainnet/testnet)
    /// - `SUI_SANDBOX_HOME` (base dir)
    pub fn with_local_object_store_from_env(mut self) -> Self {
        if let Some(store) = walrus_store_from_env() {
            self.local_object_store = Some(store);
        }
        if let Some(index) = walrus_index_from_env() {
            self.local_object_index = Some(index);
        }
        if let Some(index) = walrus_tx_index_from_env() {
            self.local_tx_index = Some(index);
        }
        if let Some(cache) = walrus_dynamic_fields_from_env() {
            self.local_dynamic_fields = Some(cache);
        }
        if let Some(index) = walrus_package_index_from_env() {
            self.local_package_index = Some(index);
        }
        self
    }

    // ==================== Checkpoint Ingestion ====================

    /// Ingest packages from a single Walrus checkpoint into the local index.
    ///
    /// This fetches the checkpoint JSON from Walrus and extracts all packages,
    /// storing them in both the cache and the package index.
    ///
    /// Returns the number of packages ingested.
    pub async fn ingest_packages_from_checkpoint(&self, checkpoint: u64) -> Result<usize> {
        let walrus = self.walrus.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Walrus client not configured. Set SUI_WALRUS_ENABLED=1")
        })?;

        let checkpoint_json = self
            .walrus_pool
            .get(walrus, checkpoint)
            .await
            .ok_or_else(|| anyhow::anyhow!("Walrus checkpoint fetch failed: {}", checkpoint))?;

        let ingested = ingest_walrus_checkpoint_packages(
            checkpoint_json.as_ref(),
            &self.cache,
            self.local_package_index.as_deref(),
            checkpoint,
        );

        Ok(ingested)
    }

    /// Ingest packages from a range of Walrus checkpoints.
    ///
    /// Fetches checkpoints in parallel (up to `concurrency` at a time) and
    /// extracts all packages into the local index.
    ///
    /// Returns the total number of packages ingested.
    pub async fn ingest_packages_from_checkpoint_range(
        &self,
        start_checkpoint: u64,
        end_checkpoint: u64,
        concurrency: usize,
    ) -> Result<usize> {
        use futures::stream::{self, StreamExt};

        let walrus = self.walrus.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Walrus client not configured. Set SUI_WALRUS_ENABLED=1")
        })?;

        let checkpoints: Vec<u64> = (start_checkpoint..=end_checkpoint).collect();
        let total_ingested = std::sync::atomic::AtomicUsize::new(0);

        stream::iter(checkpoints)
            .map(|checkpoint| {
                let walrus = walrus.clone();
                let cache = self.cache.clone();
                let package_index = self.local_package_index.clone();
                let pool = self.walrus_pool.clone();
                async move {
                    match pool.get(&walrus, checkpoint).await {
                        Some(checkpoint_json) => {
                            let ingested = ingest_walrus_checkpoint_packages(
                                checkpoint_json.as_ref(),
                                &cache,
                                package_index.as_deref(),
                                checkpoint,
                            );
                            (checkpoint, Ok(ingested))
                        }
                        None => (
                            checkpoint,
                            Err(anyhow::anyhow!(
                                "Walrus checkpoint fetch failed: {}",
                                checkpoint
                            )),
                        ),
                    }
                }
            })
            .buffer_unordered(concurrency)
            .for_each(|(checkpoint, result)| {
                match result {
                    Ok(ingested) => {
                        total_ingested.fetch_add(ingested, std::sync::atomic::Ordering::Relaxed);
                        if timing_enabled() {
                            eprintln!(
                                "[timing] stage=ingest_checkpoint checkpoint={} packages={}",
                                checkpoint, ingested
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[warning] Failed to ingest checkpoint {}: {}",
                            checkpoint, e
                        );
                    }
                }
                async {}
            })
            .await;

        Ok(total_ingested.load(std::sync::atomic::Ordering::Relaxed))
    }

    async fn resolve_checkpoint_for_tx_digest(&self, digest: &str) -> Option<u64> {
        let debug_lookup = checkpoint_lookup_debug_enabled();
        let force_remote = matches!(
            std::env::var("SUI_CHECKPOINT_LOOKUP_FORCE_REMOTE")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("1") | Some("true") | Some("yes") | Some("on")
        );
        let tx_index = self.local_tx_index.as_deref();
        if let Some(tx_index) = tx_index {
            if let Ok(Some(cp)) = tx_index.get_checkpoint(digest) {
                if !force_remote {
                    if debug_lookup {
                        eprintln!(
                            "[checkpoint_lookup] digest={} source=tx_index checkpoint={}",
                            digest, cp
                        );
                    }
                    return Some(cp);
                }
                if debug_lookup {
                    eprintln!(
                        "[checkpoint_lookup] digest={} source=tx_index checkpoint={} note=forced_remote",
                        digest, cp
                    );
                }
            }
        } else if debug_lookup {
            eprintln!(
                "[checkpoint_lookup] digest={} source=tx_index result=missing (no local index)",
                digest
            );
        }
        let allow_remote = !matches!(
            std::env::var("SUI_CHECKPOINT_LOOKUP_REMOTE")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("0") | Some("false") | Some("no") | Some("off")
        );
        if !allow_remote {
            if debug_lookup {
                eprintln!(
                    "[checkpoint_lookup] digest={} source=remote_disabled result=missing",
                    digest
                );
            }
            return None;
        }
        let allow_graphql = !matches!(
            std::env::var("SUI_CHECKPOINT_LOOKUP_GRAPHQL")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("0") | Some("false") | Some("no") | Some("off")
        );
        if allow_graphql {
            match self.graphql.fetch_transaction_meta(digest) {
                Ok(meta) => {
                    if let Some(cp) = meta.checkpoint {
                        if let Some(tx_index) = tx_index {
                            let _ = tx_index.put(digest, cp);
                        }
                        if debug_lookup {
                            eprintln!(
                                "[checkpoint_lookup] digest={} source=graphql checkpoint={}",
                                digest, cp
                            );
                        }
                        return Some(cp);
                    }
                    if debug_lookup {
                        eprintln!(
                            "[checkpoint_lookup] digest={} source=graphql result=missing",
                            digest
                        );
                    }
                }
                Err(e) => {
                    if debug_lookup {
                        eprintln!(
                            "[checkpoint_lookup] digest={} source=graphql error={}",
                            digest, e
                        );
                    }
                }
            }
        } else if debug_lookup {
            eprintln!(
                "[checkpoint_lookup] digest={} source=graphql result=disabled",
                digest
            );
        }

        let allow_grpc = !matches!(
            std::env::var("SUI_CHECKPOINT_LOOKUP_GRPC")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("0") | Some("false") | Some("no") | Some("off")
        );
        if allow_grpc {
            match self.grpc.get_transaction(digest).await {
                Ok(Some(tx)) => {
                    if let Some(cp) = tx.checkpoint {
                        if let Some(tx_index) = tx_index {
                            let _ = tx_index.put(digest, cp);
                        }
                        if debug_lookup {
                            eprintln!(
                                "[checkpoint_lookup] digest={} source=grpc checkpoint={}",
                                digest, cp
                            );
                        }
                        return Some(cp);
                    }
                    if debug_lookup {
                        eprintln!(
                            "[checkpoint_lookup] digest={} source=grpc result=missing",
                            digest
                        );
                    }
                }
                Ok(None) => {
                    if debug_lookup {
                        eprintln!(
                            "[checkpoint_lookup] digest={} source=grpc result=not_found",
                            digest
                        );
                    }
                }
                Err(e) => {
                    if debug_lookup {
                        eprintln!(
                            "[checkpoint_lookup] digest={} source=grpc error={}",
                            digest, e
                        );
                    }
                }
            }
        } else if debug_lookup {
            eprintln!(
                "[checkpoint_lookup] digest={} source=grpc result=disabled",
                digest
            );
        }
        if debug_lookup {
            eprintln!(
                "[checkpoint_lookup] digest={} result=missing source=all",
                digest
            );
        }
        None
    }

    // ==================== Main API ====================

    /// Fetch everything needed to replay a transaction.
    ///
    /// This is the primary entry point. It fetches:
    /// 1. The transaction data (commands, inputs, sender, gas)
    /// 2. All objects at their input versions (from `unchanged_loaded_runtime_objects`)
    /// 3. Dynamic field children (discovered via GraphQL enumeration)
    /// 4. All packages with linkage resolution
    ///
    /// # Arguments
    /// * `digest` - Transaction digest to fetch
    ///
    /// # Returns
    /// A [`ReplayState`] containing everything needed for local replay.
    pub async fn fetch_replay_state(&self, digest: &str) -> Result<ReplayState> {
        self.fetch_replay_state_with_config(digest, true, 3, 200, true)
            .await
    }

    /// Fetch replay state with configuration options.
    ///
    /// # Arguments
    /// * `digest` - Transaction digest to fetch
    /// * `prefetch_dynamic_fields` - Whether to prefetch dynamic field children
    /// * `df_depth` - Maximum depth for dynamic field discovery (default: 3)
    /// * `df_limit` - Maximum children per parent (default: 200)
    pub async fn fetch_replay_state_with_config(
        &self,
        digest: &str,
        prefetch_dynamic_fields: bool,
        df_depth: usize,
        df_limit: usize,
        auto_system_objects: bool,
    ) -> Result<ReplayState> {
        let start = std::time::Instant::now();
        let timing = timing_enabled();
        if checkpoint_lookup_debug_enabled()
            && std::env::var("SUI_CHECKPOINT_LOOKUP_SELF_TEST")
                .ok()
                .as_deref()
                == Some("1")
        {
            if checkpoint_lookup_debug_enabled() {
                eprintln!("[checkpoint_lookup] self_test digest={}", digest);
            }
            let _ = self.resolve_checkpoint_for_tx_digest(digest).await;
        }

        // 1. Fetch transaction via gRPC (has unchanged_loaded_runtime_objects)
        let tx_start = std::time::Instant::now();
        let mut grpc_tx = self
            .grpc
            .get_transaction(digest)
            .await?
            .ok_or_else(|| anyhow!("Transaction not found: {}", digest))?;
        debug!(
            digest = digest,
            elapsed_ms = tx_start.elapsed().as_millis(),
            "fetched transaction via gRPC"
        );
        if timing {
            eprintln!(
                "[timing] stage=grpc_get_transaction digest={} elapsed_ms={}",
                digest,
                tx_start.elapsed().as_millis()
            );
        }
        if grpc_tx.checkpoint.is_none() || grpc_tx.timestamp_ms.is_none() {
            match self.graphql.fetch_transaction_meta(digest) {
                Ok(meta) => {
                    if grpc_tx.checkpoint.is_none() {
                        grpc_tx.checkpoint = meta.checkpoint;
                    }
                    if grpc_tx.timestamp_ms.is_none() {
                        grpc_tx.timestamp_ms = meta.timestamp_ms;
                    }
                    if linkage_debug_enabled() {
                        eprintln!(
                            "[linkage] graphql_tx_meta digest={} checkpoint={:?} timestamp_ms={:?}",
                            digest, grpc_tx.checkpoint, grpc_tx.timestamp_ms
                        );
                    }
                }
                Err(e) => {
                    if linkage_debug_enabled() {
                        eprintln!(
                            "[linkage] graphql_tx_meta_failed digest={} error={}",
                            digest, e
                        );
                    }
                }
            }
        }
        if std::env::var("SUI_DUMP_TX_OBJECTS").ok().as_deref() == Some("1") {
            eprintln!(
                "[tx_objects] digest={} objects_len={}",
                digest,
                grpc_tx.objects.len()
            );
        }

        // Try to hydrate unchanged_* objects from the checkpoint payload (which includes
        // full transaction data). Merge these with whatever we got from gRPC.
        let mut unchanged_loaded_runtime_objects = grpc_tx.unchanged_loaded_runtime_objects.clone();
        let mut unchanged_consensus_objects = grpc_tx.unchanged_consensus_objects.clone();

        let checkpoint_data = if let Some(seq) = grpc_tx.checkpoint {
            let cp_start = std::time::Instant::now();
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.grpc.get_checkpoint(seq),
            )
            .await
            {
                Ok(Ok(Some(cp))) => {
                    if std::env::var("SUI_DUMP_TX_OBJECTS").ok().as_deref() == Some("1") {
                        eprintln!(
                            "[checkpoint_objects] digest={} checkpoint={} objects_len={}",
                            digest,
                            seq,
                            cp.objects.len()
                        );
                    }
                    if let Ok(target_id) = std::env::var("SUI_CHECK_OBJECT_ID") {
                        let target_norm = normalize_address(&target_id);
                        if let Some(found) = cp.objects.iter().find(|o| {
                            !o.object_id.is_empty()
                                && normalize_address(&o.object_id) == target_norm
                        }) {
                            eprintln!(
                                "[checkpoint_objects] digest={} target={} version={}",
                                digest, target_norm, found.version
                            );
                        }
                    }
                    if timing {
                        eprintln!(
                            "[timing] stage=grpc_get_checkpoint digest={} checkpoint={} elapsed_ms={}",
                            digest,
                            seq,
                            cp_start.elapsed().as_millis()
                        );
                    }
                    Some(cp)
                }
                _ => None,
            }
        } else {
            None
        };

        if let Some(cp) = checkpoint_data.as_ref() {
            if let Some(tx) = cp
                .transactions
                .iter()
                .find(|tx| tx.digest == grpc_tx.digest)
            {
                if !tx.unchanged_loaded_runtime_objects.is_empty() {
                    unchanged_loaded_runtime_objects
                        .extend(tx.unchanged_loaded_runtime_objects.clone());
                }
                if !tx.unchanged_consensus_objects.is_empty() {
                    unchanged_consensus_objects.extend(tx.unchanged_consensus_objects.clone());
                }
            }
        }

        if std::env::var("SUI_DUMP_RUNTIME_OBJECTS").ok().as_deref() == Some("1") {
            eprintln!(
                "[runtime_objects] digest={} unchanged_loaded_runtime_objects={} unchanged_consensus_objects={}",
                digest,
                unchanged_loaded_runtime_objects.len(),
                unchanged_consensus_objects.len()
            );
        }

        if let Ok(target_id) = std::env::var("SUI_CHECK_OBJECT_ID") {
            let target_norm = normalize_address(&target_id);
            let found_unchanged = unchanged_loaded_runtime_objects
                .iter()
                .find(|(id, _)| normalize_address(id) == target_norm)
                .map(|(_, v)| *v);
            let found_changed = grpc_tx
                .changed_objects
                .iter()
                .find(|(id, _)| normalize_address(id) == target_norm)
                .map(|(_, v)| *v);
            let found_consensus = unchanged_consensus_objects
                .iter()
                .find(|(id, _)| normalize_address(id) == target_norm)
                .map(|(_, v)| *v);
            let found_input = grpc_tx
                .inputs
                .iter()
                .filter_map(extract_object_id_and_version)
                .find(|(id, _)| {
                    normalize_address(&format!("0x{}", hex::encode(id.as_ref()))) == target_norm
                })
                .map(|(_, v)| v);
            if found_unchanged.is_some() || found_changed.is_some() {
                eprintln!(
                    "[runtime_objects] digest={} target={} unchanged_version={:?} changed_input_version={:?} consensus_version={:?} input_version={:?}",
                    digest,
                    target_norm,
                    found_unchanged,
                    found_changed,
                    found_consensus,
                    found_input
                );
            } else if std::env::var("SUI_DUMP_RUNTIME_OBJECTS").ok().as_deref() == Some("1") {
                eprintln!(
                    "[runtime_objects] digest={} target={} not found in unchanged/changed/consensus/input objects",
                    digest, target_norm
                );
            }
        }

        // 1b. Resolve epoch/protocol metadata via checkpoint if available
        let mut epoch = grpc_tx.epoch.unwrap_or(0);
        let mut protocol_version = 0u64;
        let mut reference_gas_price: Option<u64> = None;

        if epoch == 0 {
            if let Some(cp) = checkpoint_data.as_ref() {
                epoch = cp.epoch;
            }
        }

        if epoch > 0 {
            let epoch_start = std::time::Instant::now();
            if let Ok(Ok(Some(ep))) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.grpc.get_epoch(Some(epoch)),
            )
            .await
            {
                if let Some(pv) = ep.protocol_version {
                    protocol_version = pv;
                }
                reference_gas_price = ep.reference_gas_price;
            }
            if timing {
                eprintln!(
                    "[timing] stage=grpc_get_epoch digest={} epoch={} elapsed_ms={}",
                    digest,
                    epoch,
                    epoch_start.elapsed().as_millis()
                );
            }
        }

        debug!(
            digest = digest,
            epoch = epoch,
            protocol_version = protocol_version,
            reference_gas_price = reference_gas_price.unwrap_or(0),
            "resolved epoch metadata"
        );

        // 2. Collect all object IDs and versions we need
        let mut historical_versions: HashMap<String, u64> = HashMap::new();

        // From explicit inputs
        for input in &grpc_tx.inputs {
            if let Some((id, version)) = extract_object_id_and_version(input) {
                let id_str = format!("0x{}", hex::encode(id.as_ref()));
                historical_versions.insert(id_str, version);
            }
        }

        // From unchanged_loaded_runtime_objects (critical for replay!)
        for (id_str, version) in &unchanged_loaded_runtime_objects {
            let normalized = normalize_address(id_str);
            historical_versions.insert(normalized, *version);
        }

        // From changed_objects (we need their INPUT versions, before the tx modified them)
        for (id_str, version) in &grpc_tx.changed_objects {
            let normalized = normalize_address(id_str);
            historical_versions.insert(normalized, *version);
        }

        // From unchanged_consensus_objects (shared objects read at their actual versions)
        for (id_str, version) in &unchanged_consensus_objects {
            let normalized = normalize_address(id_str);
            historical_versions.insert(normalized, *version);
        }

        // 2b. Opportunistic Walrus checkpoint ingest (input/output objects)
        if let (Some(checkpoint), Some(walrus)) = (grpc_tx.checkpoint, self.walrus.as_ref()) {
            let walrus_timeout: u64 = env_var_or("SUI_WALRUS_TIMEOUT_SECS", 10);
            let walrus_start = std::time::Instant::now();
            let walrus_fetch = tokio::time::timeout(
                std::time::Duration::from_secs(walrus_timeout),
                self.walrus_pool.get(walrus, checkpoint),
            )
            .await;
            match walrus_fetch {
                Ok(Some(checkpoint_json)) => {
                    let checkpoint_json = checkpoint_json.as_ref();
                    let store = self.local_object_store.as_deref();
                    let index = self.local_object_index.as_deref();
                    let tx_index = self.local_tx_index.as_deref();
                    let dynamic_fields = self.local_dynamic_fields.as_deref();
                    let package_index = self.local_package_index.as_deref();
                    if let Some(tx_index) = tx_index {
                        ingest_walrus_checkpoint_tx_index(checkpoint_json, tx_index, checkpoint);
                    }
                    if let Some(tx_json) = find_walrus_tx_json(checkpoint_json, digest) {
                        let ingested = ingest_walrus_tx_objects(
                            tx_json,
                            &self.cache,
                            &mut historical_versions,
                            store,
                            index,
                            package_index,
                            dynamic_fields,
                            Some(checkpoint),
                        );
                        if std::env::var("SUI_DEBUG_WALRUS")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref()
                            == Some("1")
                        {
                            eprintln!(
                                "[walrus] digest={} checkpoint={} ingested_objects={}",
                                digest, checkpoint, ingested
                            );
                        }
                    } else if std::env::var("SUI_DEBUG_WALRUS").ok().as_deref() == Some("1") {
                        eprintln!(
                            "[walrus] digest={} checkpoint={} tx_not_found_in_checkpoint",
                            digest, checkpoint
                        );
                    }
                    if store.is_some() && walrus_store_full_ingest_enabled() {
                        let total = ingest_walrus_checkpoint_objects(
                            checkpoint_json,
                            store,
                            index,
                            package_index,
                            dynamic_fields,
                            Some(checkpoint),
                        );
                        if std::env::var("SUI_DEBUG_WALRUS").ok().as_deref() == Some("1") {
                            eprintln!(
                                "[walrus] checkpoint={} stored_objects={}",
                                checkpoint, total
                            );
                        }
                    }
                    // Also ingest packages from this checkpoint for later use
                    let ingested_pkgs = ingest_walrus_checkpoint_packages(
                        checkpoint_json,
                        &self.cache,
                        package_index,
                        checkpoint,
                    );
                    if std::env::var("SUI_DEBUG_WALRUS").ok().as_deref() == Some("1") {
                        eprintln!(
                            "[walrus] checkpoint={} ingested_packages={}",
                            checkpoint, ingested_pkgs
                        );
                    }
                    if timing {
                        eprintln!(
                            "[timing] stage=walrus_checkpoint_json digest={} checkpoint={} elapsed_ms={}",
                            digest,
                            checkpoint,
                            walrus_start.elapsed().as_millis()
                        );
                    }
                }
                _ => {
                    if std::env::var("SUI_DEBUG_WALRUS").ok().as_deref() == Some("1") {
                        eprintln!(
                            "[walrus] digest={} checkpoint={} fetch_timeout_or_error",
                            digest, checkpoint
                        );
                    }
                    if timing {
                        eprintln!(
                            "[timing] stage=walrus_checkpoint_json digest={} checkpoint={} elapsed_ms={} status=error",
                            digest,
                            checkpoint,
                            walrus_start.elapsed().as_millis()
                        );
                    }
                }
            }
        }

        // 3. Prefetch dynamic field children if enabled
        let mut prefetched_children: HashMap<ObjectID, VersionedObject> = HashMap::new();
        if prefetch_dynamic_fields {
            let df_start = std::time::Instant::now();
            let prefetched = self
                .prefetch_dynamic_fields_internal(
                    &historical_versions,
                    df_depth,
                    df_limit,
                    grpc_tx.checkpoint,
                )
                .await;
            debug!(
                digest = digest,
                elapsed_ms = df_start.elapsed().as_millis(),
                children = prefetched.len(),
                "prefetched dynamic field children"
            );
            if timing {
                eprintln!(
                    "[timing] stage=prefetch_dynamic_fields digest={} depth={} limit={} children={} elapsed_ms={}",
                    digest,
                    df_depth,
                    df_limit,
                    prefetched.len(),
                    df_start.elapsed().as_millis()
                );
            }

            // Add prefetched children to our collection
            for (id_str, version, type_str, bcs) in prefetched {
                if let Ok(id) = parse_object_id(&id_str) {
                    // Add to historical versions for object fetching
                    historical_versions.insert(id_str, version);

                    // Store prefetched data directly
                    prefetched_children.insert(
                        id,
                        VersionedObject {
                            id,
                            version,
                            digest: None,
                            type_tag: Some(type_str),
                            bcs_bytes: bcs,
                            is_shared: false,
                            is_immutable: false,
                        },
                    );
                }
            }
        }

        // 4. Convert to object requests
        let object_requests: Vec<(ObjectID, u64)> = historical_versions
            .iter()
            .filter_map(|(id_str, version)| parse_object_id(id_str).ok().map(|id| (id, *version)))
            .collect();

        // 5. Fetch objects (cache-first, then gRPC), skipping those we already prefetched
        let obj_start = std::time::Instant::now();
        let mut objects = self.fetch_objects_versioned(&object_requests).await?;
        debug!(
            digest = digest,
            elapsed_ms = obj_start.elapsed().as_millis(),
            requested = object_requests.len(),
            fetched = objects.len(),
            "fetched versioned objects"
        );
        if timing {
            eprintln!(
                "[timing] stage=fetch_objects_versioned digest={} requested={} fetched={} elapsed_ms={}",
                digest,
                object_requests.len(),
                objects.len(),
                obj_start.elapsed().as_millis()
            );
        }

        // Merge objects bundled with the transaction payload (if any).
        if !grpc_tx.objects.is_empty() {
            let mut added = 0usize;
            for grpc_obj in &grpc_tx.objects {
                let id = match parse_object_id(&grpc_obj.object_id) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                if objects.contains_key(&id) || grpc_obj.bcs.is_none() {
                    continue;
                }
                if let Ok(obj) = grpc_object_to_versioned(grpc_obj, id, grpc_obj.version) {
                    objects.insert(id, obj);
                    added += 1;
                }
            }
            debug!(digest = digest, added = added, "added transaction objects");
        }

        // Merge in dynamic field objects included in the checkpoint payload.
        // These are historical and help fill gaps when GraphQL snapshots are unavailable.
        if let Some(cp) = checkpoint_data.as_ref() {
            let mut added = 0usize;
            for grpc_obj in &cp.objects {
                let id = match parse_object_id(&grpc_obj.object_id) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                if objects.contains_key(&id) || grpc_obj.bcs.is_none() {
                    continue;
                }
                if let Ok(obj) = grpc_object_to_versioned(grpc_obj, id, grpc_obj.version) {
                    objects.insert(id, obj);
                    added += 1;
                }
            }
            debug!(
                digest = digest,
                added = added,
                "added checkpoint dynamic field objects"
            );
        }

        // Merge prefetched children (they take precedence since they have BCS data)
        for (id, obj) in prefetched_children {
            objects.entry(id).or_insert(obj);
        }

        // 5b. Ensure system objects (Clock/Random) are available for replay.
        // These are often omitted from unchanged_* sets, but required for execution.
        if auto_system_objects {
            ensure_system_objects(
                &mut objects,
                &historical_versions,
                grpc_tx.timestamp_ms,
                grpc_tx.checkpoint,
            );
        }

        // 6. Extract package IDs from commands AND from object type strings
        let mut package_ids: HashSet<AccountAddress> =
            extract_package_ids_from_tx(&grpc_tx).into_iter().collect();

        // Also extract from object type strings
        for obj in objects.values() {
            if let Some(ref type_tag) = obj.type_tag {
                for pkg_id in extract_package_ids_from_type(type_tag) {
                    if let Ok(id) = parse_object_id(&pkg_id) {
                        package_ids.insert(id);
                    }
                }
            }
        }

        // 7. Fetch packages (cache-first, then GraphQL with linkage resolution)
        let package_ids_vec: Vec<_> = package_ids.into_iter().collect();

        // Build package version hints from historical versions (if present)
        let mut package_versions: HashMap<AccountAddress, u64> = HashMap::new();
        for pkg_id in &package_ids_vec {
            let pkg_str = format!("0x{}", hex::encode(pkg_id.as_ref()));
            if let Some(ver) = historical_versions.get(&normalize_address(&pkg_str)) {
                package_versions.insert(*pkg_id, *ver);
            }
        }

        // Build package previous_transaction hints from transaction objects.
        // This enables Walrus checkpoint discovery for packages via their publish transaction.
        let mut package_prev_txs: HashMap<AccountAddress, String> = HashMap::new();
        for grpc_obj in &grpc_tx.objects {
            // Only consider package objects (have package_modules)
            if grpc_obj.package_modules.is_some() {
                if let (Ok(pkg_id), Some(prev_tx)) = (
                    parse_object_id(&grpc_obj.object_id),
                    grpc_obj.previous_transaction.as_ref(),
                ) {
                    package_prev_txs.insert(pkg_id, prev_tx.clone());
                }
            }
        }

        // If any object fetch fell back to a different version than requested,
        // avoid version-pinning packages to reduce layout mismatches.
        let used_non_historical = object_requests.iter().any(|(id, ver)| {
            objects
                .get(id)
                .map(|obj| obj.version != *ver)
                .unwrap_or(true)
        });
        let package_versions_opt = if used_non_historical {
            if linkage_debug_enabled() {
                eprintln!("[linkage] disabling version pinning (non-historical objects detected)");
            }
            None
        } else {
            Some(&package_versions)
        };
        let package_prev_txs_opt = if package_prev_txs.is_empty() {
            None
        } else {
            Some(&package_prev_txs)
        };

        let pkg_start = std::time::Instant::now();
        let packages = self
            .fetch_packages_with_deps_and_prev_txs(
                &package_ids_vec,
                package_versions_opt,
                package_prev_txs_opt,
                grpc_tx.checkpoint,
            )
            .await?;
        debug!(
            digest = digest,
            elapsed_ms = pkg_start.elapsed().as_millis(),
            requested = package_ids_vec.len(),
            fetched = packages.len(),
            "fetched packages"
        );
        if timing {
            eprintln!(
                "[timing] stage=fetch_packages_with_deps digest={} requested={} fetched={} elapsed_ms={}",
                digest,
                package_ids_vec.len(),
                packages.len(),
                pkg_start.elapsed().as_millis()
            );
        }

        // 8. Convert to FetchedTransaction format
        let transaction = grpc_to_fetched_transaction(&grpc_tx)?;

        debug!(
            digest = digest,
            elapsed_ms = start.elapsed().as_millis(),
            "completed replay state fetch"
        );
        if timing {
            eprintln!(
                "[timing] stage=fetch_replay_state_total digest={} elapsed_ms={}",
                digest,
                start.elapsed().as_millis()
            );
        }

        Ok(ReplayState {
            transaction,
            objects,
            packages,
            protocol_version,
            epoch,
            reference_gas_price,
            checkpoint: grpc_tx.checkpoint,
        })
    }

    /// Internal helper to prefetch dynamic field children.
    ///
    /// Returns Vec<(object_id, version, type_string, bcs_bytes)>
    async fn hydrate_dynamic_fields_via_tx_chain(
        &self,
        parent_id: &str,
        parent_version: Option<u64>,
        checkpoint: Option<u64>,
    ) -> bool {
        let Some(walrus) = self.walrus.as_ref() else {
            return false;
        };
        let Some(index) = self.local_object_index.as_deref() else {
            return false;
        };
        let Some(tx_index) = self.local_tx_index.as_deref() else {
            return false;
        };
        let Some(df_cache) = self.local_dynamic_fields.as_deref() else {
            return false;
        };
        let Some(version) = parent_version else {
            return false;
        };
        let parent_addr = match parse_object_id(parent_id) {
            Ok(addr) => addr,
            Err(_) => return false,
        };
        let entry = match index.get_entry(parent_addr, version) {
            Ok(Some(entry)) => entry,
            _ => return false,
        };
        let mut current_tx = entry.tx_digest;
        let mut steps = 0usize;
        let max_steps = walrus_recursive_max_tx_steps();
        let mut visited = HashSet::new();

        while let Some(tx_digest) = current_tx {
            if steps >= max_steps {
                break;
            }
            if !visited.insert(tx_digest.clone()) {
                break;
            }
            let checkpoint_for_tx = match tx_index.get_checkpoint(&tx_digest) {
                Ok(Some(cp)) => cp,
                _ => break,
            };
            if let Some(cp_limit) = checkpoint {
                if checkpoint_for_tx > cp_limit {
                    break;
                }
            }
            let checkpoint_json = match self.walrus_pool.get(walrus, checkpoint_for_tx).await {
                Some(json) => json,
                None => break,
            };

            let store = self.local_object_store.as_deref();
            let index = self.local_object_index.as_deref();
            let dynamic_fields = self.local_dynamic_fields.as_deref();
            let package_index = self.local_package_index.as_deref();
            if let Some(tx_index) = self.local_tx_index.as_deref() {
                ingest_walrus_checkpoint_tx_index(
                    checkpoint_json.as_ref(),
                    tx_index,
                    checkpoint_for_tx,
                );
            }
            let _ = ingest_walrus_checkpoint_objects(
                checkpoint_json.as_ref(),
                store,
                index,
                package_index,
                dynamic_fields,
                Some(checkpoint_for_tx),
            );

            if let Ok(children) = df_cache.get_children_at_or_before(parent_addr, checkpoint_for_tx)
            {
                if !children.is_empty() {
                    return true;
                }
            }

            let tx_json = find_walrus_tx_json(&checkpoint_json, &tx_digest);
            current_tx = tx_json.and_then(|tx| find_prev_tx_for_object_in_tx(tx, &parent_addr));
            steps += 1;
        }
        false
    }

    async fn prefetch_dynamic_fields_internal(
        &self,
        historical_versions: &HashMap<String, u64>,
        max_depth: usize,
        limit_per_parent: usize,
        checkpoint: Option<u64>,
    ) -> Vec<(String, u64, String, Vec<u8>)> {
        use base64::Engine;

        let debug_gaps = data_gap_debug_enabled();
        let mut result = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut to_process: Vec<(String, usize)> =
            historical_versions.keys().map(|k| (k.clone(), 0)).collect();
        let max_lamport_version = historical_versions.values().copied().max().unwrap_or(0);
        let max_secs: u64 = env_var_or("SUI_STATE_DF_PREFETCH_TIMEOUT_SECS", 30);
        let start = std::time::Instant::now();

        while let Some((parent_id, depth)) = to_process.pop() {
            if start.elapsed().as_secs() > max_secs {
                eprintln!(
                    "[state_prefetch_df] Timeout after {}s (fetched={})",
                    max_secs,
                    result.len()
                );
                break;
            }
            if depth >= max_depth || visited.contains(&parent_id) {
                continue;
            }
            visited.insert(parent_id.clone());

            // If we have a local dynamic field cache for this checkpoint, use it.
            if let (Some(df_cache), Some(cp)) = (self.local_dynamic_fields.as_deref(), checkpoint) {
                if let Ok(parent_addr) = parse_object_id(&parent_id) {
                    let mut cached_children = df_cache
                        .get_children_at_or_before(parent_addr, cp)
                        .unwrap_or_default();

                    if cached_children.is_empty() {
                        let parent_version = historical_versions.get(&parent_id).copied();
                        if self
                            .hydrate_dynamic_fields_via_tx_chain(
                                &parent_id,
                                parent_version,
                                checkpoint,
                            )
                            .await
                        {
                            cached_children = df_cache
                                .get_children_at_or_before(parent_addr, cp)
                                .unwrap_or_default();
                        }
                    }

                    if !cached_children.is_empty() {
                        for entry in cached_children {
                            if start.elapsed().as_secs() > max_secs {
                                eprintln!(
                                    "[state_prefetch_df] Timeout after {}s (fetched={})",
                                    max_secs,
                                    result.len()
                                );
                                return result;
                            }
                            let child_id = normalize_address(&entry.child_id);
                            let version = entry.version;
                            let child_addr = match parse_object_id(&child_id) {
                                Ok(addr) => addr,
                                Err(_) => continue,
                            };

                            // Try local store first
                            let mut type_str = entry.type_tag.clone();
                            let mut bcs = None;
                            if let Some(store) = self.local_object_store.as_deref() {
                                if let Ok(Some(cached)) = store.get(child_addr, version) {
                                    bcs = Some(cached.bcs_bytes);
                                    if type_str.is_none() {
                                        type_str = Some(cached.meta.type_tag);
                                    }
                                }
                            }

                            // Fallback to GraphQL fetch for this specific child
                            if bcs.is_none() {
                                if let Ok(obj) =
                                    self.graphql.fetch_object_at_version(&child_id, version)
                                {
                                    type_str = type_str.or(obj.type_string);
                                    if let Some(b64) = obj.bcs_base64 {
                                        if let Ok(decoded) =
                                            base64::engine::general_purpose::STANDARD.decode(&b64)
                                        {
                                            bcs = Some(decoded);
                                        }
                                    }
                                } else if let Some(cp) = checkpoint {
                                    if let Ok(obj) =
                                        self.graphql.fetch_object_at_checkpoint(&child_id, cp)
                                    {
                                        type_str = type_str.or(obj.type_string);
                                        if let Some(b64) = obj.bcs_base64 {
                                            if let Ok(decoded) =
                                                base64::engine::general_purpose::STANDARD
                                                    .decode(&b64)
                                            {
                                                bcs = Some(decoded);
                                            }
                                        }
                                    }
                                }
                            }

                            if let (Some(type_str), Some(bcs)) = (type_str, bcs) {
                                result.push((child_id.clone(), version, type_str, bcs));
                                if depth + 1 < max_depth {
                                    to_process.push((child_id, depth + 1));
                                }
                            }
                        }
                        // Skip GraphQL enumeration when cache is present.
                        continue;
                    }
                }
            }

            // Fetch dynamic fields for this parent (checkpoint snapshot if available)
            let (fields, snapshot_used) = match checkpoint {
                Some(cp) => match self.graphql.fetch_dynamic_fields_at_checkpoint(
                    &parent_id,
                    limit_per_parent,
                    cp,
                ) {
                    Ok(fields) => (fields, true),
                    Err(e) => {
                        if debug_gaps {
                            eprintln!(
                                "[data_gap] kind=dynamic_fields parent={} checkpoint={} source=graphql_checkpoint error={}",
                                parent_id, cp, e
                            );
                        }
                        match self
                            .graphql
                            .fetch_dynamic_fields(&parent_id, limit_per_parent)
                        {
                            Ok(fields) => (fields, false),
                            Err(e2) => {
                                if debug_gaps {
                                    eprintln!(
                                        "[data_gap] kind=dynamic_fields parent={} source=graphql_latest error={}",
                                        parent_id, e2
                                    );
                                }
                                continue;
                            }
                        }
                    }
                },
                None => match self
                    .graphql
                    .fetch_dynamic_fields(&parent_id, limit_per_parent)
                {
                    Ok(fields) => (fields, false),
                    Err(e) => {
                        if debug_gaps {
                            eprintln!(
                                "[data_gap] kind=dynamic_fields parent={} source=graphql_latest error={}",
                                parent_id, e
                            );
                        }
                        continue;
                    }
                },
            };
            if !fields.is_empty() {
                for df in fields {
                    if start.elapsed().as_secs() > max_secs {
                        eprintln!(
                            "[state_prefetch_df] Timeout after {}s (fetched={})",
                            max_secs,
                            result.len()
                        );
                        return result;
                    }
                    if let Some(child_id) = &df.object_id {
                        let child_normalized = normalize_address(child_id);

                        // Get version - prefer historical versions, then GraphQL, then gRPC latest
                        let version_opt = if let Some(v) =
                            historical_versions.get(&child_normalized)
                        {
                            Some(*v)
                        } else if let Some(v) = df.version {
                            if snapshot_used || v <= max_lamport_version {
                                Some(v)
                            } else {
                                continue;
                            }
                        } else if let Ok(Some(obj)) = self.grpc.get_object(&child_normalized).await
                        {
                            if snapshot_used || obj.version <= max_lamport_version {
                                Some(obj.version)
                            } else {
                                continue;
                            }
                        } else {
                            None
                        };

                        let Some(version) = version_opt else {
                            continue;
                        };

                        // Get BCS data - prefer from dynamic field response, fallback to object fetch
                        let (type_str, bcs) = if let (Some(vt), Some(vb)) =
                            (&df.value_type, &df.value_bcs)
                        {
                            if let Ok(decoded) =
                                base64::engine::general_purpose::STANDARD.decode(vb)
                            {
                                (vt.clone(), decoded)
                            } else {
                                continue;
                            }
                        } else if let Ok(obj) = self
                            .graphql
                            .fetch_object_at_version(&child_normalized, version)
                        {
                            if let (Some(ts), Some(b64)) = (obj.type_string, obj.bcs_base64) {
                                if let Ok(decoded) =
                                    base64::engine::general_purpose::STANDARD.decode(&b64)
                                {
                                    (ts, decoded)
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        } else if let Some(cp) = checkpoint {
                            if let Ok(obj) = self
                                .graphql
                                .fetch_object_at_checkpoint(&child_normalized, cp)
                            {
                                if obj.version != version {
                                    continue;
                                }
                                if let (Some(ts), Some(b64)) = (obj.type_string, obj.bcs_base64) {
                                    if let Ok(decoded) =
                                        base64::engine::general_purpose::STANDARD.decode(&b64)
                                    {
                                        (ts, decoded)
                                    } else {
                                        continue;
                                    }
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        } else if let Ok(obj) = self.graphql.fetch_object(&child_normalized) {
                            if obj.version != version {
                                continue;
                            }
                            if let (Some(ts), Some(b64)) = (obj.type_string, obj.bcs_base64) {
                                if let Ok(decoded) =
                                    base64::engine::general_purpose::STANDARD.decode(&b64)
                                {
                                    (ts, decoded)
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        };

                        result.push((child_normalized.clone(), version, type_str, bcs));

                        // Add child to processing queue for deeper discovery
                        if depth + 1 < max_depth {
                            to_process.push((child_normalized, depth + 1));
                        }
                    }
                }
            }
        }

        result
    }

    /// Fetch objects at specific versions.
    ///
    /// Checks cache first, then fetches missing objects via gRPC.
    /// Falls back to GraphQL for current version if gRPC fails (for pruned archives).
    pub async fn fetch_objects_versioned(
        &self,
        requests: &[(ObjectID, u64)],
    ) -> Result<HashMap<ObjectID, VersionedObject>> {
        self.fetch_objects_versioned_internal(requests, true).await
    }

    /// Fetch objects at specific versions without reading or writing cache.
    pub async fn fetch_objects_versioned_no_cache(
        &self,
        requests: &[(ObjectID, u64)],
    ) -> Result<HashMap<ObjectID, VersionedObject>> {
        self.fetch_objects_versioned_internal(requests, false).await
    }

    async fn fetch_objects_versioned_internal(
        &self,
        requests: &[(ObjectID, u64)],
        use_cache: bool,
    ) -> Result<HashMap<ObjectID, VersionedObject>> {
        use base64::Engine;

        let mut result = HashMap::new();
        let mut to_fetch = Vec::new();
        let timing = timing_enabled();
        let debug_gaps = data_gap_debug_enabled();
        let cache_start = std::time::Instant::now();
        let mut cache_hits = 0usize;
        let mut cache_misses = 0usize;
        let mut local_hits = 0usize;
        let mut local_misses = 0usize;

        if use_cache {
            // Check cache first
            for (id, version) in requests {
                if let Some(obj) = self.cache.get_object(id, *version) {
                    result.insert(*id, obj);
                    cache_hits += 1;
                } else {
                    to_fetch.push((*id, *version));
                    cache_misses += 1;
                }
            }
        } else {
            to_fetch.extend(requests.iter().copied());
            cache_misses = requests.len();
        }

        // Check local Walrus-backed object store before network fetches.
        if let Some(store) = self.local_object_store.as_deref() {
            if !to_fetch.is_empty() {
                let mut remaining = Vec::with_capacity(to_fetch.len());
                for (id, version) in to_fetch {
                    match store.get(id, version) {
                        Ok(Some(cached)) => {
                            let (is_shared, is_immutable) = match cached.meta.owner_kind.as_deref()
                            {
                                Some("shared") => (true, false),
                                Some("immutable") => (false, true),
                                _ => (false, false),
                            };
                            let obj = VersionedObject {
                                id,
                                version,
                                digest: None,
                                type_tag: Some(cached.meta.type_tag),
                                bcs_bytes: cached.bcs_bytes,
                                is_shared,
                                is_immutable,
                            };
                            if use_cache {
                                self.cache.put_object(obj.clone());
                            }
                            result.insert(id, obj);
                            local_hits += 1;
                        }
                        _ => {
                            remaining.push((id, version));
                            local_misses += 1;
                        }
                    }
                }
                to_fetch = remaining;
            }
        }

        // Recursive Walrus hydration: use local index to find checkpoints for missing objects.
        if let (Some(index), Some(walrus)) =
            (self.local_object_index.as_deref(), self.walrus.as_ref())
        {
            if walrus_recursive_enabled() && !to_fetch.is_empty() {
                let mut checkpoints = HashMap::new();
                for (id, version) in &to_fetch {
                    if let Ok(Some(cp)) = index.get_checkpoint(*id, *version) {
                        checkpoints
                            .entry(cp)
                            .or_insert_with(Vec::new)
                            .push((*id, *version));
                    }
                }
                if !checkpoints.is_empty() {
                    let mut checkpoint_list: Vec<u64> = checkpoints.keys().copied().collect();
                    checkpoint_list.sort_unstable();
                    let max_cp = walrus_recursive_max_checkpoints();
                    if checkpoint_list.len() > max_cp {
                        checkpoint_list.truncate(max_cp);
                    }

                    let ingest_start = std::time::Instant::now();
                    let mut join_set = tokio::task::JoinSet::new();
                    let pool = self.walrus_pool.clone();
                    for cp in checkpoint_list {
                        let walrus = walrus.clone();
                        let pool = pool.clone();
                        join_set.spawn(async move {
                            let fetched = pool.get(&walrus, cp).await;
                            (cp, fetched)
                        });
                    }
                    while let Some(res) = join_set.join_next().await {
                        if let Ok((cp, Some(checkpoint_json))) = res {
                            let store = self.local_object_store.as_deref();
                            let index = self.local_object_index.as_deref();
                            let dynamic_fields = self.local_dynamic_fields.as_deref();
                            let package_index = self.local_package_index.as_deref();
                            let _ = ingest_walrus_checkpoint_objects(
                                checkpoint_json.as_ref(),
                                store,
                                index,
                                package_index,
                                dynamic_fields,
                                Some(cp),
                            );
                        }
                    }
                    if timing {
                        eprintln!(
                            "[timing] stage=walrus_recursive_ingest checkpoints={} elapsed_ms={}",
                            checkpoints.len(),
                            ingest_start.elapsed().as_millis()
                        );
                    }

                    // Re-check local store after recursive ingest.
                    if let Some(store) = self.local_object_store.as_deref() {
                        if !to_fetch.is_empty() {
                            let mut remaining = Vec::with_capacity(to_fetch.len());
                            for (id, version) in to_fetch {
                                match store.get(id, version) {
                                    Ok(Some(cached)) => {
                                        let (is_shared, is_immutable) =
                                            match cached.meta.owner_kind.as_deref() {
                                                Some("shared") => (true, false),
                                                Some("immutable") => (false, true),
                                                _ => (false, false),
                                            };
                                        let obj = VersionedObject {
                                            id,
                                            version,
                                            digest: None,
                                            type_tag: Some(cached.meta.type_tag),
                                            bcs_bytes: cached.bcs_bytes,
                                            is_shared,
                                            is_immutable,
                                        };
                                        if use_cache {
                                            self.cache.put_object(obj.clone());
                                        }
                                        result.insert(id, obj);
                                        local_hits += 1;
                                    }
                                    _ => {
                                        remaining.push((id, version));
                                        local_misses += 1;
                                    }
                                }
                            }
                            to_fetch = remaining;
                        }
                    }
                }
            }
        }

        if to_fetch.is_empty() {
            if timing {
                eprintln!(
                    "[timing] stage=fetch_objects_cache_only requested={} hits={} misses={} local_hits={} local_misses={} elapsed_ms={}",
                    requests.len(),
                    cache_hits,
                    cache_misses,
                    local_hits,
                    local_misses,
                    cache_start.elapsed().as_millis()
                );
            }
            return Ok(result);
        }
        if timing {
            eprintln!(
                "[timing] stage=fetch_objects_cache_scan requested={} hits={} misses={} local_hits={} local_misses={} elapsed_ms={}",
                requests.len(),
                cache_hits,
                cache_misses,
                local_hits,
                local_misses,
                cache_start.elapsed().as_millis()
            );
        }

        // Fetch missing objects via gRPC, with GraphQL fallback
        let fetch_start = std::time::Instant::now();
        let mut grpc_ok = 0usize;
        let mut grpc_fail = 0usize;
        let mut gql_ok = 0usize;
        let mut gql_fail = 0usize;
        let mut grpc_elapsed = 0u128;
        let mut gql_elapsed = 0u128;
        for (id, version) in &to_fetch {
            let id_str = format!("0x{}", hex::encode(id.as_ref()));

            // Try gRPC first
            let grpc_start = std::time::Instant::now();
            let grpc_result = self
                .grpc
                .get_object_at_version(&id_str, Some(*version))
                .await;
            grpc_elapsed += grpc_start.elapsed().as_millis();

            match grpc_result {
                Ok(Some(grpc_obj)) => {
                    let obj = grpc_object_to_versioned(&grpc_obj, *id, *version)?;
                    if use_cache {
                        self.cache.put_object(obj.clone());
                    }
                    result.insert(*id, obj);
                    grpc_ok += 1;
                }
                Ok(None) | Err(_) => {
                    grpc_fail += 1;
                    // gRPC failed - try GraphQL for current version as fallback
                    // This is necessary when historical versions are pruned from the archive
                    let gql_start = std::time::Instant::now();
                    let gql_obj = self
                        .graphql
                        .fetch_object_at_version(&id_str, *version)
                        .or_else(|_| self.graphql.fetch_object(&id_str));
                    gql_elapsed += gql_start.elapsed().as_millis();
                    if let Ok(gql_obj) = gql_obj {
                        if let (Some(type_str), Some(bcs_b64)) =
                            (gql_obj.type_string, gql_obj.bcs_base64)
                        {
                            if let Ok(bcs) =
                                base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                            {
                                let obj = VersionedObject {
                                    id: *id,
                                    version: gql_obj.version, // Use current version
                                    digest: None,
                                    type_tag: Some(type_str),
                                    bcs_bytes: bcs,
                                    is_shared: false, // GraphQL doesn't give us owner info easily
                                    is_immutable: false,
                                };
                                if use_cache {
                                    self.cache.put_object(obj.clone());
                                }
                                result.insert(*id, obj);
                                gql_ok += 1;
                            }
                        }
                    } else {
                        gql_fail += 1;
                        eprintln!("Warning: Failed to fetch object {} at version {} (gRPC and GraphQL both failed)", id_str, version);
                        if debug_gaps {
                            eprintln!(
                                "[data_gap] kind=object_missing id={} version={} source=grpc_graphql",
                                id_str, version
                            );
                        }
                    }
                }
            }
        }

        if timing {
            eprintln!(
                "[timing] stage=fetch_objects_network requested={} grpc_ok={} grpc_fail={} gql_ok={} gql_fail={} grpc_ms={} gql_ms={} total_ms={}",
                to_fetch.len(),
                grpc_ok,
                grpc_fail,
                gql_ok,
                gql_fail,
                grpc_elapsed,
                gql_elapsed,
                fetch_start.elapsed().as_millis()
            );
        }

        Ok(result)
    }
    /// Fetch packages with full dependency resolution.
    ///
    /// Uses gRPC to fetch packages (includes linkage table), then follows
    /// linkage to fetch all transitive dependencies.
    pub async fn fetch_packages_with_deps(
        &self,
        package_ids: &[AccountAddress],
        package_versions: Option<&HashMap<AccountAddress, u64>>,
        checkpoint: Option<u64>,
    ) -> Result<HashMap<AccountAddress, PackageData>> {
        self.fetch_packages_with_deps_internal(
            package_ids,
            package_versions,
            None,
            checkpoint,
            true,
        )
        .await
    }

    /// Fetch packages with full dependency resolution, with optional previous transaction hints.
    ///
    /// The `package_prev_txs` parameter maps package IDs to their previous transaction digest,
    /// which can be used to look up the checkpoint where the package was published via Walrus.
    pub async fn fetch_packages_with_deps_and_prev_txs(
        &self,
        package_ids: &[AccountAddress],
        package_versions: Option<&HashMap<AccountAddress, u64>>,
        package_prev_txs: Option<&HashMap<AccountAddress, String>>,
        checkpoint: Option<u64>,
    ) -> Result<HashMap<AccountAddress, PackageData>> {
        self.fetch_packages_with_deps_internal(
            package_ids,
            package_versions,
            package_prev_txs,
            checkpoint,
            true,
        )
        .await
    }

    /// Fetch packages with full dependency resolution without reading or writing cache.
    pub async fn fetch_packages_with_deps_no_cache(
        &self,
        package_ids: &[AccountAddress],
        package_versions: Option<&HashMap<AccountAddress, u64>>,
        checkpoint: Option<u64>,
    ) -> Result<HashMap<AccountAddress, PackageData>> {
        self.fetch_packages_with_deps_internal(
            package_ids,
            package_versions,
            None,
            checkpoint,
            false,
        )
        .await
    }

    async fn fetch_packages_with_deps_internal(
        &self,
        package_ids: &[AccountAddress],
        package_versions: Option<&HashMap<AccountAddress, u64>>,
        package_prev_txs: Option<&HashMap<AccountAddress, String>>,
        checkpoint: Option<u64>,
        use_cache: bool,
    ) -> Result<HashMap<AccountAddress, PackageData>> {
        let timing = timing_enabled();
        let start = std::time::Instant::now();
        let mut result = HashMap::new();
        let mut to_process: Vec<AccountAddress> = package_ids.to_vec();
        let mut processed: HashSet<AccountAddress> = HashSet::new();
        let mut cache_hits = 0usize;
        let mut grpc_ok = 0usize;
        let mut grpc_fail = 0usize;
        let mut gql_ok = 0usize;
        let mut gql_fail = 0usize;
        let mut gql_fetches = 0usize;
        let mut grpc_elapsed = 0u128;
        let mut gql_elapsed = 0u128;
        let debug_gaps = data_gap_debug_enabled();
        let strict_checkpoint = checkpoint.is_some();
        let walrus_only = matches!(
            std::env::var("SUI_WALRUS_PACKAGE_ONLY")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("1") | Some("true") | Some("yes") | Some("on")
        );
        let allow_package_graphql = !matches!(
            std::env::var("SUI_PACKAGE_LOOKUP_GRAPHQL")
                .ok()
                .as_deref()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("0") | Some("false") | Some("no") | Some("off")
        );

        while let Some(pkg_id) = to_process.pop() {
            if processed.contains(&pkg_id) {
                continue;
            }
            processed.insert(pkg_id);

            let mut version_hint = package_versions.and_then(|m| m.get(&pkg_id).copied());
            let mut missing_reasons: Vec<String> = Vec::new();
            if version_hint.is_none() {
                if let (Some(pkg_index), Some(cp)) =
                    (self.local_package_index.as_deref(), checkpoint)
                {
                    if let Ok(Some(entry)) = pkg_index.get_at_or_before_checkpoint(pkg_id, cp) {
                        version_hint = Some(entry.version);
                    }
                }
            }
            if version_hint.is_none()
                && std::env::var("SUI_CHECKPOINT_LOOKUP_REMOTE")
                    .ok()
                    .as_deref()
                    .map(|v| v.to_ascii_lowercase())
                    .as_deref()
                    != Some("0")
                && allow_package_graphql
            {
                if let Some(cp) = checkpoint {
                    if let Ok(Some(ver)) = self.graphql.fetch_package_version_at_checkpoint(
                        &format!("0x{}", hex::encode(pkg_id.as_ref())),
                        cp,
                    ) {
                        version_hint = Some(ver);
                    }
                }
            }
            if version_hint.is_none() {
                missing_reasons.push("version_hint_missing".to_string());
            }

            if use_cache {
                // Check cache first (version-aware if possible)
                if let Some(ver) = version_hint {
                    if let Some(pkg) = self.cache.get_package(&pkg_id, ver) {
                        cache_hits += 1;
                        log_package_linkage(&pkg, "cache", version_hint, true);
                        // Add dependencies to process queue
                        for dep_id in pkg.linkage.values() {
                            if !processed.contains(dep_id) {
                                to_process.push(*dep_id);
                            }
                        }
                        result.insert(pkg_id, pkg);
                        continue;
                    }
                } else if checkpoint.is_none() {
                    if let Some(pkg) = self.cache.get_package_latest(&pkg_id) {
                        cache_hits += 1;
                        log_package_linkage(&pkg, "cache_latest", version_hint, true);
                        // Add dependencies to process queue
                        for dep_id in pkg.linkage.values() {
                            if !processed.contains(dep_id) {
                                to_process.push(*dep_id);
                            }
                        }
                        result.insert(pkg_id, pkg);
                        continue;
                    }
                }
            }

            if let (Some(pkg_index), Some(walrus)) =
                (self.local_package_index.as_deref(), self.walrus.as_ref())
            {
                let mut walrus_checkpoint = None;
                if let Some(ver) = version_hint {
                    if let Ok(Some(entry)) = pkg_index.get_entry(pkg_id, ver) {
                        walrus_checkpoint = Some(entry.checkpoint);
                    }
                }
                if walrus_checkpoint.is_none() {
                    if let Some(cp) = checkpoint {
                        if let Ok(Some(entry)) = pkg_index.get_at_or_before_checkpoint(pkg_id, cp) {
                            walrus_checkpoint = Some(entry.checkpoint);
                            if version_hint.is_none() {
                                version_hint = Some(entry.version);
                            }
                        }
                    } else if let Ok(Some(entry)) = pkg_index.get_latest(pkg_id) {
                        walrus_checkpoint = Some(entry.checkpoint);
                        if version_hint.is_none() {
                            version_hint = Some(entry.version);
                        }
                    }
                }
                if walrus_checkpoint.is_none() {
                    if let Some(obj_index) = self.local_object_index.as_deref() {
                        let entry = if let Some(cp) = checkpoint {
                            obj_index
                                .get_at_or_before_checkpoint(pkg_id, cp)
                                .ok()
                                .flatten()
                        } else {
                            obj_index.get_latest(pkg_id).ok().flatten()
                        };
                        if let Some(entry) = entry {
                            if let Some(tx_digest) = entry.tx_digest.clone() {
                                if let Some(cp) =
                                    self.resolve_checkpoint_for_tx_digest(&tx_digest).await
                                {
                                    walrus_checkpoint = Some(cp);
                                    if version_hint.is_none() {
                                        version_hint = Some(entry.version);
                                    }
                                } else {
                                    missing_reasons.push("checkpoint_lookup_failed".to_string());
                                    if debug_gaps {
                                        eprintln!(
                                            "[data_gap] kind=package_checkpoint_lookup pkg={} tx_digest={} source=obj_index",
                                            pkg_id, tx_digest
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                if walrus_checkpoint.is_none() {
                    if let Some(ver) = version_hint {
                        if std::env::var("SUI_CHECKPOINT_LOOKUP_REMOTE")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref()
                            != Some("0")
                        {
                            let pkg_id_str = format!("0x{}", hex::encode(pkg_id.as_ref()));
                            if let Ok(Some(grpc_obj)) = self
                                .grpc
                                .get_object_at_version(&pkg_id_str, Some(ver))
                                .await
                            {
                                if let Some(prev_tx) = grpc_obj.previous_transaction.as_ref() {
                                    if let Some(cp) =
                                        self.resolve_checkpoint_for_tx_digest(prev_tx).await
                                    {
                                        walrus_checkpoint = Some(cp);
                                    } else {
                                        missing_reasons
                                            .push("checkpoint_lookup_failed".to_string());
                                        if debug_gaps {
                                            eprintln!(
                                                "[data_gap] kind=package_checkpoint_lookup pkg={} prev_tx={} source=grpc_object",
                                                pkg_id, prev_tx
                                            );
                                        }
                                    }
                                } else if debug_gaps {
                                    eprintln!(
                                        "[data_gap] kind=package_prev_tx_missing pkg={} source=grpc_object",
                                        pkg_id
                                    );
                                }
                            }
                        }
                    }
                }
                if walrus_checkpoint.is_none() {
                    missing_reasons.push("walrus_checkpoint_missing".to_string());
                }
                if let Some(cp) = walrus_checkpoint {
                    if let Some(checkpoint_json) = self.walrus_pool.get(walrus, cp).await {
                        if let Some(tx_index) = self.local_tx_index.as_deref() {
                            ingest_walrus_checkpoint_tx_index(
                                checkpoint_json.as_ref(),
                                tx_index,
                                cp,
                            );
                        }
                        let _ = ingest_walrus_checkpoint_packages(
                            checkpoint_json.as_ref(),
                            &self.cache,
                            self.local_package_index.as_deref(),
                            cp,
                        );
                        if version_hint.is_none() {
                            if let Some(pkg_index) = self.local_package_index.as_deref() {
                                if let Ok(Some(entry)) =
                                    pkg_index.get_at_or_before_checkpoint(pkg_id, cp)
                                {
                                    version_hint = Some(entry.version);
                                }
                            }
                        }
                        if use_cache {
                            if let Some(ver) = version_hint {
                                if let Some(pkg) = self.cache.get_package(&pkg_id, ver) {
                                    cache_hits += 1;
                                    log_package_linkage(&pkg, "walrus_cache", version_hint, true);
                                    for dep_id in pkg.linkage.values() {
                                        if !processed.contains(dep_id) {
                                            to_process.push(*dep_id);
                                        }
                                    }
                                    for dep_id in extract_module_dependency_ids(&pkg.modules) {
                                        if !processed.contains(&dep_id) {
                                            to_process.push(dep_id);
                                        }
                                    }
                                    result.insert(pkg_id, pkg);
                                    continue;
                                }
                            } else if checkpoint.is_none() {
                                if let Some(pkg) = self.cache.get_package_latest(&pkg_id) {
                                    cache_hits += 1;
                                    log_package_linkage(
                                        &pkg,
                                        "walrus_cache_latest",
                                        version_hint,
                                        true,
                                    );
                                    for dep_id in pkg.linkage.values() {
                                        if !processed.contains(dep_id) {
                                            to_process.push(*dep_id);
                                        }
                                    }
                                    for dep_id in extract_module_dependency_ids(&pkg.modules) {
                                        if !processed.contains(&dep_id) {
                                            to_process.push(dep_id);
                                        }
                                    }
                                    result.insert(pkg_id, pkg);
                                    continue;
                                }
                            }
                        }
                        missing_reasons.push("package_missing_after_walrus_ingest".to_string());
                    } else {
                        missing_reasons.push("walrus_checkpoint_fetch_failed".to_string());
                    }
                }
            }

            // Try to find package via previous_transaction -> tx_index -> checkpoint -> Walrus
            // This allows discovering packages that aren't in the package index yet
            if let Some(walrus) = self.walrus.as_ref() {
                if let Some(prev_tx) = package_prev_txs.and_then(|m| m.get(&pkg_id)) {
                    if let Some(cp) = self.resolve_checkpoint_for_tx_digest(prev_tx).await {
                        if linkage_debug_enabled() {
                            eprintln!(
                                "[linkage] prev_tx_lookup pkg=0x{} prev_tx={} checkpoint={}",
                                hex::encode(pkg_id.as_ref()),
                                prev_tx,
                                cp
                            );
                        }
                        if let Some(checkpoint_json) = self.walrus_pool.get(walrus, cp).await {
                            // Ingest tx index entries from this checkpoint
                            if let Some(tx_idx) = self.local_tx_index.as_deref() {
                                ingest_walrus_checkpoint_tx_index(
                                    checkpoint_json.as_ref(),
                                    tx_idx,
                                    cp,
                                );
                            }
                            // Ingest packages from this checkpoint
                            let _ = ingest_walrus_checkpoint_packages(
                                checkpoint_json.as_ref(),
                                &self.cache,
                                self.local_package_index.as_deref(),
                                cp,
                            );
                            if version_hint.is_none() {
                                if let Some(pkg_index) = self.local_package_index.as_deref() {
                                    if let Ok(Some(entry)) =
                                        pkg_index.get_at_or_before_checkpoint(pkg_id, cp)
                                    {
                                        version_hint = Some(entry.version);
                                    }
                                }
                            }
                            // Try to get the package from cache now
                            if use_cache {
                                if let Some(ver) = version_hint {
                                    if let Some(pkg) = self.cache.get_package(&pkg_id, ver) {
                                        cache_hits += 1;
                                        log_package_linkage(
                                            &pkg,
                                            "walrus_prev_tx",
                                            version_hint,
                                            true,
                                        );
                                        for dep_id in pkg.linkage.values() {
                                            if !processed.contains(dep_id) {
                                                to_process.push(*dep_id);
                                            }
                                        }
                                        for dep_id in extract_module_dependency_ids(&pkg.modules) {
                                            if !processed.contains(&dep_id) {
                                                to_process.push(dep_id);
                                            }
                                        }
                                        result.insert(pkg_id, pkg);
                                        continue;
                                    }
                                } else if checkpoint.is_none() {
                                    if let Some(pkg) = self.cache.get_package_latest(&pkg_id) {
                                        cache_hits += 1;
                                        log_package_linkage(
                                            &pkg,
                                            "walrus_prev_tx_latest",
                                            version_hint,
                                            true,
                                        );
                                        for dep_id in pkg.linkage.values() {
                                            if !processed.contains(dep_id) {
                                                to_process.push(*dep_id);
                                            }
                                        }
                                        for dep_id in extract_module_dependency_ids(&pkg.modules) {
                                            if !processed.contains(&dep_id) {
                                                to_process.push(dep_id);
                                            }
                                        }
                                        result.insert(pkg_id, pkg);
                                        continue;
                                    }
                                }
                            }
                        }
                    } else {
                        missing_reasons.push("checkpoint_lookup_failed".to_string());
                        if debug_gaps {
                            eprintln!(
                                "[data_gap] kind=package_checkpoint_lookup pkg={} prev_tx={} source=prev_tx_hint",
                                pkg_id, prev_tx
                            );
                        }
                    }
                }
            }

            if walrus_only {
                eprintln!(
                    "[walrus_package_only] missing package=0x{} version_hint={:?} reasons={:?}",
                    hex::encode(pkg_id.as_ref()),
                    version_hint,
                    missing_reasons
                );
                continue;
            }

            let pkg_id_str = format!("0x{}", hex::encode(pkg_id.as_ref()));
            let mut gql_pkg: Option<GraphQLPackage> = None;
            if version_hint.is_none() {
                if let Some(cp) = checkpoint {
                    if allow_package_graphql {
                        let gql_start = std::time::Instant::now();
                        let gql_res = self.graphql.fetch_package_at_checkpoint(&pkg_id_str, cp);
                        gql_fetches += 1;
                        gql_elapsed += gql_start.elapsed().as_millis();
                        if let Ok(pkg) = gql_res {
                            version_hint = Some(pkg.version);
                            gql_pkg = Some(pkg);
                            gql_ok += 1;
                            if linkage_debug_enabled() {
                                eprintln!(
                                    "[linkage] graphql_checkpoint_version pkg={} version={}",
                                    pkg_id_str,
                                    version_hint.unwrap_or(0)
                                );
                            }
                        } else {
                            gql_fail += 1;
                        }
                    }
                }
            }

            // Fetch via gRPC (has linkage table, unlike GraphQL)
            let grpc_start = std::time::Instant::now();
            let grpc_result = if let Some(ver) = version_hint {
                self.grpc
                    .get_object_at_version(&pkg_id_str, Some(ver))
                    .await
            } else {
                self.grpc.get_object(&pkg_id_str).await
            };
            grpc_elapsed += grpc_start.elapsed().as_millis();

            match grpc_result {
                Ok(Some(grpc_obj)) => {
                    grpc_ok += 1;
                    if version_hint.is_none() && !strict_checkpoint {
                        version_hint = Some(grpc_obj.version);
                    }
                    let expected_version = version_hint;
                    let mut gql_pkg_override = gql_pkg.clone();
                    let mut version_mismatch = false;
                    if let Some(expected_version) = expected_version {
                        if grpc_obj.version != expected_version {
                            version_mismatch = true;
                            if linkage_debug_enabled() {
                                eprintln!(
                                    "[linkage] grpc_version_mismatch pkg={} expected={} got={}",
                                    pkg_id_str, expected_version, grpc_obj.version
                                );
                            }
                            if gql_pkg_override.is_none() && allow_package_graphql {
                                if let Some(cp) = checkpoint {
                                    let gql_start = std::time::Instant::now();
                                    let gql_res =
                                        self.graphql.fetch_package_at_checkpoint(&pkg_id_str, cp);
                                    gql_fetches += 1;
                                    gql_elapsed += gql_start.elapsed().as_millis();
                                    if let Ok(pkg) = gql_res {
                                        gql_ok += 1;
                                        gql_pkg_override = Some(pkg);
                                    } else {
                                        gql_fail += 1;
                                    }
                                }
                            }
                        }
                    }

                    // Attempt Walrus fetch via previous_transaction -> checkpoint for packages.
                    if let (Some(prev_tx), Some(walrus)) =
                        (grpc_obj.previous_transaction.as_ref(), self.walrus.as_ref())
                    {
                        let checkpoint_for_tx =
                            self.resolve_checkpoint_for_tx_digest(prev_tx).await;
                        if let Some(cp) = checkpoint_for_tx {
                            if let Some(checkpoint_json) = self.walrus_pool.get(walrus, cp).await {
                                if let Some(tx_index) = self.local_tx_index.as_deref() {
                                    ingest_walrus_checkpoint_tx_index(
                                        checkpoint_json.as_ref(),
                                        tx_index,
                                        cp,
                                    );
                                }
                                let _ = ingest_walrus_checkpoint_packages(
                                    checkpoint_json.as_ref(),
                                    &self.cache,
                                    self.local_package_index.as_deref(),
                                    cp,
                                );
                                if version_hint.is_none() {
                                    if let Some(pkg_index) = self.local_package_index.as_deref() {
                                        if let Ok(Some(entry)) =
                                            pkg_index.get_at_or_before_checkpoint(pkg_id, cp)
                                        {
                                            version_hint = Some(entry.version);
                                        }
                                    }
                                }
                                if use_cache {
                                    if let Some(ver) = version_hint {
                                        if let Some(pkg) = self.cache.get_package(&pkg_id, ver) {
                                            cache_hits += 1;
                                            log_package_linkage(
                                                &pkg,
                                                "walrus_prev_tx",
                                                version_hint,
                                                true,
                                            );
                                            for dep_id in pkg.linkage.values() {
                                                if !processed.contains(dep_id) {
                                                    to_process.push(*dep_id);
                                                }
                                            }
                                            for dep_id in
                                                extract_module_dependency_ids(&pkg.modules)
                                            {
                                                if !processed.contains(&dep_id) {
                                                    to_process.push(dep_id);
                                                }
                                            }
                                            result.insert(pkg_id, pkg);
                                            continue;
                                        }
                                    } else if checkpoint.is_none() {
                                        if let Some(pkg) = self.cache.get_package_latest(&pkg_id) {
                                            cache_hits += 1;
                                            log_package_linkage(
                                                &pkg,
                                                "walrus_prev_tx_latest",
                                                version_hint,
                                                true,
                                            );
                                            for dep_id in pkg.linkage.values() {
                                                if !processed.contains(dep_id) {
                                                    to_process.push(*dep_id);
                                                }
                                            }
                                            for dep_id in
                                                extract_module_dependency_ids(&pkg.modules)
                                            {
                                                if !processed.contains(&dep_id) {
                                                    to_process.push(dep_id);
                                                }
                                            }
                                            result.insert(pkg_id, pkg);
                                            continue;
                                        }
                                    }
                                }
                            }
                        } else if debug_gaps {
                            eprintln!(
                                "[data_gap] kind=package_checkpoint_lookup pkg={} prev_tx={} source=grpc_object",
                                pkg_id, prev_tx
                            );
                        }
                    }

                    if strict_checkpoint {
                        if expected_version.is_none() {
                            if let Some(pkg_at_cp) = gql_pkg_override.clone() {
                                let pkg_data = graphql_package_to_data(pkg_id, pkg_at_cp)?;
                                version_hint = Some(pkg_data.version);
                                for dep_id in extract_module_dependency_ids(&pkg_data.modules) {
                                    if !processed.contains(&dep_id) {
                                        to_process.push(dep_id);
                                    }
                                }
                                log_package_linkage(
                                    &pkg_data,
                                    "graphql_checkpoint",
                                    version_hint,
                                    false,
                                );
                                if use_cache {
                                    self.cache.put_package(pkg_data.clone());
                                }
                                result.insert(pkg_id, pkg_data);
                                continue;
                            }
                            missing_reasons.push("version_hint_missing".to_string());
                            if debug_gaps {
                                eprintln!(
                                    "[data_gap] kind=package_version_hint_missing pkg={} source=grpc_object",
                                    pkg_id
                                );
                            }
                            continue;
                        }
                        if version_mismatch {
                            if let Some(pkg_at_cp) = gql_pkg_override.clone() {
                                let pkg_data = graphql_package_to_data(pkg_id, pkg_at_cp)?;
                                version_hint = Some(pkg_data.version);
                                for dep_id in extract_module_dependency_ids(&pkg_data.modules) {
                                    if !processed.contains(&dep_id) {
                                        to_process.push(dep_id);
                                    }
                                }
                                log_package_linkage(
                                    &pkg_data,
                                    "graphql_checkpoint",
                                    version_hint,
                                    false,
                                );
                                if use_cache {
                                    self.cache.put_package(pkg_data.clone());
                                }
                                result.insert(pkg_id, pkg_data);
                                continue;
                            }
                            missing_reasons.push("grpc_version_mismatch".to_string());
                            if debug_gaps {
                                eprintln!(
                                    "[data_gap] kind=package_version_mismatch pkg={} expected={:?} got={} source=grpc_object",
                                    pkg_id, expected_version, grpc_obj.version
                                );
                            }
                            continue;
                        }
                    }

                    let mut pkg = grpc_object_to_package(&grpc_obj, pkg_id)?;
                    if let Some(cp) = checkpoint {
                        if gql_pkg_override.is_none() && allow_package_graphql {
                            let gql_start = std::time::Instant::now();
                            let gql_res = self.graphql.fetch_package_at_checkpoint(&pkg_id_str, cp);
                            gql_fetches += 1;
                            gql_elapsed += gql_start.elapsed().as_millis();
                            if let Ok(pkg_at_cp) = gql_res {
                                gql_ok += 1;
                                gql_pkg_override = Some(pkg_at_cp);
                            } else {
                                gql_fail += 1;
                            }
                        }
                    }
                    if let Some(pkg_at_cp) = gql_pkg_override {
                        let version_matches = version_hint
                            .map(|expected| expected == pkg_at_cp.version)
                            .unwrap_or(true);
                        if version_matches {
                            let modules = sui_transport::decode_graphql_modules(&pkg_id_str, &pkg_at_cp.modules)?;
                            pkg.modules = modules;
                            pkg.version = pkg_at_cp.version;
                            log_package_linkage(&pkg, "grpc+graphql_checkpoint", version_hint, false);
                        } else {
                            log_package_linkage(&pkg, "grpc", version_hint, false);
                        }
                    } else {
                        log_package_linkage(&pkg, "grpc", version_hint, false);
                    }

                    // Add dependencies to process queue
                    for dep_id in pkg.linkage.values() {
                        if !processed.contains(dep_id) {
                            to_process.push(*dep_id);
                        }
                    }
                    for dep_id in extract_module_dependency_ids(&pkg.modules) {
                        if !processed.contains(&dep_id) {
                            to_process.push(dep_id);
                        }
                    }

                    if use_cache {
                        self.cache.put_package(pkg.clone());
                    }
                    result.insert(pkg_id, pkg);
                }
                Ok(None) => {
                    grpc_fail += 1;
                    if let Some(pkg) = gql_pkg {
                        let pkg_data = graphql_package_to_data(pkg_id, pkg)?;
                        for dep_id in extract_module_dependency_ids(&pkg_data.modules) {
                            if !processed.contains(&dep_id) {
                                to_process.push(dep_id);
                            }
                        }
                        log_package_linkage(&pkg_data, "graphql_checkpoint", version_hint, false);
                        if use_cache {
                            self.cache.put_package(pkg_data.clone());
                        }
                        result.insert(pkg_id, pkg_data);
                        continue;
                    }
                    // If versioned fetch failed and no checkpoint package is available, fall back to latest
                    if !strict_checkpoint && version_hint.is_some() {
                        let grpc_start = std::time::Instant::now();
                        let grpc_latest = self.grpc.get_object(&pkg_id_str).await;
                        grpc_elapsed += grpc_start.elapsed().as_millis();
                        if let Ok(Some(grpc_obj)) = grpc_latest {
                            grpc_ok += 1;
                            let pkg = grpc_object_to_package(&grpc_obj, pkg_id)?;
                            log_package_linkage(&pkg, "grpc_fallback_latest", version_hint, false);
                            for dep_id in pkg.linkage.values() {
                                if !processed.contains(dep_id) {
                                    to_process.push(*dep_id);
                                }
                            }
                            for dep_id in extract_module_dependency_ids(&pkg.modules) {
                                if !processed.contains(&dep_id) {
                                    to_process.push(dep_id);
                                }
                            }
                            if use_cache {
                                self.cache.put_package(pkg.clone());
                            }
                            result.insert(pkg_id, pkg);
                            continue;
                        } else if grpc_latest.is_err() {
                            grpc_fail += 1;
                        }
                    }
                    eprintln!("Warning: Package not found: {}", pkg_id_str);
                }
                Err(e) => {
                    grpc_fail += 1;
                    if let Some(pkg) = gql_pkg {
                        let pkg_data = graphql_package_to_data(pkg_id, pkg)?;
                        for dep_id in extract_module_dependency_ids(&pkg_data.modules) {
                            if !processed.contains(&dep_id) {
                                to_process.push(dep_id);
                            }
                        }
                        log_package_linkage(&pkg_data, "graphql_checkpoint", version_hint, false);
                        if use_cache {
                            self.cache.put_package(pkg_data.clone());
                        }
                        result.insert(pkg_id, pkg_data);
                        continue;
                    }
                    // If versioned fetch failed and no checkpoint package is available, fall back to latest
                    if !strict_checkpoint && version_hint.is_some() {
                        let grpc_start = std::time::Instant::now();
                        let grpc_latest = self.grpc.get_object(&pkg_id_str).await;
                        grpc_elapsed += grpc_start.elapsed().as_millis();
                        if let Ok(Some(grpc_obj)) = grpc_latest {
                            grpc_ok += 1;
                            let pkg = grpc_object_to_package(&grpc_obj, pkg_id)?;
                            for dep_id in pkg.linkage.values() {
                                if !processed.contains(dep_id) {
                                    to_process.push(*dep_id);
                                }
                            }
                            for dep_id in extract_module_dependency_ids(&pkg.modules) {
                                if !processed.contains(&dep_id) {
                                    to_process.push(dep_id);
                                }
                            }
                            if use_cache {
                                self.cache.put_package(pkg.clone());
                            }
                            result.insert(pkg_id, pkg);
                            continue;
                        } else if grpc_latest.is_err() {
                            grpc_fail += 1;
                        }
                    }
                    eprintln!("Warning: Failed to fetch package {}: {}", pkg_id_str, e);
                }
            }
        }
        if timing {
            eprintln!(
                "[timing] stage=fetch_packages_with_deps requested={} processed={} cache_hits={} grpc_ok={} grpc_fail={} gql_fetches={} gql_ok={} gql_fail={} grpc_ms={} gql_ms={} total_ms={}",
                package_ids.len(),
                processed.len(),
                cache_hits,
                grpc_ok,
                grpc_fail,
                gql_fetches,
                gql_ok,
                gql_fail,
                grpc_elapsed,
                gql_elapsed,
                start.elapsed().as_millis()
            );
        }

        Ok(result)
    }

    // ==================== On-Demand Fetcher ====================

    /// Create an on-demand fetcher callback for the VM.
    ///
    /// This returns a closure that can be used by the VM during execution
    /// to fetch objects that weren't prefetched. It's a fallback mechanism
    /// for dynamic field children discovered at runtime.
    ///
    /// Note: The returned closure captures the gRPC endpoint and creates
    /// new connections as needed. This is less efficient than reusing
    /// connections but allows the closure to be Send + Sync.
    pub fn create_on_demand_fetcher(
        &self,
    ) -> impl Fn(ObjectID, u64) -> Option<VersionedObject> + Send + Sync + 'static {
        let cache = Arc::clone(&self.cache);
        let endpoint = self.grpc_endpoint.clone();

        move |id: ObjectID, version: u64| {
            // Check cache first
            if let Some(obj) = cache.get_object(&id, version) {
                return Some(obj);
            }

            // Fetch from gRPC (blocking) - create a new client for each call
            let id_str = format!("0x{}", hex::encode(id.as_ref()));
            let endpoint_clone = endpoint.clone();

            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(_) => return None,
            };

            let result = rt.block_on(async {
                let client = match GrpcClient::new(&endpoint_clone).await {
                    Ok(c) => c,
                    Err(_) => return None,
                };
                client
                    .get_object_at_version(&id_str, Some(version))
                    .await
                    .ok()
                    .flatten()
            });

            if let Some(grpc_obj) = result {
                if let Ok(obj) = grpc_object_to_versioned(&grpc_obj, id, version) {
                    cache.put_object(obj.clone());
                    return Some(obj);
                }
            }

            None
        }
    }

    // ==================== Accessors ====================

    /// Get a reference to the gRPC client.
    pub fn grpc(&self) -> &GrpcClient {
        &self.grpc
    }

    /// Get a reference to the GraphQL client.
    pub fn graphql(&self) -> &GraphQLClient {
        &self.graphql
    }

    /// Get a reference to the cache.
    pub fn cache(&self) -> &VersionedCache {
        &self.cache
    }

    /// Get the gRPC endpoint URL.
    pub fn grpc_endpoint(&self) -> &str {
        &self.grpc_endpoint
    }

    /// Flush the cache to disk (if disk caching is enabled).
    pub fn flush_cache(&self) -> Result<()> {
        self.cache.flush()
    }
}

// ==================== Helper Functions ====================

/// Parse an object ID from a hex string.
fn parse_object_id(id_str: &str) -> Result<ObjectID> {
    let normalized = normalize_address(id_str);
    let hex_str = normalized.strip_prefix("0x").unwrap_or(&normalized);
    let bytes = hex::decode(hex_str)?;
    if bytes.len() != 32 {
        return Err(anyhow!("Invalid object ID length: {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(AccountAddress::new(arr))
}

fn synthesize_clock_bytes(clock_id: &AccountAddress, timestamp_ms: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend_from_slice(clock_id.as_ref()); // UID (32 bytes)
    bytes.extend_from_slice(&timestamp_ms.to_le_bytes()); // timestamp_ms (8 bytes)
    bytes
}

fn synthesize_random_bytes(random_id: &AccountAddress, version: u64) -> Vec<u8> {
    // Random struct: { id: UID, inner: Versioned { id: UID, version: u64 } }
    let mut bytes = Vec::with_capacity(72);
    bytes.extend_from_slice(random_id.as_ref()); // UID (32 bytes)
    bytes.extend_from_slice(random_id.as_ref()); // inner UID (32 bytes)
    bytes.extend_from_slice(&version.to_le_bytes()); // version (8 bytes)
    bytes
}

fn ensure_system_objects(
    objects: &mut HashMap<ObjectID, VersionedObject>,
    historical_versions: &HashMap<String, u64>,
    tx_timestamp_ms: Option<u64>,
    checkpoint: Option<u64>,
) {
    let clock_id = match parse_object_id(CLOCK_OBJECT_ID) {
        Ok(id) => id,
        Err(_) => return,
    };
    objects.entry(clock_id).or_insert_with(|| {
        let clock_version = historical_versions
            .get(&normalize_address(CLOCK_OBJECT_ID))
            .copied()
            .or(checkpoint)
            .unwrap_or(1);
        let clock_ts = tx_timestamp_ms.unwrap_or(DEFAULT_CLOCK_BASE_MS);
        VersionedObject {
            id: clock_id,
            version: clock_version,
            digest: None,
            type_tag: Some(CLOCK_TYPE.to_string()),
            bcs_bytes: synthesize_clock_bytes(&clock_id, clock_ts),
            is_shared: true,
            is_immutable: false,
        }
    });

    let random_id = match parse_object_id(RANDOM_OBJECT_ID) {
        Ok(id) => id,
        Err(_) => return,
    };
    objects.entry(random_id).or_insert_with(|| {
        let random_version = historical_versions
            .get(&normalize_address(RANDOM_OBJECT_ID))
            .copied()
            .or(checkpoint)
            .unwrap_or(1);
        VersionedObject {
            id: random_id,
            version: random_version,
            digest: None,
            type_tag: Some(RANDOM_TYPE.to_string()),
            bcs_bytes: synthesize_random_bytes(&random_id, random_version),
            is_shared: true,
            is_immutable: false,
        }
    });
}

fn find_walrus_tx_json<'a>(checkpoint_json: &'a Value, digest: &str) -> Option<&'a Value> {
    let transactions = checkpoint_json.get("transactions")?.as_array()?;
    for tx_json in transactions {
        let tx_digest = tx_json
            .pointer("/effects/V2/transaction_digest")
            .and_then(|v| v.as_str());
        if tx_digest == Some(digest) {
            return Some(tx_json);
        }
    }
    None
}

fn extract_walrus_tx_digest(tx_json: &Value) -> Option<String> {
    tx_json
        .pointer("/effects/V2/transaction_digest")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn is_dynamic_field_type_tag(tag: &TypeTag) -> bool {
    match tag {
        TypeTag::Struct(s) => {
            let module = s.module.as_str();
            let name = s.name.as_str();
            s.address == AccountAddress::TWO
                && name == "Field"
                && (module == "dynamic_field" || module == "dynamic_object_field")
        }
        _ => false,
    }
}

fn parse_owner_parent(owner_json: &Value) -> Option<AccountAddress> {
    if let Some(parent) = owner_json.get("ObjectOwner").and_then(|v| v.as_str()) {
        return parse_object_id(parent).ok();
    }
    None
}

fn decode_walrus_contents(value: Option<&Value>) -> Option<Vec<u8>> {
    let value = value?;
    if let Some(s) = value.as_str() {
        return base64::engine::general_purpose::STANDARD.decode(s).ok();
    }
    if let Some(arr) = value.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for x in arr {
            let n = x.as_u64()?;
            if n > 255 {
                return None;
            }
            out.push(n as u8);
        }
        return Some(out);
    }
    None
}

fn extract_object_id_from_move_obj(move_obj: &Value) -> Option<AccountAddress> {
    let contents = decode_walrus_contents(move_obj.get("contents"))?;
    if contents.len() < 32 {
        return None;
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&contents[0..32]);
    Some(AccountAddress::new(bytes))
}

fn find_prev_tx_for_object_in_tx(tx_json: &Value, target: &AccountAddress) -> Option<String> {
    let inputs = tx_json.get("input_objects").and_then(|v| v.as_array());
    if let Some(arr) = inputs {
        for obj_json in arr {
            let move_obj = obj_json.get("data").and_then(|d| d.get("Move"))?;
            let id = extract_object_id_from_move_obj(move_obj)?;
            if &id == target {
                return obj_json
                    .get("previous_transaction")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }
    }
    None
}

fn parse_type_tag_json(type_json: &Value) -> Result<TypeTag> {
    if let Some(s) = type_json.as_str() {
        if s == "GasCoin" {
            return TypeTag::from_str("0x2::coin::Coin<0x2::sui::SUI>")
                .map_err(|e| anyhow!("parse GasCoin TypeTag: {e}"));
        }
        return TypeTag::from_str(s).map_err(|e| anyhow!("parse TypeTag {s:?}: {e}"));
    }

    if let Some(coin_json) = type_json.get("Coin") {
        if let Some(struct_json) = coin_json.get("struct") {
            let inner = parse_type_tag_json(&serde_json::json!({ "struct": struct_json }))?;
            let inner_str = format!("{inner}");
            let s = format!("0x2::coin::Coin<{inner_str}>");
            return TypeTag::from_str(&s)
                .map_err(|e| anyhow!("parse Coin TypeTag from {s:?}: {e}"));
        }
    }

    if let Some(vec_json) = type_json.get("vector") {
        let inner = parse_type_tag_json(vec_json)?;
        return Ok(TypeTag::Vector(Box::new(inner)));
    }
    let struct_json = if let Some(other) = type_json.get("Other") {
        other
    } else if let Some(s) = type_json.get("struct") {
        s
    } else if type_json.get("address").is_some() {
        type_json
    } else {
        return Err(anyhow!("unsupported type tag JSON: {}", type_json));
    };

    let address = struct_json
        .get("address")
        .and_then(|a| a.as_str())
        .ok_or_else(|| anyhow!("Missing address in type"))?;
    let module = struct_json
        .get("module")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow!("Missing module in type"))?;
    let name = struct_json
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow!("Missing name in type"))?;
    let type_args = struct_json
        .get("type_args")
        .and_then(|t| t.as_array())
        .unwrap_or(&vec![])
        .iter()
        .map(parse_type_tag_json)
        .collect::<Result<Vec<_>>>()?;

    let address = if address.starts_with("0x") {
        address.to_string()
    } else {
        format!("0x{address}")
    };
    let mut s = format!("{address}::{module}::{name}");
    if !type_args.is_empty() {
        let inner = type_args
            .iter()
            .map(|t| format!("{t}"))
            .collect::<Vec<_>>()
            .join(", ");
        s.push('<');
        s.push_str(&inner);
        s.push('>');
    }
    TypeTag::from_str(&s).map_err(|e| anyhow!("parse TypeTag from {s:?}: {e}"))
}

fn parse_owner_flags(owner_json: &Value) -> (bool, bool) {
    if owner_json.get("Immutable").is_some() {
        return (false, true);
    }
    if owner_json.get("Shared").is_some() {
        return (true, false);
    }
    (false, false)
}

fn owner_kind_string(owner_json: &Value) -> Option<String> {
    if owner_json.get("Immutable").is_some() {
        return Some("immutable".to_string());
    }
    if owner_json.get("Shared").is_some() {
        return Some("shared".to_string());
    }
    if owner_json.get("AddressOwner").is_some() || owner_json.get("ObjectOwner").is_some() {
        return Some("address".to_string());
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn ingest_walrus_objects(
    tx_json: &Value,
    cache: Option<&VersionedCache>,
    mut historical_versions: Option<&mut HashMap<String, u64>>,
    store: Option<&FsObjectStore>,
    index: Option<&FsObjectIndex>,
    package_index: Option<&FsPackageIndex>,
    dynamic_fields: Option<&FsDynamicFieldCache>,
    source_checkpoint: Option<u64>,
) -> usize {
    let mut ingested = 0usize;
    for key in ["input_objects", "output_objects"] {
        let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for obj_json in arr {
            let prev_tx = obj_json
                .get("previous_transaction")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(pkg_json) = obj_json.get("data").and_then(|d| d.get("Package")) {
                if let Ok(pkg) = serde_json::from_value::<MovePackage>(pkg_json.clone()) {
                    let pkg_data = package_data_from_move_package(&pkg);
                    if let Some(cache) = cache {
                        cache.put_package(pkg_data.clone());
                    }
                    if let (Some(pkg_index), Some(checkpoint)) = (package_index, source_checkpoint)
                    {
                        let _ = pkg_index.put(
                            pkg_data.address,
                            pkg_data.version,
                            checkpoint,
                            prev_tx.clone(),
                        );
                    }
                    if let Some(historical_versions) = historical_versions.as_deref_mut() {
                        let pkg_str = format!("0x{}", hex::encode(pkg_data.address.as_ref()));
                        historical_versions.insert(normalize_address(&pkg_str), pkg_data.version);
                    }
                    if cache.is_some() {
                        ingested += 1;
                    }
                }
                continue;
            }
            let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
                continue;
            };
            let contents = decode_walrus_contents(move_obj.get("contents"));
            let Some(bcs_bytes) = contents else {
                continue;
            };
            if bcs_bytes.len() < 32 {
                continue;
            }
            let id = AccountAddress::new({
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&bcs_bytes[0..32]);
                bytes
            });
            let version = match move_obj.get("version").and_then(|v| v.as_u64()) {
                Some(v) => v,
                None => continue,
            };
            let parsed_tag = move_obj
                .get("type_")
                .and_then(|t| parse_type_tag_json(t).ok());
            let type_tag = parsed_tag.as_ref().map(|t| t.to_string());
            let is_dynamic_field = parsed_tag
                .as_ref()
                .map(is_dynamic_field_type_tag)
                .unwrap_or(false);
            let owner_json = obj_json.get("owner").unwrap_or(&Value::Null);
            let (is_shared, is_immutable) = parse_owner_flags(owner_json);
            let owner_kind = owner_kind_string(owner_json);
            let parent_owner = parse_owner_parent(owner_json);

            if let Some(cache) = cache {
                cache.put_object(VersionedObject {
                    id,
                    version,
                    digest: None,
                    type_tag: type_tag.clone(),
                    bcs_bytes: bcs_bytes.clone(),
                    is_shared,
                    is_immutable,
                });
            }
            if let Some(historical_versions) = historical_versions.as_deref_mut() {
                historical_versions.insert(normalize_address(&id.to_hex_literal()), version);
            }
            if let Some(store) = store {
                if let Some(type_tag) = type_tag.clone() {
                    let meta = ObjectMeta {
                        type_tag,
                        owner_kind,
                        source_checkpoint,
                    };
                    let _ = store.put(id, version, &bcs_bytes, &meta);
                }
            }
            if let (Some(index), Some(checkpoint)) = (index, source_checkpoint) {
                let _ = index.put(id, version, checkpoint, prev_tx.clone());
            }
            if let (Some(dynamic_fields), Some(checkpoint), Some(parent_id)) =
                (dynamic_fields, source_checkpoint, parent_owner)
            {
                if is_dynamic_field {
                    let entry = DynamicFieldEntry {
                        checkpoint,
                        parent_id: parent_id.to_hex_literal(),
                        child_id: id.to_hex_literal(),
                        version,
                        type_tag: type_tag.clone(),
                        prev_tx: prev_tx.clone(),
                    };
                    let _ = dynamic_fields.put_entry(entry);
                }
            }
            if cache.is_some() || store.is_some() {
                ingested += 1;
            }
        }
    }
    ingested
}

#[allow(clippy::too_many_arguments)]
fn ingest_walrus_tx_objects(
    tx_json: &Value,
    cache: &VersionedCache,
    historical_versions: &mut HashMap<String, u64>,
    store: Option<&FsObjectStore>,
    index: Option<&FsObjectIndex>,
    package_index: Option<&FsPackageIndex>,
    dynamic_fields: Option<&FsDynamicFieldCache>,
    source_checkpoint: Option<u64>,
) -> usize {
    ingest_walrus_objects(
        tx_json,
        Some(cache),
        Some(historical_versions),
        store,
        index,
        package_index,
        dynamic_fields,
        source_checkpoint,
    )
}

fn ingest_walrus_checkpoint_objects(
    checkpoint_json: &Value,
    store: Option<&FsObjectStore>,
    index: Option<&FsObjectIndex>,
    package_index: Option<&FsPackageIndex>,
    dynamic_fields: Option<&FsDynamicFieldCache>,
    source_checkpoint: Option<u64>,
) -> usize {
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return 0;
    };
    let mut total = 0usize;
    for tx_json in transactions {
        total += ingest_walrus_objects(
            tx_json,
            None,
            None,
            store,
            index,
            package_index,
            dynamic_fields,
            source_checkpoint,
        );
    }
    total
}

fn ingest_walrus_checkpoint_packages(
    checkpoint_json: &Value,
    cache: &VersionedCache,
    package_index: Option<&FsPackageIndex>,
    source_checkpoint: u64,
) -> usize {
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return 0;
    };
    let mut ingested = 0usize;
    let debug_walrus = std::env::var("SUI_DEBUG_WALRUS").ok().as_deref() == Some("1");
    for tx_json in transactions {
        for key in ["input_objects", "output_objects"] {
            let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
                continue;
            };
            for obj_json in arr {
                let prev_tx = obj_json
                    .get("previous_transaction")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let pkg_json = obj_json.get("data").and_then(|d| d.get("Package"));
                if pkg_json.is_none() {
                    continue;
                }
                let pkg_json = pkg_json.unwrap();
                match serde_json::from_value::<MovePackage>(pkg_json.clone()) {
                    Ok(pkg) => {
                        let pkg_data = package_data_from_move_package(&pkg);
                        cache.put_package(pkg_data.clone());
                        if let Some(pkg_index) = package_index {
                            let _ = pkg_index.put(
                                pkg_data.address,
                                pkg_data.version,
                                source_checkpoint,
                                prev_tx.clone(),
                            );
                        }
                        ingested += 1;
                    }
                    Err(e) => {
                        if debug_walrus {
                            eprintln!(
                                "[walrus] checkpoint={} failed to parse package: {}",
                                source_checkpoint, e
                            );
                        }
                    }
                }
            }
        }
    }
    ingested
}

fn ingest_walrus_checkpoint_tx_index(
    checkpoint_json: &Value,
    tx_index: &FsTxDigestIndex,
    checkpoint: u64,
) {
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return;
    };
    for tx_json in transactions {
        if let Some(digest) = extract_walrus_tx_digest(tx_json) {
            let _ = tx_index.put(&digest, checkpoint);
        }
    }
}

/// Extract object ID and version from a gRPC input.
fn extract_object_id_and_version(
    input: &sui_transport::grpc::GrpcInput,
) -> Option<(ObjectID, u64)> {
    use sui_transport::grpc::GrpcInput;
    match input {
        GrpcInput::Object {
            object_id, version, ..
        } => {
            let id = parse_object_id(object_id).ok()?;
            Some((id, *version))
        }
        GrpcInput::SharedObject {
            object_id,
            initial_version,
            ..
        } => {
            let id = parse_object_id(object_id).ok()?;
            Some((id, *initial_version))
        }
        GrpcInput::Receiving {
            object_id, version, ..
        } => {
            let id = parse_object_id(object_id).ok()?;
            Some((id, *version))
        }
        GrpcInput::Pure { .. } => None,
    }
}

/// Extract package IDs from a gRPC transaction.
fn extract_package_ids_from_tx(tx: &sui_transport::grpc::GrpcTransaction) -> Vec<AccountAddress> {
    use sui_transport::grpc::GrpcCommand;
    let mut packages = HashSet::new();

    for cmd in &tx.commands {
        if let GrpcCommand::MoveCall { package, .. } = cmd {
            if let Ok(id) = parse_object_id(package) {
                packages.insert(id);
            }
        }
    }

    packages.into_iter().collect()
}

/// Extract package IDs from a type string.
///
/// Parses type strings like "0x2::coin::Coin<0x123::token::TOKEN>"
/// and extracts all package addresses found.
fn extract_package_ids_from_type(type_str: &str) -> Vec<String> {
    let mut result = Vec::new();

    // Find all 0x... addresses in the type string
    let mut chars = type_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            chars.next(); // consume 'x'
            let mut addr = String::from("0x");

            // Collect hex characters
            while let Some(&c) = chars.peek() {
                if c.is_ascii_hexdigit() {
                    addr.push(chars.next().unwrap());
                } else {
                    break;
                }
            }

            // Only add if it looks like a valid address and is followed by ::
            if addr.len() > 2 {
                let normalized = normalize_address(&addr);
                // Skip framework packages (0x1, 0x2, 0x3)
                if normalized
                    != "0x0000000000000000000000000000000000000000000000000000000000000001"
                    && normalized
                        != "0x0000000000000000000000000000000000000000000000000000000000000002"
                    && normalized
                        != "0x0000000000000000000000000000000000000000000000000000000000000003"
                {
                    result.push(normalized);
                }
            }
        }
    }

    result
}

/// Convert a gRPC object to VersionedObject.
fn grpc_object_to_versioned(
    grpc_obj: &sui_transport::grpc::GrpcObject,
    id: ObjectID,
    version: u64,
) -> Result<VersionedObject> {
    use sui_transport::grpc::GrpcOwner;

    let (is_shared, is_immutable) = match &grpc_obj.owner {
        GrpcOwner::Shared { .. } => (true, false),
        GrpcOwner::Immutable => (false, true),
        _ => (false, false),
    };

    Ok(VersionedObject {
        id,
        version,
        digest: Some(grpc_obj.digest.clone()),
        type_tag: grpc_obj.type_string.clone(),
        bcs_bytes: grpc_obj.bcs.clone().unwrap_or_default(),
        is_shared,
        is_immutable,
    })
}

/// Convert a gRPC object (package) to PackageData.
fn grpc_object_to_package(
    grpc_obj: &sui_transport::grpc::GrpcObject,
    address: AccountAddress,
) -> Result<PackageData> {
    // Get modules from package_modules field
    let modules = grpc_obj.package_modules.clone().unwrap_or_default();

    // Parse linkage table
    let mut linkage = HashMap::new();
    if let Some(ref linkage_entries) = grpc_obj.package_linkage {
        for entry in linkage_entries {
            if let (Ok(orig_id), Ok(upg_id)) = (
                parse_object_id(&entry.original_id),
                parse_object_id(&entry.upgraded_id),
            ) {
                linkage.insert(orig_id, upg_id);
            }
        }
    }

    Ok(PackageData {
        address,
        version: grpc_obj.version,
        modules,
        linkage,
        original_id: grpc_obj
            .package_original_id
            .as_ref()
            .and_then(|s| parse_object_id(s).ok()),
    })
}

use sui_package_extractor::extract_module_dependency_ids;

pub fn package_data_from_move_package(pkg: &MovePackage) -> PackageData {
    let modules = pkg
        .serialized_module_map()
        .iter()
        .map(|(name, bytes)| (name.clone(), bytes.clone()))
        .collect::<Vec<_>>();

    let linkage = pkg
        .linkage_table()
        .iter()
        .map(|(orig_id, info)| {
            (
                AccountAddress::from(*orig_id),
                AccountAddress::from(info.upgraded_id),
            )
        })
        .collect::<HashMap<_, _>>();

    let original_id = Some(AccountAddress::from(pkg.original_package_id()));

    PackageData {
        address: AccountAddress::from(pkg.id()),
        version: pkg.version().value(),
        modules,
        linkage,
        original_id,
    }
}

fn graphql_package_to_data(pkg_id: AccountAddress, pkg: GraphQLPackage) -> Result<PackageData> {
    let modules =
        sui_transport::decode_graphql_modules(&pkg_id.to_string(), &pkg.modules)?;

    Ok(PackageData {
        address: pkg_id,
        version: pkg.version,
        modules,
        linkage: HashMap::new(),
        original_id: None,
    })
}
