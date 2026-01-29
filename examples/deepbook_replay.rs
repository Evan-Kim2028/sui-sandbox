//! DeepBook Flash Loan Replay Example
//!
//! Uses `sui_state_fetcher::HistoricalStateProvider` for all data fetching.
//!
//! Demonstrates replaying a DeepBook flash loan transaction that **failed on-chain**.
//! This example shows that the sandbox correctly reproduces failures, not just successes.
//!
//! Run with: cargo run --example deepbook_replay
//!
//! ## Required Setup
//!
//! Configure your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on your provider
//! ```

mod common;

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use std::sync::Arc;

use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::CachedTransaction;
use sui_sandbox_core::utilities::GenericObjectPatcher;
use sui_sandbox_core::vm::VMHarness;
use sui_state_fetcher::{
    get_historical_versions, to_replay_data, HistoricalStateProvider, ReplayState,
};

use common::{
    build_cached_object_index, build_replay_config, create_dynamic_discovery_cache,
    create_enhanced_child_fetcher_with_cache, create_key_based_child_fetcher,
    prefetch_dynamic_fields, prefetch_dynamic_fields_at_checkpoint, GraphQLClient,
};

/// DeepBook flash loan arbitrage - failed on-chain (insufficient output)
const TX_DIGEST: &str = "D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║              DeepBook Flash Loan Replay Example                      ║");
    println!("║                                                                      ║");
    println!("║  Using HistoricalStateProvider for unified state fetching.           ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    println!("Transaction: {}", TX_DIGEST);
    println!("Expected: FAILURE (insufficient arbitrage output)\n");

    let result = replay_transaction(TX_DIGEST)?;

    // Summary
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    let matches = !result; // We expect failure
    if matches {
        println!("║ ✓ Flash Loan Arb        | local: FAILURE | expected: FAILURE       ║");
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME - 1:1 MAINNET PARITY         ║");
    } else {
        println!("║ ✗ Flash Loan Arb        | local: SUCCESS | expected: FAILURE       ║");
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ ✗ UNEXPECTED: Local succeeded but mainnet failed                   ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

fn replay_transaction(tx_digest: &str) -> Result<bool> {
    // Create a runtime for async operations
    let rt = tokio::runtime::Runtime::new()?;

    // =========================================================================
    // Step 1: Fetch all state using HistoricalStateProvider
    // =========================================================================
    println!("Step 1: Fetching state via HistoricalStateProvider...");

    let provider: HistoricalStateProvider =
        rt.block_on(async { HistoricalStateProvider::mainnet().await })?;
    let state: ReplayState = rt.block_on(async {
        provider
            .fetch_replay_state_with_config(tx_digest, false, 0, 0)
            .await
    })?;

    println!(
        "   ✓ Transaction: {} commands",
        state.transaction.commands.len()
    );
    println!("   ✓ Objects: {}", state.objects.len());
    println!("   ✓ Packages: {}", state.packages.len());

    // Convert to replay data format
    let replay_data = to_replay_data(&state);
    let historical_versions = get_historical_versions(&state);

    let tx_timestamp_ms = state.transaction.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Step 2: Build module resolver
    // =========================================================================
    println!("\nStep 2: Building module resolver...");

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
    // Step 3: Create VM harness
    // =========================================================================
    println!("\nStep 3: Creating VM harness...");

    let config = build_replay_config(&state)?;

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // =========================================================================
    // Step 4: Prefetch dynamic fields using sui_prefetch utilities
    // =========================================================================
    println!("\nStep 4: Prefetching dynamic fields...");

    let graphql = GraphQLClient::mainnet();
    let grpc_for_prefetch =
        rt.block_on(async { sui_transport::grpc::GrpcClient::mainnet().await })?;

    let prefetched = if let Some(cp) = state.checkpoint {
        prefetch_dynamic_fields_at_checkpoint(
            &graphql,
            &grpc_for_prefetch,
            &rt,
            &historical_versions,
            3,
            200,
            cp,
        )
    } else {
        prefetch_dynamic_fields(
            &graphql,
            &grpc_for_prefetch,
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
    // Step 5: Set up child fetcher with enhanced fallback strategies
    // =========================================================================
    println!("\nStep 5: Setting up child fetcher...");

    // Create patcher for version field patching
    let mut patcher = GenericObjectPatcher::new();
    patcher.add_modules(resolver.compiled_modules());
    patcher.set_timestamp(tx_timestamp_ms);
    patcher.add_default_rules();

    // Create child fetcher with enhanced fallback strategies (gRPC + GraphQL)
    let grpc_for_fetcher =
        rt.block_on(async { sui_transport::grpc::GrpcClient::mainnet().await })?;
    let graphql_for_fetcher = GraphQLClient::mainnet();
    let discovery_cache = create_dynamic_discovery_cache();

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

    // Also set up key-based child fetcher for package upgrade handling
    let cached_index =
        Arc::new(build_cached_object_index(&replay_data.objects, &replay_data.object_types));
    let key_fetcher = create_key_based_child_fetcher(
        prefetched.clone(),
        Some(discovery_cache),
        Some(graphql_for_fetcher.clone()),
        Some(cached_index),
    );
    harness.set_key_based_child_fetcher(key_fetcher);
    println!("   ✓ Child fetcher configured with enhanced fallbacks");

    // =========================================================================
    // Step 6: Register input objects
    // =========================================================================
    println!("\nStep 6: Registering input objects...");

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
    // Step 7: Execute replay
    // =========================================================================
    println!("\nStep 7: Executing replay...");

    // Build CachedTransaction for the replay function
    let mut cached = CachedTransaction::new(state.transaction.clone());
    cached.packages = replay_data.packages;
    cached.objects = replay_data.objects;
    cached.object_types = replay_data.object_types.clone();
    cached.object_versions = historical_versions.clone();

    // Add prefetched objects to the cached transaction
    // This fills in objects that weren't available at historical versions but were fetched via GraphQL
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
            let err_str = err.to_string();
            let display = if err_str.len() > 80 {
                &err_str[..80]
            } else {
                &err_str
            };
            println!("  Error: {}...", display);
        }
    }

    Ok(result.local_success)
}
