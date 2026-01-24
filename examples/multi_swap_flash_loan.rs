//! Multi-Swap Flash Loan Replay Example (No Cache)
//!
//! Demonstrates replaying a flash loan arbitrage transaction that routes through multiple
//! DEXes (Kriya, Bluefin, Cetus) to achieve profit.
//!
//! Run with: cargo run --example multi_swap_flash_loan
//!
//! ## Transaction Overview
//!
//! This example replays a complex arbitrage/routing transaction that:
//! 1. Borrows SUI via flash loan from a lending pool
//! 2. Swaps through Kriya spot_dex (SUI -> USDC Portal)
//! 3. Routes through Bluefin/Cetus CLMM pools
//! 4. Returns profit in SUI
//!
//! ## Key Techniques
//!
//! 1. **HistoricalPackageResolver**: Automatically follows linkage tables for package upgrades
//! 2. **HistoricalStateReconstructor**: Patches version fields using WELL_KNOWN_VERSION_CONFIGS
//! 3. **Address Aliasing**: Maps storage IDs to bytecode addresses for upgraded packages
//! 4. **Dynamic Field Children**: On-demand fetching of child objects
//!
//! ## Version-Lock Solution
//!
//! Cetus CLMM has a `checked_package_version` guard that compares:
//! - `GlobalConfig.package_version` (stored on-chain, e.g., 5)
//! - `CURRENT_VERSION` (constant in bytecode = 1)
//!
//! The Cetus v1 bytecode uses equality check (`package_version == 1`), not greater-than-or-equal.
//! The `HistoricalStateReconstructor` automatically patches GlobalConfig using
//! `WELL_KNOWN_VERSION_CONFIGS` which specifies:
//! - Field position: FromEnd(8) (package_version is the last u64 field)
//! - Default version: 1 (required by Cetus v1 bytecode equality check)
//!
//! ## Required Setup
//!
//! Configure gRPC endpoint and API key in your `.env` file:
//! ```
//! # Generic configuration (recommended)
//! SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443
//! SUI_GRPC_API_KEY=your-api-key-here
//!
//! # Or use legacy Surflux-specific variables (still supported)
//! SURFLUX_API_KEY=your-api-key-here
//! ```
//!
//! ## Protocols Involved
//!
//! | Protocol | Package Prefix | Function |
//! |----------|----------------|----------|
//! | Kriya DEX | `0xa0eba10b...` | `spot_dex::swap_token_y` |
//! | Flash Loan Pool | `0xcaf6ba05...` | `pool::borrow_flashloan_quote` |
//! | Cetus CLMM | `0x1eabed72...` | `pool::flash_swap` (via Bluefin adapter) |
//! | Bluefin CLMM | `0x75b2e9ec...` | `pool::flash_swap` |

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

