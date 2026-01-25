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
//! The pipeline supports two prefetch strategies:
//!
//! 1. **Ground-Truth-First** (recommended): Uses `unchanged_loaded_runtime_objects` from
//!    transaction effects as the authoritative source for what objects to fetch. This is
//!    faster and more accurate because we know exact versions upfront.
//!
//! 2. **Legacy GraphQL-First**: Discovers dynamic field children via GraphQL, then fetches
//!    at historical versions. This is slower and may miss objects due to version mismatches.
//!
//! Use the `--compare` flag to run both strategies side-by-side and compare results.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;

use sui_prefetch::{ground_truth_prefetch_for_transaction, GroundTruthPrefetchConfig};
use sui_sandbox_core::object_runtime::VersionedChildFetcherFn;
use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::{grpc_to_fetched_transaction, CachedTransaction};
use sui_sandbox_core::utilities::bcs_scanner::{
    extract_addresses_from_bytecode_constants, extract_addresses_from_type_params,
};
use sui_sandbox_core::utilities::{
    extract_dependencies_from_bytecode, extract_package_ids_from_type, is_framework_package,
    normalize_address, parse_type_tag, rewrite_type_tag,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcInput, GrpcTransaction};

/// Prefetch strategy for data fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefetchStrategy {
    /// Ground-truth-first: Use transaction effects to determine exact objects/versions.
    #[default]
    GroundTruth,
    /// MM2 Predictive: Ground-truth + MM2 bytecode analysis for dynamic field prediction.
    MM2Predictive,
    /// Legacy GraphQL-first: Discover dynamic fields via GraphQL, then fetch at historical versions.
    LegacyGraphQL,
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

    /// Complex (non-framework) match rate.
    pub fn complex_match_rate(&self) -> f64 {
        if self.complex_total == 0 {
            return 0.0;
        }
        self.complex_matches as f64 / self.complex_total as f64
    }
}

/// Comparison results between two prefetch strategies.
#[derive(Debug)]
pub struct ComparisonResult {
    /// Stats from ground-truth-first strategy.
    pub ground_truth_stats: BatchStats,
    /// Stats from legacy GraphQL-first strategy.
    pub legacy_stats: BatchStats,
    /// Per-transaction comparison: (digest, ground_truth_match, legacy_match, both_agree)
    pub per_tx_comparison: Vec<TransactionComparison>,
}

/// Per-transaction comparison between strategies.
#[derive(Debug)]
pub struct TransactionComparison {
    pub digest: String,
    pub is_framework_only: bool,
    pub ground_truth_matches: bool,
    pub legacy_matches: bool,
    /// True if both strategies produced the same outcome (both match or both mismatch).
    pub strategies_agree: bool,
    /// Error from ground-truth strategy (if any).
    pub ground_truth_error: Option<String>,
    /// Error from legacy strategy (if any).
    pub legacy_error: Option<String>,
    /// Time spent in ground-truth prefetch (ms).
    pub ground_truth_prefetch_ms: u64,
    /// Time spent in legacy prefetch (ms).
    pub legacy_prefetch_ms: u64,
}

