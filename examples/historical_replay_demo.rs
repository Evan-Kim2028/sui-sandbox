//! Historical Replay Demo - Using HistoricalPackageResolver
//!
//! This example demonstrates the new composable utilities for historical state
//! reconstruction. It shows how to use the `HistoricalPackageResolver` and
//! `HistoricalStateReconstructor` together for complete transaction replay.
//!
//! Run with: cargo run --example historical_replay_demo
//!
//! ## Features Demonstrated
//!
//! 1. `HistoricalPackageResolver` - Fetches packages following linkage tables
//! 2. `HistoricalStateReconstructor` - High-level facade for state patching
//! 3. Automatic version detection from bytecode
//! 4. Composable utilities that work together
//!
//! ## Comparison to Old Approach
//!
//! OLD (manual setup - ~200 lines of package fetching):
//! ```ignore
//! let mut linkage_upgrades: HashMap<String, String> = HashMap::new();
//! for depth in 0..10 {
//!     // Manual linkage table processing...
//!     // Manual dependency extraction...
//!     // Manual version tracking...
//! }
//! ```
//!
//! NEW (using composable utilities):
//! ```ignore
//! let mut pkg_resolver = HistoricalPackageResolver::with_grpc(grpc, rt);
//! pkg_resolver.set_historical_versions(versions);
//! pkg_resolver.resolve_packages(&initial_ids)?;
//!
//! let mut reconstructor = HistoricalStateReconstructor::new();
//! reconstructor.configure_from_modules(pkg_resolver.all_modules());
//! ```

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use common::{extract_package_ids_from_type, parse_type_tag_simple};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use sui_data_fetcher::grpc::{GrpcClient, GrpcInput};
use sui_sandbox_core::object_runtime::ChildFetcherFn;
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::{grpc_to_fetched_transaction, CachedTransaction};
use sui_sandbox_core::utilities::{
    grpc_object_to_package_data, CallbackPackageFetcher, HistoricalPackageResolver,
    HistoricalStateReconstructor,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};

