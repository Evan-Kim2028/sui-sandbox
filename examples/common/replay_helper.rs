//! High-Level Transaction Replay Helper
//!
//! This module provides a streamlined API for replaying historical transactions
//! using the sui-sandbox with HistoricalStateProvider (gRPC).
//!
//! ## Usage
//!
//! ```ignore
//! let result = ReplayBuilder::new()
//!     .with_mm2_analysis(true)
//!     .with_verbose(true)
//!     .replay("7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp")?;
//! ```
//!
//! ## Note: Walrus-First Approach
//!
//! For Walrus-first replay (recommended for historical transactions), use the
//! `walrus_checkpoint` module directly in your example. See `walrus_checkpoint_replay.rs`
//! for a complete example.
//!
//! The gRPC approach in this module may not work for old transactions that have
//! been pruned from public nodes.

use anyhow::{anyhow, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;
use std::sync::Arc;

use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::CachedTransaction;
use sui_sandbox_core::utilities::{GenericObjectPatcher, HistoricalStateReconstructor};
use sui_sandbox_core::vm::VMHarness;
use sui_state_fetcher::{
    get_historical_versions, to_replay_data, HistoricalStateProvider, ReplayState,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;

use super::{
    build_cached_object_index, build_replay_config, create_dynamic_discovery_cache,
    create_enhanced_child_fetcher_with_cache, create_key_based_child_fetcher,
    prefetch_dynamic_fields, prefetch_dynamic_fields_at_checkpoint,
};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the replay builder.
#[derive(Clone)]
pub struct ReplayConfig {
    /// Enable MM2 bytecode analysis for predictive prefetch
    pub mm2_analysis: bool,
    /// Dynamic field prefetch depth
    pub prefetch_depth: usize,
    /// Dynamic field prefetch limit per parent
    pub prefetch_limit: usize,
    /// Print verbose progress messages
    pub verbose: bool,
    /// Optional cache directory for disk-based caching
    pub cache_dir: Option<String>,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            mm2_analysis: true,
            prefetch_depth: 3,
            prefetch_limit: 200,
            verbose: false,
            cache_dir: None,
        }
    }
}

// ============================================================================
// Result Types
// ============================================================================

/// Result of a transaction replay.
#[derive(Debug)]
pub struct ReplayResult {
    /// Whether the transaction succeeded locally
    pub success: bool,
    /// Error message if execution failed
    pub error: Option<String>,
    /// Number of objects loaded
    pub objects_loaded: usize,
    /// Number of packages loaded
    pub packages_loaded: usize,
    /// Number of modules loaded
    pub modules_loaded: usize,
    /// Number of dynamic fields prefetched
    pub fields_prefetched: usize,
    /// Number of commands in the transaction
    pub commands_count: usize,
    /// MM2 prediction stats (if enabled)
    pub mm2_stats: Option<Mm2Stats>,
    /// Patching statistics
    pub patch_stats: PatchStats,
}

/// MM2 bytecode analysis statistics.
#[derive(Debug, Clone, Default)]
pub struct Mm2Stats {
    pub commands_analyzed: usize,
    pub predictions_made: usize,
    pub matched_ground_truth: usize,
    pub high_confidence: usize,
    pub medium_confidence: usize,
    pub low_confidence: usize,
    pub packages_analyzed: usize,
}

/// Object patching statistics.
#[derive(Debug, Clone, Default)]
pub struct PatchStats {
    pub struct_patched: usize,
    pub raw_patched: usize,
    pub total_patched: usize,
}

// ============================================================================
// Replay Builder
// ============================================================================

/// Builder for configuring and executing transaction replays.
pub struct ReplayBuilder {
    config: ReplayConfig,
    rt: Option<tokio::runtime::Runtime>,
}

