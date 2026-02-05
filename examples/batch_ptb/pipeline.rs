#![allow(
    clippy::type_complexity,
    clippy::field_reassign_with_default,
    clippy::manual_pattern_char_comparison,
    clippy::single_match
)]
//! Batch processing pipeline for PTB replay from static checkpoint range.
//!
//! This module fetches transactions from specific checkpoints and replays them locally.
//!
//! ## Prefetch Strategies
//!
//! The pipeline supports these prefetch strategies:
//!
//! 1. **Ground-Truth-First** (recommended): Uses `unchanged_loaded_runtime_objects` from
//!    transaction effects as the authoritative source for what objects to fetch. This is
//!    faster and more accurate because we know exact versions upfront.
//!
//! 2. **MM2 Predictive**: Ground-truth plus MM2 bytecode analysis for deeper coverage.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;

use crate::cache::{CachedCheckpoint, CachedLinkage, CachedObject, CheckpointRangeCache};
use sui_prefetch::{ground_truth_prefetch_for_transaction, GroundTruthPrefetchConfig};
use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::sandbox_runtime::VersionedChildFetcherFn;
use sui_sandbox_core::tx_replay::{grpc_to_fetched_transaction, CachedTransaction};
use sui_sandbox_core::utilities::bcs_scanner::{
    extract_addresses_from_bytecode_constants, extract_addresses_from_type_params,
};
use sui_sandbox_core::utilities::{
    extract_dependencies_from_bytecode, extract_package_ids_from_type, is_framework_package,
    normalize_address, parse_type_tag, rewrite_type_tag,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness, DEFAULT_PROTOCOL_VERSION};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcInput, GrpcTransaction};
use sui_types::digests::TransactionDigest as SuiTransactionDigest;

/// Prefetch strategy for data fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefetchStrategy {
    /// Ground-truth-first: Use transaction effects to determine exact objects/versions.
    #[default]
    GroundTruth,
    /// MM2 Predictive: Ground-truth + MM2 bytecode analysis for dynamic field prediction.
    MM2Predictive,
}

/// Extract missing package address from LINKER_ERROR or FUNCTION_RESOLUTION_FAILURE message.
/// Returns the package address if found, normalized to full 64-char hex format.
fn extract_missing_package_from_error(error: &str) -> Option<String> {
    // Pattern: "Cannot find ModuleId { address: <hex>, name: ..."
    if let Some(start) = error.find("address: ") {
        let rest = &error[start + 9..];
        // Find the end of the address (comma or space or closing brace)
        let end = rest
            .find(|c: char| c == ',' || c == ' ' || c == '}')
            .unwrap_or(rest.len());
        let addr = rest[..end].trim();
        if !addr.is_empty() && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(normalize_address(&format!("0x{}", addr)));
        }
    }
    None
}

/// Build a replay-accurate SimulationConfig for a gRPC transaction.
fn build_replay_config_for_tx(
    rt: &tokio::runtime::Runtime,
    grpc: &GrpcClient,
    grpc_tx: &GrpcTransaction,
    tx_timestamp_ms: u64,
) -> Result<SimulationConfig> {
    let tx_hash = SuiTransactionDigest::from_str(&grpc_tx.digest)
        .map_err(|e| anyhow::anyhow!("Invalid transaction digest {}: {}", grpc_tx.digest, e))?
        .into_inner();

    let mut epoch = grpc_tx.epoch.unwrap_or(0);
    if epoch == 0 {
        if let Some(checkpoint) = grpc_tx.checkpoint {
            let cp_result = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), grpc.get_checkpoint(checkpoint)).await
            });
            if let Ok(Ok(Some(cp))) = cp_result {
                epoch = cp.epoch;
            }
        }
    }

    let mut protocol_version = 0u64;
    let mut reference_gas_price: Option<u64> = None;
    if epoch > 0 {
        let ep_result = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), grpc.get_epoch(Some(epoch))).await
        });
        if let Ok(Ok(Some(ep))) = ep_result {
            if let Some(pv) = ep.protocol_version {
                protocol_version = pv;
            }
            reference_gas_price = ep.reference_gas_price;
        }
    }

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let protocol_version = if protocol_version > 0 {
        protocol_version
    } else {
        DEFAULT_PROTOCOL_VERSION
    }
    .min(DEFAULT_PROTOCOL_VERSION);

    let mut config = SimulationConfig::default()
        .with_sender_address(sender_address)
        .with_epoch(epoch)
        .with_protocol_version(protocol_version)
        .with_tx_hash(tx_hash)
        .with_tx_timestamp(tx_timestamp_ms);

    if let Some(budget) = grpc_tx.gas_budget {
        if budget > 0 {
            config = config.with_gas_budget(Some(budget));
        }
    }

    if let Some(price) = grpc_tx.gas_price {
        if price > 0 {
            config = config.with_gas_price(price);
        }
    }

    if let Some(rgp) = reference_gas_price.or_else(|| grpc_tx.gas_price.filter(|p| *p > 0)) {
        config = config.with_reference_gas_price(rgp);
    }

    Ok(config)
}

/// Build a replay SimulationConfig without network calls using cached metadata.
fn build_replay_config_for_tx_cached(
    grpc_tx: &GrpcTransaction,
    tx_timestamp_ms: u64,
    cache: &CheckpointRangeCache,
) -> Result<SimulationConfig> {
    let tx_hash = SuiTransactionDigest::from_str(&grpc_tx.digest)
        .map_err(|e| anyhow::anyhow!("Invalid transaction digest {}: {}", grpc_tx.digest, e))?
        .into_inner();

    let mut epoch = grpc_tx.epoch.unwrap_or(0);
    if epoch == 0 {
        if let Some(checkpoint) = grpc_tx.checkpoint {
            if let Some(cp) = cache
                .checkpoints
                .iter()
                .find(|c| c.sequence_number == checkpoint)
            {
                epoch = cp.epoch;
            }
        }
    }

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let protocol_version = DEFAULT_PROTOCOL_VERSION;

    let mut config = SimulationConfig::default()
        .with_sender_address(sender_address)
        .with_epoch(epoch)
        .with_protocol_version(protocol_version)
        .with_tx_hash(tx_hash)
        .with_tx_timestamp(tx_timestamp_ms);

    if let Some(budget) = grpc_tx.gas_budget {
        if budget > 0 {
            config = config.with_gas_budget(Some(budget));
        }
    }

    if let Some(price) = grpc_tx.gas_price {
        if price > 0 {
            config = config.with_gas_price(price);
            config = config.with_reference_gas_price(price);
        }
    }

    Ok(config)
}