/// Kriya multi-hop swap with flash loan - a complex transaction that requires patching
const KRIYA_SWAP_TX: &str = "63fPrufC6iYHdNzG7mXscaKkqTaYH8h4RQHuiUfUCXqz";

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║    Historical Replay Demo - Using Composable Utilities               ║");
    println!("║                                                                      ║");
    println!("║  Demonstrates HistoricalPackageResolver + HistoricalStateReconstructor║");
    println!("║  working together for complete historical transaction replay.        ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let result = replay_with_composable_utilities(KRIYA_SWAP_TX)?;

    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Kriya Multi-Hop Swap    | local: {:7} | expected: SUCCESS ║",
        if result { "SUCCESS" } else { "FAILURE" }
    );
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if result {
        println!("║ ✓ TRANSACTION REPLAYED SUCCESSFULLY                                 ║");
        println!("║                                                                      ║");
        println!("║ Composable utilities automatically:                                 ║");
        println!("║   - Resolved packages via linkage tables (HistoricalPackageResolver)║");
        println!("║   - Detected version constants from upgraded bytecode              ║");
        println!("║   - Patched objects for version compatibility                      ║");
    } else {
        println!("║ ✗ TRANSACTION REPLAY FAILED                                        ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Replay using the composable utilities.
fn replay_with_composable_utilities(tx_digest: &str) -> Result<bool> {
    let rt = Arc::new(tokio::runtime::Runtime::new()?);

    // =========================================================================
    // Step 1: Connect to gRPC and fetch transaction
    // =========================================================================
    println!("Step 1: Connecting to gRPC...");

    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .or_else(|_| std::env::var("SURFLUX_GRPC_ENDPOINT"))
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY")
        .or_else(|_| std::env::var("SURFLUX_API_KEY"))
        .ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let grpc = Arc::new(grpc);
    println!("   ✓ Connected to {}", endpoint);

    println!("\nStep 2: Fetching transaction...");
    let grpc_tx = rt
        .as_ref()
        .block_on(async { grpc.get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found: {}", tx_digest))?;

    println!("   Digest: {}", grpc_tx.digest);
    println!("   Commands: {}", grpc_tx.commands.len());
    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Step 2: Collect historical versions
    // =========================================================================
    println!("\nStep 3: Collecting historical object versions...");

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
    println!("   Total objects to fetch: {}", historical_versions.len());

    // =========================================================================
    // Step 3: Fetch objects
    // =========================================================================
    println!("\nStep 4: Fetching objects...");

    let mut raw_objects: HashMap<String, Vec<u8>> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut package_ids_to_fetch: Vec<String> = Vec::new();

    // Extract package IDs from commands
    for cmd in &grpc_tx.commands {
        if let sui_move_interface_extractor::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            package_ids_to_fetch.push(package.clone());
        }
    }

    // Fetch objects
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
            }
        }
    }
    println!("   ✓ Fetched {} objects", raw_objects.len());
    println!(
        "   Initial packages to resolve: {}",
        package_ids_to_fetch.len()
    );

    // Debug: Check what version of Cetus is in historical_versions
    // =========================================================================
    // Step 4: Use HistoricalPackageResolver to fetch packages with linkage
    // =========================================================================
    println!("\nStep 5: Resolving packages with HistoricalPackageResolver...");

    // Create a callback-based fetcher that uses gRPC
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

    println!("   ✓ Resolved {} packages", pkg_resolver.package_count());
    println!(
        "   Linkage upgrades discovered: {}",
        pkg_resolver.upgrade_count()
    );

    // Show some linkage upgrades
    let linkage_upgrades = pkg_resolver.linkage_upgrades();
    if !linkage_upgrades.is_empty() {
        println!("   Sample linkage upgrades:");
        for (orig, upgraded) in linkage_upgrades.iter().take(3) {
            println!(
                "      {} -> {}",
                &orig[..orig.len().min(20)],
                &upgraded[..upgraded.len().min(20)]
            );
        }
        if linkage_upgrades.len() > 3 {
            println!("      ... and {} more", linkage_upgrades.len() - 3);
        }
    }

    // =========================================================================
    // Step 5: Build module resolver from resolved packages
    // =========================================================================
    println!("\nStep 6: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();
    let mut module_count = 0;
    let mut alias_count = 0;

    // First pass: Collect all packages and their source addresses
    // Then sort to prioritize original packages (where pkg_id == source_addr)
    let all_packages: Vec<(String, Vec<(String, String)>)> =
        pkg_resolver.packages_as_base64().into_iter().collect();
    let mut packages_with_source: Vec<(String, Vec<(String, String)>, Option<String>, bool)> =
        Vec::new();

    for (pkg_id, modules_b64) in all_packages {
        // Skip if this package is superseded by an upgraded version
        if let Some(upgraded) = linkage_upgrades.get(&pkg_id as &str) {
            if pkg_resolver.get_package(upgraded).is_some() {
                continue;
            }
        }

        // Peek at the first module to get the source address
        let source_addr_opt: Option<String> =
            modules_b64.first().and_then(|(_, b64): &(String, String)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .and_then(|bytes| {
                        CompiledModule::deserialize_with_defaults(&bytes)
                            .ok()
                            .map(|m| m.self_id().address().to_hex_literal())
                    })
            });

        // Track if this is the original package (pkg_id contains source_addr or vice versa)
        let is_original = source_addr_opt
            .as_ref()
            .map(|src: &String| {
                pkg_id.contains(&src[..src.len().min(20)])
                    || src.contains(&pkg_id[..pkg_id.len().min(20)])
            })
            .unwrap_or(false);

        packages_with_source.push((pkg_id, modules_b64, source_addr_opt, is_original));
    }

    // Sort: original packages first, then by package ID for determinism
    packages_with_source.sort_by(|a, b| {
        // Originals first
        if a.3 != b.3 {
            return b.3.cmp(&a.3); // true (original) comes first
        }
        // Then by package ID for determinism
        a.0.cmp(&b.0)
    });

    let mut loaded_source_addrs: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for (pkg_id, modules_b64, source_addr_opt, _is_original) in packages_with_source {
        // Skip if we already loaded modules at this source address
        if let Some(ref source_addr) = source_addr_opt {
            if loaded_source_addrs.contains(source_addr) {
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(&pkg_id).ok();
        let decoded_modules: Vec<(String, Vec<u8>)> = modules_b64
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        match resolver.add_package_modules_at(decoded_modules, target_addr) {
            Ok((count, source_addr)) => {
                module_count += count;
                if let (Some(target), Some(source)) = (target_addr, source_addr) {
                    if target != source {
                        alias_count += 1;
                    }
                }
                // Track loaded source addresses to avoid duplicates
                if let Some(src) = source_addr {
                    loaded_source_addrs.insert(src.to_hex_literal());
                }
            }
            Err(e) => {
                eprintln!(
                    "   ! Failed to load package {}: {}",
                    &pkg_id[..pkg_id.len().min(20)],
                    e
                );
            }
        }
    }

    resolver.load_sui_framework()?;
    println!(
        "   ✓ Loaded {} user modules ({} with aliases)",
        module_count, alias_count
    );

    // =========================================================================
    // Step 6: Use HistoricalStateReconstructor to patch objects
    // =========================================================================
    println!("\nStep 7: Using HistoricalStateReconstructor to patch objects...");

    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.set_timestamp(tx_timestamp_ms);
    reconstructor.configure_from_modules(resolver.compiled_modules());

    // Print detected versions
    let detected_versions = reconstructor.detected_versions();
    if !detected_versions.is_empty() {
        println!("   Detected version constants:");
        for (addr, version) in detected_versions.iter().take(5) {
            println!("      {} -> v{}", &addr[..addr.len().min(24)], version);
        }
        if detected_versions.len() > 5 {
            println!("      ... and {} more", detected_versions.len() - 5);
        }
    }

    // Reconstruct state
    // The reconstructor automatically patches well-known protocol objects like
    // Cetus GlobalConfig using WELL_KNOWN_VERSION_CONFIGS which specifies:
    // - Field position: FromEnd(8) (package_version is the last u64 field)
    // - Default version: 1 (Cetus v1 bytecode uses equality check: package_version == 1)
    let reconstructed = reconstructor.reconstruct(&raw_objects, &object_types);

    // Print statistics
    let stats = &reconstructed.stats;
    println!(
        "   Patching Statistics: struct={}, raw={}, override={}, total={}",
        stats.struct_patched,
        stats.raw_patched,
        stats.override_patched,
        stats.total_patched()
    );

    // Convert to base64 for cached transaction
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

    println!("   ✓ Reconstructed {} objects", patched_objects_b64.len());

    // =========================================================================
    // Step 7: Build cached transaction and execute
    // =========================================================================
    println!("\nStep 8: Building transaction and executing...");

    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    // Add packages from resolver
    cached.packages = pkg_resolver.packages_as_base64();
    cached.objects = patched_objects_b64.clone();
    cached.object_types = object_types.clone();
    cached.object_versions = historical_versions.clone();

    // Create VM harness
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // Set up child fetcher
    let historical_arc = Arc::new(historical_versions.clone());
    let patched_arc = Arc::new(patched_objects_b64);
    let types_arc = Arc::new(object_types);

    let child_fetcher: ChildFetcherFn = Box::new({
        let grpc = grpc.clone();
        let historical = historical_arc.clone();
        let patched = patched_arc.clone();
        let types = types_arc.clone();
        move |_parent_id: AccountAddress, child_id: AccountAddress| {
            let child_id_str = child_id.to_hex_literal();

            // Try patched objects first
            if let Some(b64) = patched.get(&child_id_str) {
                if let Ok(bcs) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    if let Some(type_str) = types.get(&child_id_str) {
                        if let Some(type_tag) = parse_type_tag_simple(type_str) {
                            return Some((type_tag, bcs));
                        }
                    }
                }
            }

            // Fall back to gRPC
            let version = historical.get(&child_id_str).copied();
            let rt = tokio::runtime::Runtime::new().ok()?;
            let result =
                rt.block_on(async { grpc.get_object_at_version(&child_id_str, version).await });

            if let Ok(Some(obj)) = result {
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

    // Execute
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
