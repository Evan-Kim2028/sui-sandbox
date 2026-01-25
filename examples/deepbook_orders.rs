//! DeepBook Order Transaction Replay Example (BigVector Handling)
//!
//! Uses `sui_state_fetcher::HistoricalStateProvider` for all data fetching.
//!
//! ## Key Feature: BigVector Support
//!
//! This example demonstrates replaying transactions that use **BigVector** internally.
//! BigVector is Sui's scalable vector implementation used by DeepBook for order books.
//! It stores data in dynamic field "slices" that may not appear in transaction effects.
//!
//! **Why this matters**: Standard replay fails because BigVector slices accessed during
//! execution aren't recorded in `unchanged_loaded_runtime_objects`. This example shows
//! how to handle this using prefetching and version validation.
//!
//! Run with: cargo run --example deepbook_orders
//!
//! ## Required Setup
//!
//! Configure your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here
//! ```
//!
//! ## Transactions Tested
//!
//! All these transactions succeed both on-chain and in local replay:
//!
//! - `FbrMKMyzWm1K89qBZ45sYfCDsEtNmcnBdU9xiT7NKvmR` - cancel_order
//! - `7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY` - cancel_order
//! - `3AKpMt66kXcPutKxkQ4D3NuAu4MJ1YGEvTNkWoAzyVVE` - place_limit_order
//! - `6fZMHYnpJoShz6ZXuWW14dCTgwv9XpgZ4jbZh6HBHufU` - place_limit_order
//!
//! ## How It Works
//!
//! ### The Problem
//! BigVector slice nodes accessed during execution may not be recorded in
//! `unchanged_loaded_runtime_objects` (they were only READ, not CHANGED).
//!
//! ### The Solution
//! 1. **Prefetch dynamic fields**: `prefetch_dynamic_fields` recursively discovers
//!    and fetches child objects via GraphQL, caching them for replay
//! 2. **Enhanced child fetcher**: On-demand fetching with gRPC + GraphQL fallback
//! 3. **Version validation**: Objects not in effects are validated against
//!    `max_lamport_version` - if `object.version <= max_lamport`, it's safe to use
//!
//! ## Key Functions Used
//!
//! - `prefetch_dynamic_fields()` - Eagerly discover child objects
//! - `create_enhanced_child_fetcher_with_cache()` - On-demand fetching with validation
//! - `create_key_based_child_fetcher()` - Handle package upgrade address mismatches

mod common;

use anyhow::Result;
use base64::Engine;
use move_core_types::account_address::AccountAddress;

use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::CachedTransaction;
use sui_sandbox_core::utilities::GenericObjectPatcher;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_state_fetcher::{
    get_historical_versions, to_replay_data, HistoricalStateProvider, ReplayState,
};

use common::{
    create_dynamic_discovery_cache, create_enhanced_child_fetcher_with_cache,
    create_key_based_child_fetcher, prefetch_dynamic_fields, GraphQLClient,
};

/// DeepBook cancel_order - uses main DeepBook package
const CANCEL_ORDER_TX: &str = "FbrMKMyzWm1K89qBZ45sYfCDsEtNmcnBdU9xiT7NKvmR";

/// DeepBook cancel_order - alternate transaction
const CANCEL_ORDER_TX_2: &str = "7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY";

/// DeepBook place_limit_order
const PLACE_LIMIT_ORDER_1: &str = "3AKpMt66kXcPutKxkQ4D3NuAu4MJ1YGEvTNkWoAzyVVE";