/// Statistics collected during batch processing.
#[derive(Debug, Default)]
pub struct BatchStats {
    pub checkpoints_processed: usize,
    pub transactions_fetched: usize,
    pub transactions_processed: usize,
    pub successful_replays: usize,
    pub failed_replays: usize,
    pub skipped_fetch_errors: usize,
    pub total_objects_fetched: usize,
    pub total_packages_fetched: usize,
    pub dynamic_fields_resolved: usize,
    pub data_fetch_time: Duration,
    pub execution_time: Duration,
    pub failure_reasons: HashMap<String, usize>,
    pub outcome_matches: usize,
    /// Detailed mismatch info: (digest, local_success, onchain_success, error, is_framework_only)
    pub mismatches: Vec<(String, bool, bool, Option<String>, bool)>,
    /// Framework-only transaction stats
    pub framework_total: usize,
    pub framework_matches: usize,
    /// Complex (non-framework) transaction stats
    pub complex_total: usize,
    pub complex_matches: usize,
    /// Prefetch strategy used
    pub prefetch_strategy: Option<PrefetchStrategy>,
    /// MM2 prediction stats (only for MM2Predictive strategy)
    pub mm2_predictions_made: usize,
    pub mm2_predictions_matched: usize,
    pub mm2_packages_analyzed: usize,
    pub mm2_packages_fetched_for_analysis: usize,
}

impl BatchStats {
    pub fn match_rate(&self) -> f64 {
        if self.transactions_processed == 0 {
            return 0.0;
        }
        self.outcome_matches as f64 / self.transactions_processed as f64
    }

    fn record_failure(&mut self, reason: &str) {
        *self.failure_reasons.entry(reason.to_string()).or_insert(0) += 1;
    }
}

/// Result of processing a single transaction.
struct TransactionResult {
    digest: String,
    onchain_success: bool,
    local_success: bool,
    outcome_matches: bool,
    error: Option<String>,
    objects_fetched: usize,
    packages_fetched: usize,
    dynamic_fields_prefetched: usize,
}

/// The main batch processing pipeline.
pub struct BatchPipeline;

impl BatchPipeline {
    /// Run the pipeline for a specific checkpoint range.
    #[allow(dead_code)]
    pub async fn run_checkpoints(
        start_checkpoint: u64,
        num_checkpoints: u64,
    ) -> Result<BatchStats> {
        let stats = tokio::task::spawn_blocking(move || {
            run_checkpoint_batch(
                start_checkpoint,
                num_checkpoints,
                false,
                false,
                PrefetchStrategy::GroundTruth,
            )
        })
        .await??;

        Ok(stats)
    }

    /// Run the pipeline with cache support.
    ///
    /// If `fetch_mode` is true, will fetch all data and save to cache.
    /// If `fetch_mode` is false, will try to load from cache first.
    #[allow(dead_code)]
    pub async fn run_checkpoints_with_cache(
        start_checkpoint: u64,
        num_checkpoints: u64,
        fetch_mode: bool,
        quiet_mode: bool,
    ) -> Result<BatchStats> {
        let stats = tokio::task::spawn_blocking(move || {
            run_checkpoint_batch(
                start_checkpoint,
                num_checkpoints,
                fetch_mode,
                quiet_mode,
                PrefetchStrategy::GroundTruth,
            )
        })
        .await??;

        Ok(stats)
    }

    /// Run the pipeline with a specific prefetch strategy.
    pub async fn run_with_strategy(
        start_checkpoint: u64,
        num_checkpoints: u64,
        fetch_mode: bool,
        quiet_mode: bool,
        strategy: PrefetchStrategy,
    ) -> Result<BatchStats> {
        let stats = tokio::task::spawn_blocking(move || {
            run_checkpoint_batch(
                start_checkpoint,
                num_checkpoints,
                fetch_mode,
                quiet_mode,
                strategy,
            )
        })
        .await??;

        Ok(stats)
    }
}

use std::sync::Mutex;

/// Shared object cache for accumulating fetched objects across transactions
struct SharedObjectCache {
    /// Objects by "object_id:version"
    objects: Mutex<HashMap<String, CachedObject>>,
    /// Dynamic field children by child_id
    dynamic_children: Mutex<HashMap<String, (u64, String, Vec<u8>)>>,
}

impl SharedObjectCache {
    fn new() -> Self {
        Self {
            objects: Mutex::new(HashMap::new()),
            dynamic_children: Mutex::new(HashMap::new()),
        }
    }

    fn add_object(
        &self,
        obj_id: &str,
        version: u64,
        type_str: Option<String>,
        bcs: Option<Vec<u8>>,
    ) {
        let key = format!("{}:{}", obj_id, version);
        let cached = CachedObject {
            object_id: obj_id.to_string(),
            version,
            type_string: type_str,
            bcs,
            package_modules: None,
            package_linkage: None,
        };
        self.objects.lock().unwrap().insert(key, cached);
    }

    fn add_package(
        &self,
        pkg_id: &str,
        version: u64,
        modules: Vec<(String, Vec<u8>)>,
        linkage: Option<Vec<CachedLinkage>>,
    ) {
        let key = format!("{}:{}", pkg_id, version);
        let cached = CachedObject {
            object_id: pkg_id.to_string(),
            version,
            type_string: None,
            bcs: None,
            package_modules: Some(modules),
            package_linkage: linkage,
        };
        self.objects.lock().unwrap().insert(key, cached);
    }

    fn add_dynamic_child(&self, child_id: &str, version: u64, type_str: String, bcs: Vec<u8>) {
        self.dynamic_children
            .lock()
            .unwrap()
            .insert(child_id.to_string(), (version, type_str, bcs));
    }

    fn merge_into_cache(&self, cache: &mut CheckpointRangeCache) {
        for (key, obj) in self.objects.lock().unwrap().drain() {
            cache.objects.insert(key, obj);
        }
        for (child_id, data) in self.dynamic_children.lock().unwrap().drain() {
            cache.dynamic_field_children.insert(child_id, data);
        }
    }
}

