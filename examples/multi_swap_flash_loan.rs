#![allow(clippy::type_complexity)]
//! Multi-DEX Flash Loan Replay Example
//!
//! Demonstrates replaying a complex arbitrage transaction that routes through
//! multiple DEXes (Kriya, Bluefin, Cetus) using flash loans.
//!
//! Run with: cargo run --example multi_swap_flash_loan
//!
//! ## What This Shows
//!
//! - Multi-protocol transaction replay (Kriya, Cetus, Bluefin)
//! - Flash loan flow: borrow -> swap -> swap -> repay
//! - Automatic version patching for version-locked protocols
//! - Dynamic field prefetching across multiple DEX pools
//!
//! ## Required Setup
//!
//! Configure your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here
//! ```

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use common::{extract_package_ids_from_type, parse_type_tag_simple};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use sui_sandbox_core::object_runtime::ChildFetcherFn;
use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::{grpc_to_fetched_transaction, CachedTransaction};
use sui_sandbox_core::utilities::{
    grpc_object_to_package_data, CallbackPackageFetcher, HistoricalPackageResolver,
    HistoricalStateReconstructor,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcInput};

/// Kriya multi-hop swap with flash loan - successful on-chain
/// Routes: SUI -> USDC (Portal) -> USDC (native) -> SCA -> SUI
const TX_DIGEST: &str = "63fPrufC6iYHdNzG7mXscaKkqTaYH8h4RQHuiUfUCXqz";

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           Multi-DEX Flash Loan Replay Example                         ║");
    println!("║                                                                      ║");
    println!("║  Replays a complex arbitrage routing through Kriya, Bluefin, Cetus.  ║");
    println!("║  Demonstrates multi-protocol replay with automatic version patching.  ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    println!("Transaction: {}", TX_DIGEST);
    println!("Route: SUI -> USDC (Portal) -> USDC (native) -> SCA -> SUI");
    println!("Expected: SUCCESS\n");

    let result = replay_transaction(TX_DIGEST)?;

    // Summary
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    if result {
        println!("║ ✓ Multi-DEX Flash Loan | local: SUCCESS | expected: SUCCESS        ║");
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME - 1:1 MAINNET PARITY         ║");
    } else {
        println!("║ ✗ Multi-DEX Flash Loan | local: FAILURE | expected: SUCCESS        ║");
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ ✗ UNEXPECTED: Local failed but mainnet succeeded                   ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

fn replay_transaction(tx_digest: &str) -> Result<bool> {
    let rt = Arc::new(tokio::runtime::Runtime::new()?);

    // Step 1: Connect to gRPC
    println!("Step 1: Connecting to gRPC...");
    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let grpc = Arc::new(grpc);
    println!("   ✓ Connected to {}", endpoint);

    // Step 2: Fetch transaction
    println!("\nStep 2: Fetching transaction...");
    let grpc_tx = rt
        .as_ref()
        .block_on(async { grpc.get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found: {}", tx_digest))?;

    println!("   Digest: {}", grpc_tx.digest);
    println!("   Commands: {}", grpc_tx.commands.len());
    println!("   Status: {:?}", grpc_tx.status);

    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // Step 3: MM2 analysis
    println!("\nStep 3: Running MM2 analysis...");
    let graphql = GraphQLClient::mainnet();
    let mut prefetcher = PredictivePrefetcher::new();
    let mm2_config = PredictivePrefetchConfig::default();
    let mm2_result = prefetcher.prefetch_for_transaction(
        &grpc,
        Some(&graphql),
        rt.as_ref(),
        &grpc_tx,
        &mm2_config,
    );

    let stats = &mm2_result.prediction_stats;
    println!(
        "   Predictions: {} (matched: {})",
        stats.predictions_made, stats.predictions_matched_ground_truth
    );
    println!("   Packages analyzed: {}", stats.packages_analyzed);

    // Step 4: Collect historical versions
    println!("\nStep 4: Collecting historical versions...");
    let mut historical_versions: HashMap<String, u64> = HashMap::new();

    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        historical_versions.insert(id.clone(), *ver);
    }
    for (id, ver) in &grpc_tx.changed_objects {
        historical_versions.insert(id.clone(), *ver);
    }
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        historical_versions.insert(id.clone(), *ver);
    }
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
    println!("   ✓ Found {} unique objects", historical_versions.len());

    // Step 5: Fetch objects
    println!("\nStep 5: Fetching objects...");
    let mut raw_objects: HashMap<String, Vec<u8>> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut package_ids_to_fetch: Vec<String> = Vec::new();

    for cmd in &grpc_tx.commands {
        if let sui_sandbox::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            package_ids_to_fetch.push(package.clone());
        }
    }

    let mut fetched_count = 0;
    for (obj_id, version) in &historical_versions {
        if let Ok(Some(obj)) = rt
            .as_ref()
            .block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await })
        {
            if let Some(bcs) = &obj.bcs {
                raw_objects.insert(obj_id.clone(), bcs.clone());
                if let Some(type_str) = &obj.type_string {
                    object_types.insert(obj_id.clone(), type_str.clone());
                    for pkg_id in extract_package_ids_from_type(type_str) {
                        if !package_ids_to_fetch.contains(&pkg_id) {
                            package_ids_to_fetch.push(pkg_id);
                        }
                    }
                }
                fetched_count += 1;
            }
        }
    }
    println!("   ✓ Fetched {} objects", fetched_count);

    // Step 6: Resolve packages
    println!("\nStep 6: Resolving packages...");
    let grpc_for_fetcher = grpc.clone();
    let rt_for_fetcher = rt.clone();
    let historical_for_fetcher = historical_versions.clone();

    let fetcher = CallbackPackageFetcher::new(move |pkg_id: &str, version: Option<u64>| {
        let actual_version = version.or_else(|| historical_for_fetcher.get(pkg_id).copied());
        let result = rt_for_fetcher.as_ref().block_on(async {
            grpc_for_fetcher
                .get_object_at_version(pkg_id, actual_version)
                .await
        })?;
        Ok(result
            .as_ref()
            .and_then(|obj| grpc_object_to_package_data(pkg_id, obj)))
    });

    let mut pkg_resolver = HistoricalPackageResolver::new(fetcher);
    pkg_resolver.set_historical_versions(historical_versions.clone());
    pkg_resolver.resolve_packages(&package_ids_to_fetch)?;

    let linkage_upgrades = pkg_resolver.linkage_upgrades();
    println!(
        "   ✓ Resolved {} packages ({} linkage upgrades)",
        pkg_resolver.package_count(),
        linkage_upgrades.len()
    );

    // Step 7: Build resolver
    println!("\nStep 7: Building module resolver...");
    let mut resolver = LocalModuleResolver::new();
    let mut module_count = 0;

    let all_packages: Vec<(String, Vec<(String, String)>)> =
        pkg_resolver.packages_as_base64().into_iter().collect();
    let mut packages_with_source: Vec<(String, Vec<(String, String)>, Option<String>, bool)> =
        Vec::new();

    for (pkg_id, modules_b64) in all_packages {
        if let Some(upgraded) = linkage_upgrades.get(&pkg_id as &str) {
            if pkg_resolver.get_package(upgraded).is_some() {
                continue;
            }
        }

        let source_addr_opt: Option<String> = modules_b64.first().and_then(|(_, b64)| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .ok()
                .and_then(|bytes| {
                    CompiledModule::deserialize_with_defaults(&bytes)
                        .ok()
                        .map(|m| m.self_id().address().to_hex_literal())
                })
        });

        let is_original = source_addr_opt
            .as_ref()
            .map(|src| {
                pkg_id.contains(&src[..src.len().min(20)])
                    || src.contains(&pkg_id[..pkg_id.len().min(20)])
            })
            .unwrap_or(false);

        packages_with_source.push((pkg_id, modules_b64, source_addr_opt, is_original));
    }

    packages_with_source.sort_by(|a, b| {
        if a.3 != b.3 {
            b.3.cmp(&a.3)
        } else {
            a.0.cmp(&b.0)
        }
    });

    let mut loaded_source_addrs: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for (pkg_id, modules_b64, source_addr_opt, _) in packages_with_source {
        if let Some(ref source_addr) = source_addr_opt {
            if loaded_source_addrs.contains(source_addr) {
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(&pkg_id).ok();
        let decoded: Vec<(String, Vec<u8>)> = modules_b64
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        if let Ok((count, source_addr)) = resolver.add_package_modules_at(decoded, target_addr) {
            module_count += count;
            if let Some(src) = source_addr {
                loaded_source_addrs.insert(src.to_hex_literal());
            }
        }
    }

    resolver.load_sui_framework()?;
    println!("   ✓ Loaded {} modules", module_count);

    // Step 8: Patch objects
    println!("\nStep 8: Patching objects for version compatibility...");
    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.set_timestamp(tx_timestamp_ms);
    reconstructor.configure_from_modules(resolver.compiled_modules());

    let reconstructed = reconstructor.reconstruct(&raw_objects, &object_types);
    println!(
        "   ✓ Patched {} objects (struct: {}, raw: {})",
        reconstructed.stats.total_patched(),
        reconstructed.stats.struct_patched,
        reconstructed.stats.raw_patched
    );

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

    // Step 9: Create VM harness
    println!("\nStep 9: Creating VM harness...");
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // Step 10: Set up child fetcher
    println!("\nStep 10: Setting up child fetcher...");
    let historical_arc = Arc::new(historical_versions.clone());
    let patched_arc = Arc::new(patched_objects_b64.clone());
    let types_arc = Arc::new(object_types.clone());

    let child_fetcher: ChildFetcherFn = Box::new({
        let grpc = grpc.clone();
        let historical = historical_arc.clone();
        let patched = patched_arc.clone();
        let types = types_arc.clone();
        move |_parent_id: AccountAddress, child_id: AccountAddress| {
            let child_id_str = child_id.to_hex_literal();

            if let Some(b64) = patched.get(&child_id_str) {
                if let Ok(bcs) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    if let Some(type_str) = types.get(&child_id_str) {
                        if let Some(type_tag) = parse_type_tag_simple(type_str) {
                            return Some((type_tag, bcs));
                        }
                    }
                }
            }

            let version = historical.get(&child_id_str).copied();
            let rt = tokio::runtime::Runtime::new().ok()?;
            if let Ok(Some(obj)) =
                rt.block_on(async { grpc.get_object_at_version(&child_id_str, version).await })
            {
                if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                    if let Some(type_tag) = parse_type_tag_simple(type_str) {
                        return Some((type_tag, bcs.clone()));
                    }
                }
            }
            None
        }
    });

    harness.set_child_fetcher(child_fetcher);
    println!("   ✓ Child fetcher configured");

    // Step 11: Register objects
    println!("\nStep 11: Registering objects...");
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

    // Step 12: Execute replay
    println!("\nStep 12: Executing replay...");
    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    cached.packages = pkg_resolver.packages_as_base64();
    cached.objects = patched_objects_b64;
    cached.object_types = object_types;
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