impl Default for ReplayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayBuilder {
    /// Create a new replay builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: ReplayConfig::default(),
            rt: None,
        }
    }

    /// Enable or disable MM2 bytecode analysis.
    pub fn with_mm2_analysis(mut self, enabled: bool) -> Self {
        self.config.mm2_analysis = enabled;
        self
    }

    /// Set dynamic field prefetch depth.
    pub fn with_prefetch_depth(mut self, depth: usize) -> Self {
        self.config.prefetch_depth = depth;
        self
    }

    /// Set dynamic field prefetch limit per parent.
    pub fn with_prefetch_limit(mut self, limit: usize) -> Self {
        self.config.prefetch_limit = limit;
        self
    }

    /// Enable verbose progress messages.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.config.verbose = verbose;
        self
    }

    /// Set cache directory for disk-based caching.
    pub fn with_cache_dir(mut self, dir: impl Into<String>) -> Self {
        self.config.cache_dir = Some(dir.into());
        self
    }

    /// Use an existing tokio runtime.
    pub fn with_runtime(mut self, rt: tokio::runtime::Runtime) -> Self {
        self.rt = Some(rt);
        self
    }

    /// Execute the replay for the given transaction digest.
    ///
    /// Uses HistoricalStateProvider with gRPC. For Walrus-first replay,
    /// use the walrus_checkpoint module directly.
    pub fn replay(self, tx_digest: &str) -> Result<ReplayResult> {
        let rt = self.rt.unwrap_or_else(|| {
            tokio::runtime::Runtime::new().expect("Failed to create runtime")
        });

        replay_transaction_impl(&rt, tx_digest, &self.config)
    }
}

// ============================================================================
// Implementation
// ============================================================================