fn run_checkpoint_batch(
    start_checkpoint: u64,
    num_checkpoints: u64,
    fetch_mode: bool,
    quiet_mode: bool,
    strategy: PrefetchStrategy,
) -> Result<BatchStats> {
    let rt = tokio::runtime::Runtime::new()?;
    let mut stats = BatchStats::default();
    stats.prefetch_strategy = Some(strategy);
    let end_checkpoint = start_checkpoint + num_checkpoints - 1;

    // Try to load from cache first (unless in fetch mode)
    let cache = if fetch_mode {
        println!("Step 0: Fetch mode - will build fresh cache");
        None
    } else {
        println!("Step 0: Checking for cached data...");
        CheckpointRangeCache::load(start_checkpoint, end_checkpoint)
    };

    // If we have a cache, use it directly
    if let Some(ref cached_data) = cache {
        println!(
            "   Using cache: {} checkpoints, {} objects cached",
            cached_data.checkpoints.len(),
            cached_data.objects.len()
        );
        return run_from_cache(cached_data, quiet_mode);
    }

    // Otherwise, fetch from network
    println!("\nStep 1: Connecting to services...");

    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let grpc = Arc::new(grpc);
    let graphql = GraphQLClient::mainnet();

    // Initialize cache for building
    let mut building_cache = CheckpointRangeCache::new(start_checkpoint, end_checkpoint);

    println!("   Connected to {}", endpoint);

    // =========================================================================
    // Step 2: Fetch transactions from checkpoints
    // =========================================================================
    println!(
        "\nStep 2: Fetching transactions from checkpoints {}..{}",
        start_checkpoint,
        start_checkpoint + num_checkpoints - 1
    );
    let fetch_start = Instant::now();

    let mut all_transactions: Vec<GrpcTransaction> = Vec::new();

    for cp_num in start_checkpoint..(start_checkpoint + num_checkpoints) {
        match rt.block_on(async { grpc.get_checkpoint(cp_num).await }) {
            Ok(Some(checkpoint)) => {
                let tx_count = checkpoint.transactions.len();
                // Filter to PTB transactions only (skip system transactions)
                let ptb_txs: Vec<GrpcTransaction> = checkpoint
                    .transactions
                    .iter()
                    .filter(|tx| tx.is_ptb())
                    .cloned()
                    .collect();

                println!(
                    "   Checkpoint {}: {} total txs, {} PTBs",
                    cp_num,
                    tx_count,
                    ptb_txs.len()
                );

                // Add to cache
                building_cache
                    .checkpoints
                    .push(CachedCheckpoint::from(&checkpoint));

                all_transactions.extend(ptb_txs);
                stats.checkpoints_processed += 1;
            }
            Ok(None) => {
                println!("   Checkpoint {}: not found", cp_num);
            }
            Err(e) => {
                println!("   Checkpoint {}: error - {}", cp_num, e);
            }
        }
    }

    stats.transactions_fetched = all_transactions.len();
    stats.data_fetch_time = fetch_start.elapsed();

    println!("\n   Total PTB transactions: {}", all_transactions.len());

    // =========================================================================
    // Step 3: Process each transaction
    // =========================================================================
    println!("\nStep 3: Processing transactions...\n");
    let exec_start = Instant::now();

    // Create shared cache for accumulating fetched objects
    let shared_cache = Arc::new(SharedObjectCache::new());

    for (idx, grpc_tx) in all_transactions.iter().enumerate() {
        // Progress indicator
        if (idx + 1) % 10 == 0 || idx == 0 {
            print!(
                "\r   Processing transaction {}/{}...",
                idx + 1,
                all_transactions.len()
            );
            std::io::Write::flush(&mut std::io::stdout())?;
        }

        // Categorize transaction: framework-only vs complex (uses non-framework packages)
        let tx_packages: Vec<String> = grpc_tx
            .commands
            .iter()
            .filter_map(|cmd| {
                if let sui_transport::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
                    Some(package.clone())
                } else {
                    None
                }
            })
            .collect();
        let is_framework_only = tx_packages.iter().all(|p| is_framework_package(p));
        let tx_category = if is_framework_only {
            "framework"
        } else {
            "complex"
        };

        match process_single_transaction(
            &rt,
            &grpc,
            &graphql,
            grpc_tx,
            Some(&shared_cache),
            strategy,
        ) {
            Ok(result) => {
                stats.transactions_processed += 1;
                stats.total_objects_fetched += result.objects_fetched;
                stats.total_packages_fetched += result.packages_fetched;
                stats.dynamic_fields_resolved += result.dynamic_fields_prefetched;

                // Track framework vs complex stats
                if is_framework_only {
                    stats.framework_total += 1;
                    if result.outcome_matches {
                        stats.framework_matches += 1;
                    }
                } else {
                    stats.complex_total += 1;
                    if result.outcome_matches {
                        stats.complex_matches += 1;
                    }
                }

                // Log transaction result with category
                let status = if result.outcome_matches { "✓" } else { "✗" };
                let local_str = if result.local_success { "OK" } else { "FAIL" };
                let onchain_str = if result.onchain_success { "OK" } else { "FAIL" };
                eprintln!(
                    "   {} {} [{}] local={} onchain={}",
                    status,
                    &grpc_tx.digest[..16],
                    tx_category,
                    local_str,
                    onchain_str
                );

                if result.outcome_matches {
                    stats.outcome_matches += 1;
                } else {
                    stats.mismatches.push((
                        result.digest.clone(),
                        result.local_success,
                        result.onchain_success,
                        result.error.clone(),
                        is_framework_only,
                    ));
                }

                if result.local_success {
                    stats.successful_replays += 1;
                } else {
                    stats.failed_replays += 1;
                    if let Some(err) = &result.error {
                        stats.record_failure(err);
                    }
                }
            }
            Err(e) => {
                stats.skipped_fetch_errors += 1;
                eprintln!(
                    "\n   SKIP {} [{}]: {}",
                    &grpc_tx.digest[..16],
                    tx_category,
                    e
                );
            }
        }
    }

    println!(
        "\r   Processed {}/{} transactions.    ",
        stats.transactions_processed,
        all_transactions.len()
    );

    stats.execution_time = exec_start.elapsed();

    // Save cache if we're in fetch mode
    if fetch_mode {
        println!("\nStep 4: Saving cache...");
        // Merge accumulated objects into the cache
        shared_cache.merge_into_cache(&mut building_cache);
        println!(
            "   Cached {} objects, {} dynamic field children",
            building_cache.objects.len(),
            building_cache.dynamic_field_children.len()
        );
        if let Err(e) = building_cache.save() {
            eprintln!("   Warning: Failed to save cache: {}", e);
        }
    }

    Ok(stats)
}

/// Run replay from cached data (no network fetching).
fn run_from_cache(cache: &CheckpointRangeCache, quiet_mode: bool) -> Result<BatchStats> {
    let mut stats = BatchStats::default();

    // Collect all PTB transactions from cache
    let mut all_transactions: Vec<GrpcTransaction> = Vec::new();
    for cached_cp in &cache.checkpoints {
        stats.checkpoints_processed += 1;
        let ptb_txs: Vec<GrpcTransaction> = cached_cp
            .transactions
            .iter()
            .filter(|tx| {
                // Check if it's a PTB
                !tx.sender.is_empty() && (!tx.commands.is_empty() || tx.gas_budget.unwrap_or(0) > 0)
            })
            .map(|tx| tx.to_grpc())
            .collect();
        all_transactions.extend(ptb_txs);
    }

    stats.transactions_fetched = all_transactions.len();
    println!(
        "   Loaded {} transactions from {} cached checkpoints",
        all_transactions.len(),
        cache.checkpoints.len()
    );

    println!("\nProcessing transactions from cache...\n");
    let exec_start = Instant::now();

    for (idx, grpc_tx) in all_transactions.iter().enumerate() {
        if !quiet_mode && ((idx + 1) % 10 == 0 || idx == 0) {
            print!(
                "\r   Processing transaction {}/{}...",
                idx + 1,
                all_transactions.len()
            );
            std::io::Write::flush(&mut std::io::stdout())?;
        }

        match process_single_transaction_from_cache(cache, grpc_tx, quiet_mode) {
            Ok(result) => {
                stats.transactions_processed += 1;
                stats.total_objects_fetched += result.objects_fetched;
                stats.total_packages_fetched += result.packages_fetched;
                stats.dynamic_fields_resolved += result.dynamic_fields_prefetched;

                if result.outcome_matches {
                    stats.outcome_matches += 1;
                } else {
                    // In cache mode we don't track framework vs complex (would need to parse commands)
                    stats.mismatches.push((
                        result.digest.clone(),
                        result.local_success,
                        result.onchain_success,
                        result.error.clone(),
                        false, // unknown in cache mode
                    ));
                }

                if result.local_success {
                    stats.successful_replays += 1;
                } else {
                    stats.failed_replays += 1;
                    if let Some(err) = &result.error {
                        stats.record_failure(err);
                    }
                }
            }
            Err(e) => {
                stats.skipped_fetch_errors += 1;
                if !quiet_mode {
                    eprintln!("\n   SKIP {}: {}", &grpc_tx.digest[..16], e);
                }
            }
        }
    }

    println!(
        "\r   Processed {}/{} transactions.    ",
        stats.transactions_processed,
        all_transactions.len()
    );

    stats.execution_time = exec_start.elapsed();
    Ok(stats)
}

