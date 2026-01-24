//! Batch processing pipeline for PTB replay from static checkpoint range.
//!
//! This module fetches transactions from specific checkpoints and replays them locally.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;

use sui_data_fetcher::graphql::GraphQLClient;
use sui_data_fetcher::grpc::{GrpcClient, GrpcInput, GrpcTransaction};
use sui_sandbox_core::object_runtime::VersionedChildFetcherFn;
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
    /// Detailed mismatch info: (digest, local_success, onchain_success, error)
    pub mismatches: Vec<(String, bool, bool, Option<String>)>,
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
            run_checkpoint_batch(start_checkpoint, num_checkpoints, false, false)
        })
        .await??;

        Ok(stats)
    }

    /// Run the pipeline with cache support.
    ///
    /// If `fetch_mode` is true, will fetch all data and save to cache.
    /// If `fetch_mode` is false, will try to load from cache first.
    pub async fn run_checkpoints_with_cache(
        start_checkpoint: u64,
        num_checkpoints: u64,
        fetch_mode: bool,
        quiet_mode: bool,
    ) -> Result<BatchStats> {
        let stats = tokio::task::spawn_blocking(move || {
            run_checkpoint_batch(start_checkpoint, num_checkpoints, fetch_mode, quiet_mode)
        })
        .await??;

        Ok(stats)
    }
}

use crate::cache::{CachedCheckpoint, CheckpointRangeCache};

fn run_checkpoint_batch(
    start_checkpoint: u64,
    num_checkpoints: u64,
    fetch_mode: bool,
    quiet_mode: bool,
) -> Result<BatchStats> {
    let rt = tokio::runtime::Runtime::new()?;
    let mut stats = BatchStats::default();
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
        .or_else(|_| std::env::var("SURFLUX_GRPC_ENDPOINT"))
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());

    let api_key = std::env::var("SUI_GRPC_API_KEY")
        .or_else(|_| std::env::var("SURFLUX_API_KEY"))
        .ok();

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

        match process_single_transaction(&rt, &grpc, &graphql, grpc_tx) {
            Ok(result) => {
                stats.transactions_processed += 1;
                stats.total_objects_fetched += result.objects_fetched;
                stats.total_packages_fetched += result.packages_fetched;
                stats.dynamic_fields_resolved += result.dynamic_fields_prefetched;

                if result.outcome_matches {
                    stats.outcome_matches += 1;
                } else {
                    stats.mismatches.push((
                        result.digest.clone(),
                        result.local_success,
                        result.onchain_success,
                        result.error.clone(),
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
                eprintln!("\n   SKIP {}: {}", &grpc_tx.digest[..16], e);
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
                    stats.mismatches.push((
                        result.digest.clone(),
                        result.local_success,
                        result.onchain_success,
                        result.error.clone(),
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
) -> Result<TransactionResult> {
    let onchain_success = grpc_tx
        .status
        .as_ref()
        .map(|s| s == "success")
        .unwrap_or(false);

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
    let mut prefetched = sui_data_fetcher::utilities::prefetch_dynamic_fields(
        graphql,
        grpc,
        rt,
        &historical_versions,
        4,   // max_depth (increased from 2 for DeepBook's nested structures)
        100, // max_fields_per_object (increased from 50)
    );

    // Also prefetch epoch-keyed dynamic fields for DeepBook's historical data
    // This is essential for functions like `history::historic_maker_fee`
    let tx_epoch = grpc_tx.epoch.unwrap_or(0);

    // Debug: print epoch info and prefetch stats
    eprintln!(
        "[EPOCH DEBUG] Transaction {} epoch: {}",
        &grpc_tx.digest[..16],
        tx_epoch
    );
    eprintln!(
        "[EPOCH DEBUG] Historical versions to prefetch: {}",
        historical_versions.len()
    );
    eprintln!(
        "[EPOCH DEBUG] Total prefetched children: {}",
        prefetched.children.len()
    );
    eprintln!(
        "[EPOCH DEBUG] Total prefetched children_by_key: {}",
        prefetched.children_by_key.len()
    );
    eprintln!(
        "[EPOCH DEBUG] Prefetch discovered: {}, fetched: {}",
        prefetched.total_discovered, prefetched.fetched_count
    );

    // Show all key types found
    let mut key_types: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for key in prefetched.children_by_key.keys() {
        *key_types.entry(key.name_type.clone()).or_insert(0) += 1;
    }
    if !key_types.is_empty() {
        eprintln!("[EPOCH DEBUG] Key types found:");
        for (name_type, count) in &key_types {
            eprintln!("[EPOCH DEBUG]   {}: {} entries", name_type, count);
        }
    }

    // Look for DeepBook-related objects in historical_versions
    for (obj_id, ver) in &historical_versions {
        if obj_id.contains("2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809") {
            eprintln!(
                "[EPOCH DEBUG] DeepBook object: {} @ version {}",
                obj_id, ver
            );
        }
    }

    let epoch_fields_fetched = sui_data_fetcher::utilities::prefetch_epoch_keyed_fields(
        graphql,
        grpc,
        rt,
        &mut prefetched,
        tx_epoch,
        10, // lookback 10 epochs to cover historical fee lookups
    );

    for (child_id, (version, _, _)) in &prefetched.children {
        historical_versions
            .entry(child_id.clone())
            .or_insert(*version);
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
        if let sui_data_fetcher::grpc::GrpcCommand::MoveCall {
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
        if let sui_data_fetcher::grpc::GrpcCommand::MakeMoveVec {
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
                }
            }
        }
    }

    // =========================================================================
    // Extract additional package addresses from type strings
    // =========================================================================
    // Type strings reliably contain package addresses (e.g., Pool<0xabc::coin::COIN>)
    // This is more precise than scanning raw BCS bytes for 32-byte sequences.
    for (_obj_id, type_str) in &object_types {
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

            let version = historical_versions.get(pkg_id).copied();
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
                    fetched_packages.insert(pkg_id_normalized);
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

    for (pkg_id, modules) in &cached.packages {
        let pkg_id_normalized = normalize_address(pkg_id);

        if let Some(upgraded_id) = linkage_upgrades.get(&pkg_id_normalized) {
            if cached.packages.contains_key(upgraded_id) {
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(pkg_id).ok();

        let decoded_modules: Vec<(String, Vec<u8>)> = modules
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        let _ = resolver.add_package_modules_at(decoded_modules, target_addr);
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

            // Fallback: fetch on-demand
            let version = historical_clone.get(&child_id_str).copied();

            if let Ok(rt) = tokio::runtime::Runtime::new() {
                let result = rt.block_on(async {
                    grpc_clone
                        .get_object_at_version(&child_id_str, version)
                        .await
                });

                if let Ok(Some(obj)) = result {
                    if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                        if let Some(type_tag) = parse_type_tag(type_str) {
                            // Rewrite type addresses using linkage upgrade mappings
                            let rewritten = rewrite_type_tag(type_tag, &type_aliases);
                            return Some((rewritten, bcs.clone(), obj.version));
                        }
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