impl ComparisonResult {
    /// Print a summary of the comparison.
    pub fn print_summary(&self) {
        println!("\n========================================");
        println!("       PREFETCH STRATEGY COMPARISON");
        println!("========================================\n");

        // Overall stats
        println!("OVERALL MATCH RATES:");
        println!(
            "  Ground-Truth-First: {}/{} ({:.1}%)",
            self.ground_truth_stats.outcome_matches,
            self.ground_truth_stats.transactions_processed,
            self.ground_truth_stats.match_rate() * 100.0
        );
        println!(
            "  Legacy GraphQL:     {}/{} ({:.1}%)",
            self.legacy_stats.outcome_matches,
            self.legacy_stats.transactions_processed,
            self.legacy_stats.match_rate() * 100.0
        );

        // Complex transaction stats
        println!("\nCOMPLEX TRANSACTION MATCH RATES:");
        println!(
            "  Ground-Truth-First: {}/{} ({:.1}%)",
            self.ground_truth_stats.complex_matches,
            self.ground_truth_stats.complex_total,
            self.ground_truth_stats.complex_match_rate() * 100.0
        );
        println!(
            "  Legacy GraphQL:     {}/{} ({:.1}%)",
            self.legacy_stats.complex_matches,
            self.legacy_stats.complex_total,
            self.legacy_stats.complex_match_rate() * 100.0
        );

        // Timing comparison
        let gt_total_ms: u64 = self
            .per_tx_comparison
            .iter()
            .map(|c| c.ground_truth_prefetch_ms)
            .sum();
        let legacy_total_ms: u64 = self
            .per_tx_comparison
            .iter()
            .map(|c| c.legacy_prefetch_ms)
            .sum();
        println!("\nPREFETCH TIMING:");
        println!("  Ground-Truth-First: {}ms total", gt_total_ms);
        println!("  Legacy GraphQL:     {}ms total", legacy_total_ms);
        if legacy_total_ms > 0 {
            println!(
                "  Speedup:            {:.1}x",
                legacy_total_ms as f64 / gt_total_ms.max(1) as f64
            );
        }

        // Strategy agreement
        let agree_count = self
            .per_tx_comparison
            .iter()
            .filter(|c| c.strategies_agree)
            .count();
        let gt_better = self
            .per_tx_comparison
            .iter()
            .filter(|c| !c.strategies_agree && c.ground_truth_matches && !c.legacy_matches)
            .count();
        let legacy_better = self
            .per_tx_comparison
            .iter()
            .filter(|c| !c.strategies_agree && !c.ground_truth_matches && c.legacy_matches)
            .count();

        println!("\nSTRATEGY AGREEMENT:");
        println!(
            "  Both agree:             {}/{} ({:.1}%)",
            agree_count,
            self.per_tx_comparison.len(),
            (agree_count as f64 / self.per_tx_comparison.len().max(1) as f64) * 100.0
        );
        println!("  Ground-Truth wins:      {}", gt_better);
        println!("  Legacy wins:            {}", legacy_better);

        // Show transactions where ground-truth won (most interesting)
        if gt_better > 0 {
            println!("\nTRANSACTIONS WHERE GROUND-TRUTH WON:");
            for cmp in self
                .per_tx_comparison
                .iter()
                .filter(|c| !c.strategies_agree && c.ground_truth_matches && !c.legacy_matches)
                .take(5)
            {
                let category = if cmp.is_framework_only {
                    "framework"
                } else {
                    "complex"
                };
                println!("  {} [{}]", &cmp.digest[..16], category);
                if let Some(err) = &cmp.legacy_error {
                    println!("    Legacy error: {}", truncate_error(err, 80));
                }
            }
        }

        // Show transactions where legacy won (potential regressions to investigate)
        if legacy_better > 0 {
            println!("\nTRANSACTIONS WHERE LEGACY WON (POTENTIAL REGRESSIONS):");
            for cmp in self
                .per_tx_comparison
                .iter()
                .filter(|c| !c.strategies_agree && !c.ground_truth_matches && c.legacy_matches)
                .take(5)
            {
                let category = if cmp.is_framework_only {
                    "framework"
                } else {
                    "complex"
                };
                println!("  {} [{}]", &cmp.digest[..16], category);
                if let Some(err) = &cmp.ground_truth_error {
                    println!("    Ground-truth error: {}", truncate_error(err, 80));
                }
            }
        }

        println!("\n========================================\n");
    }
}