fn add_version(versions: &mut HashMap<String, u64>, object_id: &str, version: u64) {
    let normalized = normalize_address(object_id);
    match versions.get_mut(&normalized) {
        Some(existing) => {
            if version > *existing {
                *existing = version;
            }
        }
        None => {
            versions.insert(normalized, version);
        }
    }
}

fn collect_historical_versions(grpc_tx: &GrpcTransaction) -> HashMap<String, u64> {
    let mut versions = HashMap::new();

    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                add_version(&mut versions, object_id, *version);
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                add_version(&mut versions, object_id, *version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                add_version(&mut versions, object_id, *initial_version);
            }
            GrpcInput::Pure { .. } => {}
        }
    }

    for (obj_id, version) in &grpc_tx.unchanged_loaded_runtime_objects {
        add_version(&mut versions, obj_id, *version);
    }
    for (obj_id, version) in &grpc_tx.changed_objects {
        add_version(&mut versions, obj_id, *version);
    }
    for (obj_id, version) in &grpc_tx.created_objects {
        add_version(&mut versions, obj_id, *version);
    }
    for (obj_id, version) in &grpc_tx.unchanged_consensus_objects {
        add_version(&mut versions, obj_id, *version);
    }

    versions
}

fn collect_needed_package_ids(
    grpc_tx: &GrpcTransaction,
    object_types: &HashMap<String, String>,
) -> HashSet<String> {
    let mut package_ids = HashSet::new();

    for cmd in &grpc_tx.commands {
        match cmd {
            sui_transport::grpc::GrpcCommand::MoveCall {
                package,
                type_arguments,
                ..
            } => {
                package_ids.insert(normalize_address(package));
                for type_arg in type_arguments {
                    for pkg_id in extract_package_ids_from_type(type_arg) {
                        package_ids.insert(normalize_address(&pkg_id));
                    }
                }
            }
            sui_transport::grpc::GrpcCommand::MakeMoveVec {
                element_type: Some(element_type),
                ..
            } => {
                for pkg_id in extract_package_ids_from_type(element_type) {
                    package_ids.insert(normalize_address(&pkg_id));
                }
            }
            sui_transport::grpc::GrpcCommand::Publish { dependencies, .. }
            | sui_transport::grpc::GrpcCommand::Upgrade { dependencies, .. } => {
                for dep in dependencies {
                    package_ids.insert(normalize_address(dep));
                }
            }
            _ => {}
        }
    }

    // Packages that were dynamically loaded during execution
    for (obj_id, _version) in &grpc_tx.unchanged_loaded_runtime_objects {
        package_ids.insert(normalize_address(obj_id));
    }

    // Packages referenced by type strings
    for type_str in object_types.values() {
        for addr in extract_addresses_from_type_params(type_str) {
            package_ids.insert(normalize_address(&addr));
        }
    }

    package_ids
}

fn build_cached_child_map(cache: &CheckpointRangeCache) -> HashMap<String, (u64, String, Vec<u8>)> {
    let mut out = cache.dynamic_field_children.clone();
    for obj in cache.objects.values() {
        if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
            let key = normalize_address(&obj.object_id);
            out.entry(key)
                .or_insert((obj.version, type_str.clone(), bcs.clone()));
        }
    }
    out
}

fn find_cached_package<'a>(
    cache: &'a CheckpointRangeCache,
    pkg_id: &str,
    version: Option<u64>,
) -> Option<&'a CachedObject> {
    if let Some(ver) = version {
        if let Some(obj) = cache.get_object(pkg_id, ver) {
            if obj.package_modules.is_some() {
                return Some(obj);
            }
        }
    }
    cache
        .get_object_any_version(pkg_id)
        .and_then(|obj| obj.package_modules.as_ref().map(|_| obj))
}

fn resolve_cached_packages(
    cache: &CheckpointRangeCache,
    initial_packages: HashSet<String>,
    historical_versions: &mut HashMap<String, u64>,
    linkage_upgrades: &mut HashMap<String, String>,
) -> (HashMap<String, Vec<(String, String)>>, HashSet<String>) {
    let mut packages = HashMap::new();
    let mut missing = HashSet::new();
    let mut visited = HashSet::new();
    let mut queue: Vec<String> = initial_packages.into_iter().collect();

    const MAX_DEPENDENCY_DEPTH: usize = 25;
    for _depth in 0..MAX_DEPENDENCY_DEPTH {
        if queue.is_empty() {
            break;
        }

        let mut next = Vec::new();
        for pkg_id in queue.drain(..) {
            let normalized = normalize_address(&pkg_id);
            if !visited.insert(normalized.clone()) {
                continue;
            }

            let preferred_version = historical_versions.get(&normalized).copied();
            let cached_obj = find_cached_package(cache, &normalized, preferred_version);
            let Some(obj) = cached_obj else {
                missing.insert(normalized);
                continue;
            };

            let Some(modules) = &obj.package_modules else {
                missing.insert(normalized);
                continue;
            };

            let modules_b64: Vec<(String, String)> = modules
                .iter()
                .map(|(name, bytes)| {
                    (
                        name.clone(),
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                    )
                })
                .collect();
            packages.insert(normalized.clone(), modules_b64);
            historical_versions
                .entry(normalized.clone())
                .or_insert(obj.version);

            if let Some(linkage) = &obj.package_linkage {
                for l in linkage {
                    if is_framework_package(&l.original_id) {
                        continue;
                    }
                    let orig_norm = normalize_address(&l.original_id);
                    let upgraded_norm = normalize_address(&l.upgraded_id);
                    if orig_norm != upgraded_norm {
                        linkage_upgrades.insert(orig_norm, upgraded_norm.clone());
                        next.push(upgraded_norm);
                    }
                }
            }

            for (_name, bytecode) in modules {
                let deps = extract_dependencies_from_bytecode(bytecode);
                for dep in deps {
                    let dep_norm = normalize_address(&dep);
                    let actual = linkage_upgrades.get(&dep_norm).cloned().unwrap_or(dep_norm);
                    next.push(actual);
                }

                let const_addrs = extract_addresses_from_bytecode_constants(bytecode);
                for addr in const_addrs {
                    next.push(normalize_address(&addr));
                }
            }
        }

        queue = next;
    }

    if !queue.is_empty() {
        for pkg_id in queue {
            missing.insert(normalize_address(&pkg_id));
        }
    }

    (packages, missing)
}

