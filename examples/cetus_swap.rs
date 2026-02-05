//! Cetus AMM Swap Replay Example
//!
//! Uses `sui_state_fetcher::HistoricalStateProvider` for data fetching with
//! MM2 analysis and HistoricalStateReconstructor features.
//!
//! Demonstrates replaying a historical Cetus swap transaction locally using the Move VM sandbox
//! with MM2 bytecode analysis for predictive dynamic field prefetching.
//!
//! Run with: cargo run --example cetus_swap
//!
//! ## Key Features
//!
//! - **MM2 Predictive Prefetch**: Bytecode analysis to predict dynamic field accesses
//! - **HistoricalStateReconstructor**: Patches version fields using WELL_KNOWN_VERSION_CONFIGS
//! - **On-demand child fetching**: For dynamic fields discovered at runtime
//!
//! ## Required Setup
//!
//! Configure your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on your provider
//! ```

mod common;

use std::collections::HashMap;

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use std::sync::Arc;

use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::CachedTransaction;
use sui_sandbox_core::utilities::{GenericObjectPatcher, HistoricalStateReconstructor};
use sui_sandbox_core::vm::VMHarness;
use sui_state_fetcher::{
    get_historical_versions, to_replay_data, HistoricalStateProvider, ReplayState,
    ReplayStateConfig,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;

use common::{
    build_cached_object_index, build_replay_config, create_dynamic_discovery_cache,
    create_enhanced_child_fetcher_with_cache, create_key_based_child_fetcher,
    prefetch_dynamic_fields, prefetch_dynamic_fields_at_checkpoint,
};

/// Transaction digest for a Cetus LEIA/SUI swap - succeeded on-chain
const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                  Cetus AMM Swap Replay Example                       ║");
    println!("║                                                                      ║");
    println!("║  Using HistoricalStateProvider with MM2 analysis & reconstruction.   ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let result = replay_transaction(TX_DIGEST)?;

    // Summary
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Cetus LEIA/SUI Swap     | local: {:7} | expected: SUCCESS ║",
        if result { "SUCCESS" } else { "FAILURE" }
    );
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if result {
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME - 1:1 MAINNET PARITY         ║");
        println!("║                                                                      ║");
        println!("║ MM2 Analysis helped predict dynamic field accesses for Cetus pool   ║");
    } else {
        println!("║ ✗ TRANSACTION DID NOT MATCH EXPECTED OUTCOME                        ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

fn replay_transaction(tx_digest: &str) -> Result<bool> {
    // Create a runtime for async operations
    let rt = tokio::runtime::Runtime::new()?;

    // =========================================================================
    // Step 1: Fetch state using HistoricalStateProvider
    // =========================================================================
    println!("Step 1: Fetching state via HistoricalStateProvider...");

    let provider: HistoricalStateProvider =
        rt.block_on(async { HistoricalStateProvider::mainnet().await })?;
    let config = ReplayStateConfig {
        prefetch_dynamic_fields: false,
        df_depth: 0,
        df_limit: 0,
        auto_system_objects: true,
    };
    let state: ReplayState = rt.block_on(async {
        provider
            .replay_state_builder()
            .with_config(config)
            .build(tx_digest)
            .await
    })?;

    println!(
        "   ✓ Transaction: {} commands",
        state.transaction.commands.len()
    );
    println!("   ✓ Objects: {}", state.objects.len());
    println!("   ✓ Packages: {}", state.packages.len());

    // We still need the original gRPC transaction for MM2 analysis
    let grpc_tx = rt
        .block_on(async { provider.grpc().get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow::anyhow!("Transaction not found"))?;

    let tx_timestamp_ms = state.transaction.timestamp_ms.unwrap_or(1700000000000);

    // Convert to replay data format
    let replay_data = to_replay_data(&state);
    let historical_versions = get_historical_versions(&state);

    // =========================================================================
    // Step 2: Run MM2 predictive prefetch analysis
    // =========================================================================
    println!("\nStep 2: Running MM2 predictive prefetch analysis...");

    let grpc_for_mm2 = rt.block_on(async { GrpcClient::mainnet().await })?;
    let graphql_for_mm2 = GraphQLClient::mainnet();

    let mut prefetcher = PredictivePrefetcher::new();
    let mm2_config = PredictivePrefetchConfig::default();
    let mm2_result = prefetcher.prefetch_for_transaction(
        &grpc_for_mm2,
        Some(&graphql_for_mm2),
        &rt,
        &grpc_tx,
        &mm2_config,
    );

    let stats = &mm2_result.prediction_stats;
    println!("   MM2 Analysis Results:");
    println!("      Commands analyzed: {}", stats.commands_analyzed);
    println!("      Predictions made: {}", stats.predictions_made);
    println!(
        "      Predictions matched ground truth: {}",
        stats.predictions_matched_ground_truth
    );
    println!(
        "      High confidence: {}, Medium: {}, Low: {}",
        stats.high_confidence_predictions,
        stats.medium_confidence_predictions,
        stats.low_confidence_predictions
    );

    // =========================================================================
    // Step 2b: Prefetch dynamic fields (checkpoint snapshot when available)
    // =========================================================================
    println!("\nStep 2b: Prefetching dynamic fields...");
    let prefetched = if let Some(cp) = state.checkpoint {
        prefetch_dynamic_fields_at_checkpoint(
            &graphql_for_mm2,
            &grpc_for_mm2,
            &rt,
            &historical_versions,
            3,
            200,
            cp,
        )
    } else {
        prefetch_dynamic_fields(
            &graphql_for_mm2,
            &grpc_for_mm2,
            &rt,
            &historical_versions,
            3,
            200,
        )
    };
    println!(
        "   ✓ Discovered {} fields, fetched {} children",
        prefetched.total_discovered, prefetched.fetched_count
    );

    // =========================================================================
    // Step 3: Build module resolver
    // =========================================================================
    println!("\nStep 3: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();
    let mut module_count = 0;

    for (pkg_id, modules_b64) in &replay_data.packages {
        // Skip packages superseded by upgrades
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
    println!("   ✓ Loaded {} user modules", module_count);

    // =========================================================================
    // Step 4: Use HistoricalStateReconstructor to patch objects
    // =========================================================================
    println!("\nStep 4: Using HistoricalStateReconstructor to patch objects...");

    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.set_timestamp(tx_timestamp_ms);
    reconstructor.configure_from_modules(resolver.compiled_modules());

    // Convert objects from replay_data to raw BCS for reconstruction
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

    let patch_stats = &reconstructed.stats;
    println!(
        "   Patching Statistics: struct={}, raw={}, override={}, total={}",
        patch_stats.struct_patched,
        patch_stats.raw_patched,
        patch_stats.override_patched,
        patch_stats.total_patched()
    );

    // Convert patched objects back to base64
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

    println!("   ✓ Patched {} objects", patched_objects_b64.len());

    // Build patcher for on-demand child objects
    let mut patcher = GenericObjectPatcher::new();
    patcher.add_modules(resolver.compiled_modules());
    patcher.set_timestamp(tx_timestamp_ms);
    patcher.add_default_rules();

    // =========================================================================
    // Step 5: Create VM harness
    // =========================================================================
    println!("\nStep 5: Creating VM harness...");

    let config = build_replay_config(&state)?;

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // =========================================================================
    // Step 6: Set up child fetcher with patching
    // =========================================================================
    println!("\nStep 6: Setting up child fetcher...");

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
    let key_fetcher = create_key_based_child_fetcher(
        prefetched.clone(),
        Some(discovery_cache),
        Some(graphql_for_fetcher.clone()),
        Some(cached_index),
    );
    harness.set_key_based_child_fetcher(key_fetcher);
    println!("   ✓ Child fetcher configured");

    // =========================================================================
    // Step 7: Register input objects
    // =========================================================================
    println!("\nStep 7: Registering input objects...");

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
    println!("   ✓ Registered {} objects", historical_versions.len());

    // =========================================================================
    // Step 8: Execute replay
    // =========================================================================
    println!("\nStep 8: Executing replay...");

    // Build CachedTransaction
    let mut cached = CachedTransaction::new(state.transaction.clone());
    cached.packages = replay_data.packages;
    cached.objects = patched_objects_b64;
    cached.object_types = replay_data.object_types;
    cached.object_versions = historical_versions.clone();

    let address_aliases = sui_sandbox_core::tx_replay::build_address_aliases_for_test(&cached);
    harness.set_address_aliases(address_aliases.clone());

    let result = sui_sandbox_core::tx_replay::replay_with_objects_and_aliases(
        &cached.transaction,
        &mut harness,
        &cached.objects,
        &address_aliases,
    )?;

    println!(
        "\n  Local execution: {}",
        if result.local_success {
            "SUCCESS"
        } else {
            "FAILURE"
        }
    );
    if !result.local_success {
        if let Some(err) = &result.local_error {
            println!("  Error: {}", err);
        }
    }

    Ok(result.local_success)
}
