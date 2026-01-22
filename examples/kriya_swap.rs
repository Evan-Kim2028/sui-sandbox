//! Kriya DEX Multi-Hop Swap Replay Example (No Cache)
//!
//! Demonstrates replaying a Kriya DEX swap transaction that routes through multiple
//! pools and protocols (Kriya, Bluefin, Cetus) to achieve best execution.
//!
//! Run with: cargo run --example kriya_swap
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
//! 1. **Pure gRPC Fetching**: All data fetched via gRPC with Surflux API key
//! 2. **Multi-Protocol Routing**: Transaction spans Kriya, Bluefin, Cetus, and framework modules
//! 3. **Flash Loan Integration**: Uses `borrow_flashloan_quote` / `return_flashloan_quote`
//! 4. **Package Linkage Resolution**: Automatically follows upgrade chains
//! 5. **Object Patching**: Automatically fixes version-locked protocols (Cetus GlobalConfig)
//!
//! ## Version-Lock Solution
//!
//! Cetus CLMM has a `checked_package_version` guard that compares:
//! - `GlobalConfig.package_version` (stored on-chain, e.g., 5)
//! - `CURRENT_VERSION` (constant in bytecode, e.g., 1)
//!
//! When these don't match, execution aborts. This example uses `ObjectPatcher` to
//! automatically patch the GlobalConfig's package_version field to match the bytecode,
//! enabling successful historical replay.
//!
//! ## Required Setup
//!
//! Set `SURFLUX_API_KEY` in your `.env` file:
//! ```
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
//!
//! ## Historical Data from gRPC
//!
//! Surflux gRPC provides three critical fields for historical replay:
//! - `unchanged_loaded_runtime_objects`: Objects read but not modified (exact versions)
//! - `unchanged_consensus_objects`: Actual consensus versions for shared objects
//! - `changed_objects`: Objects modified with their INPUT versions (before tx)

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use sui_move_interface_extractor::benchmark::object_patcher::ObjectPatcher;
use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::tx_replay::{
    grpc_to_fetched_transaction, CachedTransaction,
};
use sui_move_interface_extractor::benchmark::vm::{SimulationConfig, VMHarness};
use sui_move_interface_extractor::grpc::{GrpcClient, GrpcInput};

/// Kriya multi-hop swap with flash loan - successful on-chain
/// Routes: SUI -> USDC (Portal) -> USDC (native) -> SCA -> SUI
const KRIYA_SWAP_TX: &str = "63fPrufC6iYHdNzG7mXscaKkqTaYH8h4RQHuiUfUCXqz";