/// Process a single transaction from cache (no network fetching).
fn process_single_transaction_from_cache(
    cache: &CheckpointRangeCache,
    grpc_tx: &GrpcTransaction,
    _quiet_mode: bool,
) -> Result<TransactionResult> {
    let onchain_success = grpc_tx
        .status
        .as_ref()
        .map(|s| s == "success")
        .unwrap_or(false);

    let mut historical_versions = collect_historical_versions(grpc_tx);
    let mut objects: HashMap<String, String> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut linkage_upgrades: HashMap<String, String> = HashMap::new();
    let mut package_ids: HashSet<String> = HashSet::new();
    let mut missing_objects: Vec<String> = Vec::new();

    let versions_snapshot: Vec<(String, u64)> = historical_versions
        .iter()
        .map(|(id, ver)| (id.clone(), *ver))
        .collect();

    for (obj_id, version) in versions_snapshot {
        let cached_obj = cache
            .get_object(&obj_id, version)
            .or_else(|| cache.get_object_any_version(&obj_id));

        let Some(obj) = cached_obj else {
            missing_objects.push(obj_id);
            continue;
        };

        if let Some(bcs) = &obj.bcs {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bcs);
            objects.insert(obj_id.clone(), b64);
        } else if obj.package_modules.is_none() {
            missing_objects.push(obj_id.clone());
        }

        if let Some(type_str) = &obj.type_string {
            object_types.insert(obj_id.clone(), type_str.clone());
        }

        if obj.package_modules.is_some() {
            let normalized = normalize_address(&obj.object_id);
            package_ids.insert(normalized.clone());
            historical_versions.entry(normalized).or_insert(obj.version);
        }

        if let Some(linkage) = &obj.package_linkage {
            for l in linkage {
                if is_framework_package(&l.original_id) {
                    continue;
                }
                let orig_norm = normalize_address(&l.original_id);
                let upgraded_norm = normalize_address(&l.upgraded_id);
                if orig_norm != upgraded_norm {
                    linkage_upgrades.insert(orig_norm, upgraded_norm);
                }
            }
        }
    }

    if !missing_objects.is_empty() {
        return Err(anyhow::anyhow!(
            "cache missing {} object(s): {}",
            missing_objects.len(),
            missing_objects.join(", ")
        ));
    }

    package_ids.extend(collect_needed_package_ids(grpc_tx, &object_types));

    let (packages, missing_packages) = resolve_cached_packages(
        cache,
        package_ids,
        &mut historical_versions,
        &mut linkage_upgrades,
    );

    if !missing_packages.is_empty() {
        let mut missing_list: Vec<String> = missing_packages.into_iter().collect();
        missing_list.sort();
        return Err(anyhow::anyhow!(
            "cache missing {} package(s): {}",
            missing_list.len(),
            missing_list.join(", ")
        ));
    }

    let cached_children = build_cached_child_map(cache);

    execute_replay_from_cache(
        cache,
        grpc_tx,
        objects,
        object_types,
        packages,
        historical_versions,
        linkage_upgrades,
        Arc::new(cached_children),
        onchain_success,
    )
}

/// Process a single transaction through the full replay pipeline.
fn process_single_transaction(
    rt: &tokio::runtime::Runtime,
    grpc: &Arc<GrpcClient>,
    graphql: &GraphQLClient,
    grpc_tx: &GrpcTransaction,
    shared_cache: Option<&Arc<SharedObjectCache>>,
    strategy: PrefetchStrategy,
) -> Result<TransactionResult> {
    let onchain_success = grpc_tx
        .status
        .as_ref()
        .map(|s| s == "success")
        .unwrap_or(false);

    // Dispatch to appropriate strategy
    match strategy {
        PrefetchStrategy::GroundTruth => process_with_ground_truth_prefetch(
            rt,
            grpc,
            graphql,
            grpc_tx,
            shared_cache,
            onchain_success,
        ),
        PrefetchStrategy::MM2Predictive => process_with_mm2_predictive_prefetch(
            rt,
            grpc,
            graphql,
            grpc_tx,
            shared_cache,
            onchain_success,
        ),
    }
}

/// Process transaction using ground-truth-first prefetch strategy.
fn process_with_ground_truth_prefetch(
    rt: &tokio::runtime::Runtime,
    grpc: &Arc<GrpcClient>,
    graphql: &GraphQLClient,
    grpc_tx: &GrpcTransaction,
    shared_cache: Option<&Arc<SharedObjectCache>>,
    onchain_success: bool,
) -> Result<TransactionResult> {
    // Use the new ground-truth-first prefetch
    let config = GroundTruthPrefetchConfig::default();
    let prefetch_result =
        ground_truth_prefetch_for_transaction(grpc, Some(graphql), rt, grpc_tx, &config);

    // Convert prefetch result to the format expected by replay
    let mut objects: HashMap<String, String> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut packages: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut historical_versions: HashMap<String, u64> = HashMap::new();
    let mut linkage_upgrades: HashMap<String, String> =
        prefetch_result.discovered_linkage_upgrades.clone();

    // Process objects from ground truth
    for (obj_id, obj) in &prefetch_result.objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes);
        objects.insert(obj_id.clone(), bcs_b64);
        historical_versions.insert(obj_id.clone(), obj.version);
        if let Some(type_str) = &obj.type_string {
            object_types.insert(obj_id.clone(), type_str.clone());
        }

        // Store to shared cache
        if let Some(cache) = shared_cache {
            cache.add_object(
                obj_id,
                obj.version,
                obj.type_string.clone(),
                Some(obj.bcs_bytes.clone()),
            );
        }
    }

    // Process packages
    for (pkg_id, pkg) in &prefetch_result.packages {
        let modules_b64: Vec<(String, String)> = pkg
            .modules
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                )
            })
            .collect();
        packages.insert(pkg_id.clone(), modules_b64);
        historical_versions.insert(pkg_id.clone(), pkg.version);

        // Store to shared cache
        if let Some(cache) = shared_cache {
            let linkage = if pkg.linkage.is_empty() {
                None
            } else {
                Some(
                    pkg.linkage
                        .iter()
                        .map(|(original, upgraded)| CachedLinkage {
                            original_id: original.clone(),
                            upgraded_id: upgraded.clone(),
                        })
                        .collect(),
                )
            };
            cache.add_package(pkg_id, pkg.version, pkg.modules.clone(), linkage);
        }

        // Accumulate linkage
        for (orig, upgraded) in &pkg.linkage {
            linkage_upgrades.insert(orig.clone(), upgraded.clone());
        }
    }

    // Process supplemental objects (if any)
    for (obj_id, obj) in &prefetch_result.supplemental_objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes);
        objects.insert(obj_id.clone(), bcs_b64);
        historical_versions.insert(obj_id.clone(), obj.version);
        if let Some(type_str) = &obj.type_string {
            object_types.insert(obj_id.clone(), type_str.clone());
        }
    }

    let objects_fetched =
        prefetch_result.stats.ground_truth_fetched + prefetch_result.stats.supplemental_fetched;
    let packages_fetched = prefetch_result.stats.packages_fetched;
    let dynamic_fields_prefetched = prefetch_result.stats.ground_truth_count;

    // Now run the replay using the collected data
    execute_replay(
        rt,
        grpc,
        graphql,
        grpc_tx,
        objects,
        object_types,
        packages,
        historical_versions,
        linkage_upgrades,
        shared_cache,
        onchain_success,
        objects_fetched,
        packages_fetched,
        dynamic_fields_prefetched,
    )
}