/// Helper to truncate error messages.
fn truncate_error(err: &str, max_len: usize) -> String {
    if err.len() <= max_len {
        err.to_string()
    } else {
        format!("{}...", &err[..max_len])
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
    /// Time spent in prefetch phase (ms).
    prefetch_time_ms: u64,
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

    /// Run comparison mode: execute both prefetch strategies side-by-side.
    ///
    /// This is useful for validating the ground-truth-first implementation
    /// against the legacy GraphQL-first approach.
    pub async fn run_comparison(
        start_checkpoint: u64,
        num_checkpoints: u64,
    ) -> Result<ComparisonResult> {
        let result = tokio::task::spawn_blocking(move || {
            run_comparison_batch(start_checkpoint, num_checkpoints)
        })
        .await??;

        Ok(result)
    }
}

use crate::cache::{CachedCheckpoint, CachedObject, CheckpointRangeCache};
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

    fn add_package(&self, pkg_id: &str, version: u64, modules: Vec<(String, Vec<u8>)>) {
        let key = format!("{}:{}", pkg_id, version);
        let cached = CachedObject {
            object_id: pkg_id.to_string(),
            version,
            type_string: None,
            bcs: None,
            package_modules: Some(modules),
            package_linkage: None,
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

/// Process a single transaction from cache (no network fetching).
fn process_single_transaction_from_cache(
    _cache: &CheckpointRangeCache,
    grpc_tx: &GrpcTransaction,
    _quiet_mode: bool,
) -> Result<TransactionResult> {
    let _onchain_success = grpc_tx
        .status
        .as_ref()
        .map(|s| s == "success")
        .unwrap_or(false);

    // For now, just return a placeholder - we need to implement the full replay
    // This is simplified since we need to collect all necessary data from cache

    // TODO: Implement full replay from cache
    // For now, indicate that we need to fetch data to populate cache first
    Err(anyhow::anyhow!(
        "Cache replay not fully implemented - run with --fetch first to populate cache with object data"
    ))
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
        PrefetchStrategy::LegacyGraphQL => {
            process_with_legacy_prefetch(rt, grpc, graphql, grpc_tx, shared_cache, onchain_success)
        }
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
    let prefetch_start = Instant::now();

    // Use the new ground-truth-first prefetch
    let config = GroundTruthPrefetchConfig::default();
    let prefetch_result =
        ground_truth_prefetch_for_transaction(grpc, Some(graphql), rt, grpc_tx, &config);

    let prefetch_time_ms = prefetch_start.elapsed().as_millis() as u64;

    // Convert prefetch result to the format expected by replay
    let mut objects: HashMap<String, String> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut packages: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut historical_versions: HashMap<String, u64> = HashMap::new();
    let mut linkage_upgrades: HashMap<String, String> =
        prefetch_result.discovered_linkage_upgrades.clone();

    // Process objects from ground truth
    for (obj_id, obj) in &prefetch_result.objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs);
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
                Some(obj.bcs.clone()),
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
            cache.add_package(pkg_id, pkg.version, pkg.modules.clone());
        }

        // Accumulate linkage
        for (orig, upgraded) in &pkg.linkage {
            linkage_upgrades.insert(orig.clone(), upgraded.clone());
        }
    }

    // Process supplemental objects (if any)
    for (obj_id, obj) in &prefetch_result.supplemental_objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs);
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
        prefetch_time_ms,
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
    let prefetch_start = Instant::now();

    // Use the predictive prefetcher with MM2 analysis
    let mut prefetcher = PredictivePrefetcher::new();
    let config = PredictivePrefetchConfig::default();
    let prefetch_result =
        prefetcher.prefetch_for_transaction(grpc, Some(graphql), rt, grpc_tx, &config);

    let prefetch_time_ms = prefetch_start.elapsed().as_millis() as u64;

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
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs);
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
                Some(obj.bcs.clone()),
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
            cache.add_package(pkg_id, pkg.version, pkg.modules.clone());
        }

        // Accumulate linkage
        for (orig, upgraded) in &pkg.linkage {
            linkage_upgrades.insert(orig.clone(), upgraded.clone());
        }
    }

    // Process supplemental objects (if any)
    for (obj_id, obj) in &prefetch_result.base_result.supplemental_objects {
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs);
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
        prefetch_time_ms,
    )
}