fn main() -> anyhow::Result<()> {
    // Load environment from .env file (SURFLUX_API_KEY required)
    // Searches for .env in current directory, then walks up parent directories
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║      Kriya DEX Multi-Hop Swap Replay - Pure gRPC (No Cache)          ║");
    println!("║                                                                      ║");
    println!("║  Demonstrates replaying a complex swap routing through multiple      ║");
    println!("║  protocols (Kriya, Bluefin CLMM, etc.) with flash loan integration.  ║");
    println!("║  Requires SURFLUX_API_KEY in .env file.                              ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    // Kriya swap transaction - succeeded on-chain
    // This transaction routes through Cetus-style pools which have version checks.
    // We use ObjectPatcher to patch GlobalConfig.package_version to match bytecode.
    let tx_digest = KRIYA_SWAP_TX;
    let description = "Kriya Multi-Hop Swap";
    // Expected to succeed with object patching
    let expected_success = true;

    println!("\n{}", "=".repeat(74));
    println!("  {} - {}", description, tx_digest);
    println!("  On-chain result: SUCCESS");
    println!("  Expected local:  SUCCESS (with object patching)");
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

    // Summary
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
        println!("║ ObjectPatcher automatically fixed version-locked GlobalConfig        ║");
        println!("║ by patching package_version to match bytecode's CURRENT_VERSION.    ║");
    } else if matches {
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME                              ║");
    } else {
        println!("║ ✗ TRANSACTION DID NOT MATCH EXPECTED OUTCOME                        ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Replay a transaction using ONLY gRPC for data fetching (no cache).
///
/// This demonstrates the complete workflow for historical transaction replay:
/// 1. Connect to Surflux gRPC with API key
/// 2. Fetch transaction to get PTB commands and historical object versions
/// 3. Fetch all objects at their exact historical versions via gRPC
/// 4. Fetch packages via gRPC
/// 5. Execute locally and compare results
fn replay_via_grpc_no_cache(tx_digest: &str) -> Result<bool> {
    let rt = tokio::runtime::Runtime::new()?;

    // =========================================================================
    // Step 1: Connect to Surflux gRPC
    // =========================================================================
    println!("Step 1: Connecting to Surflux gRPC...");

    let api_key = std::env::var("SURFLUX_API_KEY")
        .map_err(|_| anyhow!("SURFLUX_API_KEY not set in environment. Add it to .env file."))?;

    let grpc = rt.block_on(async {
        GrpcClient::with_api_key("https://grpc.surflux.dev:443", Some(api_key)).await
    })?;
    println!("   ✓ Connected to Surflux gRPC");

    // =========================================================================
    // Step 2: Fetch transaction via gRPC
    // =========================================================================
    println!("\nStep 2: Fetching transaction via gRPC...");

    let grpc_tx = rt
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

    // unchanged_loaded_runtime_objects - objects read but not modified (includes child objects!)
    println!(
        "   unchanged_loaded_runtime_objects: {}",
        grpc_tx.unchanged_loaded_runtime_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // changed_objects - objects that were modified (INPUT version)
    println!("   changed_objects: {}", grpc_tx.changed_objects.len());
    for (id, ver) in &grpc_tx.changed_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // unchanged_consensus_objects - ACTUAL shared object versions used during execution
    // NOTE: This can include packages! Packages are immutable objects.
    println!(
        "   unchanged_consensus_objects: {}",
        grpc_tx.unchanged_consensus_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // Also add input objects from the transaction
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
                // Note: This is initial_shared_version, may be overridden by unchanged_consensus_objects
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

    let mut objects: HashMap<String, String> = HashMap::new(); // object_id -> bcs_base64
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut packages: HashMap<String, Vec<(String, String)>> = HashMap::new(); // pkg_id -> [(name, bytecode_b64)]
    let mut package_ids_to_fetch: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // Create object patcher for version-locked protocols (Cetus, Scallop, etc.)
    let mut object_patcher = ObjectPatcher::with_timestamp(tx_timestamp_ms);

    // Extract package IDs from MoveCall commands
    for cmd in &grpc_tx.commands {
        if let sui_move_interface_extractor::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            package_ids_to_fetch.insert(package.clone());
        }
    }
    println!(
        "   Packages referenced in commands: {}",
        package_ids_to_fetch.len()
    );

    let mut fetched_count = 0;
    let mut failed_count = 0;

    for (obj_id, version) in &historical_versions {
        let result =
            rt.block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await });

        match result {
            Ok(Some(obj)) => {
                if let Some(bcs) = &obj.bcs {
                    // Apply object patching for version-locked protocols
                    let type_str = obj.type_string.as_deref().unwrap_or("");
                    let patched_bcs = object_patcher.patch_object(type_str, bcs);
                    let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&patched_bcs);
                    objects.insert(obj_id.clone(), bcs_b64);
                    if let Some(type_str) = &obj.type_string {
                        object_types.insert(obj_id.clone(), type_str.clone());
                        // Extract package IDs from type string (e.g., "0xabc::module::Struct<0xdef::m::T>")
                        for pkg_id in extract_package_ids_from_type(type_str) {
                            package_ids_to_fetch.insert(pkg_id);
                        }
                    }
                    fetched_count += 1;

                    // Check if this is a package
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
        "   ✓ Fetched {} objects ({} failed)",
        fetched_count, failed_count
    );

    // Report patches applied
    let patch_stats = object_patcher.stats();
    if !patch_stats.is_empty() {
        println!("   Object patches applied:");
        for (pattern, count) in patch_stats {
            println!("      {} -> {} patches", pattern, count);
        }
    }

    // =========================================================================
    // Step 5: Fetch packages with transitive dependencies (following linkage tables)
    // =========================================================================
    println!("\nStep 5: Fetching packages with transitive dependencies...");

    let mut fetched_packages: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut packages_to_fetch = package_ids_to_fetch.clone();
    let max_depth = 10;

    // Track linkage upgrades: original_id -> upgraded_id
    // When a package's linkage table says to use an upgraded version, we track it here
    let mut linkage_upgrades: HashMap<String, String> = HashMap::new();

    for depth in 0..max_depth {
        if packages_to_fetch.is_empty() {
            break;
        }

        println!(
            "   Depth {}: fetching {} packages...",
            depth,
            packages_to_fetch.len()
        );
        let mut new_deps: std::collections::HashSet<String> = std::collections::HashSet::new();

        for pkg_id in packages_to_fetch.iter() {
            let pkg_id_normalized = normalize_address(pkg_id);
            if fetched_packages.contains(&pkg_id_normalized) {
                continue;
            }

            // Use historical version if available, otherwise fetch current version
            let version = historical_versions.get(pkg_id).copied();
            let result = rt.block_on(async { grpc.get_object_at_version(pkg_id, version).await });

            match result {
                Ok(Some(obj)) => {
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
                        println!(
                            "      ✓ {} v{} ({} modules)",
                            &pkg_id[..20.min(pkg_id.len())],
                            obj.version,
                            modules.len()
                        );

                        // Check linkage table for upgraded package versions
                        // This is CRITICAL for correct replay: when package A has linkage A -> B @ v5,
                        // we need to fetch B at the upgraded storage ID, not the original ID
                        if let Some(linkage) = &obj.package_linkage {
                            for l in linkage {
                                // Skip framework packages (0x1, 0x2, 0x3 in full form only)
                                // Note: We must check for exact framework IDs, not just prefix!
                                // User packages can start with 0x3... (e.g., 0x3492c874...)
                                let is_framework = matches!(l.original_id.as_str(),
                                    "0x0000000000000000000000000000000000000000000000000000000000000001" |
                                    "0x0000000000000000000000000000000000000000000000000000000000000002" |
                                    "0x0000000000000000000000000000000000000000000000000000000000000003" |
                                    "0x1" | "0x2" | "0x3"
                                );
                                if is_framework {
                                    continue;
                                }

                                // Check for dependency upgrades (original != upgraded)
                                // Normalize addresses to ensure consistent key format
                                let orig_normalized = normalize_address(&l.original_id);
                                let upgraded_normalized = normalize_address(&l.upgraded_id);
                                if orig_normalized != upgraded_normalized {
                                    // Record the upgrade mapping with normalized keys
                                    linkage_upgrades.insert(
                                        orig_normalized.clone(),
                                        upgraded_normalized.clone(),
                                    );

                                    // Queue the upgraded package for fetching (not the original)
                                    if !fetched_packages.contains(&upgraded_normalized)
                                        && !packages.contains_key(&upgraded_normalized)
                                    {
                                        new_deps.insert(upgraded_normalized.clone());
                                    }
                                }
                            }
                        }

                        // Extract dependencies from bytecode
                        for (_name, bytecode_b64) in &modules_b64 {
                            if let Ok(bytecode) =
                                base64::engine::general_purpose::STANDARD.decode(bytecode_b64)
                            {
                                let deps = extract_dependencies_from_bytecode(&bytecode);
                                for dep in deps {
                                    // Normalize the dependency address
                                    let dep_normalized = normalize_address(&dep);
                                    // If this dependency has an upgraded version, use that instead
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
                            }
                        }

                        // Store with normalized key
                        let pkg_id_normalized = normalize_address(pkg_id);
                        packages.insert(pkg_id_normalized.clone(), modules_b64);
                        fetched_packages.insert(pkg_id_normalized);
                    }
                }
                Ok(None) => {
                    println!(
                        "      ! Package not found: {}",
                        &pkg_id[..20.min(pkg_id.len())]
                    );
                    fetched_packages.insert(pkg_id_normalized.clone()); // Mark as visited
                }
                Err(e) => {
                    println!(
                        "      ! Failed to fetch {}: {}",
                        &pkg_id[..20.min(pkg_id.len())],
                        e
                    );
                    fetched_packages.insert(pkg_id_normalized.clone()); // Mark as visited
                }
            }
        }

        packages_to_fetch = new_deps;
    }

    // Log linkage upgrades summary
    if !linkage_upgrades.is_empty() {
        println!("   Linkage upgrades: {} mappings", linkage_upgrades.len());
    }

    println!("   Total packages: {}", packages.len());

    // =========================================================================
    // Step 6: Convert gRPC transaction to FetchedTransaction and build CachedTransaction
    // =========================================================================
    println!("\nStep 6: Building transaction structure...");

    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    // Add packages
    for (pkg_id, modules) in packages {
        cached.packages.insert(pkg_id, modules);
    }

    // Add objects
    cached.objects = objects;
    cached.object_types = object_types;
    cached.object_versions = historical_versions.clone();

    println!("   ✓ Built CachedTransaction");
    println!("      Packages: {}", cached.packages.len());
    println!("      Objects: {}", cached.objects.len());

    // =========================================================================
    // Step 7: Build module resolver
    // =========================================================================
    println!("\nStep 7: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();

    // Load packages from cached data with address aliasing for upgraded packages
    // IMPORTANT: Skip loading original packages when upgraded versions exist (via linkage)
    let mut module_load_count = 0;
    let mut alias_count = 0;
    let mut skipped_count = 0;

    for (pkg_id, modules) in &cached.packages {
        // Normalize the package ID for consistent lookup
        let pkg_id_normalized = normalize_address(pkg_id);
        // Check if this package is superseded by an upgraded version
        // If linkage_upgrades says "original -> upgraded" and we have the upgraded package,
        // skip loading the original to avoid module conflicts
        if let Some(upgraded_id) = linkage_upgrades.get(&pkg_id_normalized) {
            if cached.packages.contains_key(upgraded_id) {
                skipped_count += 1;
                println!(
                    "      Skipping {} (superseded by {})",
                    &pkg_id[..16.min(pkg_id.len())],
                    &upgraded_id[..16.min(upgraded_id.len())]
                );
                continue;
            }
        }

        // Parse the package ID (storage address) from hex
        let target_addr = AccountAddress::from_hex_literal(pkg_id).ok();

        // Decode modules from base64
        let decoded_modules: Vec<(String, Vec<u8>)> = modules
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        // Load modules with aliasing support
        match resolver.add_package_modules_at(decoded_modules, target_addr) {
            Ok((count, source_addr)) => {
                module_load_count += count;
                // Check if aliasing was set up (bytecode addr differs from storage addr)
                if let (Some(target), Some(source)) = (target_addr, source_addr) {
                    if target != source {
                        alias_count += 1;
                    }
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

    // Load Sui framework (bundled)
    match resolver.load_sui_framework() {
        Ok(n) => println!("   ✓ Loaded {} framework modules", n),
        Err(e) => println!("   ! Framework load failed: {}", e),
    }

    println!("   ✓ Resolver ready");

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
    // Step 9: Set up on-demand child fetcher (for any missed objects)
    // =========================================================================
    println!("\nStep 9: Setting up on-demand child fetcher...");

    let grpc_arc = Arc::new(grpc);
    let historical_arc = Arc::new(historical_versions.clone());
    let grpc_for_closure = grpc_arc.clone();
    let historical_for_closure = historical_arc.clone();

    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        let child_id_str = child_id.to_hex_literal();

        // Try to fetch at historical version if known
        let version = historical_for_closure.get(&child_id_str).copied();

        let rt = tokio::runtime::Runtime::new().ok()?;
        let result = rt.block_on(async {
            grpc_for_closure
                .get_object_at_version(&child_id_str, version)
                .await
        });

        if let Ok(Some(obj)) = result {
            if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                if let Some(type_tag) = parse_type_tag_simple(type_str) {
                    return Some((type_tag, bcs.clone()));
                }
            }
        }

        None
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
    // Step 11: Build address aliases and execute replay
    // =========================================================================
    println!("\nStep 11: Executing transaction replay...");

    // Build address aliases for upgraded packages (maps storage ID -> bytecode ID)
    let address_aliases =
        sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test(&cached);
    if !address_aliases.is_empty() {
        println!("   Address aliases for replay: {}", address_aliases.len());
        for (runtime, bytecode) in &address_aliases {
            println!(
                "      {} -> {}",
                &runtime.to_hex_literal()[..20],
                &bytecode.to_hex_literal()[..20]
            );
        }
    }

    // Also set aliases on the VM harness for module resolution during execution
    harness.set_address_aliases(address_aliases.clone());

    let result = cached.transaction.replay_with_objects_and_aliases(
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

// ═══════════════════════════════════════════════════════════════════════════════
// HELPER FUNCTIONS
//
// These helper functions are self-contained within this example to demonstrate
// the complete workflow without external dependencies. They handle:
//
// 1. Type parsing - Converting Sui type strings to Move TypeTags
// 2. Address normalization - Ensuring consistent 66-char address format
// 3. Dependency extraction - Parsing bytecode to find package dependencies
// ═══════════════════════════════════════════════════════════════════════════════

/// Parse a Sui type string into a Move TypeTag.
///
/// Handles primitive types (u8, u64, etc.), vectors, and struct types with
/// nested type parameters. This is needed for the child object fetcher to
/// correctly deserialize objects by type.
///
/// # Examples
///
/// ```ignore
/// parse_type_tag_simple("u64")  // -> Some(TypeTag::U64)
/// parse_type_tag_simple("0x2::coin::Coin<0x2::sui::SUI>")  // -> Some(TypeTag::Struct(...))
/// ```
fn parse_type_tag_simple(type_str: &str) -> Option<TypeTag> {
    match type_str {
        "u8" => return Some(TypeTag::U8),
        "u64" => return Some(TypeTag::U64),
        "u128" => return Some(TypeTag::U128),
        "u256" => return Some(TypeTag::U256),
        "bool" => return Some(TypeTag::Bool),
        "address" => return Some(TypeTag::Address),
        _ => {}
    }

    if type_str.starts_with("vector<") && type_str.ends_with('>') {
        let inner = &type_str[7..type_str.len() - 1];
        return parse_type_tag_simple(inner).map(|t| TypeTag::Vector(Box::new(t)));
    }

    let (base_type, type_params_str) = if let Some(idx) = type_str.find('<') {
        (
            &type_str[..idx],
            Some(&type_str[idx + 1..type_str.len() - 1]),
        )
    } else {
        (type_str, None)
    };

    let parts: Vec<&str> = base_type.split("::").collect();
    if parts.len() != 3 {
        return None;
    }

    let address = AccountAddress::from_hex_literal(parts[0]).ok()?;
    let module = Identifier::new(parts[1]).ok()?;
    let name = Identifier::new(parts[2]).ok()?;

    let type_params = type_params_str
        .map(|s| {
            split_type_params(s)
                .iter()
                .filter_map(|t| parse_type_tag_simple(t.trim()))
                .collect()
        })
        .unwrap_or_default();

    Some(TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    })))
}

/// Split type parameters respecting nested angle brackets.
///
/// Given "A, B<C, D>, E", returns ["A", " B<C, D>", " E"] by tracking bracket depth.
/// Used by `parse_type_tag_simple` to correctly handle generic types.
fn split_type_params(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

/// Extract package IDs from a type string.
///
/// Scans a type string like "0xabc::module::Struct<0xdef::m::T, 0xghi::n::U>"
/// and returns all package addresses found (["0xabc", "0xdef", "0xghi"]).
/// Framework packages (0x1, 0x2, 0x3) are automatically excluded.
///
/// This is used to discover additional packages that need to be fetched based
/// on the types of objects involved in the transaction.
fn extract_package_ids_from_type(type_str: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut package_ids = HashSet::new();

    // Framework packages to skip
    let framework_prefixes = [
        "0x1::",
        "0x2::",
        "0x3::",
        "0x0000000000000000000000000000000000000000000000000000000000000001::",
        "0x0000000000000000000000000000000000000000000000000000000000000002::",
        "0x0000000000000000000000000000000000000000000000000000000000000003::",
    ];

    // Find all package addresses in the type string
    // Pattern: 0x followed by hex chars, then ::
    let mut i = 0;
    let chars: Vec<char> = type_str.chars().collect();

    while i < chars.len() {
        if i + 2 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
            let start = i;
            i += 2;
            // Consume hex chars
            while i < chars.len() && (chars[i].is_ascii_hexdigit()) {
                i += 1;
            }
            // Check if followed by ::
            if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':' {
                let pkg_id: String = chars[start..i].iter().collect();
                // Skip framework packages
                let full_prefix = format!("{}::", pkg_id);
                if !framework_prefixes.iter().any(|p| full_prefix == *p) {
                    package_ids.insert(pkg_id);
                }
            }
        } else {
            i += 1;
        }
    }

    package_ids.into_iter().collect()
}

/// Normalize a Sui address to a consistent 66-character format (0x + 64 hex chars).
///
/// Sui addresses can appear in shortened form (0x2) or full form
/// (0x0000...0002). This function ensures consistent formatting for
/// HashMap key lookups and address comparisons.
///
/// # Examples
///
/// ```ignore
/// normalize_address("0x2")      // -> "0x0000...0002" (64 hex chars)
/// normalize_address("0x3637")   // -> "0x0000...3637" (64 hex chars)
/// ```
fn normalize_address(addr: &str) -> String {
    let addr = addr.strip_prefix("0x").unwrap_or(addr);
    // Pad to 64 hex characters
    format!("0x{:0>64}", addr)
}

/// Extract package addresses that a module depends on from its bytecode.
///
/// Parses the compiled Move bytecode to find all module handles (references to
/// other modules), and returns the package addresses of non-framework dependencies.
/// This enables transitive dependency resolution - fetching all packages needed
/// to execute a transaction.
///
/// Framework packages (0x1, 0x2, 0x3) are excluded since they are bundled
/// with the VM and don't need to be fetched.
fn extract_dependencies_from_bytecode(bytecode: &[u8]) -> Vec<String> {
    use std::collections::BTreeSet;

    // Framework addresses to skip
    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    let mut deps = Vec::new();

    if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
        for handle in &module.module_handles {
            let addr = *module.address_identifier_at(handle.address);
            // Skip framework modules and self
            if !framework_addrs.contains(&addr) {
                deps.push(addr.to_hex_literal());
            }
        }
    }

    deps
}