/// Process transaction using MM2 predictive prefetch strategy.
///
/// This combines ground-truth prefetch with MM2 bytecode analysis to predict
/// dynamic field accesses. The predictor is created per-transaction for now.
fn process_with_mm2_predictive_prefetch(
    rt: &tokio::runtime::Runtime,
    grpc: &Arc<GrpcClient>,
    graphql: &GraphQLClient,
    grpc_tx: &GrpcTransaction,
    shared_cache: Option<&Arc<SharedObjectCache>>,
    onchain_success: bool,
) -> Result<TransactionResult> {
    // Use the predictive prefetcher with MM2 analysis
    let mut prefetcher = PredictivePrefetcher::new();
    let config = PredictivePrefetchConfig::default();
    let prefetch_result =
        prefetcher.prefetch_for_transaction(grpc, Some(graphql), rt, grpc_tx, &config);

    // Log MM2 prediction stats
    let pred_stats = &prefetch_result.prediction_stats;
    if pred_stats.predictions_made > 0 {
        eprintln!(
            "  [MM2] Commands: {}, Predictions: {}, Matched: {}, Packages: {} (+{} fetched for MM2)",
            pred_stats.commands_analyzed,
            pred_stats.predictions_made,
            pred_stats.predictions_matched_ground_truth,
            pred_stats.packages_analyzed,
            pred_stats.packages_fetched_for_mm2
        );
    }

    // Convert prefetch result to the format expected by replay
    // (Same as ground-truth prefetch since the base_result is compatible)
    let mut objects: HashMap<String, String> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut packages: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut historical_versions: HashMap<String, u64> = HashMap::new();
    let mut linkage_upgrades: HashMap<String, String> = prefetch_result
        .base_result
        .discovered_linkage_upgrades
        .clone();

    // Process objects from ground truth
    for (obj_id, obj) in &prefetch_result.base_result.objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes);
        objects.insert(obj_id.clone(), bcs_b64);
        historical_versions.insert(obj_id.clone(), obj.version);
        if let Some(type_str) = &obj.type_string {
            object_types.insert(obj_id.clone(), type_str.clone());
        }

        // Store to shared cache
        if let Some(cache) = shared_cache {
            cache.add_object(
                obj_id,
                obj.version,
                obj.type_string.clone(),
                Some(obj.bcs_bytes.clone()),
            );
        }
    }

    // Process packages
    for (pkg_id, pkg) in &prefetch_result.base_result.packages {
        let modules_b64: Vec<(String, String)> = pkg
            .modules
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                )
            })
            .collect();
        packages.insert(pkg_id.clone(), modules_b64);
        historical_versions.insert(pkg_id.clone(), pkg.version);

        // Store to shared cache
        if let Some(cache) = shared_cache {
            let linkage = if pkg.linkage.is_empty() {
                None
            } else {
                Some(
                    pkg.linkage
                        .iter()
                        .map(|(original, upgraded)| CachedLinkage {
                            original_id: original.clone(),
                            upgraded_id: upgraded.clone(),
                        })
                        .collect(),
                )
            };
            cache.add_package(pkg_id, pkg.version, pkg.modules.clone(), linkage);
        }

        // Accumulate linkage
        for (orig, upgraded) in &pkg.linkage {
            linkage_upgrades.insert(orig.clone(), upgraded.clone());
        }
    }

    // Process supplemental objects (if any)
    for (obj_id, obj) in &prefetch_result.base_result.supplemental_objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes);
        objects.insert(obj_id.clone(), bcs_b64);
        historical_versions.insert(obj_id.clone(), obj.version);
        if let Some(type_str) = &obj.type_string {
            object_types.insert(obj_id.clone(), type_str.clone());
        }
    }

    let base_stats = &prefetch_result.base_result.stats;
    let objects_fetched = base_stats.ground_truth_fetched + base_stats.supplemental_fetched;
    let packages_fetched = base_stats.packages_fetched;
    let dynamic_fields_prefetched = base_stats.ground_truth_count;

    // Now run the replay using the collected data
    execute_replay(
        rt,
        grpc,
        graphql,
        grpc_tx,
        objects,
        object_types,
        packages,
        historical_versions,
        linkage_upgrades,
        shared_cache,
        onchain_success,
        objects_fetched,
        packages_fetched,
        dynamic_fields_prefetched,
    )
}