/// Process transaction using legacy GraphQL-first prefetch strategy.
fn process_with_legacy_prefetch(
    rt: &tokio::runtime::Runtime,
    grpc: &Arc<GrpcClient>,
    graphql: &GraphQLClient,
    grpc_tx: &GrpcTransaction,
    shared_cache: Option<&Arc<SharedObjectCache>>,
    onchain_success: bool,
) -> Result<TransactionResult> {
    let prefetch_start = Instant::now();

    // =========================================================================
    // Collect historical object versions
    // =========================================================================
    let mut historical_versions: HashMap<String, u64> = HashMap::new();

    // From unchanged_loaded_runtime_objects
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // From changed_objects (INPUT version)
    for (id, ver) in &grpc_tx.changed_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // From unchanged_consensus_objects
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // From transaction inputs
    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*initial_version);
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*version);
            }
            GrpcInput::Pure { .. } => {}
        }
    }

    // =========================================================================
    // Prefetch dynamic fields
    // =========================================================================
    let mut prefetched = sui_prefetch::prefetch_dynamic_fields(
        graphql,
        grpc,
        rt,
        &historical_versions,
        6,   // max_depth (increased for deeply nested structures like DeepBook history)
        200, // max_fields_per_object (increased for larger tables)
    );

    // Also prefetch epoch-keyed dynamic fields for DeepBook's historical data
    // This is essential for functions like `history::historic_maker_fee`
    let tx_epoch = grpc_tx.epoch.unwrap_or(0);

    // Debug summary (minimal)
    if prefetched
        .children_by_key
        .keys()
        .any(|k| k.name_type == "u64")
    {
        eprintln!(
            "[EPOCH] Transaction {} epoch={}, found u64-keyed fields",
            &grpc_tx.digest[..16],
            tx_epoch
        );
    }

    let epoch_fields_fetched = sui_prefetch::prefetch_epoch_keyed_fields(
        graphql,
        grpc,
        rt,
        &mut prefetched,
        tx_epoch,
        10, // lookback 10 epochs to cover historical fee lookups
    );

    for (child_id, (version, type_str, bcs)) in &prefetched.children {
        historical_versions
            .entry(child_id.clone())
            .or_insert(*version);

        // Store dynamic field children to shared cache
        if let Some(cache) = shared_cache {
            cache.add_dynamic_child(child_id, *version, type_str.clone(), bcs.clone());
        }
    }

    let dynamic_fields_prefetched = prefetched.fetched_count + epoch_fields_fetched;

    // =========================================================================
    // Fetch objects and packages
    // =========================================================================
    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    let mut objects: HashMap<String, String> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut packages: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut package_ids_to_fetch: HashSet<String> = HashSet::new();

    // Extract package IDs from commands (including type arguments)
    for cmd in &grpc_tx.commands {
        if let sui_transport::grpc::GrpcCommand::MoveCall {
            package,
            type_arguments,
            ..
        } = cmd
        {
            package_ids_to_fetch.insert(package.clone());
            // Also extract packages from type arguments (e.g., 0xabc::coin::COIN)
            for type_arg in type_arguments {
                for pkg_id in extract_package_ids_from_type(type_arg) {
                    package_ids_to_fetch.insert(pkg_id);
                }
            }
        }
        // Also handle MakeMoveVec element_type
        if let sui_transport::grpc::GrpcCommand::MakeMoveVec {
            element_type: Some(elem_type),
            ..
        } = cmd
        {
            for pkg_id in extract_package_ids_from_type(elem_type) {
                package_ids_to_fetch.insert(pkg_id);
            }
        }
    }

    // Also extract packages from unchanged_loaded_runtime_objects - these include
    // packages that were dynamically accessed during transaction execution
    for (obj_id, _version) in &grpc_tx.unchanged_loaded_runtime_objects {
        // If the object ID looks like a package (64-char hex), add it
        let normalized = normalize_address(obj_id);
        // Try to fetch as a potential package
        package_ids_to_fetch.insert(normalized);
    }

    // Fetch objects
    for (obj_id, version) in &historical_versions {
        let result =
            rt.block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await });

        if let Ok(Some(obj)) = result {
            if let Some(bcs) = &obj.bcs {
                let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(bcs);
                objects.insert(obj_id.clone(), bcs_b64);

                // Store to shared cache for later disk persistence
                if let Some(cache) = shared_cache {
                    cache.add_object(obj_id, *version, obj.type_string.clone(), Some(bcs.clone()));
                }

                if let Some(type_str) = &obj.type_string {
                    object_types.insert(obj_id.clone(), type_str.clone());
                    for pkg_id in extract_package_ids_from_type(type_str) {
                        package_ids_to_fetch.insert(pkg_id);
                    }
                }

                if let Some(modules) = &obj.package_modules {
                    let modules_b64: Vec<(String, String)> = modules
                        .iter()
                        .map(|(name, bytes)| {
                            (
                                name.clone(),
                                base64::engine::general_purpose::STANDARD.encode(bytes),
                            )
                        })
                        .collect();
                    packages.insert(obj_id.clone(), modules_b64);
                    package_ids_to_fetch.remove(obj_id);

                    // Store package to shared cache
                    if let Some(cache) = shared_cache {
                        cache.add_package(obj_id, *version, modules.clone());
                    }
                }
            }
        }
    }

    // =========================================================================
    // Extract additional package addresses from type strings
    // =========================================================================
    // Type strings reliably contain package addresses (e.g., Pool<0xabc::coin::COIN>)
    // This is more precise than scanning raw BCS bytes for 32-byte sequences.
    for type_str in object_types.values() {
        let type_addrs = extract_addresses_from_type_params(type_str);
        for addr in type_addrs {
            package_ids_to_fetch.insert(addr);
        }
    }

    // Note: We intentionally don't scan raw BCS bytes here because:
    // 1. It generates many false positives (random 32-byte sequences)
    // 2. Type strings and linkage tables already capture most package references
    // 3. The BcsAddressScanner is available for targeted use cases where needed

    // =========================================================================
    // Fetch packages with transitive dependencies
    // =========================================================================
    let mut fetched_packages: HashSet<String> = HashSet::new();
    let mut packages_to_fetch = package_ids_to_fetch.clone();
    let mut linkage_upgrades: HashMap<String, String> = HashMap::new();
    // Track package_id -> version from linkage tables
    let mut linkage_versions: HashMap<String, u64> = HashMap::new();

    // Resolve transitive dependencies with a reasonable depth limit
    // Most real-world package graphs have depth < 20
    const MAX_DEPENDENCY_DEPTH: usize = 25;
    for depth in 0..MAX_DEPENDENCY_DEPTH {
        if packages_to_fetch.is_empty() {
            break;
        }

        // Warn if we're getting deep into the dependency graph
        if depth == MAX_DEPENDENCY_DEPTH - 1 && !packages_to_fetch.is_empty() {
            eprintln!(
                "WARNING: Reached max dependency depth ({}), {} packages may be missing",
                MAX_DEPENDENCY_DEPTH,
                packages_to_fetch.len()
            );
        }

        let mut new_deps: HashSet<String> = HashSet::new();

        for pkg_id in packages_to_fetch.iter() {
            let pkg_id_normalized = normalize_address(pkg_id);
            if fetched_packages.contains(&pkg_id_normalized) {
                continue;
            }

            // Try linkage version first, then historical_versions, then None (latest)
            let version = linkage_versions
                .get(pkg_id)
                .copied()
                .or_else(|| historical_versions.get(pkg_id).copied());
            let result = rt.block_on(async { grpc.get_object_at_version(pkg_id, version).await });

            if let Ok(Some(obj)) = result {
                if let Some(modules) = &obj.package_modules {
                    let modules_b64: Vec<(String, String)> = modules
                        .iter()
                        .map(|(name, bytes)| {
                            (
                                name.clone(),
                                base64::engine::general_purpose::STANDARD.encode(bytes),
                            )
                        })
                        .collect();

                    if let Some(linkage) = &obj.package_linkage {
                        for l in linkage {
                            if is_framework_package(&l.original_id) {
                                continue;
                            }

                            let orig_normalized = normalize_address(&l.original_id);
                            let upgraded_normalized = normalize_address(&l.upgraded_id);
                            if orig_normalized != upgraded_normalized {
                                linkage_upgrades
                                    .insert(orig_normalized.clone(), upgraded_normalized.clone());
                                // Track the version from linkage table
                                linkage_versions
                                    .insert(upgraded_normalized.clone(), l.upgraded_version);
                                if !fetched_packages.contains(&upgraded_normalized)
                                    && !packages.contains_key(&upgraded_normalized)
                                {
                                    new_deps.insert(upgraded_normalized);
                                }
                            }
                        }
                    }

                    for (_name, bytecode_b64) in &modules_b64 {
                        if let Ok(bytecode) =
                            base64::engine::general_purpose::STANDARD.decode(bytecode_b64)
                        {
                            // Extract dependencies from module handles
                            let deps = extract_dependencies_from_bytecode(&bytecode);
                            for dep in deps {
                                let dep_normalized = normalize_address(&dep);
                                let actual_dep = linkage_upgrades
                                    .get(&dep_normalized)
                                    .cloned()
                                    .unwrap_or(dep_normalized);
                                if !fetched_packages.contains(&actual_dep)
                                    && !packages.contains_key(&actual_dep)
                                {
                                    new_deps.insert(actual_dep);
                                }
                            }

                            // Also extract addresses from bytecode constants
                            // This catches dynamically-referenced packages stored as constants
                            let const_addrs = extract_addresses_from_bytecode_constants(&bytecode);
                            for addr in const_addrs {
                                if !fetched_packages.contains(&addr)
                                    && !packages.contains_key(&addr)
                                {
                                    new_deps.insert(addr);
                                }
                            }
                        }
                    }

                    packages.insert(pkg_id_normalized.clone(), modules_b64);
                    fetched_packages.insert(pkg_id_normalized.clone());

                    // Store package to shared cache
                    if let Some(cache) = shared_cache {
                        cache.add_package(&pkg_id_normalized, obj.version, modules.clone());
                    }
                }
            } else {
                fetched_packages.insert(pkg_id_normalized);
            }
        }

        packages_to_fetch = new_deps;
    }

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

    let objects_fetched = cached.objects.len();
    let packages_fetched = cached.packages.len();

    // =========================================================================
    // Build module resolver
    // =========================================================================
    let mut resolver = LocalModuleResolver::new();

    // First pass: load all packages at their native addresses (from bytecode)
    // Also track: package_id -> bytecode_address mappings for alias setup
    let mut pkg_id_to_bytecode_addr: HashMap<String, AccountAddress> = HashMap::new();

    // CRITICAL: Sort packages by version (ascending) so that higher versions load last and overwrite.
    // This ensures that for package upgrades where both v1 (original) and vN (upgrade) share the
    // same bytecode address, the newer version's bytecode is used.
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

        // add_package_modules_at returns the bytecode address (address in bytecode, not package ID)
        if let Ok((count, Some(bytecode_addr))) =
            resolver.add_package_modules_at(decoded_modules, None)
        {
            if count > 0 {
                pkg_id_to_bytecode_addr.insert(pkg_id.clone(), bytecode_addr);
            }
        }
    }

    // Second pass: set up aliases
    // 1. Alias from fetched package ID -> bytecode address (for upgraded packages)
    // 2. Alias from linkage original -> upgraded bytecode address
    for (pkg_id, bytecode_addr) in &pkg_id_to_bytecode_addr {
        let pkg_addr = match AccountAddress::from_hex_literal(pkg_id) {
            Ok(addr) => addr,
            Err(_) => continue,
        };

        // If package ID differs from bytecode address, set up alias
        if pkg_addr != *bytecode_addr {
            // Alias: pkg_id -> bytecode_addr (so lookups at pkg_id find modules at bytecode_addr)
            resolver.add_address_alias(pkg_addr, *bytecode_addr);
        }
    }

    // Also set up linkage upgrade aliases
    for (original_id, upgraded_id) in &linkage_upgrades {
        let original_addr = match AccountAddress::from_hex_literal(&format!("0x{}", original_id)) {
            Ok(addr) => addr,
            Err(_) => continue,
        };

        // Find the bytecode address for the upgraded package
        let bytecode_addr = pkg_id_to_bytecode_addr
            .get(&format!("0x{}", upgraded_id))
            .or_else(|| pkg_id_to_bytecode_addr.get(upgraded_id));

        if let Some(&bytecode_addr) = bytecode_addr {
            // Alias: original -> bytecode_addr of upgraded package
            resolver.add_address_alias(original_addr, bytecode_addr);
        }
    }

    resolver.load_sui_framework()?;

    // =========================================================================
    // Create VM harness
    // =========================================================================
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    // Use with_tx_timestamp for frozen clock behavior (required for replay)
    // and set the epoch from the transaction metadata
    let tx_epoch = grpc_tx.epoch.unwrap_or(0);
    let config = SimulationConfig::default()
        .with_tx_timestamp(tx_timestamp_ms)
        .with_epoch(tx_epoch)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // =========================================================================
    // Set up versioned child fetcher (for proper transaction replay)
    // =========================================================================
    let prefetched_children = Arc::new(prefetched.children.clone());
    let grpc_clone = grpc.clone();
    let historical_clone = Arc::new(historical_versions.clone());

    // Build address aliases for type rewriting in child fetcher
    // Map: upgraded_address -> original_address
    let type_rewrite_aliases: HashMap<AccountAddress, AccountAddress> = linkage_upgrades
        .iter()
        .filter_map(|(original, upgraded)| {
            let original_norm = normalize_address(original);
            let upgraded_norm = normalize_address(upgraded);
            let original_addr =
                AccountAddress::from_hex_literal(&format!("0x{}", original_norm)).ok()?;
            let upgraded_addr =
                AccountAddress::from_hex_literal(&format!("0x{}", upgraded_norm)).ok()?;
            Some((upgraded_addr, original_addr))
        })
        .collect();
    let type_aliases = Arc::new(type_rewrite_aliases);

    // Clone shared cache for use in the child fetcher closure
    let cache_for_fetcher: Option<Arc<SharedObjectCache>> = shared_cache.cloned();

    // Create a dedicated runtime for on-demand fetching (reused across all calls)
    // This is much more efficient than creating a new runtime for each fetch
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

            // Check prefetched cache first (contains version info)
            if let Some((version, type_str, bcs)) = prefetched_children.get(&child_id_str) {
                if let Some(type_tag) = parse_type_tag(type_str) {
                    // Rewrite type addresses using linkage upgrade mappings
                    let rewritten = rewrite_type_tag(type_tag, &type_aliases);
                    return Some((rewritten, bcs.clone(), *version));
                }
            }

            // Fallback: fetch on-demand using the shared runtime
            let version = historical_clone.get(&child_id_str).copied();

            let result = fetch_runtime_clone.block_on(async {
                grpc_clone
                    .get_object_at_version(&child_id_str, version)
                    .await
            });

            if let Ok(Some(obj)) = result {
                if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                    // Store to shared cache for later disk persistence
                    if let Some(ref cache) = cache_for_fetcher {
                        cache.add_dynamic_child(
                            &child_id_str,
                            obj.version,
                            type_str.clone(),
                            bcs.clone(),
                        );
                    }

                    if let Some(type_tag) = parse_type_tag(type_str) {
                        // Rewrite type addresses using linkage upgrade mappings
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
    for (obj_id, version) in &historical_versions {
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
    // Build comprehensive address aliases from both bytecode and linkage information
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

    let prefetch_time_ms = prefetch_start.elapsed().as_millis() as u64;

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
                prefetch_time_ms,
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
            prefetch_time_ms,
        }),
    }
}