/// DeepBook place_limit_order - alternate transaction
const PLACE_LIMIT_ORDER_2: &str = "6fZMHYnpJoShz6ZXuWW14dCTgwv9XpgZ4jbZh6HBHufU";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║         DeepBook Orders Replay Example (BigVector Handling)          ║");
    println!("║                                                                      ║");
    println!("║  Demonstrates replaying transactions that use BigVector internally.  ║");
    println!("║  BigVector slices require dynamic field prefetching + on-demand      ║");
    println!("║  child fetching with version validation.                             ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    // Order transactions to test - all expected to succeed on mainnet
    let transactions = [
        (CANCEL_ORDER_TX, "Cancel Order (main pkg)", true),
        (CANCEL_ORDER_TX_2, "Cancel Order 2", true),
        (PLACE_LIMIT_ORDER_1, "Place Limit Order 1", true),
        (PLACE_LIMIT_ORDER_2, "Place Limit Order 2", true),
    ];

    let mut results = Vec::new();

    for (tx_digest, description, expected_success) in &transactions {
        println!("\n{}", "=".repeat(74));
        println!("  {} - {}", description, tx_digest);
        println!(
            "  Expected on-chain: {}",
            if *expected_success {
                "SUCCESS"
            } else {
                "FAILURE"
            }
        );
        println!("{}\n", "=".repeat(74));

        let result = replay_transaction(tx_digest);

        let (local_success, error_msg) = match result {
            Ok(success) => (success, None),
            Err(e) => (false, Some(e.to_string())),
        };

        let matches = local_success == *expected_success;

        println!("\n  ══════════════════════════════════════════════════════════════");
        println!(
            "  Local result: {}",
            if local_success { "SUCCESS" } else { "FAILURE" }
        );
        println!(
            "  Expected:     {}",
            if *expected_success {
                "SUCCESS"
            } else {
                "FAILURE"
            }
        );
        println!("  Match:        {}", if matches { "✓ YES" } else { "✗ NO" });
        if let Some(err) = &error_msg {
            let truncated = if err.len() > 60 { &err[..60] } else { err };
            println!("  Error:        {}...", truncated);
        }
        println!("  ══════════════════════════════════════════════════════════════");

        results.push((
            description.to_string(),
            local_success,
            *expected_success,
            matches,
        ));
    }

    // Summary
    println!("\n\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    let mut all_match = true;
    for (desc, local, expected, matches) in &results {
        let status = if *matches { "✓" } else { "✗" };
        let local_str = if *local { "SUCCESS" } else { "FAILURE" };
        let expected_str = if *expected { "SUCCESS" } else { "FAILURE" };
        println!(
            "║ {} {:25} | local: {:7} | expected: {:7} ║",
            status, desc, local_str, expected_str
        );
        if !matches {
            all_match = false;
        }
    }

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if all_match {
        println!("║ ✓ ALL TRANSACTIONS MATCH EXPECTED OUTCOMES - 1:1 MAINNET PARITY     ║");
    } else {
        println!("║ ✗ SOME TRANSACTIONS DID NOT MATCH                                   ║");
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
    let state: ReplayState = rt.block_on(async { provider.fetch_replay_state(tx_digest).await })?;

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
    // Step 3: Prefetch dynamic fields
    // =========================================================================
    println!("\nStep 3: Prefetching dynamic fields...");

    let graphql = GraphQLClient::mainnet();
    let grpc_for_prefetch =
        rt.block_on(async { sui_transport::grpc::GrpcClient::mainnet().await })?;

    let prefetched = prefetch_dynamic_fields(
        &graphql,
        &grpc_for_prefetch,
        &rt,
        &historical_versions,
        3,
        200,
    );

    println!(
        "   ✓ Discovered {} fields, fetched {} children",
        prefetched.total_discovered, prefetched.fetched_count
    );

    // =========================================================================
    // Step 4: Create VM harness
    // =========================================================================
    println!("\nStep 4: Creating VM harness...");

    let sender_address = state.transaction.sender;

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

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
    // Use same endpoint and API key as HistoricalStateProvider
    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let grpc_api_key = std::env::var("SUI_GRPC_API_KEY").ok();
    let grpc_for_fetcher = rt.block_on(async {
        sui_transport::grpc::GrpcClient::with_api_key(&grpc_endpoint, grpc_api_key).await
    })?;
    let graphql_for_fetcher = GraphQLClient::mainnet();
    let discovery_cache = create_dynamic_discovery_cache();

    let child_fetcher = create_enhanced_child_fetcher_with_cache(
        grpc_for_fetcher,
        graphql_for_fetcher,
        historical_versions.clone(),
        prefetched.clone(),
        Some(patcher),
        Some(discovery_cache),
    );
    harness.set_child_fetcher(child_fetcher);

    // Also set up key-based child fetcher for package upgrade handling
    let key_fetcher = create_key_based_child_fetcher(prefetched.clone());
    harness.set_key_based_child_fetcher(key_fetcher);
    println!("   ✓ Child fetcher configured");

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