fn execute_replay(
    rt: &tokio::runtime::Runtime,
    grpc: &Arc<GrpcClient>,
    graphql: &GraphQLClient,
    grpc_tx: &GrpcTransaction,
    objects: HashMap<String, String>,
    object_types: HashMap<String, String>,
    mut packages: HashMap<String, Vec<(String, String)>>,
    historical_versions: HashMap<String, u64>,
    mut linkage_upgrades: HashMap<String, String>,
    shared_cache: Option<&Arc<SharedObjectCache>>,
    onchain_success: bool,
    objects_fetched: usize,
    mut packages_fetched: usize,
    dynamic_fields_prefetched: usize,
) -> Result<TransactionResult> {
    const MAX_PACKAGE_RETRIES: usize = 5;
    let mut attempted_packages: HashSet<String> = HashSet::new();

    for retry in 0..=MAX_PACKAGE_RETRIES {
        let result = execute_replay_inner(
            rt,
            grpc,
            grpc_tx,
            objects.clone(),
            object_types.clone(),
            packages.clone(),
            historical_versions.clone(),
            linkage_upgrades.clone(),
            shared_cache,
            onchain_success,
            objects_fetched,
            packages_fetched,
            dynamic_fields_prefetched,
        )?;

        // Check if we got a LINKER_ERROR or FUNCTION_RESOLUTION_FAILURE
        if !result.outcome_matches && !result.local_success {
            if let Some(ref error) = result.error {
                let is_linker_error =
                    error.contains("LINKER_ERROR") || error.contains("FUNCTION_RESOLUTION_FAILURE");
                if is_linker_error && retry < MAX_PACKAGE_RETRIES {
                    let extracted = extract_missing_package_from_error(error);
                    if let Some(missing_pkg) = extracted {
                        let already_attempted = attempted_packages.contains(&missing_pkg);
                        let already_have = packages.contains_key(&missing_pkg);

                        // Check if this is an original package ID that might need an upgrade.
                        // Even if we "have" the package, it might be version 1 (original) but
                        // we need a newer version with the function we're trying to call.
                        // In that case, we need to fetch the latest upgrade via GraphQL.
                        if !already_attempted {
                            // Try to find if this is an original package with upgrades
                            if let Ok(Some((latest_addr, _latest_ver))) =
                                graphql.get_latest_package_upgrade(&missing_pkg)
                            {
                                let have_latest = packages.contains_key(&latest_addr);
                                let attempted_latest = attempted_packages.contains(&latest_addr);
                                // Fetch the latest upgrade instead
                                if !have_latest && !attempted_latest {
                                    match rt.block_on(grpc.get_object(&latest_addr)) {
                                        Ok(Some(obj)) => {
                                            if let Some(modules) = obj.package_modules {
                                                let encoded: Vec<(String, String)> = modules
                                                    .iter()
                                                    .map(|(name, bytes)| {
                                                        (
                                                            name.clone(),
                                                            base64::engine::general_purpose::STANDARD.encode(bytes),
                                                        )
                                                    })
                                                    .collect();
                                                packages.insert(latest_addr.clone(), encoded);
                                                // Also record the upgrade mapping
                                                linkage_upgrades.insert(
                                                    missing_pkg.clone(),
                                                    latest_addr.clone(),
                                                );
                                                packages_fetched += 1;
                                                attempted_packages.insert(latest_addr);
                                                attempted_packages.insert(missing_pkg.clone());
                                                continue; // Retry with the upgraded package
                                            }
                                        }
                                        Ok(None) | Err(_) => {
                                            // Upgrade not found or error - will fall through to regular fetch
                                        }
                                    }
                                }
                            }
                        }

                        if already_have {
                            // Package exists but module not found - this happens when the module
                            // is loaded at a different address due to package upgrades.
                            // The resolver alias system should handle this, but some edge cases
                            // related to VM loader caching may still fail.
                        }
                        if !already_attempted && !already_have {
                            // Try to fetch the missing package
                            match rt.block_on(grpc.get_object(&missing_pkg)) {
                                Ok(Some(obj)) => {
                                    if let Some(modules) = obj.package_modules {
                                        let encoded: Vec<(String, String)> = modules
                                            .iter()
                                            .map(|(name, bytes)| {
                                                (
                                                    name.clone(),
                                                    base64::engine::general_purpose::STANDARD
                                                        .encode(bytes),
                                                )
                                            })
                                            .collect();
                                        packages.insert(missing_pkg.clone(), encoded);
                                        packages_fetched += 1;
                                        attempted_packages.insert(missing_pkg);
                                        continue; // Retry with the new package
                                    }
                                }
                                _ => {}
                            }
                            attempted_packages.insert(missing_pkg);
                        }
                    }
                }
            }
        }

        // Either success or no retryable error
        return Ok(result);
    }

    // Should not reach here, but return last result if we somehow do
    execute_replay_inner(
        rt,
        grpc,
        grpc_tx,
        objects,
        object_types,
        packages,
        historical_versions,
        linkage_upgrades,
        shared_cache,
        onchain_success,
        objects_fetched,
        packages_fetched,
        dynamic_fields_prefetched,
    )
}

/// Build a module resolver with package bytecode and address aliases.
fn build_module_resolver(
    cached: &CachedTransaction,
    historical_versions: &HashMap<String, u64>,
    linkage_upgrades: &HashMap<String, String>,
) -> Result<LocalModuleResolver> {
    let mut resolver = LocalModuleResolver::new();

    // Track: package_id -> bytecode_address mappings for alias setup
    let mut pkg_id_to_bytecode_addr: HashMap<String, AccountAddress> = HashMap::new();

    // Sort packages by version so higher versions load last and override.
    let mut packages_sorted: Vec<(&String, &Vec<(String, String)>)> =
        cached.packages.iter().collect();
    packages_sorted.sort_by(|a, b| {
        let ver_a = historical_versions.get(a.0).copied().unwrap_or(1);
        let ver_b = historical_versions.get(b.0).copied().unwrap_or(1);
        ver_a.cmp(&ver_b)
    });

    for (pkg_id, modules) in packages_sorted {
        let decoded_modules: Vec<(String, Vec<u8>)> = modules
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        if let Ok((count, Some(bytecode_addr))) =
            resolver.add_package_modules_at(decoded_modules, None)
        {
            if count > 0 {
                pkg_id_to_bytecode_addr.insert(pkg_id.clone(), bytecode_addr);
            }
        }
    }

    // Alias from fetched package ID -> bytecode address
    for (pkg_id, bytecode_addr) in &pkg_id_to_bytecode_addr {
        let pkg_addr = match AccountAddress::from_hex_literal(pkg_id) {
            Ok(addr) => addr,
            Err(_) => continue,
        };

        if pkg_addr != *bytecode_addr {
            resolver.add_address_alias(pkg_addr, *bytecode_addr);
        }
    }

    // Linkage upgrade aliases (original -> upgraded bytecode address)
    for (original_id, upgraded_id) in linkage_upgrades {
        let original_norm = normalize_address(original_id);
        let upgraded_norm = normalize_address(upgraded_id);

        let original_addr = match AccountAddress::from_hex_literal(&original_norm) {
            Ok(addr) => addr,
            Err(_) => continue,
        };

        let bytecode_addr = pkg_id_to_bytecode_addr
            .get(&upgraded_norm)
            .or_else(|| pkg_id_to_bytecode_addr.get(upgraded_id));

        if let Some(&bytecode_addr) = bytecode_addr {
            resolver.add_address_alias(original_addr, bytecode_addr);
        }
    }

    resolver.load_sui_framework()?;

    Ok(resolver)
}

/// Build type-rewrite aliases (upgraded -> original) for dynamic field decoding.
fn build_type_rewrite_aliases(
    linkage_upgrades: &HashMap<String, String>,
) -> Arc<HashMap<AccountAddress, AccountAddress>> {
    let type_rewrite_aliases: HashMap<AccountAddress, AccountAddress> = linkage_upgrades
        .iter()
        .filter_map(|(original, upgraded)| {
            let original_norm = normalize_address(original);
            let upgraded_norm = normalize_address(upgraded);
            let original_addr = AccountAddress::from_hex_literal(&original_norm).ok()?;
            let upgraded_addr = AccountAddress::from_hex_literal(&upgraded_norm).ok()?;
            Some((upgraded_addr, original_addr))
        })
        .collect();
    Arc::new(type_rewrite_aliases)
}

