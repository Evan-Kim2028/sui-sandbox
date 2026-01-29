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
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use tracing::debug;

use sui_prefetch::grpc_to_fetched_transaction;
use sui_resolver::address::normalize_address;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;

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
}

/// Default mainnet gRPC endpoint
const MAINNET_GRPC: &str = "https://fullnode.mainnet.sui.io:443";
/// Default testnet gRPC endpoint
const TESTNET_GRPC: &str = "https://fullnode.testnet.sui.io:443";

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
        })
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
        self.fetch_replay_state_with_config(digest, true, 3, 200)
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
    ) -> Result<ReplayState> {
        let start = std::time::Instant::now();

        // 1. Fetch transaction via gRPC (has unchanged_loaded_runtime_objects)
        let tx_start = std::time::Instant::now();
        let grpc_tx = self
            .grpc
            .get_transaction(digest)
            .await?
            .ok_or_else(|| anyhow!("Transaction not found: {}", digest))?;
        debug!(
            digest = digest,
            elapsed_ms = tx_start.elapsed().as_millis(),
            "fetched transaction via gRPC"
        );
        if std::env::var("SUI_DUMP_TX_OBJECTS")
            .ok()
            .as_deref()
            == Some("1")
        {
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
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.grpc.get_checkpoint(seq),
            )
            .await
            {
                Ok(Ok(Some(cp))) => {
                    if std::env::var("SUI_DUMP_TX_OBJECTS")
                        .ok()
                        .as_deref()
                        == Some("1")
                    {
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

        if std::env::var("SUI_DUMP_RUNTIME_OBJECTS")
            .ok()
            .as_deref()
            == Some("1")
        {
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
                .filter_map(|input| extract_object_id_and_version(input))
                .find(|(id, _)| normalize_address(&format!("0x{}", hex::encode(id.as_ref()))) == target_norm)
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
            } else if std::env::var("SUI_DUMP_RUNTIME_OBJECTS")
                .ok()
                .as_deref()
                == Some("1")
            {
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
            debug!(digest = digest, added = added, "added checkpoint dynamic field objects");
        }

        // Merge prefetched children (they take precedence since they have BCS data)
        for (id, obj) in prefetched_children {
            objects.entry(id).or_insert(obj);
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

        // If any object fetch fell back to a different version than requested,
        // avoid version-pinning packages to reduce layout mismatches.
        let used_non_historical = object_requests.iter().any(|(id, ver)| {
            objects
                .get(id)
                .map(|obj| obj.version != *ver)
                .unwrap_or(true)
        });
        let package_versions_opt = if used_non_historical {
            None
        } else {
            Some(&package_versions)
        };

        let pkg_start = std::time::Instant::now();
        let packages = self
            .fetch_packages_with_deps(
                &package_ids_vec,
                package_versions_opt,
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

        // 8. Convert to FetchedTransaction format
        let transaction = grpc_to_fetched_transaction(&grpc_tx)?;

        debug!(
            digest = digest,
            elapsed_ms = start.elapsed().as_millis(),
            "completed replay state fetch"
        );

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
    async fn prefetch_dynamic_fields_internal(
        &self,
        historical_versions: &HashMap<String, u64>,
        max_depth: usize,
        limit_per_parent: usize,
        checkpoint: Option<u64>,
    ) -> Vec<(String, u64, String, Vec<u8>)> {
        use base64::Engine;

        let mut result = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut to_process: Vec<(String, usize)> =
            historical_versions.keys().map(|k| (k.clone(), 0)).collect();
        let max_lamport_version = historical_versions.values().copied().max().unwrap_or(0);
        let max_secs = std::env::var("SUI_STATE_DF_PREFETCH_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(30);
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

            // Fetch dynamic fields for this parent (checkpoint snapshot if available)
            let (fields, snapshot_used) = match checkpoint {
                Some(cp) => match self
                    .graphql
                    .fetch_dynamic_fields_at_checkpoint(&parent_id, limit_per_parent, cp)
                {
                    Ok(fields) => (fields, true),
                    Err(_) => match self.graphql.fetch_dynamic_fields(&parent_id, limit_per_parent)
                    {
                        Ok(fields) => (fields, false),
                        Err(_) => continue,
                    },
                },
                None => match self.graphql.fetch_dynamic_fields(&parent_id, limit_per_parent) {
                    Ok(fields) => (fields, false),
                    Err(_) => continue,
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
                        let version_opt = if let Some(v) = historical_versions.get(&child_normalized)
                        {
                            Some(*v)
                        } else if let Some(v) = df.version {
                            if snapshot_used || v <= max_lamport_version {
                                Some(v)
                            } else {
                                continue;
                            }
                        } else if let Ok(Some(obj)) =
                            self.grpc.get_object(&child_normalized).await
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
                        let (type_str, bcs) =
                            if let (Some(vt), Some(vb)) = (&df.value_type, &df.value_bcs) {
                                if let Ok(decoded) =
                                    base64::engine::general_purpose::STANDARD.decode(vb)
                                {
                                    (vt.clone(), decoded)
                                } else {
                                    continue;
                                }
                            } else if let Ok(obj) =
                                self.graphql.fetch_object_at_version(&child_normalized, version)
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
                                if let Ok(obj) =
                                    self.graphql.fetch_object_at_checkpoint(&child_normalized, cp)
                                {
                                    if obj.version != version {
                                        continue;
                                    }
                                    if let (Some(ts), Some(b64)) =
                                        (obj.type_string, obj.bcs_base64)
                                    {
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
        use base64::Engine;

        let mut result = HashMap::new();
        let mut to_fetch = Vec::new();

        // Check cache first
        for (id, version) in requests {
            if let Some(obj) = self.cache.get_object(id, *version) {
                result.insert(*id, obj);
            } else {
                to_fetch.push((*id, *version));
            }
        }

        if to_fetch.is_empty() {
            return Ok(result);
        }

        // Fetch missing objects via gRPC, with GraphQL fallback
        for (id, version) in &to_fetch {
            let id_str = format!("0x{}", hex::encode(id.as_ref()));

            // Try gRPC first
            let grpc_result = self
                .grpc
                .get_object_at_version(&id_str, Some(*version))
                .await;

            match grpc_result {
                Ok(Some(grpc_obj)) => {
                    let obj = grpc_object_to_versioned(&grpc_obj, *id, *version)?;
                    self.cache.put_object(obj.clone());
                    result.insert(*id, obj);
                }
                Ok(None) | Err(_) => {
                    // gRPC failed - try GraphQL for current version as fallback
                    // This is necessary when historical versions are pruned from the archive
                    let gql_obj = self
                        .graphql
                        .fetch_object_at_version(&id_str, *version)
                        .or_else(|_| self.graphql.fetch_object(&id_str));
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
                                self.cache.put_object(obj.clone());
                                result.insert(*id, obj);
                            }
                        }
                    } else {
                        eprintln!("Warning: Failed to fetch object {} at version {} (gRPC and GraphQL both failed)", id_str, version);
                    }
                }
            }
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
        use base64::Engine;
        let mut result = HashMap::new();
        let mut to_process: Vec<AccountAddress> = package_ids.to_vec();
        let mut processed: HashSet<AccountAddress> = HashSet::new();

        while let Some(pkg_id) = to_process.pop() {
            if processed.contains(&pkg_id) {
                continue;
            }
            processed.insert(pkg_id);

            let version_hint = package_versions.and_then(|m| m.get(&pkg_id).copied());

            // Check cache first (version-aware if possible)
            if let Some(ver) = version_hint {
                if let Some(pkg) = self.cache.get_package(&pkg_id, ver) {
                    // Add dependencies to process queue
                    for dep_id in pkg.linkage.values() {
                        if !processed.contains(dep_id) {
                            to_process.push(*dep_id);
                        }
                    }
                    result.insert(pkg_id, pkg);
                    continue;
                }
            } else if let Some(pkg) = self.cache.get_package_latest(&pkg_id) {
                // Add dependencies to process queue
                for dep_id in pkg.linkage.values() {
                    if !processed.contains(dep_id) {
                        to_process.push(*dep_id);
                    }
                }
                result.insert(pkg_id, pkg);
                continue;
            }

            // Fetch via gRPC (has linkage table, unlike GraphQL)
            let pkg_id_str = format!("0x{}", hex::encode(pkg_id.as_ref()));
            let grpc_result = if let Some(ver) = version_hint {
                self.grpc.get_object_at_version(&pkg_id_str, Some(ver)).await
            } else {
                self.grpc.get_object(&pkg_id_str).await
            };

            match grpc_result {
                Ok(Some(grpc_obj)) => {
                    let pkg = grpc_object_to_package(&grpc_obj, pkg_id)?;

                    // Add dependencies to process queue
                    for dep_id in pkg.linkage.values() {
                        if !processed.contains(dep_id) {
                            to_process.push(*dep_id);
                        }
                    }

                    self.cache.put_package(pkg.clone());
                    result.insert(pkg_id, pkg);
                }
                Ok(None) => {
                    // If versioned fetch failed, fall back to latest
                    if version_hint.is_some() {
                        if let Ok(Some(grpc_obj)) = self.grpc.get_object(&pkg_id_str).await {
                            let pkg = grpc_object_to_package(&grpc_obj, pkg_id)?;
                            for dep_id in pkg.linkage.values() {
                                if !processed.contains(dep_id) {
                                    to_process.push(*dep_id);
                                }
                            }
                            self.cache.put_package(pkg.clone());
                            result.insert(pkg_id, pkg);
                            continue;
                        }
                    }
                    eprintln!("Warning: Package not found: {}", pkg_id_str);
                }
                Err(e) => {
                    // If versioned fetch failed, fall back to latest
                    if version_hint.is_some() {
                        if let Ok(Some(grpc_obj)) = self.grpc.get_object(&pkg_id_str).await {
                            let pkg = grpc_object_to_package(&grpc_obj, pkg_id)?;
                            for dep_id in pkg.linkage.values() {
                                if !processed.contains(dep_id) {
                                    to_process.push(*dep_id);
                                }
                            }
                            self.cache.put_package(pkg.clone());
                            result.insert(pkg_id, pkg);
                            continue;
                        }
                    }
                    // Try GraphQL checkpoint snapshot as fallback
                    if let Some(cp) = checkpoint {
                        if let Ok(pkg) =
                            self.graphql.fetch_package_at_checkpoint(&pkg_id_str, cp)
                        {
                            let pkg_data = PackageData {
                                address: pkg_id,
                                version: pkg.version,
                                modules: pkg
                                    .modules
                                    .iter()
                                    .filter_map(|m| {
                                        m.bytecode_base64.as_ref().and_then(|b64| {
                                            base64::engine::general_purpose::STANDARD
                                                .decode(b64)
                                                .ok()
                                                .map(|bytes| (m.name.clone(), bytes))
                                        })
                                    })
                                    .collect(),
                                linkage: HashMap::new(),
                                original_id: None,
                            };
                            self.cache.put_package(pkg_data.clone());
                            result.insert(pkg_id, pkg_data);
                            continue;
                        }
                    }
                    eprintln!("Warning: Failed to fetch package {}: {}", pkg_id_str, e);
                }
            }
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
