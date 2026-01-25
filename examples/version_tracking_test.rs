#![allow(clippy::type_complexity)]
//! Version Tracking Test - Verify version tracking functionality
//!
//! This example tests the new version tracking feature:
//! 1. Synthetic test: Verify core version tracking with framework-only operations
//! 2. Mainnet test: Replay a real transaction with version tracking enabled
//!
//! Run with: cargo run --example version_tracking_test
//!
//! Required environment variables (for mainnet test):
//!   SUI_GRPC_ENDPOINT - gRPC endpoint (default: mainnet fullnode)
//!   SUI_GRPC_API_KEY  - API key for gRPC (optional)

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use common::extract_package_ids_from_type;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use sui_sandbox_core::object_runtime::ChildFetcherFn;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput, PTBExecutor};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_sandbox_core::tx_replay::{
    build_address_aliases_for_test, grpc_to_fetched_transaction, replay_with_version_tracking,
    CachedTransaction,
};
use sui_sandbox_core::utilities::{
    grpc_object_to_package_data, CallbackPackageFetcher, HistoricalPackageResolver,
    HistoricalStateReconstructor,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_transport::grpc::{GrpcClient, GrpcInput};

/// Multi-swap flash loan transaction - routes through multiple DEXes
/// Same transaction used in multi_swap_flash_loan.rs example
const MULTI_SWAP_TX: &str = "63fPrufC6iYHdNzG7mXscaKkqTaYH8h4RQHuiUfUCXqz";

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║              Version Tracking Integration Test                        ║");
    println!("║                                                                       ║");
    println!("║  Tests that version tracking correctly captures input/output versions ║");
    println!("║  using replay_with_version_tracking() and PTBExecutor directly.       ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    // Test 1: Synthetic test with framework operations (no external packages)
    test_synthetic_version_tracking()?;

    // Test 2: Mainnet replay with full version tracking
    println!("\n");
    test_version_tracking(MULTI_SWAP_TX, "Multi-Swap Flash Loan")?;

    println!("\n✅ Version tracking test completed!");
    Ok(())
}

/// Test version tracking with a synthetic PTB using only framework operations.
/// This validates the core version tracking machinery without external package dependencies.
fn test_synthetic_version_tracking() -> Result<()> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Test 1: Synthetic Version Tracking (Framework Only)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Create simulation environment with framework packages
    let mut env = SimulationEnvironment::new()?;

    let sender = AccountAddress::from_hex_literal(
        "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )?;
    env.set_sender(sender);

    println!("Step 1: Creating test objects with versions...");

    // Create two coins for merge testing (simulating historical state)
    let coin1_id = env.create_coin("0x2::sui::SUI", 5_000_000_000)?;
    let coin2_id = env.create_coin("0x2::sui::SUI", 3_000_000_000)?;
    println!("   ✓ Created coin1: 0x{:x}", coin1_id);
    println!("   ✓ Created coin2: 0x{:x}", coin2_id);

    // Manually set versions for the objects to simulate historical input versions
    // (In real replay, this comes from gRPC response)
    let coin1_input_version = 42u64;
    let coin2_input_version = 100u64;

    println!(
        "   ✓ Simulated coin1 input version: {}",
        coin1_input_version
    );
    println!(
        "   ✓ Simulated coin2 input version: {}",
        coin2_input_version
    );

    println!("\nStep 2: Building PTB with version tracking...");

    // Build a simple PTB: merge coins (coin2 into coin1)
    let sui_type = TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
        address: AccountAddress::from_hex_literal("0x2")?,
        module: Identifier::new("sui")?,
        name: Identifier::new("SUI")?,
        type_params: vec![],
    }));

    // Get coin bytes from environment before creating executor
    let coin1_bytes = env
        .get_object(&coin1_id)
        .ok_or_else(|| anyhow!("Coin1 not found"))?
        .bcs_bytes
        .clone();
    let coin2_bytes = env
        .get_object(&coin2_id)
        .ok_or_else(|| anyhow!("Coin2 not found"))?
        .bcs_bytes
        .clone();

    // Get the harness from environment
    let resolver = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;
    let config = SimulationConfig::default().with_sender_address(sender);
    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // Create a PTB executor with version tracking enabled
    let mut executor = PTBExecutor::new(&mut harness);
    executor.set_track_versions(true);
    // Lamport timestamp is max(input_versions) + 1
    let max_input_version = coin1_input_version.max(coin2_input_version);
    executor.set_lamport_timestamp(max_input_version + 1);

    // Add the coins as inputs with versions
    executor.add_input(InputValue::Object(ObjectInput::Owned {
        id: coin1_id,
        bytes: coin1_bytes,
        type_tag: Some(sui_type.clone()),
        version: Some(coin1_input_version), // <-- Version tracking!
    }));
    executor.add_input(InputValue::Object(ObjectInput::Owned {
        id: coin2_id,
        bytes: coin2_bytes,
        type_tag: Some(sui_type.clone()),
        version: Some(coin2_input_version), // <-- Version tracking!
    }));

    // Build command: merge coin2 into coin1
    // MergeCoins will mutate coin1 (destination) and delete coin2 (source)
    let commands = vec![Command::MergeCoins {
        destination: Argument::Input(0),
        sources: vec![Argument::Input(1)],
    }];

    println!("   ✓ Commands: MergeCoins (coin2 into coin1)");

    println!("\nStep 3: Executing with version tracking...");

    let effects = executor.execute_commands(&commands)?;

    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                    VERSION TRACKING RESULTS                          ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    if effects.success {
        println!("║ ✓ Execution: SUCCESS                                                ║");
    } else {
        println!("║ ✗ Execution: FAILED                                                 ║");
        if let Some(err) = &effects.error {
            println!("║   Error: {}...", &err[..err.len().min(50)]);
        }
    }

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Created: {:<4}  Mutated: {:<4}  Deleted: {:<4}                         ║",
        effects.created.len(),
        effects.mutated.len(),
        effects.deleted.len()
    );

    if let Some(lamport) = effects.lamport_timestamp {
        println!(
            "║ Lamport timestamp: {:<10}                                        ║",
            lamport
        );
    }

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║ Object Versions:                                                     ║");

    if let Some(versions) = &effects.object_versions {
        for (obj_id, info) in versions {
            let id_short = &obj_id.to_hex_literal()[..18];
            let input_v = info
                .input_version
                .map_or("new".to_string(), |v| v.to_string());
            println!(
                "║   {}...: {} -> {} ({:?})      ║",
                id_short, input_v, info.output_version, info.change_type
            );
        }

        // Validate version increments
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ Version Validation:                                                  ║");

        let mut valid_increments = 0;
        let mut total_increments = 0;

        for info in versions.values() {
            if let Some(input_v) = info.input_version {
                total_increments += 1;
                let expected_output = input_v + 1;
                if info.output_version == expected_output {
                    valid_increments += 1;
                }
            }
        }

        if total_increments > 0 {
            println!(
                "║   • Objects with version increment: {}/{} correct                 ║",
                valid_increments, total_increments
            );
            if valid_increments == total_increments {
                println!("║   • ✓ All version increments valid (output = input + 1)           ║");
            }
        }

        let created_count = versions
            .values()
            .filter(|v| v.input_version.is_none())
            .count();
        if created_count > 0 {
            println!(
                "║   • Created objects: {} (assigned lamport timestamp)               ║",
                created_count
            );
        }
    } else {
        println!("║   (No version tracking data - this shouldn't happen!)              ║");
    }

    println!("╚══════════════════════════════════════════════════════════════════════╝");

    // Verify expectations
    assert!(effects.success, "Execution should succeed");
    assert!(
        effects.object_versions.is_some(),
        "Version tracking should be enabled"
    );

    let versions = effects.object_versions.as_ref().unwrap();
    assert!(!versions.is_empty(), "Should have tracked object versions");

    // Verify coin1 was mutated with correct version
    // In Sui, all objects in a transaction get the SAME output version (lamport timestamp)
    let expected_lamport = max_input_version + 1;

    if let Some(coin1_info) = versions.get(&coin1_id) {
        assert_eq!(
            coin1_info.input_version,
            Some(coin1_input_version),
            "Input version should match what we provided"
        );
        assert!(
            coin1_info.output_version > coin1_input_version,
            "Output version should be > input version for mutated object"
        );
        assert_eq!(
            coin1_info.output_version, expected_lamport,
            "Output version should equal lamport timestamp"
        );
        println!(
            "\n✓ Version tracking validated: coin1 mutated {} -> {} (lamport={})",
            coin1_input_version, coin1_info.output_version, expected_lamport
        );
    }

    // Verify coin2 was deleted with correct version
    if let Some(coin2_info) = versions.get(&coin2_id) {
        assert_eq!(
            coin2_info.input_version,
            Some(coin2_input_version),
            "Input version should match what we provided"
        );
        assert!(
            coin2_info.output_version > coin2_input_version,
            "Output version should be > input version even for deleted object"
        );
        assert_eq!(
            coin2_info.output_version, expected_lamport,
            "Output version should equal lamport timestamp"
        );
        println!(
            "✓ Version tracking validated: coin2 deleted {} -> {} (lamport={})",
            coin2_input_version, coin2_info.output_version, expected_lamport
        );
    }

    println!("\n✅ Synthetic version tracking test PASSED!");
    println!(
        "   All objects correctly tracked with lamport timestamp {}",
        expected_lamport
    );

    Ok(())
}