/// Inner execute replay function (without retry logic).
#[allow(clippy::too_many_arguments)]
fn execute_replay_inner(
    rt: &tokio::runtime::Runtime,
    grpc: &Arc<GrpcClient>,
    grpc_tx: &GrpcTransaction,
    objects: HashMap<String, String>,
    object_types: HashMap<String, String>,
    packages: HashMap<String, Vec<(String, String)>>,
    historical_versions: HashMap<String, u64>,
    linkage_upgrades: HashMap<String, String>,
    shared_cache: Option<&Arc<SharedObjectCache>>,
    onchain_success: bool,
    objects_fetched: usize,
    packages_fetched: usize,
    dynamic_fields_prefetched: usize,
) -> Result<TransactionResult> {
    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Build CachedTransaction
    // =========================================================================
    let fetched_tx = grpc_to_fetched_transaction(grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    for (pkg_id, modules) in &packages {
        cached.packages.insert(pkg_id.clone(), modules.clone());
    }
    cached.objects = objects;
    cached.object_types = object_types;
    cached.object_versions = historical_versions.clone();

    // =========================================================================
    // Build module resolver
    // =========================================================================
    let resolver = build_module_resolver(&cached, &historical_versions, &linkage_upgrades)?;

    // =========================================================================
    // Create VM harness
    // =========================================================================
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let mut config = build_replay_config_for_tx(rt, grpc, grpc_tx, tx_timestamp_ms)?;
    config = config.with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // =========================================================================
    // Set up versioned child fetcher
    // =========================================================================
    let grpc_clone = grpc.clone();
    let historical_clone = Arc::new(historical_versions.clone());

    let type_aliases = build_type_rewrite_aliases(&linkage_upgrades);

    let cache_for_fetcher: Option<Arc<SharedObjectCache>> = shared_cache.cloned();

    let fetch_runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create fetch runtime"),
    );
    let fetch_runtime_clone = fetch_runtime.clone();

    let child_fetcher: VersionedChildFetcherFn = Box::new(
        move |_parent_id: AccountAddress, child_id: AccountAddress| {
            let child_id_str = child_id.to_hex_literal();
            let version = historical_clone.get(&child_id_str).copied();

            let result = fetch_runtime_clone.block_on(async {
                grpc_clone
                    .get_object_at_version(&child_id_str, version)
                    .await
            });

            if let Ok(Some(obj)) = result {
                if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                    if let Some(ref cache) = cache_for_fetcher {
                        cache.add_dynamic_child(
                            &child_id_str,
                            obj.version,
                            type_str.clone(),
                            bcs.clone(),
                        );
                    }

                    if let Some(type_tag) = parse_type_tag(type_str) {
                        let rewritten = rewrite_type_tag(type_tag, &type_aliases);
                        return Some((rewritten, bcs.clone(), obj.version));
                    }
                }
            }

            None
        },
    );

    harness.set_versioned_child_fetcher(child_fetcher);

    // =========================================================================
    // Register input objects
    // =========================================================================
    for (obj_id, version) in &cached.object_versions {
        if let Ok(addr) = AccountAddress::from_hex_literal(obj_id) {
            harness.add_sui_input_object(
                addr,
                *version,
                sui_types::object::Owner::Shared {
                    initial_shared_version: sui_types::base_types::SequenceNumber::from_u64(
                        *version,
                    ),
                },
            );
        }
    }

    // =========================================================================
    // Execute replay
    // =========================================================================
    let address_aliases = sui_sandbox_core::tx_replay::build_comprehensive_address_aliases(
        &cached,
        &linkage_upgrades,
    );
    harness.set_address_aliases(address_aliases.clone());

    let result = sui_sandbox_core::tx_replay::replay_with_objects_and_aliases(
        &cached.transaction,
        &mut harness,
        &cached.objects,
        &address_aliases,
    );

    match result {
        Ok(replay_result) => {
            let local_success = replay_result.local_success;
            let outcome_matches = local_success == onchain_success;

            Ok(TransactionResult {
                digest: grpc_tx.digest.clone(),
                onchain_success,
                local_success,
                outcome_matches,
                error: replay_result.local_error,
                objects_fetched,
                packages_fetched,
                dynamic_fields_prefetched,
            })
        }
        Err(e) => Ok(TransactionResult {
            digest: grpc_tx.digest.clone(),
            onchain_success,
            local_success: false,
            outcome_matches: !onchain_success,
            error: Some(e.to_string()),
            objects_fetched,
            packages_fetched,
            dynamic_fields_prefetched,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_replay_from_cache(
    cache: &CheckpointRangeCache,
    grpc_tx: &GrpcTransaction,
    objects: HashMap<String, String>,
    object_types: HashMap<String, String>,
    packages: HashMap<String, Vec<(String, String)>>,
    historical_versions: HashMap<String, u64>,
    linkage_upgrades: HashMap<String, String>,
    cached_children: Arc<HashMap<String, (u64, String, Vec<u8>)>>,
    onchain_success: bool,
) -> Result<TransactionResult> {
    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    let fetched_tx = grpc_to_fetched_transaction(grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    for (pkg_id, modules) in &packages {
        cached.packages.insert(pkg_id.clone(), modules.clone());
    }
    cached.objects = objects;
    cached.object_types = object_types;
    cached.object_versions = historical_versions.clone();

    let resolver = build_module_resolver(&cached, &historical_versions, &linkage_upgrades)?;

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let mut config = build_replay_config_for_tx_cached(grpc_tx, tx_timestamp_ms, cache)?;
    config = config.with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    let type_aliases = build_type_rewrite_aliases(&linkage_upgrades);
    let child_hits = Arc::new(AtomicUsize::new(0));
    let child_hits_clone = child_hits.clone();
    let cached_children_clone = cached_children.clone();

    let child_fetcher: VersionedChildFetcherFn = Box::new(
        move |_parent_id: AccountAddress, child_id: AccountAddress| {
            let child_id_str = child_id.to_hex_literal();
            let lookup = cached_children_clone.get(&child_id_str).or_else(|| {
                let normalized = normalize_address(&child_id_str);
                cached_children_clone.get(&normalized)
            });

            if let Some((version, type_str, bcs)) = lookup {
                if let Some(type_tag) = parse_type_tag(type_str) {
                    let rewritten = rewrite_type_tag(type_tag, &type_aliases);
                    child_hits_clone.fetch_add(1, Ordering::Relaxed);
                    return Some((rewritten, bcs.clone(), *version));
                }
            }

            None
        },
    );

    harness.set_versioned_child_fetcher(child_fetcher);

    for (obj_id, version) in &cached.object_versions {
        if let Ok(addr) = AccountAddress::from_hex_literal(obj_id) {
            harness.add_sui_input_object(
                addr,
                *version,
                sui_types::object::Owner::Shared {
                    initial_shared_version: sui_types::base_types::SequenceNumber::from_u64(
                        *version,
                    ),
                },
            );
        }
    }

    let address_aliases = sui_sandbox_core::tx_replay::build_comprehensive_address_aliases(
        &cached,
        &linkage_upgrades,
    );
    harness.set_address_aliases(address_aliases.clone());

    let result = sui_sandbox_core::tx_replay::replay_with_objects_and_aliases(
        &cached.transaction,
        &mut harness,
        &cached.objects,
        &address_aliases,
    );

    let dynamic_fields_prefetched = child_hits.load(Ordering::Relaxed);
    let objects_fetched = cached.objects.len();
    let packages_fetched = cached.packages.len();

    match result {
        Ok(replay_result) => {
            let local_success = replay_result.local_success;
            let outcome_matches = local_success == onchain_success;

            Ok(TransactionResult {
                digest: grpc_tx.digest.clone(),
                onchain_success,
                local_success,
                outcome_matches,
                error: replay_result.local_error,
                objects_fetched,
                packages_fetched,
                dynamic_fields_prefetched,
            })
        }
        Err(e) => Ok(TransactionResult {
            digest: grpc_tx.digest.clone(),
            onchain_success,
            local_success: false,
            outcome_matches: !onchain_success,
            error: Some(e.to_string()),
            objects_fetched,
            packages_fetched,
            dynamic_fields_prefetched,
        }),
    }
}