/// Kriya multi-hop swap with flash loan - successful on-chain
/// Routes: SUI -> USDC (Portal) -> USDC (native) -> SCA -> SUI
const KRIYA_SWAP_TX: &str = "63fPrufC6iYHdNzG7mXscaKkqTaYH8h4RQHuiUfUCXqz";

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║      Kriya DEX Multi-Hop Swap Replay - Pure gRPC (No Cache)          ║");
    println!("║                                                                      ║");
    println!("║  Demonstrates replaying a complex swap routing through multiple      ║");
    println!("║  protocols (Kriya, Bluefin CLMM, etc.) with flash loan integration.  ║");
    println!("║  Configure SUI_GRPC_ENDPOINT and SUI_GRPC_API_KEY in .env file.      ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let tx_digest = KRIYA_SWAP_TX;
    let description = "Kriya Multi-Hop Swap";
    let expected_success = true;

    println!("\n{}", "=".repeat(74));
    println!("  {} - {}", description, tx_digest);
    println!("  On-chain result: SUCCESS");
    println!("  Expected local:  SUCCESS (with automatic object patching)");
    println!("{}\n", "=".repeat(74));

    let result = replay_via_grpc_no_cache(tx_digest);

    let (local_success, error_msg) = match result {
        Ok(success) => (success, None),
        Err(e) => (false, Some(e.to_string())),
    };

    let matches = local_success == expected_success;

    println!("\n  ══════════════════════════════════════════════════════════════");
    println!(
        "  Local result: {}",
        if local_success { "SUCCESS" } else { "FAILURE" }
    );
    println!(
        "  Expected:     {}",
        if expected_success {
            "SUCCESS"
        } else {
            "FAILURE"
        }
    );
    println!("  Match:        {}", if matches { "✓ YES" } else { "✗ NO" });
    if let Some(err) = &error_msg {
        let truncated = if err.len() > 100 { &err[..100] } else { err };
        println!("  Error:        {}...", truncated);
    }
    println!("  ══════════════════════════════════════════════════════════════");

    println!("\n\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    let status = if matches { "✓" } else { "✗" };
    let local_str = if local_success { "SUCCESS" } else { "FAILURE" };
    let expected_str = if expected_success {
        "SUCCESS"
    } else {
        "FAILURE"
    };
    println!(
        "║ {} {:25} | local: {:7} | expected: {:7} ║",
        status, description, local_str, expected_str
    );

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if matches && local_success {
        println!("║ ✓ TRANSACTION REPLAYED SUCCESSFULLY                                 ║");
        println!("║                                                                      ║");
        println!("║ HistoricalStateReconstructor automatically fixed version-locked     ║");
        println!("║ GlobalConfig using WELL_KNOWN_VERSION_CONFIGS (FromEnd(8), v=1).    ║");
    } else if matches {
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME                              ║");
    } else {
        println!("║ ✗ TRANSACTION DID NOT MATCH EXPECTED OUTCOME                        ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Replay a transaction using composable utilities for package resolution and object patching.
fn replay_via_grpc_no_cache(tx_digest: &str) -> Result<bool> {
    let rt = Arc::new(tokio::runtime::Runtime::new()?);

    // =========================================================================
    // Step 1: Connect to gRPC endpoint
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

    // =========================================================================
    // Step 2: Fetch transaction via gRPC
    // =========================================================================
    println!("\nStep 2: Fetching transaction via gRPC...");

    let grpc_tx = rt
        .as_ref()
        .block_on(async { grpc.get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found: {}", tx_digest))?;

    println!("   Digest: {}", grpc_tx.digest);
    println!("   Sender: {}", grpc_tx.sender);
    println!("   Commands: {}", grpc_tx.commands.len());
    println!("   Inputs: {}", grpc_tx.inputs.len());
    println!("   Status: {:?}", grpc_tx.status);

    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Step 3: Collect all historical object versions from gRPC effects
    // =========================================================================
    println!("\nStep 3: Collecting historical object versions...");

    let mut historical_versions: HashMap<String, u64> = HashMap::new();

    println!(
        "   unchanged_loaded_runtime_objects: {}",
        grpc_tx.unchanged_loaded_runtime_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    println!("   changed_objects: {}", grpc_tx.changed_objects.len());
    for (id, ver) in &grpc_tx.changed_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    println!(
        "   unchanged_consensus_objects: {}",
        grpc_tx.unchanged_consensus_objects.len()
    );
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

    println!("   Total unique objects: {}", historical_versions.len());

    // =========================================================================
    // Step 4: Fetch all objects at historical versions via gRPC
    // =========================================================================
    println!("\nStep 4: Fetching objects at historical versions via gRPC...");

    let mut raw_objects: HashMap<String, Vec<u8>> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut package_ids_to_fetch: Vec<String> = Vec::new();

    for cmd in &grpc_tx.commands {
        if let sui_move_interface_extractor::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            package_ids_to_fetch.push(package.clone());
        }
    }
    println!(
        "   Packages referenced in commands: {}",
        package_ids_to_fetch.len()
    );

    let mut fetched_count = 0;
    let mut failed_count = 0;

    for (obj_id, version) in &historical_versions {
        let result = rt
            .as_ref()
            .block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await });

        match result {
            Ok(Some(obj)) => {
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
            Ok(None) => {
                println!(
                    "   ! Object not found: {} @ v{}",
                    &obj_id[..20.min(obj_id.len())],
                    version
                );
                failed_count += 1;
            }
            Err(e) => {
                println!(
                    "   ! Failed to fetch {} @ v{}: {}",
                    &obj_id[..20.min(obj_id.len())],
                    version,
                    e
                );
                failed_count += 1;
            }
        }
    }

    println!(
        "   ✓ Fetched {} raw objects ({} failed)",
        fetched_count, failed_count
    );

    // =========================================================================
    // Step 5: Use HistoricalPackageResolver to fetch packages with linkage
    // =========================================================================
    println!("\nStep 5: Resolving packages with HistoricalPackageResolver...");

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
    if !linkage_upgrades.is_empty() {
        println!("   Linkage upgrades: {} mappings", linkage_upgrades.len());
    }

    println!("   ✓ Resolved {} packages", pkg_resolver.package_count());

    // =========================================================================
    // Step 6: Build module resolver
    // =========================================================================
    println!("\nStep 6: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();

    let mut module_load_count = 0;
    let mut alias_count = 0;
    let mut skipped_count = 0;

    let all_packages: Vec<(String, Vec<(String, String)>)> =
        pkg_resolver.packages_as_base64().into_iter().collect();
    let mut packages_with_source: Vec<(String, Vec<(String, String)>, Option<String>, bool)> =
        Vec::new();

    for (pkg_id, modules_b64) in all_packages {
        if let Some(upgraded) = linkage_upgrades.get(&pkg_id as &str) {
            if pkg_resolver.get_package(upgraded).is_some() {
                skipped_count += 1;
                continue;
            }
        }

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

        let is_original = source_addr_opt
            .as_ref()
            .map(|src: &String| {
                pkg_id.contains(&src[..src.len().min(20)])
                    || src.contains(&pkg_id[..pkg_id.len().min(20)])
            })
            .unwrap_or(false);

        packages_with_source.push((pkg_id, modules_b64, source_addr_opt, is_original));
    }

    packages_with_source.sort_by(|a, b| {
        if a.3 != b.3 {
            return b.3.cmp(&a.3);
        }
        a.0.cmp(&b.0)
    });

    let mut loaded_source_addrs: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for (pkg_id, modules_b64, source_addr_opt, _is_original) in packages_with_source {
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
                module_load_count += count;
                if let (Some(target), Some(source)) = (target_addr, source_addr) {
                    if target != source {
                        alias_count += 1;
                    }
                }
                if let Some(src) = source_addr {
                    loaded_source_addrs.insert(src.to_hex_literal());
                }
            }
            Err(e) => {
                println!(
                    "   ! Failed to load package {}: {}",
                    &pkg_id[..16.min(pkg_id.len())],
                    e
                );
            }
        }
    }
    println!(
        "   ✓ Loaded {} user modules ({} packages with aliases, {} skipped)",
        module_load_count, alias_count, skipped_count
    );

    match resolver.load_sui_framework() {
        Ok(n) => println!("   ✓ Loaded {} framework modules", n),
        Err(e) => println!("   ! Framework load failed: {}", e),
    }

    println!("   ✓ Resolver ready");

    // =========================================================================
    // Step 7: Use HistoricalStateReconstructor to patch objects
    // =========================================================================
    println!("\nStep 7: Using HistoricalStateReconstructor to patch objects...");

    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.set_timestamp(tx_timestamp_ms);
    reconstructor.configure_from_modules(resolver.compiled_modules());

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

    // Reconstruct state - automatically patches well-known protocol objects
    // like Cetus GlobalConfig using WELL_KNOWN_VERSION_CONFIGS:
    // - Field position: FromEnd(8) (package_version is the last u64 field)
    // - Default version: 1 (Cetus v1 bytecode uses equality check: package_version == 1)
    let reconstructed = reconstructor.reconstruct(&raw_objects, &object_types);

    let stats = &reconstructed.stats;
    println!(
        "   Patching Statistics: struct={}, raw={}, override={}, total={}",
        stats.struct_patched,
        stats.raw_patched,
        stats.override_patched,
        stats.total_patched()
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

    println!("   ✓ Patched {} objects", patched_objects_b64.len());

    // =========================================================================
    // Step 8: Create VM harness
    // =========================================================================
    println!("\nStep 8: Creating VM harness...");

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;
    println!("   Sender: 0x{}", hex::encode(sender_address.as_ref()));

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;
    println!("   ✓ VM harness created");

    // =========================================================================
    // Step 9: Set up on-demand child fetcher
    // =========================================================================
    println!("\nStep 9: Setting up on-demand child fetcher...");

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
    println!("   ✓ Child fetcher configured");

    // =========================================================================
    // Step 10: Register input objects
    // =========================================================================
    println!("\nStep 10: Registering input objects...");

    let mut registered_count = 0;
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
            registered_count += 1;
        }
    }
    println!("   ✓ Registered {} input objects", registered_count);

    // =========================================================================
    // Step 11: Build transaction and execute replay
    // =========================================================================
    println!("\nStep 11: Executing transaction replay...");

    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    cached.packages = pkg_resolver.packages_as_base64();
    cached.objects = patched_objects_b64;
    cached.object_types = object_types;
    cached.object_versions = historical_versions.clone();

    let address_aliases = sui_sandbox_core::tx_replay::build_address_aliases_for_test(&cached);
    if !address_aliases.is_empty() {
        println!("   Address aliases for replay: {}", address_aliases.len());
        for (runtime, bytecode) in address_aliases.iter().take(5) {
            println!(
                "      {} -> {}",
                &runtime.to_hex_literal()[..20],
                &bytecode.to_hex_literal()[..20]
            );
        }
        if address_aliases.len() > 5 {
            println!("      ... and {} more", address_aliases.len() - 5);
        }
    }

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
