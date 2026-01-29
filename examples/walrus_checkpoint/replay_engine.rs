use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sui_historical_cache::{
    CacheMetrics, FsObjectStore, FsPackageStore, ObjectVersionStore, PackageStore,
};
use sui_prefetch::compute_dynamic_field_id;
use sui_protocol_config::ProtocolConfig;
use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::ptb::{Command, InputValue, ObjectID, ObjectVersionInfo, VersionChangeType};
use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_sandbox_core::utilities::extract_dependencies_from_bytecode;
use sui_state_fetcher::replay::build_address_aliases;
use sui_state_fetcher::HistoricalStateProvider;
use sui_transport::graphql::{DynamicFieldInfo, GraphQLClient};
use sui_transport::grpc::{GrpcClient, GrpcInput};
use sui_transport::walrus::WalrusClient;
use sui_types::base_types::{MoveObjectType, ObjectID as SuiObjectID, SequenceNumber, SuiAddress};
use sui_types::digests::{ObjectDigest, TransactionDigest};
use sui_types::object::{Data, MoveObject, Object, ObjectInner, Owner};

use super::ptb_from_walrus_json::{parse_ptb_transaction, ParsedWalrusPtb};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReasonCode {
    StrictMatch,
    ParseError,
    UnsupportedCommand,
    MissingPackage,
    MissingObject,
    DynamicFieldMiss,
    Timeout,
    ExecutionFailure,
    StatusMismatch,
    LamportMismatch,
    ObjectMismatch,
    GasMismatch,
    WalrusInconsistent,
    NotModeled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptKind {
    WalrusOnly,
    RetryWithChildFetcher,
    RetryWithPrefetch,
    RetryWithMM2,
}

#[derive(Debug, Clone)]
pub struct AttemptReport {
    pub kind: AttemptKind,
    pub success: bool,
    pub parity: bool,
    pub reason: ReasonCode,
    pub duration: Duration,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TxOutcome {
    pub digest: String,
    pub checkpoint: u64,
    pub attempts: Vec<AttemptReport>,
    pub final_parity: bool,
    pub final_reason: ReasonCode,
}

#[derive(Default)]
pub struct PackageCache {
    modules_by_package: HashMap<AccountAddress, Vec<(String, Vec<u8>)>>,
    versions_by_package: HashMap<AccountAddress, u64>,
    runtime_to_storage: HashMap<AccountAddress, AccountAddress>,
    loaded_packages: HashSet<AccountAddress>,
}

#[derive(Clone)]
pub struct ObjectEntry {
    pub bytes: Vec<u8>,
    pub type_tag: TypeTag,
    pub version: u64,
}

#[derive(Default, Clone)]
pub struct ObjectCache {
    by_id_version: HashMap<(AccountAddress, u64), ObjectEntry>,
    by_id_latest: HashMap<AccountAddress, ObjectEntry>,
}

impl ObjectCache {
    pub fn insert(&mut self, id: AccountAddress, version: u64, entry: ObjectEntry) {
        self.by_id_version.insert((id, version), entry.clone());
        let replace = match self.by_id_latest.get(&id) {
            Some(existing) => version >= existing.version,
            None => true,
        };
        if replace {
            self.by_id_latest.insert(id, entry);
        }
    }

    pub fn get(&self, id: AccountAddress, version: u64) -> Option<&ObjectEntry> {
        self.by_id_version.get(&(id, version))
    }

    pub fn get_any(&self, id: AccountAddress) -> Option<&ObjectEntry> {
        self.by_id_latest.get(&id)
    }

    pub fn remove_all(&mut self, id: AccountAddress) {
        self.by_id_latest.remove(&id);
        self.by_id_version.retain(|(obj_id, _), _| obj_id != &id);
    }
}

#[derive(Default)]
pub struct ReplayStats {
    pub ptbs_seen: usize,
    pub ptbs_executed: usize,
    pub strict_matches: usize,
    pub non_parity: usize,
    pub skipped: usize,
    pub reason_counts: BTreeMap<ReasonCode, usize>,
    pub strict_match_digests: Vec<String>,
    pub strict_match_summaries: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct BatchPrefetch {
    pub tx_versions: HashMap<String, HashMap<String, u64>>,
    pub prefetched_objects: usize,
    pub txs_prefetched: usize,
    pub notes: Vec<String>,
}

impl ReplayStats {
    pub fn record(&mut self, reason: ReasonCode) {
        *self.reason_counts.entry(reason).or_insert(0) += 1;
    }
}

pub struct ReplayEngine<'a> {
    pub walrus: &'a WalrusClient,
    pub grpc: Arc<GrpcClient>,
    pub graphql: &'a GraphQLClient,
    pub rt: &'a tokio::runtime::Runtime,
    pub state_fetcher: Option<Arc<HistoricalStateProvider>>,
    pub packages: PackageCache,
    pub objects: Arc<RwLock<ObjectCache>>,
    pub dynamic_fields_cache: Arc<RwLock<HashMap<AccountAddress, Vec<DynamicFieldInfo>>>>,
    pub disk_object_store: Option<Arc<FsObjectStore>>,
    pub disk_package_store: Option<Arc<FsPackageStore>>,
    pub metrics: Arc<CacheMetrics>,
}

#[derive(Debug, Clone)]
struct StrictDiffError {
    code: ReasonCode,
    message: String,
}

impl StrictDiffError {
    fn new(code: ReasonCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StrictDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", format!("{:?}", self.code), self.message)
    }
}

impl std::error::Error for StrictDiffError {}

impl<'a> ReplayEngine<'a> {
    pub fn new(
        walrus: &'a WalrusClient,
        grpc: Arc<GrpcClient>,
        graphql: &'a GraphQLClient,
        rt: &'a tokio::runtime::Runtime,
        state_fetcher: Option<Arc<HistoricalStateProvider>>,
        disk_object_store: Option<FsObjectStore>,
        disk_package_store: Option<FsPackageStore>,
    ) -> Self {
        Self {
            walrus,
            grpc,
            graphql,
            rt,
            state_fetcher,
            packages: PackageCache::default(),
            objects: Arc::new(RwLock::new(ObjectCache::default())),
            dynamic_fields_cache: Arc::new(RwLock::new(HashMap::new())),
            disk_object_store: disk_object_store.map(Arc::new),
            disk_package_store: disk_package_store.map(Arc::new),
            metrics: Arc::new(CacheMetrics::default()),
        }
    }

    /// Ingest Walrus input/output objects into the run-wide versioned cache.
    pub fn ingest_tx_objects(&mut self, tx_json: &serde_json::Value) -> Result<()> {
        for key in ["input_objects", "output_objects"] {
            let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
                continue;
            };
            for obj_json in arr {
                let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
                    continue;
                };
                // Some Walrus objects may omit contents; skip and let gRPC/cache fill it later.
                let Some(contents_b64) = move_obj.get("contents").and_then(|c| c.as_str()) else {
                    continue;
                };
                let bcs_bytes = base64::engine::general_purpose::STANDARD
                    .decode(contents_b64)
                    .context("base64 decode Move.contents")?;

                if bcs_bytes.len() < 32 {
                    continue;
                }
                let id = AccountAddress::new({
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(&bcs_bytes[0..32]);
                    bytes
                });

                let version = move_obj
                    .get("version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                let type_json = move_obj
                    .get("type_")
                    .ok_or_else(|| anyhow!("missing Move.type_"))?;
                let type_tag = parse_type_tag_json(type_json)?;

                self.objects.write().insert(
                    id,
                    version,
                    ObjectEntry {
                        bytes: bcs_bytes.clone(),
                        type_tag: type_tag.clone(),
                        version,
                    },
                );

                // Also persist to disk cache if available (best-effort, ignore errors)
                if let Some(ref disk_store) = self.disk_object_store {
                    let meta = sui_historical_cache::ObjectMeta {
                        type_tag: format!("{}", type_tag),
                        owner_kind: None,
                        source_checkpoint: None,
                    };
                    let _ = disk_store.put(id, version, &bcs_bytes, &meta);
                }
            }
        }
        Ok(())
    }

    /// Pre-scan a batch of transactions to build per-tx version maps and prefetch objects.
    ///
    /// This uses gRPC transaction effects (unchanged_loaded_runtime_objects, changed_objects,
    /// unchanged_consensus_objects) to reconstruct the exact historical object versions needed
    /// for replay, without relying on GraphQL enumeration.
    pub fn pre_scan_batch(
        &mut self,
        txs: &[&serde_json::Value],
        max_ptbs: Option<usize>,
        verbose: bool,
    ) -> BatchPrefetch {
        let mut result = BatchPrefetch::default();

        // Collect PTB digests (respecting max limit).
        let mut digests: Vec<String> = Vec::new();
        for tx in txs {
            if let Some(limit) = max_ptbs {
                if result.txs_prefetched >= limit {
                    break;
                }
            }

            let is_ptb = tx
                .pointer("/transaction/data/0/intent_message/value/V1/kind/ProgrammableTransaction")
                .is_some();
            if !is_ptb {
                continue;
            }

            if let Some(digest) = on_chain_tx_digest(tx) {
                digests.push(digest);
                result.txs_prefetched += 1;
            }
        }

        if digests.is_empty() {
            return result;
        }

        // Fetch transactions via gRPC in batches.
        let mut tx_versions: HashMap<String, HashMap<String, u64>> = HashMap::new();
        for chunk in digests.chunks(100) {
            let chunk_refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
            let fetched = self
                .rt
                .block_on(async { self.grpc.batch_get_transactions(&chunk_refs).await });

            match fetched {
                Ok(list) => {
                    for tx_opt in list {
                        if let Some(tx) = tx_opt {
                            let versions = build_versions_from_grpc_tx(&tx);
                            if !versions.is_empty() {
                                tx_versions.insert(tx.digest.clone(), versions);
                            }
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("batch gRPC tx fetch failed: {e:#}");
                    if verbose {
                        eprintln!("{msg}");
                    }
                    result.notes.push(msg);
                }
            }
        }

        // Merge PTB input object versions from Walrus JSON when present.
        for tx in txs {
            let Some(digest) = on_chain_tx_digest(tx) else {
                continue;
            };
            let mut entry = tx_versions.remove(&digest).unwrap_or_default();
            for (id, ver) in extract_ptb_input_object_versions(tx) {
                entry.entry(id).or_insert(ver);
            }
            if !entry.is_empty() {
                tx_versions.insert(digest, entry);
            }
        }

        // Prefetch unique (object_id, version) pairs.
        let mut unique: HashSet<(String, u64)> = HashSet::new();
        for versions in tx_versions.values() {
            for (id, ver) in versions {
                unique.insert((normalize_addr(id), *ver));
            }
        }

        if !unique.is_empty() {
            let pairs: Vec<(String, u64)> = unique.into_iter().collect();
            let fetched = self
                .rt
                .block_on(async { self.grpc.batch_fetch_objects_at_versions(&pairs, 10).await });

            let mut inserted = 0usize;
            for (_id, obj_result) in fetched {
                let Ok(Some(obj)) = obj_result else { continue };
                let (Some(bcs), Some(type_str)) = (obj.bcs, obj.type_string) else {
                    continue;
                };
                let Ok(tt) = TypeTag::from_str(&type_str) else {
                    continue;
                };
                let Ok(id) = AccountAddress::from_hex_literal(&obj.object_id) else {
                    continue;
                };

                self.objects.write().insert(
                    id,
                    obj.version,
                    ObjectEntry {
                        bytes: bcs,
                        type_tag: tt,
                        version: obj.version,
                    },
                );
                inserted += 1;
            }
            result.prefetched_objects = inserted;
        }

        result.tx_versions = tx_versions;
        result
    }

    pub fn summarize_ptb_commands(&self, tx_json: &serde_json::Value) -> Option<String> {
        let cache = self.objects.read();
        let disk_cache_ref: Option<&dyn ObjectVersionStore> = self
            .disk_object_store
            .as_ref()
            .map(|s| s.as_ref() as &dyn ObjectVersionStore);
        let parsed = parse_ptb_transaction(
            self.walrus,
            tx_json,
            Some((self.grpc.as_ref(), self.rt)),
            Some(&cache),
            None,
            disk_cache_ref,
            None,
        )
        .ok()?;

        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for cmd in &parsed.commands {
            let name = match cmd {
                Command::MoveCall { .. } => "MoveCall",
                Command::TransferObjects { .. } => "TransferObjects",
                Command::SplitCoins { .. } => "SplitCoins",
                Command::MergeCoins { .. } => "MergeCoins",
                Command::Publish { .. } => "Publish",
                Command::MakeMoveVec { .. } => "MakeMoveVec",
                Command::Upgrade { .. } => "Upgrade",
                Command::Receive { .. } => "Receive",
            };
            *counts.entry(name).or_insert(0) += 1;
        }

        if counts.is_empty() {
            return Some("NoCommands".to_string());
        }

        let mut parts: Vec<String> = counts
            .into_iter()
            .map(|(name, count)| format!("{name}({count})"))
            .collect();
        parts.sort();
        Some(parts.join(", "))
    }

    pub fn replay_one_ptb_best_effort(
        &mut self,
        env: &mut SimulationEnvironment,
        checkpoint: u64,
        tx_json: &serde_json::Value,
        max_attempts: usize,
        verbose: bool,
    ) -> TxOutcome {
        self.replay_one_ptb_best_effort_with_prefetch(
            env,
            checkpoint,
            tx_json,
            max_attempts,
            verbose,
            None,
        )
    }

    pub fn replay_one_ptb_best_effort_with_prefetch(
        &mut self,
        env: &mut SimulationEnvironment,
        checkpoint: u64,
        tx_json: &serde_json::Value,
        max_attempts: usize,
        verbose: bool,
        prefetch_versions_override: Option<&HashMap<String, u64>>,
    ) -> TxOutcome {
        let digest = on_chain_tx_digest(tx_json).unwrap_or_else(|| "unknown".to_string());

        let mut outcome = TxOutcome {
            digest: digest.clone(),
            checkpoint,
            attempts: Vec::new(),
            final_parity: false,
            final_reason: ReasonCode::Unknown,
        };

        let (prefetch_aliases, prefetch_note, prefetch_versions) =
            if let Some(override_map) = prefetch_versions_override {
                let note = if override_map.is_empty() {
                    None
                } else {
                    Some(format!("batch pre-scan: versions={}", override_map.len()))
                };
                (None, note, Some(override_map.clone()))
            } else {
                (None, None, None)
            };
        let deny_children: Arc<RwLock<HashSet<AccountAddress>>> =
            Arc::new(RwLock::new(HashSet::new()));
        let deny_parents: Arc<RwLock<HashSet<AccountAddress>>> =
            Arc::new(RwLock::new(HashSet::new()));

        // Attempt 0: Walrus-first
        let attempt0 = self.try_execute_once(
            env,
            checkpoint,
            tx_json,
            AttemptKind::WalrusOnly,
            verbose,
            prefetch_aliases.as_ref(),
            prefetch_note.clone(),
            prefetch_versions.as_ref(),
            Arc::clone(&deny_children),
            Arc::clone(&deny_parents),
        );
        outcome.attempts.push(attempt0.clone());
        if attempt0.parity {
            outcome.final_parity = true;
            outcome.final_reason = ReasonCode::StrictMatch;
            return outcome;
        }

        if max_attempts <= 1 || !is_retryable(attempt0.reason) {
            outcome.final_parity = false;
            outcome.final_reason = attempt0.reason;
            return outcome;
        }

        // Attempt 1: Install child fetcher + dynamic field prefetch
        let attempt1 = self.try_execute_once(
            env,
            checkpoint,
            tx_json,
            AttemptKind::RetryWithChildFetcher,
            verbose,
            prefetch_aliases.as_ref(),
            None,
            prefetch_versions.as_ref(),
            Arc::clone(&deny_children),
            Arc::clone(&deny_parents),
        );
        outcome.attempts.push(attempt1.clone());
        if attempt1.parity {
            outcome.final_parity = true;
            outcome.final_reason = ReasonCode::StrictMatch;
            return outcome;
        }

        if max_attempts <= 2 || !is_retryable(attempt1.reason) {
            outcome.final_parity = false;
            outcome.final_reason = attempt1.reason;
            return outcome;
        }

        // Attempt 2: GraphQL prefetch escalation (best-effort)
        let attempt2 = self.try_execute_once(
            env,
            checkpoint,
            tx_json,
            AttemptKind::RetryWithPrefetch,
            verbose,
            prefetch_aliases.as_ref(),
            None,
            prefetch_versions.as_ref(),
            Arc::clone(&deny_children),
            Arc::clone(&deny_parents),
        );
        outcome.attempts.push(attempt2.clone());
        if attempt2.parity {
            outcome.final_parity = true;
            outcome.final_reason = ReasonCode::StrictMatch;
            return outcome;
        }

        if max_attempts <= 3 || !is_retryable(attempt2.reason) {
            outcome.final_parity = false;
            outcome.final_reason = attempt2.reason;
            return outcome;
        }

        // Attempt 3: MM2 predictive prefetch (best-effort)
        let attempt3 = self.try_execute_once(
            env,
            checkpoint,
            tx_json,
            AttemptKind::RetryWithMM2,
            verbose,
            prefetch_aliases.as_ref(),
            None,
            prefetch_versions.as_ref(),
            Arc::clone(&deny_children),
            Arc::clone(&deny_parents),
        );
        outcome.attempts.push(attempt3.clone());
        if attempt3.parity {
            outcome.final_parity = true;
            outcome.final_reason = ReasonCode::StrictMatch;
            return outcome;
        }

        outcome.final_parity = false;
        outcome.final_reason = attempt3.reason;
        outcome
    }

    fn try_execute_once(
        &mut self,
        env: &mut SimulationEnvironment,
        checkpoint: u64,
        tx_json: &serde_json::Value,
        attempt_kind: AttemptKind,
        verbose: bool,
        prefetch_aliases: Option<&HashMap<AccountAddress, AccountAddress>>,
        prefetch_note: Option<String>,
        prefetch_versions: Option<&HashMap<String, u64>>,
        deny_children: Arc<RwLock<HashSet<AccountAddress>>>,
        deny_parents: Arc<RwLock<HashSet<AccountAddress>>>,
    ) -> AttemptReport {
        let start = Instant::now();
        let mut notes = Vec::new();
        if let Some(note) = prefetch_note {
            notes.push(note);
        }

        // Parse PTB from Walrus JSON
        // Always allow gRPC fallback for missing object data (Walrus-first, gRPC when missing).
        let fallback = Some((self.grpc.as_ref(), self.rt));
        let parsed = {
            let cache = self.objects.read();
            let disk_cache_ref: Option<&dyn ObjectVersionStore> = self
                .disk_object_store
                .as_ref()
                .map(|s| s.as_ref() as &dyn ObjectVersionStore);
            parse_ptb_transaction(
                self.walrus,
                tx_json,
                fallback,
                Some(&cache),
                prefetch_versions,
                disk_cache_ref,
                Some(self.metrics.as_ref()),
            )
        };
        let parsed = match parsed {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("{e:#}");
                let reason = if msg.contains("unsupported") {
                    ReasonCode::UnsupportedCommand
                } else if msg.contains("missing object data") {
                    ReasonCode::MissingObject
                } else {
                    ReasonCode::ParseError
                };
                return AttemptReport {
                    kind: attempt_kind,
                    success: false,
                    parity: false,
                    reason,
                    duration: start.elapsed(),
                    notes: vec![format!("parse error: {e:#}")],
                };
            }
        };

        // Cache any input objects we already have (Walrus or gRPC fallback).
        self.cache_input_objects(&parsed.inputs);

        // Ensure packages are loaded
        for (pkg, ver) in build_package_version_map_from_effects(tx_json) {
            self.packages.versions_by_package.entry(pkg).or_insert(ver);
        }
        if let Err(e) = self.ensure_packages_loaded(
            env,
            checkpoint,
            &parsed.package_ids,
            &parsed.inputs,
            &parsed.commands,
        ) {
            return AttemptReport {
                kind: attempt_kind,
                success: false,
                parity: false,
                reason: ReasonCode::MissingPackage,
                duration: start.elapsed(),
                notes: vec![format!("package load failed: {e:#}")],
            };
        }

        // Configure env for strict replay
        env.reset_state().ok();
        env.set_sender(parsed.sender);
        if let Some(ts) = parsed.timestamp_ms {
            env.set_timestamp_ms(ts);
        }
        env.set_track_versions(true);
        let use_sui_natives = true;
        env.config_mut().use_sui_natives = use_sui_natives;
        let mut alias_map = prefetch_aliases.cloned();
        if alias_map.is_none() {
            let inferred = build_aliases_from_runtime_to_storage(&self.packages);
            if !inferred.is_empty() {
                alias_map = Some(inferred);
            }
        }
        if let Some(aliases) = alias_map {
            let resolver = env.resolver_mut();
            for (storage, original) in aliases.iter() {
                resolver.add_address_alias(storage.clone(), original.clone());
            }
            env.set_address_aliases_with_versions(
                aliases.clone(),
                self.packages.versions_by_package.clone(),
            );
        } else {
            env.clear_address_aliases();
        }

        // Set lamport clock so executor uses on-chain lamport_version.
        let lamport_version = on_chain_lamport_version(tx_json);
        if let Some(lamport_version) = lamport_version {
            let has_shared = parsed.inputs.iter().any(|iv| {
                matches!(
                    iv,
                    InputValue::Object(sui_sandbox_core::ptb::ObjectInput::Shared { .. })
                )
            });
            let adjust = if has_shared { 2 } else { 1 };
            env.set_lamport_clock(lamport_version.saturating_sub(adjust));
        }

        // Preload all PTB input objects into env (used by object runtime)
        if let Err(e) = preload_objects_from_inputs(env, &parsed.inputs) {
            return AttemptReport {
                kind: attempt_kind,
                success: false,
                parity: false,
                reason: ReasonCode::MissingObject,
                duration: start.elapsed(),
                notes: vec![format!("preload failed: {e:#}")],
            };
        }

        // Escalations
        let created_objects = Arc::new(created_objects_from_effects(tx_json));
        let output_objects = Arc::new(output_object_ids_from_walrus(tx_json));
        let mapping = Arc::new(build_object_version_map_for_tx(prefetch_versions, tx_json));
        match attempt_kind {
            AttemptKind::WalrusOnly => {
                env.clear_child_fetcher();
            }
            AttemptKind::RetryWithChildFetcher
            | AttemptKind::RetryWithPrefetch
            | AttemptKind::RetryWithMM2 => {
                if matches!(
                    attempt_kind,
                    AttemptKind::RetryWithPrefetch | AttemptKind::RetryWithMM2
                ) {
                    // MM2 predictive prefetch using gRPC transaction.
                    if let Some(digest) = on_chain_tx_digest(tx_json) {
                        let grpc_tx = self
                            .rt
                            .block_on(async { self.grpc.get_transaction(&digest).await })
                            .ok()
                            .flatten();
                        if let Some(tx) = grpc_tx {
                            let mut prefetcher = PredictivePrefetcher::new();
                            let mm2_config = PredictivePrefetchConfig::default();
                            let mm2_result = prefetcher.prefetch_for_transaction(
                                self.grpc.as_ref(),
                                None,
                                self.rt,
                                &tx,
                                &mm2_config,
                            );

                            let mut inserted = 0usize;
                            for obj in mm2_result
                                .base_result
                                .objects
                                .values()
                                .chain(mm2_result.base_result.supplemental_objects.values())
                            {
                                if let Ok(id) = AccountAddress::from_hex_literal(&obj.object_id) {
                                    if let Some(ty) = obj.type_string.as_ref() {
                                        if let Ok(tt) = TypeTag::from_str(ty) {
                                            self.objects.write().insert(
                                                id,
                                                obj.version,
                                                ObjectEntry {
                                                    bytes: obj.bcs.clone(),
                                                    type_tag: tt,
                                                    version: obj.version,
                                                },
                                            );
                                            inserted += 1;
                                        }
                                    }
                                }
                            }
                            notes.push(format!(
                                "mm2 prefetch: fetched_objects={}, predictions={}",
                                inserted, mm2_result.prediction_stats.predictions_made
                            ));
                        } else {
                            notes.push("mm2 prefetch: gRPC tx not found".to_string());
                        }
                    }
                }

                // Child fetcher uses cache, then disk cache, then gRPC archive for specific versions when available.
                let mapping = Arc::clone(&mapping);
                let objects = Arc::clone(&self.objects);
                let grpc = Arc::clone(&self.grpc);
                let rt_handle = self.rt.handle().clone();
                let deny_filter = Arc::clone(&deny_children);
                let deny_parent_filter = Arc::clone(&deny_parents);
                let disk_store = self.disk_object_store.clone();
                let metrics = Arc::clone(&self.metrics);
                env.set_versioned_child_fetcher(Box::new(move |parent, child_id| {
                    if deny_parent_filter.read().contains(&parent) {
                        return None;
                    }
                    if deny_filter.read().contains(&child_id) {
                        return None;
                    }
                    // Prefer exact (child_id, version) if known from mapping
                    if let Some(ver) = mapping.get(&child_id) {
                        // First check in-memory cache
                        if let Some(entry) = objects.read().get(child_id, *ver) {
                            metrics.record_memory_hit();
                            return Some((entry.type_tag.clone(), entry.bytes.clone(), *ver));
                        }
                        // Then check disk cache
                        if let Some(ref disk) = disk_store {
                            if let Ok(Some(cached_obj)) = disk.get(child_id, *ver) {
                                metrics.record_disk_hit();
                                let type_tag = match TypeTag::from_str(&cached_obj.meta.type_tag) {
                                    Ok(tt) => tt,
                                    Err(_) => return None,
                                };
                                // Insert into in-memory cache for next time
                                objects.write().insert(
                                    child_id,
                                    *ver,
                                    ObjectEntry {
                                        bytes: cached_obj.bcs_bytes.clone(),
                                        type_tag: type_tag.clone(),
                                        version: *ver,
                                    },
                                );
                                return Some((type_tag, cached_obj.bcs_bytes, *ver));
                            }
                        }
                        // Fall back to gRPC archive fetch at version
                        if use_sui_natives {
                            let id_hex = child_id.to_hex_literal();
                            metrics.record_grpc_fetch();
                            if let Ok(obj) = rt_handle.block_on(async {
                                grpc.get_object_at_version(&id_hex, Some(*ver)).await
                            }) {
                                if let Some(o) = obj {
                                    if let (Some(bcs), Some(type_str)) = (o.bcs, o.type_string) {
                                        if let Ok(tt) = TypeTag::from_str(&type_str) {
                                            objects.write().insert(
                                                child_id,
                                                *ver,
                                                ObjectEntry {
                                                    bytes: bcs.clone(),
                                                    type_tag: tt.clone(),
                                                    version: *ver,
                                                },
                                            );
                                            return Some((tt, bcs, *ver));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    None
                }));
            }
        }

        // Install key-based child fetcher for dynamic field lookup (all attempts).
        let objects = Arc::clone(&self.objects);
        let grpc = Arc::clone(&self.grpc);
        let rt_handle = self.rt.handle().clone();
        let created_filter = Arc::clone(&created_objects);
        let output_filter = Arc::clone(&output_objects);
        let deny_filter = Arc::clone(&deny_children);
        let deny_parent_filter = Arc::clone(&deny_parents);
        let mapping = Arc::clone(&mapping);
        let disk_store = self.disk_object_store.clone();
        let metrics = Arc::clone(&self.metrics);
        env.set_key_based_child_fetcher(Box::new(
            move |parent_id, _child_id, key_type, key_bytes| {
                if deny_parent_filter.read().contains(&parent_id) {
                    return None;
                }
                let key_type_bcs = bcs::to_bytes(key_type).ok()?;
                let child_hex = compute_dynamic_field_id(
                    &parent_id.to_hex_literal(),
                    key_bytes,
                    &key_type_bcs,
                )?;
                let child_id = AccountAddress::from_hex_literal(&child_hex).ok()?;
                if deny_filter.read().contains(&child_id) {
                    return None;
                }
                if output_filter.contains(&child_id) {
                    return None;
                }
                if created_filter.contains(&child_id) {
                    return None;
                }
                let ver = *mapping.get(&child_id)?;
                // First check in-memory cache
                if let Some(entry) = objects.read().get(child_id, ver) {
                    metrics.record_memory_hit();
                    return Some((entry.type_tag.clone(), entry.bytes.clone()));
                }
                // Then check disk cache
                if let Some(ref disk) = disk_store {
                    if let Ok(Some(cached_obj)) = disk.get(child_id, ver) {
                        metrics.record_disk_hit();
                        metrics.record_dynamic_field_disk_hit();
                        let type_tag = match TypeTag::from_str(&cached_obj.meta.type_tag) {
                            Ok(tt) => tt,
                            Err(_) => return None,
                        };
                        // Insert into in-memory cache for next time
                        objects.write().insert(
                            child_id,
                            ver,
                            ObjectEntry {
                                bytes: cached_obj.bcs_bytes.clone(),
                                type_tag: type_tag.clone(),
                                version: ver,
                            },
                        );
                        return Some((type_tag, cached_obj.bcs_bytes));
                    }
                }
                // Fall back to gRPC
                metrics.record_grpc_fetch();
                metrics.record_dynamic_field_grpc_fetch();
                if let Ok(obj) = rt_handle
                    .block_on(async { grpc.get_object_at_version(&child_hex, Some(ver)).await })
                {
                    if let Some(obj) = obj {
                        if let (Some(bcs), Some(type_str)) = (obj.bcs, obj.type_string) {
                            if let Ok(tt) = TypeTag::from_str(&type_str) {
                                objects.write().insert(
                                    child_id,
                                    ver,
                                    ObjectEntry {
                                        bytes: bcs.clone(),
                                        type_tag: tt.clone(),
                                        version: ver,
                                    },
                                );
                                return Some((tt, bcs));
                            }
                        }
                    }
                }
                None
            },
        ));

        // Execute
        let mut exec = env.execute_ptb_with_gas_budget(
            parsed.inputs.clone(),
            parsed.commands.clone(),
            parsed.gas_budget,
        );

        // Patch gas coin mutation into local effects so strict diff can include it.
        if let Some(effects) = exec.effects.as_mut() {
            if let Err(e) = patch_gas_coin_mutation(&parsed, tx_json, effects) {
                notes.push(format!("gas patch failed: {e:#}"));
            } else {
                notes.push("gas patch applied".to_string());
            }
        }

        if !exec.success {
            // If missing package, try to load it and retry once for this attempt.
            if let Some(sui_sandbox_core::simulation::SimulationError::MissingPackage {
                address,
                ..
            }) = &exec.error
            {
                if !matches!(attempt_kind, AttemptKind::WalrusOnly) {
                    if let Ok(pkg) = parse_address(address) {
                        if let Ok(()) =
                            self.load_package_if_needed(env, pkg, Some(checkpoint), true)
                        {
                            notes.push(format!(
                                "loaded missing package {} and retried",
                                pkg.to_hex_literal()
                            ));
                            exec = env.execute_ptb_with_gas_budget(
                                parsed.inputs.clone(),
                                parsed.commands.clone(),
                                parsed.gas_budget,
                            );
                            if let Some(effects) = exec.effects.as_mut() {
                                if let Err(e) = patch_gas_coin_mutation(&parsed, tx_json, effects) {
                                    notes.push(format!("gas patch failed: {e:#}"));
                                } else {
                                    notes.push("gas patch applied".to_string());
                                }
                            }
                            if exec.success {
                                // Continue to strict compare
                            } else {
                                if let Some(raw) = &exec.raw_error {
                                    if let Some((parent, child)) =
                                        parse_new_parent_child_conflict(raw)
                                    {
                                        deny_children.write().insert(child);
                                        deny_parents.write().insert(parent);
                                        self.objects.write().remove_all(child);
                                        notes.push(format!(
                                            "evicted child {} for parent {} after new-parent conflict",
                                            child.to_hex_literal(),
                                            parent.to_hex_literal()
                                        ));
                                    }
                                }
                                let reason = classify_execution_failure(&exec);
                                if verbose {
                                    if let Some(raw) = &exec.raw_error {
                                        notes.push(format!("exec error: {raw}"));
                                    }
                                }
                                return AttemptReport {
                                    kind: attempt_kind,
                                    success: false,
                                    parity: false,
                                    reason,
                                    duration: start.elapsed(),
                                    notes,
                                };
                            }
                        }
                    }
                }
            }

            if !exec.success {
                if let Some(raw) = &exec.raw_error {
                    if let Some((parent, child)) = parse_new_parent_child_conflict(raw) {
                        deny_children.write().insert(child);
                        deny_parents.write().insert(parent);
                        self.objects.write().remove_all(child);
                        notes.push(format!(
                            "evicted child {} for parent {} after new-parent conflict",
                            child.to_hex_literal(),
                            parent.to_hex_literal()
                        ));
                    }
                }
                let reason = classify_execution_failure(&exec);
                if verbose {
                    if let Some(raw) = &exec.raw_error {
                        notes.push(format!("exec error: {raw}"));
                    }
                }
                return AttemptReport {
                    kind: attempt_kind,
                    success: false,
                    parity: false,
                    reason,
                    duration: start.elapsed(),
                    notes,
                };
            }
        }

        let Some(local_effects) = exec.effects.as_ref() else {
            return AttemptReport {
                kind: attempt_kind,
                success: false,
                parity: false,
                reason: ReasonCode::ExecutionFailure,
                duration: start.elapsed(),
                notes: vec!["missing local effects".to_string()],
            };
        };

        // Strict compare
        match strict_compare(tx_json, local_effects) {
            Ok(()) => AttemptReport {
                kind: attempt_kind,
                success: true,
                parity: true,
                reason: ReasonCode::StrictMatch,
                duration: start.elapsed(),
                notes,
            },
            Err(e) => {
                notes.push(format!("diff: {e}"));
                AttemptReport {
                    kind: attempt_kind,
                    success: true,
                    parity: false,
                    reason: e.code,
                    duration: start.elapsed(),
                    notes,
                }
            }
        }
    }

    fn ensure_packages_loaded(
        &mut self,
        env: &mut SimulationEnvironment,
        checkpoint: u64,
        package_ids: &[AccountAddress],
        inputs: &[InputValue],
        commands: &[Command],
    ) -> Result<()> {
        let mut needed: HashSet<AccountAddress> = package_ids.iter().copied().collect();

        for input in inputs {
            if let InputValue::Object(obj) = input {
                if let Some(tt) = obj.type_tag() {
                    collect_type_packages(tt, &mut needed);
                }
            }
        }

        for cmd in commands {
            match cmd {
                Command::MoveCall { type_args, .. } => {
                    for ty in type_args {
                        collect_type_packages(ty, &mut needed);
                    }
                }
                Command::MakeMoveVec { type_tag, .. } => {
                    if let Some(ty) = type_tag {
                        collect_type_packages(ty, &mut needed);
                    }
                }
                _ => {}
            }
        }

        for pkg in needed {
            self.load_package_if_needed(env, pkg, Some(checkpoint), true)?;
        }
        Ok(())
    }
}

fn is_system_package(id: AccountAddress) -> bool {
    id == AccountAddress::from_hex_literal("0x1").unwrap()
        || id == AccountAddress::from_hex_literal("0x2").unwrap()
        || id == AccountAddress::from_hex_literal("0x3").unwrap()
}

fn parse_address(addr: &str) -> Result<AccountAddress> {
    let trimmed = addr.trim_start_matches("0x");
    let normalized = format!("0x{}", trimmed);
    AccountAddress::from_hex_literal(&normalized)
        .map_err(|e| anyhow!("invalid address {addr}: {e}"))
}

impl<'a> ReplayEngine<'a> {
    fn load_package_if_needed(
        &mut self,
        env: &mut SimulationEnvironment,
        pkg: AccountAddress,
        checkpoint: Option<u64>,
        use_checkpoint_fetch: bool,
    ) -> Result<()> {
        let storage_addr = self
            .packages
            .runtime_to_storage
            .get(&pkg)
            .copied()
            .unwrap_or(pkg);
        if is_system_package(storage_addr) || self.packages.loaded_packages.contains(&storage_addr)
        {
            return Ok(());
        }

        let mut storage_addr = storage_addr;
        if use_checkpoint_fetch {
            if let Some(cp) = checkpoint {
                if let Some((resolved_addr, modules, version)) =
                    self.try_fetch_package_at_checkpoint(pkg, cp)?
                {
                    storage_addr = resolved_addr;
                    self.packages.runtime_to_storage.insert(pkg, resolved_addr);
                    self.packages
                        .modules_by_package
                        .insert(resolved_addr, modules.clone());
                    self.packages
                        .versions_by_package
                        .insert(resolved_addr, version);

                    // Use gRPC metadata to populate linkage/aliases for dependency resolution.
                    let resolved_hex = resolved_addr.to_hex_literal();
                    if let Ok(Some(obj)) = self
                        .rt
                        .block_on(async { self.grpc.get_object(&resolved_hex).await })
                    {
                        if let Some(orig) = obj.package_original_id.as_ref() {
                            if let Ok(original_addr) = AccountAddress::from_hex_literal(orig) {
                                if original_addr != resolved_addr {
                                    self.packages
                                        .runtime_to_storage
                                        .insert(original_addr, resolved_addr);
                                }
                            }
                        }
                        if let Some(linkage) = obj.package_linkage.as_ref() {
                            for entry in linkage {
                                if let (Ok(orig), Ok(upgraded)) = (
                                    AccountAddress::from_hex_literal(&entry.original_id),
                                    AccountAddress::from_hex_literal(&entry.upgraded_id),
                                ) {
                                    if orig != upgraded {
                                        self.packages.runtime_to_storage.insert(orig, upgraded);
                                    }
                                    self.packages
                                        .versions_by_package
                                        .entry(upgraded)
                                        .or_insert(entry.upgraded_version);
                                }
                            }
                        }
                    }
                }
            }
        }

        let modules: Vec<(String, Vec<u8>)> = if let Some(m) =
            self.packages.modules_by_package.get(&storage_addr)
        {
            m.clone()
        } else if let Some(ref disk_pkg_store) = self.disk_package_store {
            // Try disk cache first
            if let Ok(Some(cached_pkg)) = disk_pkg_store.get(storage_addr) {
                self.metrics.record_package_disk_hit();
                let decoded = cached_pkg
                    .decode_modules()
                    .map_err(|e| anyhow!("Failed to decode cached package modules: {}", e))?;
                // Store in memory cache
                self.packages
                    .modules_by_package
                    .insert(storage_addr, decoded.clone());
                self.packages
                    .versions_by_package
                    .insert(storage_addr, cached_pkg.version);
                // Handle linkage/aliases if present
                if let Some(original_id) = cached_pkg.original_id {
                    if let Ok(orig_addr) = AccountAddress::from_hex_literal(&original_id) {
                        if orig_addr != storage_addr {
                            self.packages
                                .runtime_to_storage
                                .insert(orig_addr, storage_addr);
                        }
                    }
                }
                if let Some(linkage) = cached_pkg.linkage {
                    for entry in linkage {
                        if let (Ok(orig), Ok(upgraded)) = (
                            AccountAddress::from_hex_literal(&entry.original_id),
                            AccountAddress::from_hex_literal(&entry.upgraded_id),
                        ) {
                            if orig != upgraded {
                                self.packages.runtime_to_storage.insert(orig, upgraded);
                            }
                            self.packages
                                .versions_by_package
                                .entry(upgraded)
                                .or_insert(entry.upgraded_version);
                        }
                    }
                }
                decoded
            } else {
                // Not in disk cache, fetch from gRPC
                self.metrics.record_package_grpc_fetch();
                let pkg_hex = storage_addr.to_hex_literal();
                let obj = if let Some(ver) = self.packages.versions_by_package.get(&storage_addr) {
                    self.rt
                        .block_on(async {
                            self.grpc.get_object_at_version(&pkg_hex, Some(*ver)).await
                        })?
                        .ok_or_else(|| anyhow!("package not found: {}", pkg_hex))?
                } else {
                    self.rt
                        .block_on(async { self.grpc.get_object(&pkg_hex).await })?
                        .ok_or_else(|| anyhow!("package not found: {}", pkg_hex))?
                };

                if let Some(orig) = obj.package_original_id.as_ref() {
                    if let Ok(original_addr) = AccountAddress::from_hex_literal(orig) {
                        if original_addr != storage_addr {
                            self.packages
                                .runtime_to_storage
                                .insert(original_addr, storage_addr);
                        }
                    }
                }

                if let Some(linkage) = obj.package_linkage.as_ref() {
                    for entry in linkage {
                        if let (Ok(orig), Ok(upgraded)) = (
                            AccountAddress::from_hex_literal(&entry.original_id),
                            AccountAddress::from_hex_literal(&entry.upgraded_id),
                        ) {
                            if orig != upgraded {
                                self.packages.runtime_to_storage.insert(orig, upgraded);
                            }
                            self.packages
                                .versions_by_package
                                .entry(upgraded)
                                .or_insert(entry.upgraded_version);
                        }
                    }
                }

                let mods = obj
                    .package_modules
                    .ok_or_else(|| anyhow!("no package modules for {}", pkg_hex))?;
                let decoded = mods.clone();
                self.packages
                    .modules_by_package
                    .insert(storage_addr, decoded.clone());
                self.packages
                    .versions_by_package
                    .insert(storage_addr, obj.version);

                // Store in disk cache if available (best-effort, ignore errors)
                if let Some(ref disk_pkg_store) = self.disk_package_store {
                    let linkage_entries: Option<Vec<sui_historical_cache::LinkageEntry>> =
                        obj.package_linkage.as_ref().map(|linkage| {
                            linkage
                                .iter()
                                .map(|e| sui_historical_cache::LinkageEntry {
                                    original_id: e.original_id.clone(),
                                    upgraded_id: e.upgraded_id.clone(),
                                    upgraded_version: e.upgraded_version,
                                })
                                .collect()
                        });
                    let cached_pkg = sui_historical_cache::CachedPackage {
                        version: obj.version,
                        modules: decoded
                            .iter()
                            .map(|(name, bytes)| {
                                use base64::Engine;
                                (
                                    name.clone(),
                                    base64::engine::general_purpose::STANDARD.encode(bytes),
                                )
                            })
                            .collect(),
                        original_id: obj.package_original_id.clone(),
                        linkage: linkage_entries,
                    };
                    let _ = disk_pkg_store.put(storage_addr, &cached_pkg);
                }

                // Ensure linkage dependencies are loaded before deploying this package.
                if let Some(linkage) = obj.package_linkage.as_ref() {
                    for entry in linkage {
                        let upgraded_addr = parse_address(&entry.upgraded_id)?;
                        if upgraded_addr != storage_addr {
                            self.load_package_if_needed(env, upgraded_addr, checkpoint, false)?;
                        }
                    }
                }

                decoded
            }
        } else {
            // No disk cache configured; fetch from gRPC and keep only in-memory.
            self.metrics.record_package_grpc_fetch();
            let pkg_hex = storage_addr.to_hex_literal();
            let obj = if let Some(ver) = self.packages.versions_by_package.get(&storage_addr) {
                self.rt
                    .block_on(async {
                        self.grpc.get_object_at_version(&pkg_hex, Some(*ver)).await
                    })?
                    .ok_or_else(|| anyhow!("package not found: {}", pkg_hex))?
            } else {
                self.rt
                    .block_on(async { self.grpc.get_object(&pkg_hex).await })?
                    .ok_or_else(|| anyhow!("package not found: {}", pkg_hex))?
            };
            let mods = obj
                .package_modules
                .ok_or_else(|| anyhow!("no package modules for {}", pkg_hex))?;
            let decoded = mods.clone();
            self.packages
                .modules_by_package
                .insert(storage_addr, decoded.clone());
            self.packages
                .versions_by_package
                .insert(storage_addr, obj.version);
            decoded
        };

        // Also load dependencies discovered from bytecode (covers missing linkage entries).
        let mut deps: HashSet<AccountAddress> = HashSet::new();
        for (_name, bytes) in &modules {
            for dep in extract_dependencies_from_bytecode(bytes) {
                if let Ok(dep_addr) = AccountAddress::from_hex_literal(&dep) {
                    let storage_dep = self
                        .packages
                        .runtime_to_storage
                        .get(&dep_addr)
                        .copied()
                        .unwrap_or(dep_addr);
                    if storage_dep != storage_addr && !is_system_package(storage_dep) {
                        deps.insert(storage_dep);
                    }
                }
            }
        }
        for dep in deps {
            self.load_package_if_needed(env, dep, checkpoint, false)?;
        }

        env.deploy_package_at_address(&storage_addr.to_hex_literal(), modules)?;
        self.packages.loaded_packages.insert(storage_addr);
        Ok(())
    }

    fn try_fetch_package_at_checkpoint(
        &self,
        runtime_addr: AccountAddress,
        checkpoint: u64,
    ) -> Result<Option<(AccountAddress, Vec<(String, Vec<u8>)>, u64)>> {
        let runtime_hex = runtime_addr.to_hex_literal();
        let mut candidates: Vec<String> = match self.graphql.get_package_upgrades(&runtime_hex) {
            Ok(list) if !list.is_empty() => list.into_iter().map(|(addr, _)| addr).collect(),
            _ => vec![runtime_hex.clone()],
        };

        if !candidates.iter().any(|c| c == &runtime_hex) {
            candidates.push(runtime_hex.clone());
        }

        for addr in candidates.into_iter().rev() {
            let pkg = match self.graphql.fetch_package_at_checkpoint(&addr, checkpoint) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let storage_addr = match AccountAddress::from_hex_literal(&pkg.address) {
                Ok(a) => a,
                Err(_) => continue,
            };
            let mut decoded = Vec::new();
            for module in pkg.modules {
                if let Some(b64) = module.bytecode_base64.as_ref() {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                        decoded.push((module.name.clone(), bytes));
                    }
                }
            }
            if decoded.is_empty() {
                continue;
            }
            return Ok(Some((storage_addr, decoded, pkg.version)));
        }

        Ok(None)
    }

    fn cache_input_objects(&mut self, inputs: &[InputValue]) {
        for input in inputs {
            let InputValue::Object(obj) = input else {
                continue;
            };
            let Some(tt) = obj.type_tag().cloned() else {
                continue;
            };
            let Some(version) = obj.version() else {
                continue;
            };
            self.objects.write().insert(
                *obj.id(),
                version,
                ObjectEntry {
                    bytes: obj.bytes().to_vec(),
                    type_tag: tt,
                    version,
                },
            );
        }
    }

    fn ingest_replay_state(&mut self, state: &sui_state_fetcher::ReplayState) -> (usize, usize) {
        let mut obj_count = 0usize;
        let mut pkg_count = 0usize;

        for (id, obj) in &state.objects {
            if let Some(type_str) = obj.type_tag.as_ref() {
                if let Ok(tt) = TypeTag::from_str(type_str) {
                    self.objects.write().insert(
                        *id,
                        obj.version,
                        ObjectEntry {
                            bytes: obj.bcs_bytes.clone(),
                            type_tag: tt,
                            version: obj.version,
                        },
                    );
                    obj_count += 1;
                }
            }
        }

        for (addr, pkg) in &state.packages {
            self.packages
                .modules_by_package
                .insert(*addr, pkg.modules.clone());
            self.packages.versions_by_package.insert(*addr, pkg.version);
            let runtime_id = pkg.runtime_id();
            let replace = match self.packages.runtime_to_storage.get(&runtime_id) {
                Some(existing) => {
                    let existing_version = self
                        .packages
                        .versions_by_package
                        .get(existing)
                        .copied()
                        .unwrap_or(0);
                    pkg.version >= existing_version
                }
                None => true,
            };
            if replace {
                self.packages.runtime_to_storage.insert(runtime_id, *addr);
            }
            pkg_count += 1;
        }

        (obj_count, pkg_count)
    }

    fn prefetch_state_for_tx(
        &mut self,
        tx_json: &serde_json::Value,
    ) -> (
        Option<HashMap<AccountAddress, AccountAddress>>,
        Option<String>,
        Option<HashMap<String, u64>>,
    ) {
        let Some(fetcher) = self.state_fetcher.as_ref() else {
            return (None, None, None);
        };
        let Some(digest) = on_chain_tx_digest(tx_json) else {
            return (None, None, None);
        };

        let fetcher = Arc::clone(fetcher);
        match self.rt.block_on(async {
            fetcher
                .fetch_replay_state_with_config(&digest, false, 5, 500)
                .await
        }) {
            Ok(state) => {
                let (obj_count, pkg_count) = self.ingest_replay_state(&state);
                let aliases = build_address_aliases(&state);
                let versions = sui_state_fetcher::replay::get_historical_versions(&state);
                let note = format!(
                    "state_fetcher: objects={}, packages={}, aliases={}",
                    obj_count,
                    pkg_count,
                    aliases.len()
                );
                (Some(aliases), Some(note), Some(versions))
            }
            Err(e) => (None, Some(format!("state_fetcher failed: {e:#}")), None),
        }
    }
}

fn collect_type_packages(ty: &TypeTag, out: &mut HashSet<AccountAddress>) {
    match ty {
        TypeTag::Struct(s) => {
            out.insert(s.address);
            for t in &s.type_params {
                collect_type_packages(t, out);
            }
        }
        TypeTag::Vector(inner) => collect_type_packages(inner, out),
        _ => {}
    }
}

fn on_chain_tx_digest(tx_json: &serde_json::Value) -> Option<String> {
    tx_json
        .pointer("/effects/V2/transaction_digest")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn on_chain_lamport_version(tx_json: &serde_json::Value) -> Option<u64> {
    tx_json
        .pointer("/effects/V2/lamport_version")
        .and_then(|v| v.as_u64())
}

fn parse_type_tag_json(type_json: &serde_json::Value) -> Result<TypeTag> {
    // A subset of Walrus shapes used in events/output objects.
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
        .context("Missing address in type")?;
    let module = struct_json
        .get("module")
        .and_then(|m| m.as_str())
        .context("Missing module in type")?;
    let name = struct_json
        .get("name")
        .and_then(|n| n.as_str())
        .context("Missing name in type")?;
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

fn preload_objects_from_inputs(
    env: &mut SimulationEnvironment,
    inputs: &[InputValue],
) -> Result<()> {
    use sui_sandbox_core::ptb::ObjectInput;

    for input in inputs {
        let InputValue::Object(obj) = input else {
            continue;
        };
        let (id, bytes, type_tag, version, is_shared, is_immutable) = match obj {
            ObjectInput::ImmRef {
                id,
                bytes,
                type_tag,
                version,
            } => (*id, bytes.clone(), type_tag.clone(), *version, false, true),
            ObjectInput::MutRef {
                id,
                bytes,
                type_tag,
                version,
            } => (*id, bytes.clone(), type_tag.clone(), *version, false, false),
            ObjectInput::Owned {
                id,
                bytes,
                type_tag,
                version,
            } => (*id, bytes.clone(), type_tag.clone(), *version, false, false),
            ObjectInput::Shared {
                id,
                bytes,
                type_tag,
                version,
            } => (*id, bytes.clone(), type_tag.clone(), *version, true, false),
            ObjectInput::Receiving {
                id,
                bytes,
                type_tag,
                version,
                ..
            } => (*id, bytes.clone(), type_tag.clone(), *version, false, false),
        };
        env.add_object_with_version_and_status(
            id,
            bytes,
            type_tag.unwrap_or(TypeTag::Address),
            version.unwrap_or(1),
            is_shared,
            is_immutable,
        );
    }
    Ok(())
}

fn classify_execution_failure(exec: &sui_sandbox_core::simulation::ExecutionResult) -> ReasonCode {
    if let Some(raw) = &exec.raw_error {
        let lower = raw.to_lowercase();
        if lower.contains("dynamic field") || lower.contains("dynamic_field") {
            return ReasonCode::DynamicFieldMiss;
        }
        if lower.contains("timeout") || lower.contains("timed out") {
            return ReasonCode::Timeout;
        }
    }
    if let Some(err) = &exec.error {
        match err {
            sui_sandbox_core::simulation::SimulationError::MissingPackage { .. } => {
                return ReasonCode::MissingPackage;
            }
            sui_sandbox_core::simulation::SimulationError::MissingObject { .. } => {
                return ReasonCode::MissingObject;
            }
            _ => {}
        }
    }
    ReasonCode::ExecutionFailure
}

fn parse_new_parent_child_conflict(raw: &str) -> Option<(AccountAddress, AccountAddress)> {
    let parent_hex = extract_hex_after(raw, "parent ")?;
    let child_hex = extract_hex_after(raw, "child object ")?;
    let parent = AccountAddress::from_hex_literal(&parent_hex).ok()?;
    let child = AccountAddress::from_hex_literal(&child_hex).ok()?;
    Some((parent, child))
}

fn extract_hex_after(raw: &str, marker: &str) -> Option<String> {
    let start = raw.find(marker)? + marker.len();
    let rest = &raw[start..];
    let mut end = rest.len();
    for (idx, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            end = idx;
            break;
        }
    }
    let token = rest[..end]
        .trim_end_matches(|c: char| !c.is_ascii_hexdigit() && c != 'x')
        .trim_end_matches(|c: char| c == '.' || c == ',' || c == ')');
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn is_retryable(reason: ReasonCode) -> bool {
    matches!(
        reason,
        ReasonCode::MissingPackage
            | ReasonCode::MissingObject
            | ReasonCode::DynamicFieldMiss
            | ReasonCode::Timeout
            | ReasonCode::ExecutionFailure
    )
}

fn strict_compare(
    tx_json: &serde_json::Value,
    local: &sui_sandbox_core::ptb::TransactionEffects,
) -> std::result::Result<(), StrictDiffError> {
    // Status
    let status = tx_json.pointer("/effects/V2/status").ok_or_else(|| {
        StrictDiffError::new(ReasonCode::WalrusInconsistent, "missing effects.V2.status")
    })?;
    let on_chain_success = match status {
        serde_json::Value::String(s) => s == "Success",
        serde_json::Value::Object(o) => o.contains_key("Success"),
        _ => false,
    };
    if on_chain_success != local.success {
        return Err(StrictDiffError::new(
            ReasonCode::StatusMismatch,
            format!(
                "status mismatch: on-chain={}, local={}",
                on_chain_success, local.success
            ),
        ));
    }

    // Lamport version
    let lamport = on_chain_lamport_version(tx_json).ok_or_else(|| {
        StrictDiffError::new(ReasonCode::WalrusInconsistent, "missing lamport_version")
    })?;
    let local_lamport = local.lamport_timestamp.ok_or_else(|| {
        StrictDiffError::new(
            ReasonCode::NotModeled,
            "local missing lamport_timestamp (enable version tracking)",
        )
    })?;
    if lamport != local_lamport {
        return Err(StrictDiffError::new(
            ReasonCode::LamportMismatch,
            format!(
                "lamport_version mismatch: on-chain={}, local={}",
                lamport, local_lamport
            ),
        ));
    }

    // Object changes
    let expected = parse_changed_objects(tx_json).map_err(|e| {
        StrictDiffError::new(
            ReasonCode::WalrusInconsistent,
            format!("parse changed_objects: {e:#}"),
        )
    })?;
    let output_objects = index_walrus_move_objects(tx_json, "output_objects").map_err(|e| {
        StrictDiffError::new(
            ReasonCode::WalrusInconsistent,
            format!("parse output_objects: {e:#}"),
        )
    })?;

    // Ensure Walrus output_objects digests are self-consistent with effects digests (when available).
    for (id, exp) in &expected {
        if matches!(
            exp.change_type,
            VersionChangeType::Deleted | VersionChangeType::Wrapped
        ) {
            continue;
        }
        let Some(out_obj) = output_objects.get(id) else {
            return Err(StrictDiffError::new(
                ReasonCode::WalrusInconsistent,
                format!("missing output_object for {}", id.to_hex_literal()),
            ));
        };
        let computed = compute_sui_object_digest_from_walrus(out_obj).map_err(|e| {
            StrictDiffError::new(
                ReasonCode::WalrusInconsistent,
                format!("compute output digest: {e:#}"),
            )
        })?;
        if exp.output_digest != computed.to_string() {
            return Err(StrictDiffError::new(
                ReasonCode::WalrusInconsistent,
                format!(
                    "output digest mismatch in Walrus data for {}: effects={}, computed={}",
                    id.to_hex_literal(),
                    exp.output_digest,
                    computed
                ),
            ));
        }
    }

    let local_versions = local.object_versions.as_ref().ok_or_else(|| {
        StrictDiffError::new(
            ReasonCode::NotModeled,
            "local missing object_versions (enable version tracking)",
        )
    })?;

    for (id, exp) in &expected {
        let Some(info) = local_versions.get(id) else {
            return Err(StrictDiffError::new(
                ReasonCode::ObjectMismatch,
                format!("missing local version info for {}", id.to_hex_literal()),
            ));
        };
        if info.output_version != lamport {
            return Err(StrictDiffError::new(
                ReasonCode::ObjectMismatch,
                format!(
                    "output version mismatch for {}: expected {}, got {}",
                    id.to_hex_literal(),
                    lamport,
                    info.output_version
                ),
            ));
        }
        if let Some(exp_in) = exp.input_version {
            if info.input_version != Some(exp_in) {
                return Err(StrictDiffError::new(
                    ReasonCode::ObjectMismatch,
                    format!(
                        "input version mismatch for {}: expected {:?}, got {:?}",
                        id.to_hex_literal(),
                        Some(exp_in),
                        info.input_version
                    ),
                ));
            }
        }
        if exp.change_type != info.change_type {
            return Err(StrictDiffError::new(
                ReasonCode::ObjectMismatch,
                format!(
                    "change type mismatch for {}: expected {:?}, got {:?}",
                    id.to_hex_literal(),
                    exp.change_type,
                    info.change_type
                ),
            ));
        }

        // For writes, compare exact output Move.contents bytes (and let digest equality fall out).
        match exp.change_type {
            VersionChangeType::Created | VersionChangeType::Mutated | VersionChangeType::Unwrapped => {
                let expected_bytes = output_objects
                    .get(id)
                    .ok_or_else(|| {
                        StrictDiffError::new(
                            ReasonCode::WalrusInconsistent,
                            format!("missing output_object for {}", id.to_hex_literal()),
                        )
                    })?
                    .contents
                    .as_slice();

                let got = local
                    .mutated_object_bytes
                    .get(id)
                    .or_else(|| local.created_object_bytes.get(id))
                    .ok_or_else(|| {
                        StrictDiffError::new(
                            ReasonCode::ObjectMismatch,
                            format!(
                                "missing local output bytes for {} (mutated/created)",
                                id.to_hex_literal()
                            ),
                        )
                    })?;

                if got.as_slice() != expected_bytes {
                    // Special-case gas objects for more actionable reporting.
                    let is_gas = is_likely_gas_object(tx_json, *id);
                    return Err(StrictDiffError::new(
                        if is_gas {
                            ReasonCode::GasMismatch
                        } else {
                            ReasonCode::ObjectMismatch
                        },
                        format!("output bytes mismatch for {}", id.to_hex_literal()),
                    ));
                }
            }
            VersionChangeType::Deleted => {
                if !local.deleted.contains(id) {
                    return Err(StrictDiffError::new(
                        ReasonCode::ObjectMismatch,
                        format!(
                            "expected deletion for {}, but local.deleted does not contain it",
                            id.to_hex_literal()
                        ),
                    ));
                }
            }
            VersionChangeType::Wrapped => {
                if !local.wrapped.contains(id) {
                    return Err(StrictDiffError::new(
                        ReasonCode::ObjectMismatch,
                        format!(
                            "expected wrapped for {}, but local.wrapped does not contain it",
                            id.to_hex_literal()
                        ),
                    ));
                }
            }
        }
    }

    // Ensure no extra local version entries.
    if local_versions.len() != expected.len() {
        return Err(StrictDiffError::new(
            ReasonCode::ObjectMismatch,
            format!(
                "object_versions count mismatch: expected {}, got {}",
                expected.len(),
                local_versions.len()
            ),
        ));
    }

    Ok(())
}

#[derive(Clone)]
struct ExpectedChange {
    input_version: Option<u64>,
    output_digest: String,
    change_type: VersionChangeType,
}

fn parse_changed_objects(tx_json: &serde_json::Value) -> Result<HashMap<ObjectID, ExpectedChange>> {
    let lamport = on_chain_lamport_version(tx_json).context("missing lamport_version")?;
    let changed = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing effects.V2.changed_objects"))?;

    let mut out = HashMap::new();
    for entry in changed {
        let arr = entry
            .as_array()
            .ok_or_else(|| anyhow!("changed_objects entry not array"))?;
        let id_str = arr
            .get(0)
            .and_then(|v| v.as_str())
            .context("changed_objects[0] id")?;
        let id = AccountAddress::from_hex_literal(id_str).context("parse object id")?;

        let meta = arr.get(1).context("changed_objects[1] meta")?;

        let (input_version, input_exists) = if let Some(exist) = meta.pointer("/input_state/Exist")
        {
            let exist_arr = exist.as_array().context("input_state.Exist is not array")?;
            let ver = exist_arr
                .get(0)
                .and_then(|v| v.as_array())
                .and_then(|v| v.get(0))
                .and_then(|v| v.as_u64())
                .context("input_state.Exist version")?;
            (Some(ver), true)
        } else {
            (None, false)
        };

        let output_state = meta
            .get("output_state")
            .ok_or_else(|| anyhow!("missing output_state"))?;
        let mut output_digest = String::new();
        let mut output_exists = false;
        let mut output_wrapped = false;
        let mut output_deleted = false;

        if let Some(ow) = output_state.get("ObjectWrite") {
            let ow_arr = ow
                .as_array()
                .context("output_state.ObjectWrite is not array")?;
            let dig = ow_arr
                .get(0)
                .and_then(|v| v.as_str())
                .context("output_state.ObjectWrite digest")?;
            output_digest = dig.to_string();
            output_exists = true;
        } else if let Some(pw) = output_state.get("PackageWrite") {
            let pw_arr = pw
                .as_array()
                .context("output_state.PackageWrite is not array")?;
            let dig = pw_arr
                .get(0)
                .and_then(|v| v.as_str())
                .context("output_state.PackageWrite digest")?;
            output_digest = dig.to_string();
            output_exists = true;
        } else if output_state.get("ObjectWrap").is_some() {
            output_wrapped = true;
        } else if output_state.get("ObjectDelete").is_some()
            || output_state.get("ObjectDeleteShared").is_some()
        {
            output_deleted = true;
        }

        let change_type = if output_wrapped {
            VersionChangeType::Wrapped
        } else if output_exists {
            if input_exists {
                VersionChangeType::Mutated
            } else {
                VersionChangeType::Created
            }
        } else if output_deleted {
            VersionChangeType::Deleted
        } else {
            // Default to Deleted when output state is absent or unrecognized.
            VersionChangeType::Deleted
        };

        // For writes, output version is lamport. We don't store it separately because local compares to lamport.
        let _ = lamport;

        out.insert(
            id,
            ExpectedChange {
                input_version,
                output_digest,
                change_type,
            },
        );
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct WalrusMoveObject {
    id: AccountAddress,
    version: u64,
    type_tag: TypeTag,
    has_public_transfer: bool,
    contents: Vec<u8>,
    owner: Owner,
    previous_transaction: TransactionDigest,
    storage_rebate: u64,
}

fn index_walrus_move_objects(
    tx_json: &serde_json::Value,
    key: &str,
) -> Result<HashMap<AccountAddress, WalrusMoveObject>> {
    let mut out = HashMap::new();
    let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
        return Ok(out);
    };

    for obj_json in arr {
        let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
            continue;
        };

        let contents =
            decode_bytes_or_base64(move_obj.get("contents")).context("missing Move.contents")?;
        if contents.len() < 32 {
            continue;
        }

        let id = AccountAddress::new({
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&contents[0..32]);
            bytes
        });
        let version = move_obj
            .get("version")
            .and_then(|v| v.as_u64())
            .context("missing Move.version")?;

        let type_json = move_obj.get("type_").context("missing Move.type_")?;
        let type_tag = parse_type_tag_json(type_json)?;

        let has_public_transfer = move_obj
            .get("has_public_transfer")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let owner_json = obj_json.get("owner").unwrap_or(&serde_json::Value::Null);
        let owner = parse_owner(owner_json)?;

        let prev = obj_json
            .get("previous_transaction")
            .and_then(|v| v.as_str())
            .unwrap_or("11111111111111111111111111111111");
        let previous_transaction = TransactionDigest::from_str(prev)
            .map_err(|e| anyhow!("parse previous_transaction {prev}: {e}"))?;

        let storage_rebate = match obj_json.get("storage_rebate") {
            Some(serde_json::Value::String(s)) => s.parse::<u64>().unwrap_or(0),
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
            _ => 0,
        };

        out.insert(
            id,
            WalrusMoveObject {
                id,
                version,
                type_tag,
                has_public_transfer,
                contents,
                owner,
                previous_transaction,
                storage_rebate,
            },
        );
    }
    Ok(out)
}

fn decode_bytes_or_base64(v: Option<&serde_json::Value>) -> Option<Vec<u8>> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return base64::engine::general_purpose::STANDARD.decode(s).ok();
    }
    if let Some(arr) = v.as_array() {
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

fn parse_owner(owner_json: &serde_json::Value) -> Result<Owner> {
    if owner_json.get("Immutable").is_some() {
        return Ok(Owner::Immutable);
    }
    if let Some(addr) = owner_json.get("AddressOwner").and_then(|v| v.as_str()) {
        let a =
            SuiAddress::from_str(addr).map_err(|e| anyhow!("parse AddressOwner {addr}: {e}"))?;
        return Ok(Owner::AddressOwner(a));
    }
    if let Some(obj) = owner_json.get("ObjectOwner").and_then(|v| v.as_str()) {
        let id = SuiObjectID::from_hex_literal(obj)
            .map_err(|e| anyhow!("parse ObjectOwner {obj}: {e}"))?;
        return Ok(Owner::ObjectOwner(id.into()));
    }
    if let Some(shared) = owner_json.get("Shared") {
        let ver = shared
            .get("initial_shared_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(1);
        return Ok(Owner::Shared {
            initial_shared_version: SequenceNumber::from_u64(ver),
        });
    }
    if let Some(cao) = owner_json.get("ConsensusAddressOwner") {
        let start_version = cao
            .get("start_version")
            .and_then(|v| v.as_u64())
            .context("ConsensusAddressOwner.start_version")?;
        let owner = cao
            .get("owner")
            .and_then(|v| v.as_str())
            .context("ConsensusAddressOwner.owner")?;
        let a = SuiAddress::from_str(owner)
            .map_err(|e| anyhow!("parse ConsensusAddressOwner.owner {owner}: {e}"))?;
        return Ok(Owner::ConsensusAddressOwner {
            start_version: SequenceNumber::from_u64(start_version),
            owner: a,
        });
    }
    Err(anyhow!("unsupported owner format: {owner_json}"))
}

fn compute_sui_object_digest_from_walrus(obj: &WalrusMoveObject) -> Result<ObjectDigest> {
    let protocol = ProtocolConfig::get_for_max_version_UNSAFE();

    let struct_tag = match &obj.type_tag {
        TypeTag::Struct(s) => (**s).clone(),
        other => {
            return Err(anyhow!(
                "expected Struct type for object {}, got {other:?}",
                obj.id.to_hex_literal()
            ))
        }
    };
    let move_ty = MoveObjectType::from(struct_tag);
    let move_obj = unsafe {
        MoveObject::new_from_execution(
            move_ty,
            obj.has_public_transfer,
            SequenceNumber::from_u64(obj.version),
            obj.contents.clone(),
            &protocol,
            false,
        )
    }
    .map_err(|e| anyhow!("MoveObject::new_from_execution failed: {e}"))?;

    let inner = ObjectInner {
        data: Data::Move(move_obj),
        owner: obj.owner.clone(),
        previous_transaction: obj.previous_transaction,
        storage_rebate: obj.storage_rebate,
    };
    let object: Object = inner.into();
    Ok(object.digest())
}

fn is_likely_gas_object(tx_json: &serde_json::Value, id: AccountAddress) -> bool {
    let Some(payments) = tx_json
        .pointer("/transaction/data/0/intent_message/value/V1/gas_data/payment")
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    for p in payments {
        if let Some(arr) = p.as_array() {
            if let Some(id_str) = arr.get(0).and_then(|v| v.as_str()) {
                if let Ok(pid) = AccountAddress::from_hex_literal(id_str) {
                    if pid == id {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn build_object_version_map_from_effects(
    tx_json: &serde_json::Value,
) -> HashMap<AccountAddress, u64> {
    // Best-effort map from changed_objects input_state versions (only objects in effects).
    let mut m = HashMap::new();
    if let Some(changed) = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
    {
        for entry in changed {
            let Some(arr) = entry.as_array() else {
                continue;
            };
            let Some(id_str) = arr.get(0).and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(id) = AccountAddress::from_hex_literal(id_str) else {
                continue;
            };
            let meta = arr.get(1).unwrap_or(&serde_json::Value::Null);
            if let Some(exist) = meta.pointer("/input_state/Exist") {
                if let Some(ver) = exist
                    .as_array()
                    .and_then(|v| v.get(0))
                    .and_then(|v| v.as_array())
                    .and_then(|v| v.get(0))
                    .and_then(|v| v.as_u64())
                {
                    m.insert(id, ver);
                }
            }
        }
    }
    m
}

fn build_object_version_map_for_tx(
    prefetch_versions: Option<&HashMap<String, u64>>,
    tx_json: &serde_json::Value,
) -> HashMap<AccountAddress, u64> {
    let mut m: HashMap<AccountAddress, u64> = HashMap::new();
    if let Some(prefetch) = prefetch_versions {
        for (id_str, ver) in prefetch {
            let normalized = normalize_addr(id_str);
            if let Ok(id) = AccountAddress::from_hex_literal(&normalized) {
                m.insert(id, *ver);
            }
        }
    }
    for (id, ver) in build_object_version_map_from_effects(tx_json) {
        m.entry(id).or_insert(ver);
    }
    m
}

fn created_objects_from_effects(tx_json: &serde_json::Value) -> HashSet<AccountAddress> {
    let mut created = HashSet::new();
    if let Some(changed) = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
    {
        for entry in changed {
            let Some(arr) = entry.as_array() else {
                continue;
            };
            let Some(id_str) = arr.get(0).and_then(|v| v.as_str()) else {
                continue;
            };
            let meta = arr.get(1).unwrap_or(&serde_json::Value::Null);
            if meta.pointer("/input_state/NotExist").is_some() {
                if let Ok(id) = AccountAddress::from_hex_literal(id_str) {
                    created.insert(id);
                }
            }
        }
    }
    created
}

fn output_object_ids_from_walrus(tx_json: &serde_json::Value) -> HashSet<AccountAddress> {
    let mut output = HashSet::new();
    let Some(arr) = tx_json.get("output_objects").and_then(|v| v.as_array()) else {
        return output;
    };

    for obj_json in arr {
        let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
            continue;
        };
        let Some(contents_b64) = move_obj.get("contents").and_then(|c| c.as_str()) else {
            continue;
        };
        let Ok(bcs_bytes) = base64::engine::general_purpose::STANDARD.decode(contents_b64) else {
            continue;
        };
        if bcs_bytes.len() < 32 {
            continue;
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&bcs_bytes[0..32]);
        output.insert(AccountAddress::new(bytes));
    }
    output
}

fn build_package_version_map_from_effects(
    tx_json: &serde_json::Value,
) -> HashMap<AccountAddress, u64> {
    let mut m = HashMap::new();
    let Some(changed) = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
    else {
        return m;
    };

    for entry in changed {
        let Some(arr) = entry.as_array() else {
            continue;
        };
        let Some(id_str) = arr.get(0).and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(id) = AccountAddress::from_hex_literal(id_str) else {
            continue;
        };
        let meta = arr.get(1).unwrap_or(&serde_json::Value::Null);

        if let Some(pkg) = meta.pointer("/output_state/PackageWrite") {
            if let Some(ver) = pkg
                .as_array()
                .and_then(|v| v.get(0))
                .and_then(|v| v.as_u64())
            {
                m.insert(id, ver);
            }
        }
    }
    m
}

fn build_historical_versions_for_prefetch(
    tx_json: &serde_json::Value,
    extra: Option<&HashMap<String, u64>>,
    inputs: &[InputValue],
) -> Option<HashMap<String, u64>> {
    let mut m: HashMap<String, u64> = HashMap::new();
    let mut created: HashSet<String> = HashSet::new();

    if let Some(changed) = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
    {
        for entry in changed {
            let arr = entry.as_array()?;
            let id_str = arr.get(0)?.as_str()?;
            let meta = arr.get(1)?;
            if meta.pointer("/input_state/NotExist").is_some() {
                created.insert(normalize_addr(id_str));
            }
        }
    }

    if let Some(extra) = extra {
        for (id, ver) in extra {
            let normalized = normalize_addr(id);
            if created.contains(&normalized) {
                continue;
            }
            m.entry(normalized).or_insert(*ver);
        }
    }

    for input in inputs {
        let InputValue::Object(obj) = input else {
            continue;
        };
        let Some(ver) = obj.version() else {
            continue;
        };
        let normalized = normalize_addr(&obj.id().to_hex_literal());
        if created.contains(&normalized) {
            continue;
        }
        m.entry(normalized).or_insert(ver);
    }

    if let Some(changed) = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
    {
        for entry in changed {
            let arr = entry.as_array()?;
            let id_str = arr.get(0)?.as_str()?;
            let meta = arr.get(1)?;
            if let Some(exist) = meta.pointer("/input_state/Exist") {
                let ver = exist
                    .as_array()
                    .and_then(|v| v.get(0))
                    .and_then(|v| v.as_array())
                    .and_then(|v| v.get(0))
                    .and_then(|v| v.as_u64())?;
                let normalized = normalize_addr(id_str);
                if created.contains(&normalized) {
                    continue;
                }
                m.entry(normalized).or_insert(ver);
            }
        }
    }
    if m.is_empty() {
        None
    } else {
        Some(m)
    }
}

fn normalize_addr(addr: &str) -> String {
    let hex = addr.strip_prefix("0x").unwrap_or(addr);
    format!("0x{}", hex.to_lowercase())
}

fn extract_ptb_input_object_versions(tx_json: &serde_json::Value) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let inputs = tx_json
        .pointer("/transaction/data/0/intent_message/value/V1/kind/ProgrammableTransaction/inputs")
        .and_then(|v| v.as_array());
    let Some(inputs) = inputs else { return out };

    for inp in inputs {
        let Some(obj) = inp.get("Object") else {
            continue;
        };

        if let Some(shared) = obj.get("SharedObject") {
            let id = shared.get("id").and_then(|v| v.as_str());
            let ver = shared.get("initial_shared_version").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| v.as_u64())
            });
            if let (Some(id), Some(ver)) = (id, ver) {
                out.push((normalize_addr(id), ver));
            }
            continue;
        }

        if let Some(arr) = obj.get("ImmOrOwnedObject").and_then(|v| v.as_array()) {
            let id = arr.get(0).and_then(|v| v.as_str());
            let ver = arr.get(1).and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| v.as_u64())
            });
            if let (Some(id), Some(ver)) = (id, ver) {
                out.push((normalize_addr(id), ver));
            }
            continue;
        }

        if let Some(arr) = obj.get("Receiving").and_then(|v| v.as_array()) {
            let id = arr.get(0).and_then(|v| v.as_str());
            let ver = arr.get(1).and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| v.as_u64())
            });
            if let (Some(id), Some(ver)) = (id, ver) {
                out.push((normalize_addr(id), ver));
            }
            continue;
        }
    }

    out
}

fn build_aliases_from_runtime_to_storage(
    packages: &PackageCache,
) -> HashMap<AccountAddress, AccountAddress> {
    let mut aliases = HashMap::new();
    for (runtime, storage) in &packages.runtime_to_storage {
        if runtime != storage {
            aliases.insert(*storage, *runtime);
        }
    }
    aliases
}

fn build_versions_from_grpc_tx(tx: &sui_transport::grpc::GrpcTransaction) -> HashMap<String, u64> {
    let mut m = HashMap::new();

    for input in &tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            }
            | GrpcInput::Receiving {
                object_id, version, ..
            } => {
                m.insert(normalize_addr(object_id), *version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                m.entry(normalize_addr(object_id))
                    .or_insert(*initial_version);
            }
            GrpcInput::Pure { .. } => {}
        }
    }

    for (id, ver) in &tx.unchanged_loaded_runtime_objects {
        m.insert(normalize_addr(id), *ver);
    }
    for (id, ver) in &tx.changed_objects {
        m.insert(normalize_addr(id), *ver);
    }
    for (id, ver) in &tx.unchanged_consensus_objects {
        m.insert(normalize_addr(id), *ver);
    }

    m
}

fn patch_gas_coin_mutation(
    parsed: &ParsedWalrusPtb,
    tx_json: &serde_json::Value,
    effects: &mut sui_sandbox_core::ptb::TransactionEffects,
) -> Result<()> {
    // Determine gas coin id and version from the on-chain gas payment refs.
    let (gas_id, gas_ver) = parse_gas_object_ref(tx_json)?;
    // Locate gas coin bytes from PTB inputs.
    let (input_bytes, input_ver) = find_object_input_by_id(&parsed.inputs, gas_id)
        .ok_or_else(|| anyhow!("gas coin not found in PTB inputs"))?;
    let gas_ver = if gas_ver == 0 {
        input_ver.unwrap_or(0)
    } else {
        gas_ver
    };

    let gas_used = tx_json
        .pointer("/effects/V2/gas_used")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("missing effects.V2.gas_used"))?;
    let comp: u64 = gas_used
        .get("computationCost")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let stor: u64 = gas_used
        .get("storageCost")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let rebate: u64 = gas_used
        .get("storageRebate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total = comp.saturating_add(stor).saturating_sub(rebate);

    // If the PTB already mutated the gas coin, use that as the base before applying gas cost.
    let base_bytes = effects
        .mutated_object_bytes
        .get(&gas_id)
        .cloned()
        .unwrap_or_else(|| input_bytes.clone());
    let mut new_bytes = base_bytes.clone();
    if new_bytes.len() < 40 {
        return Err(anyhow!("gas coin bytes too short to patch balance"));
    }
    let bal = u64::from_le_bytes(new_bytes[32..40].try_into().unwrap());
    let new_bal = bal.saturating_sub(total);
    new_bytes[32..40].copy_from_slice(&new_bal.to_le_bytes());

    // If already mutated by the PTB, keep existing and just ensure digest/bytes match.
    effects.mutated.push(gas_id);
    effects
        .mutated_object_bytes
        .insert(gas_id, new_bytes.clone());

    // Update version tracking entry.
    let lamport = on_chain_lamport_version(tx_json).context("missing lamport_version")?;
    let input_digest = blake2b256(&input_bytes);
    let output_digest = blake2b256(&new_bytes);

    let versions = effects.object_versions.get_or_insert_with(HashMap::new);
    versions.insert(
        gas_id,
        ObjectVersionInfo {
            input_version: Some(gas_ver),
            output_version: lamport,
            input_digest: Some(input_digest),
            output_digest,
            change_type: VersionChangeType::Mutated,
        },
    );
    effects.lamport_timestamp = Some(lamport);
    Ok(())
}

fn blake2b256(bytes: &[u8]) -> [u8; 32] {
    use fastcrypto::hash::{Blake2b256, HashFunction};
    Blake2b256::digest(bytes).into()
}

fn parse_gas_object_ref(tx_json: &serde_json::Value) -> Result<(AccountAddress, u64)> {
    let payments = tx_json
        .pointer("/transaction/data/0/intent_message/value/V1/gas_data/payment")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing gas_data.payment"))?;
    let idx = tx_json
        .pointer("/effects/V2/gas_object_index")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let entry = payments
        .get(idx)
        .or_else(|| payments.first())
        .ok_or_else(|| anyhow!("gas_data.payment empty"))?;
    let arr = entry
        .as_array()
        .ok_or_else(|| anyhow!("gas_data.payment entry not array"))?;
    let id_str = arr
        .get(0)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("gas_data.payment[0] id"))?;
    let version = match arr.get(1) {
        Some(serde_json::Value::String(s)) => s.parse::<u64>().unwrap_or(0),
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    };
    let id = AccountAddress::from_hex_literal(id_str)
        .map_err(|e| anyhow!("invalid gas object id {id_str}: {e}"))?;
    Ok((id, version))
}

fn find_object_input_by_id(
    inputs: &[InputValue],
    target: AccountAddress,
) -> Option<(Vec<u8>, Option<u64>)> {
    use sui_sandbox_core::ptb::ObjectInput;
    for input in inputs {
        let InputValue::Object(obj) = input else {
            continue;
        };
        match obj {
            ObjectInput::Owned {
                id, bytes, version, ..
            } => {
                if *id == target {
                    return Some((bytes.clone(), *version));
                }
            }
            ObjectInput::ImmRef {
                id, bytes, version, ..
            } => {
                if *id == target {
                    return Some((bytes.clone(), *version));
                }
            }
            ObjectInput::MutRef {
                id, bytes, version, ..
            } => {
                if *id == target {
                    return Some((bytes.clone(), *version));
                }
            }
            ObjectInput::Shared {
                id, bytes, version, ..
            } => {
                if *id == target {
                    return Some((bytes.clone(), *version));
                }
            }
            ObjectInput::Receiving {
                id, bytes, version, ..
            } => {
                if *id == target {
                    return Some((bytes.clone(), *version));
                }
            }
        }
    }
    None
}