fn test_version_tracking(tx_digest: &str, tx_name: &str) -> Result<()> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Testing: {} ({})", tx_name, tx_digest);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let rt = Arc::new(tokio::runtime::Runtime::new()?);

    // =========================================================================
    // Step 1: Connect to gRPC and fetch transaction
    // =========================================================================
    println!("Step 1: Fetching transaction from mainnet...");

    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let grpc = Arc::new(grpc);

    let grpc_tx = rt
        .as_ref()
        .block_on(async { grpc.get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found: {}", tx_digest))?;

    println!("   ✓ Transaction digest: {}", grpc_tx.digest);
    println!("   ✓ Commands: {}", grpc_tx.commands.len());

    // Print command details
    println!("\n   PTB Command Sequence:");
    for (i, cmd) in grpc_tx.commands.iter().enumerate() {
        match cmd {
            sui_move_interface_extractor::grpc::GrpcCommand::MoveCall {
                package,
                module,
                function,
                ..
            } => {
                println!(
                    "     {:2}. MoveCall: {}::{}::{}",
                    i,
                    &package[..package.len().min(16)],
                    module,
                    function
                );
            }
            sui_move_interface_extractor::grpc::GrpcCommand::TransferObjects { .. } => {
                println!("     {:2}. TransferObjects", i);
            }
            sui_move_interface_extractor::grpc::GrpcCommand::SplitCoins { .. } => {
                println!("     {:2}. SplitCoins", i);
            }
            sui_move_interface_extractor::grpc::GrpcCommand::MergeCoins { .. } => {
                println!("     {:2}. MergeCoins", i);
            }
            _ => {
                println!("     {:2}. {:?}", i, std::mem::discriminant(cmd));
            }
        }
    }

    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Step 2: Collect historical versions from gRPC response
    // =========================================================================
    println!("\nStep 2: Collecting historical object versions...");

    let mut historical_versions: HashMap<String, u64> = HashMap::new();

    // These are the input versions we expect to see in version tracking
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

    println!(
        "   ✓ Found {} objects with versions",
        historical_versions.len()
    );

    // =========================================================================
    // Step 3: Fetch objects and packages
    // =========================================================================
    println!("\nStep 3: Fetching objects...");

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

    // =========================================================================
    // Step 4: Resolve packages using HistoricalPackageResolver
    // =========================================================================
    println!("\nStep 4: Resolving packages...");

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

    // =========================================================================
    // Step 5: Build module resolver (with proper sorting - originals first)
    // =========================================================================
    println!("\nStep 5: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();
    let linkage_upgrades = pkg_resolver.linkage_upgrades();

    if !linkage_upgrades.is_empty() {
        println!("   Linkage upgrades: {} mappings", linkage_upgrades.len());
    }

    let all_packages: Vec<(String, Vec<(String, String)>)> =
        pkg_resolver.packages_as_base64().into_iter().collect();

    // Build packages_with_source for sorting (originals first)
    let mut packages_with_source: Vec<(String, Vec<(String, String)>, Option<String>, bool)> =
        Vec::new();

    for (pkg_id, modules_b64) in all_packages {
        if let Some(upgraded) = linkage_upgrades.get(&pkg_id as &str) {
            if pkg_resolver.get_package(upgraded).is_some() {
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

    // Sort: originals first, then by pkg_id
    packages_with_source.sort_by(|a, b| {
        if a.3 != b.3 {
            return b.3.cmp(&a.3);
        }
        a.0.cmp(&b.0)
    });

    let mut loaded_source_addrs: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut module_load_count = 0;
    let mut alias_count = 0;

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
        "   ✓ Loaded {} user modules ({} packages with aliases)",
        module_load_count, alias_count
    );

    match resolver.load_sui_framework() {
        Ok(n) => println!("   ✓ Loaded {} framework modules", n),
        Err(e) => println!("   ! Framework load failed: {}", e),
    }

    // =========================================================================
    // Step 6: Patch objects using HistoricalStateReconstructor
    // =========================================================================
    println!("\nStep 6: Patching objects for version compatibility...");

    let mut reconstructor = HistoricalStateReconstructor::new();
    reconstructor.set_timestamp(tx_timestamp_ms);
    reconstructor.configure_from_modules(resolver.compiled_modules());

    let reconstructed = reconstructor.reconstruct(&raw_objects, &object_types);
    println!(
        "   ✓ Patched {} objects (struct={}, raw={})",
        reconstructed.stats.total_patched(),
        reconstructed.stats.struct_patched,
        reconstructed.stats.raw_patched
    );

    // Convert to base64 for replay
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

    // =========================================================================
    // Step 7: Create VM harness
    // =========================================================================
    println!("\nStep 7: Creating VM harness...");

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;
    println!("   Sender: 0x{}", hex::encode(sender_address.as_ref()));

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;
    println!("   ✓ VM harness created");

    // =========================================================================
    // Step 8: Set up on-demand child fetcher (for dynamic field loading)
    // =========================================================================
    println!("\nStep 8: Setting up on-demand child fetcher...");

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
                        if let Some(type_tag) = common::parse_type_tag_simple(type_str) {
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
                    if let Some(type_tag) = common::parse_type_tag_simple(type_str) {
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
    // Step 9: Register input objects with versions
    // =========================================================================
    println!("\nStep 9: Registering input objects...");

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
    // Step 10: Build transaction and execute with version tracking
    // =========================================================================
    println!("\nStep 10: Executing transaction with version tracking...");

    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx.clone());
    cached.packages = pkg_resolver.packages_as_base64();
    cached.objects = patched_objects_b64.clone();
    cached.object_types = object_types.clone();
    cached.object_versions = historical_versions.clone();

    // Build address aliases
    let address_aliases = build_address_aliases_for_test(&cached);
    if !address_aliases.is_empty() {
        println!("   Address aliases for replay: {}", address_aliases.len());
        for (runtime, bytecode) in address_aliases.iter().take(3) {
            println!(
                "      {} -> {}",
                &runtime.to_hex_literal()[..20],
                &bytecode.to_hex_literal()[..20]
            );
        }
    }
    harness.set_address_aliases(address_aliases.clone());

    // Execute with version tracking using the new function!
    let result = replay_with_version_tracking(
        &fetched_tx,
        &mut harness,
        &patched_objects_b64,
        &address_aliases,
        Some(&historical_versions), // <-- This enables version tracking!
    )?;

    // =========================================================================
    // Step 11: Display version tracking results
    // =========================================================================
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                    VERSION TRACKING RESULTS                          ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    if result.local_success {
        println!("║ ✓ Execution: SUCCESS                                                ║");
    } else {
        println!("║ ✗ Execution: FAILED                                                 ║");
        if let Some(err) = &result.local_error {
            // Print full error for debugging
            eprintln!("\n  Full error: {}", err);
        }
    }

    // Note: The ReplayResult doesn't include effects directly, but the version
    // tracking info is in the TransactionEffects returned by the executor.
    // For a complete test, we'd need to modify ReplayResult to include this.

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║ Version Tracking Status:                                             ║");
    println!(
        "║   • Input versions collected: {:>4}                                  ║",
        historical_versions.len()
    );
    println!("║   • replay_with_version_tracking() called: YES                       ║");
    println!(
        "║   • Lamport timestamp: {:>10}                                    ║",
        historical_versions.values().copied().max().unwrap_or(0) + 1
    );

    // Print on-chain effects for comparison
    if let Some(on_chain) = &fetched_tx.effects {
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ On-Chain Effects:                                                    ║");
        println!(
            "║   Created: {:<4}  Mutated: {:<4}  Deleted: {:<4}  Wrapped: {:<4}       ║",
            on_chain.created.len(),
            on_chain.mutated.len(),
            on_chain.deleted.len(),
            on_chain.wrapped.len()
        );
    }

    if let Some(comparison) = &result.comparison {
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ Effects Comparison:                                                  ║");
        println!(
            "║   Status match: {}                                                  ║",
            if comparison.status_match {
                "YES"
            } else {
                "NO "
            }
        );
        println!(
            "║   Created count match: {}                                           ║",
            if comparison.created_count_match {
                "YES"
            } else {
                "NO "
            }
        );
        println!(
            "║   Mutated count match: {}                                           ║",
            if comparison.mutated_count_match {
                "YES"
            } else {
                "NO "
            }
        );
        println!(
            "║   Match score: {:.1}%                                                ║",
            comparison.match_score * 100.0
        );

        // Display version tracking comparison results
        if comparison.version_tracking_enabled {
            println!("╠══════════════════════════════════════════════════════════════════════╣");
            println!("║ Version Tracking Validation:                                         ║");
            println!(
                "║   • Input versions matched: {}/{:<4}                                ║",
                comparison.input_versions_matched, comparison.input_versions_total
            );
            println!(
                "║   • Version increments valid: {}/{:<4}                              ║",
                comparison.version_increments_valid, comparison.version_increments_total
            );

            if !comparison.version_mismatches.is_empty() {
                println!(
                    "║   • Mismatches found: {:<4}                                         ║",
                    comparison.version_mismatches.len()
                );
                for (i, mismatch) in comparison.version_mismatches.iter().take(3).enumerate() {
                    let obj_short = &mismatch.object_id[..mismatch.object_id.len().min(16)];
                    println!(
                        "║     {}. {}... {:?}                         ║",
                        i + 1,
                        obj_short,
                        mismatch.mismatch_type
                    );
                }
            } else {
                println!("║   • All versions validated correctly ✓                             ║");
            }
        }

        if !comparison.notes.is_empty() {
            println!("╠══════════════════════════════════════════════════════════════════════╣");
            println!("║ Notes:                                                               ║");
            for note in comparison.notes.iter().take(3) {
                let note_short = &note[..note.len().min(60)];
                println!("║   • {}{}║", note_short, " ".repeat(60 - note_short.len()));
            }
        }
    }

    // Display version summary from ReplayResult
    if let Some(summary) = &result.version_summary {
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!("║ Version Summary (from local execution):                              ║");
        println!(
            "║   • Objects tracked: {:<4}                                            ║",
            result.objects_tracked
        );
        println!(
            "║   • Created: {:<4}  Mutated: {:<4}  Deleted: {:<4}  Wrapped: {:<4}    ║",
            summary.created, summary.mutated, summary.deleted, summary.wrapped
        );
        if let Some(lamport) = result.lamport_timestamp {
            println!(
                "║   • Lamport timestamp: {:<10}                                    ║",
                lamport
            );
        }
    }

    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}