fn replay_transaction_impl(
    rt: &tokio::runtime::Runtime,
    tx_digest: &str,
    config: &ReplayConfig,
) -> Result<ReplayResult> {
    let verbose = config.verbose;

    // Step 1: Fetch state
    if verbose {
        println!("Step 1: Fetching state via HistoricalStateProvider...");
    }

    let provider: HistoricalStateProvider =
        rt.block_on(async { HistoricalStateProvider::mainnet().await })?;
    let state: ReplayState = rt.block_on(async {
        provider
            .replay_state_builder()
            .prefetch_dynamic_fields(false)
            .dynamic_field_depth(0)
            .dynamic_field_limit(0)
            .auto_system_objects(true)
            .build(tx_digest)
            .await
    })?;

    if verbose {
        println!("   ✓ Transaction: {} commands", state.transaction.commands.len());
        println!("   ✓ Objects: {}", state.objects.len());
        println!("   ✓ Packages: {}", state.packages.len());
    }

    let grpc_tx = rt
        .block_on(async { provider.grpc().get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found"))?;

    let tx_timestamp_ms = state.transaction.timestamp_ms.unwrap_or(1700000000000);
    let replay_data = to_replay_data(&state);
    let historical_versions = get_historical_versions(&state);

    // Step 2: MM2 analysis (optional)
    let mut mm2_stats = None;
    if config.mm2_analysis {
        if verbose {
            println!("\nStep 2: Running MM2 predictive prefetch analysis...");
        }

        let grpc_for_mm2 = rt.block_on(async { GrpcClient::mainnet().await })?;
        let graphql_for_mm2 = GraphQLClient::mainnet();

        let mut prefetcher = PredictivePrefetcher::new();
        let mm2_config = PredictivePrefetchConfig::default();
        let mm2_result = prefetcher.prefetch_for_transaction(
            &grpc_for_mm2,
            Some(&graphql_for_mm2),
            rt,
            &grpc_tx,
            &mm2_config,
        );

        let stats = &mm2_result.prediction_stats;
        mm2_stats = Some(Mm2Stats {
            commands_analyzed: stats.commands_analyzed,
            predictions_made: stats.predictions_made,
            matched_ground_truth: stats.predictions_matched_ground_truth,
            high_confidence: stats.high_confidence_predictions,
            medium_confidence: stats.medium_confidence_predictions,
            low_confidence: stats.low_confidence_predictions,
            packages_analyzed: stats.packages_analyzed,
        });

        if verbose {
            println!("   Commands analyzed: {}", stats.commands_analyzed);
            println!("   Predictions made: {}", stats.predictions_made);
            println!("   Matched ground truth: {}", stats.predictions_matched_ground_truth);
            println!(
                "   Confidence: high={}, medium={}, low={}",
                stats.high_confidence_predictions,
                stats.medium_confidence_predictions,
                stats.low_confidence_predictions
            );
        }
    }

    // Step 3: Prefetch dynamic fields
    if verbose {
        println!("\nStep 3: Prefetching dynamic fields...");
    }

    let grpc_for_prefetch = rt.block_on(async { GrpcClient::mainnet().await })?;
    let graphql_for_prefetch = GraphQLClient::mainnet();

    let prefetched = if let Some(cp) = state.checkpoint {
        prefetch_dynamic_fields_at_checkpoint(
            &graphql_for_prefetch,
            &grpc_for_prefetch,
            rt,
            &historical_versions,
            config.prefetch_depth,
            config.prefetch_limit,
            cp,
        )
    } else {
        prefetch_dynamic_fields(
            &graphql_for_prefetch,
            &grpc_for_prefetch,
            rt,
            &historical_versions,
            config.prefetch_depth,
            config.prefetch_limit,
        )
    };

    let fields_prefetched = prefetched.fetched_count;
    if verbose {
        println!(
            "   ✓ Discovered {} fields, fetched {} children",
            prefetched.total_discovered, prefetched.fetched_count
        );
    }

    // Step 4: Build module resolver
    if verbose {
        println!("\nStep 4: Building module resolver...");
    }

    let mut resolver = LocalModuleResolver::new();
    let mut module_count = 0;

    for (pkg_id, modules_b64) in &replay_data.packages {
        if let Some(upgraded_id) = replay_data.linkage_upgrades.get(pkg_id) {
            if replay_data.packages.contains_key(upgraded_id) {
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(pkg_id).ok();
        let decoded: Vec<(String, Vec<u8>)> = modules_b64
            .iter()
            .filter_map(|(name, b64): &(String, String)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        if let Ok((count, _)) = resolver.add_package_modules_at(decoded, target_addr) {
            module_count += count;
        }
    }

    resolver.load_sui_framework()?;
    if verbose {
        println!("   ✓ Loaded {} user modules", module_count);
    }

    // Step 5: Reconstruct/patch objects
    if verbose {
        println!("\nStep 5: Reconstructing historical state...");
    }

    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.set_timestamp(tx_timestamp_ms);
    reconstructor.configure_from_modules(resolver.compiled_modules());

    let raw_objects: HashMap<String, Vec<u8>> = replay_data
        .objects
        .iter()
        .filter_map(|(id, b64)| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .ok()
                .map(|bcs| (id.clone(), bcs))
        })
        .collect();

    let reconstructed = reconstructor.reconstruct(&raw_objects, &replay_data.object_types);

    let patch_stats = PatchStats {
        struct_patched: reconstructed.stats.struct_patched,
        raw_patched: reconstructed.stats.raw_patched,
        total_patched: reconstructed.stats.total_patched(),
    };

    if verbose {
        println!(
            "   Patching: struct={}, raw={}, total={}",
            patch_stats.struct_patched, patch_stats.raw_patched, patch_stats.total_patched
        );
    }

    let patched_objects_b64: HashMap<String, String> = reconstructed
        .objects
        .iter()
        .map(|(id, bcs)| {
            (
                id.clone(),
                base64::engine::general_purpose::STANDARD.encode(bcs),
            )
        })
        .collect();

    // Build patcher for on-demand fetches
    let mut patcher = GenericObjectPatcher::new();
    patcher.add_modules(resolver.compiled_modules());
    patcher.set_timestamp(tx_timestamp_ms);
    patcher.add_default_rules();

    // Step 6: Create VM harness
    if verbose {
        println!("\nStep 6: Creating VM harness...");
    }

    let vm_config = build_replay_config(&state)?;
    let mut harness = VMHarness::with_config(&resolver, false, vm_config)?;

    // Step 7: Set up child fetcher
    if verbose {
        println!("\nStep 7: Setting up child fetcher...");
    }

    let discovery_cache = create_dynamic_discovery_cache();
    let grpc_for_fetcher = rt.block_on(async { GrpcClient::mainnet().await })?;
    let graphql_for_fetcher = GraphQLClient::mainnet();

    let child_fetcher = create_enhanced_child_fetcher_with_cache(
        grpc_for_fetcher,
        graphql_for_fetcher.clone(),
        historical_versions.clone(),
        prefetched.clone(),
        Some(patcher),
        state.checkpoint,
        Some(discovery_cache.clone()),
    );

    harness.set_child_fetcher(child_fetcher);

    let cached_index = Arc::new(build_cached_object_index(
        &replay_data.objects,
        &replay_data.object_types,
    ));
    let prefetched_for_key_fetcher = prefetched.clone();
    let key_fetcher = create_key_based_child_fetcher(
        prefetched_for_key_fetcher,
        Some(discovery_cache),
        Some(graphql_for_fetcher),
        Some(cached_index),
    );
    harness.set_key_based_child_fetcher(key_fetcher);

    // Register input objects
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

    if verbose {
        println!("   ✓ Registered {} objects", historical_versions.len());
    }

    // Step 8: Execute replay
    if verbose {
        println!("\nStep 8: Executing replay...");
    }

    let mut cached = CachedTransaction::new(state.transaction.clone());
    cached.packages = replay_data.packages;
    cached.objects = patched_objects_b64;
    cached.object_types = replay_data.object_types.clone();
    cached.object_versions = historical_versions.clone();

    // Merge prefetched dynamic field objects into cached transaction
    for (child_id, (version, type_str, bcs)) in &prefetched.children {
        cached
            .objects
            .entry(child_id.clone())
            .or_insert_with(|| base64::engine::general_purpose::STANDARD.encode(bcs));
        cached
            .object_types
            .entry(child_id.clone())
            .or_insert_with(|| type_str.clone());
        cached
            .object_versions
            .entry(child_id.clone())
            .or_insert(*version);
    }

    let address_aliases = sui_sandbox_core::tx_replay::build_address_aliases_for_test(&cached);
    harness.set_address_aliases(address_aliases.clone());

    let result = sui_sandbox_core::tx_replay::replay_with_objects_and_aliases(
        &cached.transaction,
        &mut harness,
        &cached.objects,
        &address_aliases,
    )?;

    if verbose {
        println!(
            "\n  Local execution: {}",
            if result.local_success { "SUCCESS" } else { "FAILURE" }
        );
        if !result.local_success {
            if let Some(err) = &result.local_error {
                println!("  Error: {}", err);
            }
        }
    }

    Ok(ReplayResult {
        success: result.local_success,
        error: result.local_error,
        objects_loaded: state.objects.len(),
        packages_loaded: state.packages.len(),
        modules_loaded: module_count,
        fields_prefetched,
        commands_count: state.transaction.commands.len(),
        mm2_stats,
        patch_stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_config_default() {
        let config = ReplayConfig::default();
        assert!(config.mm2_analysis);
        assert_eq!(config.prefetch_depth, 3);
        assert_eq!(config.prefetch_limit, 200);
        assert!(!config.verbose);
    }

    #[test]
    fn test_replay_builder_chain() {
        let builder = ReplayBuilder::new()
            .with_mm2_analysis(false)
            .with_prefetch_depth(5)
            .with_verbose(true);

        assert!(!builder.config.mm2_analysis);
        assert_eq!(builder.config.prefetch_depth, 5);
        assert!(builder.config.verbose);
    }
}