/// Execute replay with on-demand package fetching on LINKER_ERROR.
/// Wraps execute_replay_inner and retries up to MAX_PACKAGE_RETRIES times.
#[allow(clippy::too_many_arguments)]
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
    prefetch_time_ms: u64,
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
            prefetch_time_ms,
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
        prefetch_time_ms,
    )
}

/// Inner execute replay function (without retry logic).
#[allow(clippy::too_many_arguments)]
fn execute_replay_inner(
    _rt: &tokio::runtime::Runtime,
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
    prefetch_time_ms: u64,
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
    let mut resolver = LocalModuleResolver::new();

    // First pass: load all packages at their native addresses (from bytecode)
    // Also track: package_id -> bytecode_address mappings for alias setup
    let mut pkg_id_to_bytecode_addr: HashMap<String, AccountAddress> = HashMap::new();

    // CRITICAL: Sort packages by version (ascending) so that higher versions load last and overwrite.
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

        // add_package_modules_at returns the bytecode address (address in bytecode, not package ID)
        if let Ok((count, Some(bytecode_addr))) =
            resolver.add_package_modules_at(decoded_modules, None)
        {
            if count > 0 {
                pkg_id_to_bytecode_addr.insert(pkg_id.clone(), bytecode_addr);
            }
        }
    }

    // Second pass: set up aliases
    // 1. Alias from fetched package ID -> bytecode address (for upgraded packages)
    // 2. Alias from linkage original -> upgraded bytecode address
    for (pkg_id, bytecode_addr) in &pkg_id_to_bytecode_addr {
        let pkg_addr = match AccountAddress::from_hex_literal(pkg_id) {
            Ok(addr) => addr,
            Err(_) => continue,
        };

        // If package ID differs from bytecode address, set up alias
        if pkg_addr != *bytecode_addr {
            // Alias: pkg_id -> bytecode_addr (so lookups at pkg_id find modules at bytecode_addr)
            resolver.add_address_alias(pkg_addr, *bytecode_addr);
        }
    }

    // Also set up linkage upgrade aliases
    for (original_id, upgraded_id) in &linkage_upgrades {
        let original_addr = match AccountAddress::from_hex_literal(&format!("0x{}", original_id)) {
            Ok(addr) => addr,
            Err(_) => continue,
        };

        // Find the bytecode address for the upgraded package
        let bytecode_addr = pkg_id_to_bytecode_addr
            .get(&format!("0x{}", upgraded_id))
            .or_else(|| pkg_id_to_bytecode_addr.get(upgraded_id));

        if let Some(&bytecode_addr) = bytecode_addr {
            // Alias: original -> bytecode_addr of upgraded package
            resolver.add_address_alias(original_addr, bytecode_addr);
        }
    }

    resolver.load_sui_framework()?;

    // =========================================================================
    // Create VM harness
    // =========================================================================
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let tx_epoch = grpc_tx.epoch.unwrap_or(0);
    let config = SimulationConfig::default()
        .with_tx_timestamp(tx_timestamp_ms)
        .with_epoch(tx_epoch)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // =========================================================================
    // Set up versioned child fetcher
    // =========================================================================
    let grpc_clone = grpc.clone();
    let historical_clone = Arc::new(historical_versions.clone());

    let type_rewrite_aliases: HashMap<AccountAddress, AccountAddress> = linkage_upgrades
        .iter()
        .filter_map(|(original, upgraded)| {
            let original_norm = normalize_address(original);
            let upgraded_norm = normalize_address(upgraded);
            let original_addr =
                AccountAddress::from_hex_literal(&format!("0x{}", original_norm)).ok()?;
            let upgraded_addr =
                AccountAddress::from_hex_literal(&format!("0x{}", upgraded_norm)).ok()?;
            Some((upgraded_addr, original_addr))
        })
        .collect();
    let type_aliases = Arc::new(type_rewrite_aliases);

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
                prefetch_time_ms,
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
            prefetch_time_ms,
        }),
    }
}

/// Run comparison between both prefetch strategies.
fn run_comparison_batch(start_checkpoint: u64, num_checkpoints: u64) -> Result<ComparisonResult> {
    let rt = tokio::runtime::Runtime::new()?;
    let end_checkpoint = start_checkpoint + num_checkpoints - 1;

    println!("\n========================================");
    println!("   PREFETCH STRATEGY COMPARISON MODE");
    println!("========================================\n");

    // Connect to services
    println!("Step 1: Connecting to services...");
    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let grpc = Arc::new(grpc);
    let graphql = GraphQLClient::mainnet();

    println!("   Connected to {}", endpoint);

    // Fetch transactions
    println!(
        "\nStep 2: Fetching transactions from checkpoints {}..{}",
        start_checkpoint, end_checkpoint
    );

    let mut all_transactions: Vec<GrpcTransaction> = Vec::new();

    for cp_num in start_checkpoint..=end_checkpoint {
        match rt.block_on(async { grpc.get_checkpoint(cp_num).await }) {
            Ok(Some(checkpoint)) => {
                let ptb_txs: Vec<GrpcTransaction> = checkpoint
                    .transactions
                    .iter()
                    .filter(|tx| tx.is_ptb())
                    .cloned()
                    .collect();

                println!(
                    "   Checkpoint {}: {} total txs, {} PTBs",
                    cp_num,
                    checkpoint.transactions.len(),
                    ptb_txs.len()
                );

                all_transactions.extend(ptb_txs);
            }
            Ok(None) => {
                println!("   Checkpoint {}: not found", cp_num);
            }
            Err(e) => {
                println!("   Checkpoint {}: error - {}", cp_num, e);
            }
        }
    }

    println!("\n   Total PTB transactions: {}", all_transactions.len());

    // Run comparison for each transaction
    println!("\nStep 3: Running side-by-side comparison...\n");

    let mut ground_truth_stats = BatchStats::default();
    let mut legacy_stats = BatchStats::default();
    let mut per_tx_comparison: Vec<TransactionComparison> = Vec::new();

    ground_truth_stats.prefetch_strategy = Some(PrefetchStrategy::GroundTruth);
    legacy_stats.prefetch_strategy = Some(PrefetchStrategy::LegacyGraphQL);

    for (idx, grpc_tx) in all_transactions.iter().enumerate() {
        if (idx + 1) % 5 == 0 || idx == 0 {
            print!(
                "\r   Comparing transaction {}/{}...",
                idx + 1,
                all_transactions.len()
            );
            std::io::Write::flush(&mut std::io::stdout())?;
        }

        // Determine if framework-only
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

        // Run ground-truth strategy
        let gt_result = process_single_transaction(
            &rt,
            &grpc,
            &graphql,
            grpc_tx,
            None,
            PrefetchStrategy::GroundTruth,
        );

        // Run legacy strategy
        let legacy_result = process_single_transaction(
            &rt,
            &grpc,
            &graphql,
            grpc_tx,
            None,
            PrefetchStrategy::LegacyGraphQL,
        );

        // Record results
        let (gt_matches, gt_error, gt_prefetch_ms) = match &gt_result {
            Ok(r) => {
                ground_truth_stats.transactions_processed += 1;
                ground_truth_stats.total_objects_fetched += r.objects_fetched;
                ground_truth_stats.total_packages_fetched += r.packages_fetched;

                if is_framework_only {
                    ground_truth_stats.framework_total += 1;
                    if r.outcome_matches {
                        ground_truth_stats.framework_matches += 1;
                    }
                } else {
                    ground_truth_stats.complex_total += 1;
                    if r.outcome_matches {
                        ground_truth_stats.complex_matches += 1;
                    }
                }

                if r.outcome_matches {
                    ground_truth_stats.outcome_matches += 1;
                }
                if r.local_success {
                    ground_truth_stats.successful_replays += 1;
                } else {
                    ground_truth_stats.failed_replays += 1;
                }

                (r.outcome_matches, r.error.clone(), r.prefetch_time_ms)
            }
            Err(e) => {
                ground_truth_stats.skipped_fetch_errors += 1;
                (false, Some(e.to_string()), 0)
            }
        };

        let (legacy_matches, legacy_error, legacy_prefetch_ms) = match &legacy_result {
            Ok(r) => {
                legacy_stats.transactions_processed += 1;
                legacy_stats.total_objects_fetched += r.objects_fetched;
                legacy_stats.total_packages_fetched += r.packages_fetched;

                if is_framework_only {
                    legacy_stats.framework_total += 1;
                    if r.outcome_matches {
                        legacy_stats.framework_matches += 1;
                    }
                } else {
                    legacy_stats.complex_total += 1;
                    if r.outcome_matches {
                        legacy_stats.complex_matches += 1;
                    }
                }

                if r.outcome_matches {
                    legacy_stats.outcome_matches += 1;
                }
                if r.local_success {
                    legacy_stats.successful_replays += 1;
                } else {
                    legacy_stats.failed_replays += 1;
                }

                (r.outcome_matches, r.error.clone(), r.prefetch_time_ms)
            }
            Err(e) => {
                legacy_stats.skipped_fetch_errors += 1;
                (false, Some(e.to_string()), 0)
            }
        };

        let strategies_agree = gt_matches == legacy_matches;

        // Log comparison
        let gt_status = if gt_matches { "✓" } else { "✗" };
        let legacy_status = if legacy_matches { "✓" } else { "✗" };
        let agree_str = if strategies_agree { "=" } else { "≠" };
        let category = if is_framework_only { "fw" } else { "cx" };

        eprintln!(
            "   {} GT:{} Legacy:{} [{}] {}",
            agree_str,
            gt_status,
            legacy_status,
            category,
            &grpc_tx.digest[..16]
        );

        per_tx_comparison.push(TransactionComparison {
            digest: grpc_tx.digest.clone(),
            is_framework_only,
            ground_truth_matches: gt_matches,
            legacy_matches,
            strategies_agree,
            ground_truth_error: gt_error,
            legacy_error,
            ground_truth_prefetch_ms: gt_prefetch_ms,
            legacy_prefetch_ms,
        });
    }

    println!(
        "\r   Compared {}/{} transactions.        ",
        all_transactions.len(),
        all_transactions.len()
    );

    Ok(ComparisonResult {
        ground_truth_stats,
        legacy_stats,
        per_tx_comparison,
    })
}
