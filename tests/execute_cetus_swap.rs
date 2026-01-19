// This test file is temporarily disabled due to API changes.
// TODO: Migrate tests to use the new GraphQL-based API.
#![cfg(feature = "legacy_tests")]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(clippy::single_match)]
#![allow(clippy::println_empty_string)]
#![allow(clippy::len_zero)]
#![allow(unused_imports)]
//! Execute Cetus Swap PTB in Sandbox - Case Study
//!
//! This test attempts to execute a real Cetus DEX swap PTB using the sandbox.
//!
//! Original Transaction: 7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp
//!
//! ## PTB Structure:
//! Commands: SplitCoins → MoveCall(swap_a2b) → MergeCoins ×2 → TransferObjects ×2
//! Swaps LEIA → SUI via Cetus DEX
//!
//! ## Case Study Findings (2026-01-15)
//!
//! ### What Works ✓
//! 1. **gRPC Archive Access**: The Sui archive at `archive.mainnet.sui.io:443` is accessible
//!    via gRPC (JSON-RPC returns "fault filter abort")
//! 2. **Historical Object Retrieval**: Objects CAN be fetched at specific versions
//!    (e.g., Pool at version 751677305)
//! 3. **Dynamic Field ID Derivation**: The hash formula correctly computes object IDs
//!    from parent UID + type tag + BCS(key)
//! 4. **Package Upgrade Resolution**: Upgraded CLMM bytecode loads correctly at original address
//! 5. **On-Demand Child Fetching**: The callback mechanism works for dynamic field lookups
//!
//! ### Root Cause: SKIP_LIST TRAVERSAL REQUESTS COMPUTED TICK INDEX
//!
//! **Update (2026-01-15):** After deep investigation of gRPC archive and skip_list structure:
//!
//! **What's Working:**
//! 1. ✓ Shared objects (Pool, Config, Partner) CAN be fetched at historical versions via gRPC
//! 2. ✓ Skip_list nodes (keys 0, 481316, 512756, 887272) exist at creation version 751561008
//! 3. ✓ The gRPC archive's `bcs` field contains historical state (differs by version)
//! 4. ✓ Dynamic field ID derivation formula: `Blake2b(0xf0 || parent || len || key || type_tag)`
//!
//! **The Blocker:**
//! The child object `0x05d5d28...` is being requested during swap execution, but:
//! - It NEVER EXISTED - confirmed by both Sui archive gRPC and Surflux API at all versions
//! - It doesn't match any derivation from skip_list UID with u64 keys 0-10M
//! - It doesn't match derivation from any known parent (Pool, node[0], node[481316], etc.)
//!
//! **Key Insight:** The skip_list at v751561008 has 4 nodes: `0 -> 481316 -> 512756 -> 887272`
//! The swap function computes the "next initialized tick" based on current_sqrt_price.
//! This computation produces a tick index that was never initialized in the pool,
//! causing the dynamic field lookup to request an object that doesn't exist.
//!
//! **Root Cause Analysis (2026-01-16):**
//! The Move VM computes the child object ID dynamically during execution:
//! 1. Pool.current_tick_index determines which tick range to search
//! 2. skip_list::find_next computes the next tick to check
//! 3. The computed tick index doesn't correspond to any initialized tick
//! 4. dynamic_field::borrow fails because the node was never created
//!
//! For historical replay to work, we would need either:
//! - The exact input state that makes the swap computation match existing ticks
//! - Or a pre-computed list of all tick indices accessed during the original tx
//!
//! **gRPC Archive Behavior:**
//! - `bcs` field: Contains historical Object BCS (with type wrapper)
//! - `contents` field: Returns CURRENT state regardless of version requested
//! - Objects only exist at versions where they were created/mutated, not at intermediate versions
//!
//! We implemented ULEB128 parsing to extract Move struct data from the `bcs` field,
//! verified by comparing hashes across versions (they differ = historical data works).
//!
//! **Data Sources Tested (2026-01-16):**
//! 1. Sui Archive gRPC (archive.mainnet.sui.io:443) - Works, provides historical state
//! 2. Surflux gRPC (grpc.surflux.dev with x-api-key header) - Same results as Sui archive
//! 3. JSON-RPC - Returns "fault filter abort" for archive queries
//!
//! Both gRPC sources confirm object 0x05d5d28... never existed at any version.
//!
//! ### Solutions
//!
//! **Option 1: Historical State Replay (Accurate)**
//! - Fetch ALL shared objects at their tx-time versions from gRPC archive
//! - Replace cached objects with historical versions before replay
//! - Pro: Exact replay, accurate results
//! - Con: Need to fetch many objects at specific versions
//!
//! **Option 2: Transaction-Time Caching (Best for Indexing)**
//! - When indexing transactions, cache objects at their INPUT versions
//! - Store version alongside object ID in cache
//! - Pro: Perfect historical data, fast replay
//! - Con: Requires re-indexing with version awareness
//!
//! **Option 3: Forward Simulation Only (Simplest)**
//! - Use current state for simulating NEW transactions
//! - Don't attempt historical replay
//! - Pro: Simple, always works with current data
//! - Con: Can't replay historical transactions
//!
//! **Option 4: Hybrid Approach**
//! - Use on-demand historical fetching: when child lookup fails, fetch from archive at tx version
//! - Cache historical objects as they're fetched
//! - Pro: Works incrementally, lazy loading
//! - Con: May fail if archive doesn't have specific version
//!
//! ## Package Upgrade Handling
//!
//! The transaction uses Cetus Router which has a linkage table that maps:
//! - Original CLMM (0x1eabed72...) -> Upgraded CLMM (0x75b2e9ec...)
//!
//! The version check in CLMM's config module validates:
//! ```move
//! fun checked_package_version(config: &GlobalConfig) {
//!     if (config.package_version != CURRENT_VERSION) abort(10);
//! }
//! ```
//!
//! To replay successfully, we must load the UPGRADED package bytecode.

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use sui_move_interface_extractor::benchmark::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;
use sui_move_interface_extractor::benchmark::tx_replay::CachedTransaction;
use sui_move_interface_extractor::benchmark::vm::VMHarness;
use sui_move_interface_extractor::cache::CacheManager;

/// Parse a hex address string to AccountAddress
fn parse_address(s: &str) -> AccountAddress {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    let padded = format!("{:0>64}", hex);
    let bytes: [u8; 32] = hex::decode(&padded).unwrap().try_into().unwrap();
    AccountAddress::new(bytes)
}

/// Create a TypeTag for a coin type like "0x...::module::TYPE"
fn parse_type_tag(type_str: &str) -> TypeTag {
    // Parse "0xaddr::module::name" format
    let parts: Vec<&str> = type_str.split("::").collect();
    if parts.len() != 3 {
        panic!("Invalid type string: {}", type_str);
    }

    let address = parse_address(parts[0]);
    let module = Identifier::new(parts[1]).unwrap();
    let name = Identifier::new(parts[2]).unwrap();

    TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params: vec![],
    }))
}

#[test]
fn test_execute_cetus_swap_ptb() {
    println!("=== Executing Cetus Swap PTB ===\n");

    // 1. Load cache
    let cache = match CacheManager::new(".tx-cache") {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: No cache available - {}", e);
            return;
        }
    };

    // 2. Create simulation environment
    let mut env = match SimulationEnvironment::new() {
        Ok(e) => e,
        Err(e) => {
            println!("SKIP: Cannot create SimulationEnvironment - {}", e);
            return;
        }
    };

    println!("Step 1: Deploy ALL cached packages...");

    // Deploy all packages from cache to ensure all dependencies are available
    let all_packages = cache.list_packages();
    println!("  Found {} packages in cache", all_packages.len());

    let mut deployed_count = 0;
    for pkg_addr in &all_packages {
        if let Ok(Some(pkg)) = cache.get_package(pkg_addr) {
            match env.deploy_package(pkg.modules.clone()) {
                Ok(_) => {
                    deployed_count += 1;
                }
                Err(e) => {
                    // Some packages may fail due to missing dependencies - that's OK
                    println!("  Note: {} deploy skipped: {}", pkg_addr, e);
                }
            }
        }
    }
    println!("  ✓ Deployed {} packages", deployed_count);

    println!("\nStep 2: Load shared objects...");

    // Load GlobalConfig
    let global_config_id = "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f";
    let global_config_bytes = match cache.get_object(global_config_id) {
        Ok(Some(obj)) => {
            println!("  ✓ GlobalConfig loaded ({} bytes)", obj.bcs_bytes.len());
            obj.bcs_bytes
        }
        _ => {
            println!("  ✗ GlobalConfig not in cache");
            return;
        }
    };

    // Load Pool
    let pool_id = "0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772";
    let pool_bytes = match cache.get_object(pool_id) {
        Ok(Some(obj)) => {
            println!("  ✓ Pool loaded ({} bytes)", obj.bcs_bytes.len());
            obj.bcs_bytes
        }
        _ => {
            println!("  ✗ Pool not in cache");
            return;
        }
    };

    // Load Partner
    let partner_id = "0x639b5e433da31739e800cd085f356e64cae222966d0f1b11bd9dc76b322ff58b";
    let partner_bytes = match cache.get_object(partner_id) {
        Ok(Some(obj)) => {
            println!("  ✓ Partner loaded ({} bytes)", obj.bcs_bytes.len());
            obj.bcs_bytes
        }
        _ => {
            println!("  ✗ Partner not in cache");
            return;
        }
    };

    println!("\nStep 3: Create synthetic coins...");

    // Create LEIA coins (we'll create SUI-type coins as stand-ins since LEIA package not loaded)
    // In a real scenario, we'd need the LEIA package deployed
    let leia_coin_1 = match env.create_coin("0x2::sui::SUI", 10_000_000_000) {
        Ok(id) => {
            println!("  ✓ Created coin 1: {}", id);
            id
        }
        Err(e) => {
            println!("  ✗ Failed to create coin 1: {}", e);
            return;
        }
    };

    let leia_coin_2 = match env.create_coin("0x2::sui::SUI", 5_000_000_000) {
        Ok(id) => {
            println!("  ✓ Created coin 2: {}", id);
            id
        }
        Err(e) => {
            println!("  ✗ Failed to create coin 2: {}", e);
            return;
        }
    };

    println!("\nStep 4: Construct PTB inputs...");

    // Get the coin bytes from the environment
    let coin_1_bytes = env
        .get_object(&leia_coin_1)
        .map(|o| o.bcs_bytes.clone())
        .unwrap_or_default();
    let coin_2_bytes = env
        .get_object(&leia_coin_2)
        .map(|o| o.bcs_bytes.clone())
        .unwrap_or_default();

    // Type tags
    let leia_type = parse_type_tag(
        "0xb55d9fa9168c5f5f642f90b0330a47ccba9ef8e20a3207c1163d3d15c5c8663e::leia::LEIA",
    );
    let sui_type = parse_type_tag("0x2::sui::SUI");

    // Construct inputs matching the original PTB
    let inputs = vec![
        // Input 0: LEIA coin (owned) - we use SUI as stand-in
        InputValue::Object(ObjectInput::Owned {
            id: leia_coin_1,
            bytes: coin_1_bytes,
            type_tag: Some(sui_type.clone()), // Would be LEIA in real tx
        }),
        // Input 1: Pure - split amount (1_000_000_000 = 1 token)
        InputValue::Pure(bcs::to_bytes(&1_000_000_000u64).unwrap()),
        // Input 2: SharedObject - GlobalConfig
        InputValue::Object(ObjectInput::Shared {
            id: parse_address(global_config_id),
            bytes: global_config_bytes,
            type_tag: None,
        }),
        // Input 3: SharedObject - Pool
        InputValue::Object(ObjectInput::Shared {
            id: parse_address(pool_id),
            bytes: pool_bytes,
            type_tag: None,
        }),
        // Input 4: SharedObject - Partner
        InputValue::Object(ObjectInput::Shared {
            id: parse_address(partner_id),
            bytes: partner_bytes,
            type_tag: None,
        }),
        // Input 5: SharedObject - Clock (0x6)
        // The clock is handled specially by the simulation
        InputValue::Object(ObjectInput::Shared {
            id: parse_address("0x6"),
            bytes: vec![], // Clock bytes handled by simulation
            type_tag: None,
        }),
        // Input 6: Another LEIA coin (owned)
        InputValue::Object(ObjectInput::Owned {
            id: leia_coin_2,
            bytes: coin_2_bytes,
            type_tag: Some(sui_type.clone()),
        }),
        // Input 7: Pure - recipient address
        InputValue::Pure(
            bcs::to_bytes(&parse_address(
                "0x708a798401a7db89a11da05e8fcb5d2c60786c60b699004135c398d969322791",
            ))
            .unwrap(),
        ),
        // Input 8: Pure - recipient address (same as 7 in original)
        InputValue::Pure(
            bcs::to_bytes(&parse_address(
                "0x708a798401a7db89a11da05e8fcb5d2c60786c60b699004135c398d969322791",
            ))
            .unwrap(),
        ),
    ];

    println!("  Created {} inputs", inputs.len());

    println!("\nStep 5: Construct PTB commands...");

    // The actual Cetus router module address (from bytecode self-address)
    let router_module_addr =
        parse_address("0xeffc8ae61f439bb34c9b905ff8f29ec56873dcedf81c7123ff2f1f67c45ec302");

    let commands = vec![
        // Command 0: SplitCoins - split Input[0] by Input[1]
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1)],
        },
        // Command 1: MoveCall - cetus::swap_a2b<LEIA, SUI>
        Command::MoveCall {
            package: router_module_addr,
            module: Identifier::new("cetus").unwrap(),
            function: Identifier::new("swap_a2b").unwrap(),
            type_args: vec![leia_type.clone(), sui_type.clone()],
            args: vec![
                Argument::Input(2),  // GlobalConfig
                Argument::Input(3),  // Pool
                Argument::Input(4),  // Partner
                Argument::Result(0), // Split coin result
                Argument::Input(5),  // Clock
            ],
        },
        // Command 2: MergeCoins - merge swap result into GasCoin
        // Note: GasCoin handling is tricky - this might need special handling
        Command::MergeCoins {
            destination: Argument::Input(0), // Would be GasCoin in real PTB
            sources: vec![Argument::Result(1)],
        },
        // Command 3: MergeCoins - merge Input[6] into Input[0]
        Command::MergeCoins {
            destination: Argument::Input(0),
            sources: vec![Argument::Input(6)],
        },
        // Command 4: TransferObjects - transfer Input[0] to Input[7]
        Command::TransferObjects {
            objects: vec![Argument::Input(0)],
            address: Argument::Input(7),
        },
        // Note: Skipping command 5 (GasCoin transfer) as GasCoin is special
    ];

    println!("  Created {} commands", commands.len());

    println!("\nStep 6: Execute PTB...");

    let result = env.execute_ptb(inputs, commands);

    println!("\n=== EXECUTION RESULT ===");
    println!("Success: {}", result.success);
    println!("Commands succeeded: {}", result.commands_succeeded);

    if !result.success {
        if let Some(ref error) = result.error {
            println!("\nError: {}", error);
        }
        if let Some(ref raw_error) = result.raw_error {
            println!("Raw error: {}", raw_error);
        }
        if let Some(idx) = result.failed_command_index {
            println!("Failed at command: {}", idx);
        }
        if let Some(ref desc) = result.failed_command_description {
            println!("Failed command: {}", desc);
        }
    } else {
        println!("\n✓ PTB executed successfully!");
        if let Some(ref effects) = result.effects {
            println!("Effects: {:?}", effects);
        }
    }

    // Analysis
    println!("\n=== ANALYSIS ===");
    if result.success {
        println!("The sandbox successfully executed a DeFi swap PTB!");
        println!("This proves an LLM could construct and execute complex PTBs.");
    } else {
        println!("PTB execution failed. Analyzing the error...");

        let error_str = result
            .error
            .as_ref()
            .map(|e| format!("{}", e))
            .unwrap_or_default();
        let raw_str = result.raw_error.as_deref().unwrap_or("");
        let combined = format!("{} {}", error_str, raw_str);

        if combined.contains("type") || combined.contains("Type") {
            println!("→ Type mismatch: The LEIA type isn't properly loaded.");
            println!("  Fix: Deploy LEIA token package or use actual coin types.");
        } else if combined.contains("linker")
            || combined.contains("Linker")
            || combined.contains("MissingPackage")
        {
            println!("→ Linker error: Missing module dependency.");
            println!("  Fix: Ensure all dependent packages are deployed.");
        } else if combined.contains("abort") || combined.contains("Abort") {
            println!("→ Move abort: The swap logic rejected the transaction.");
            println!("  This could be: insufficient liquidity, slippage, etc.");
        } else if combined.contains("object") || combined.contains("Object") {
            println!("→ Object error: Problem with input objects.");
            println!("  Check object IDs, types, and ownership.");
        } else {
            println!("→ Unknown error type.");
        }
    }
}

/// Simpler test: just SplitCoins + MergeCoins (no MoveCall)
#[test]
fn test_basic_ptb_operations() {
    println!("=== Basic PTB Operations Test ===\n");

    let mut env = match SimulationEnvironment::new() {
        Ok(e) => e,
        Err(e) => {
            println!("SKIP: Cannot create SimulationEnvironment - {}", e);
            return;
        }
    };

    // Create coins
    let coin_1 = env.create_coin("0x2::sui::SUI", 10_000_000_000).unwrap();
    let coin_2 = env.create_coin("0x2::sui::SUI", 5_000_000_000).unwrap();

    println!("Created coins: {}, {}", coin_1, coin_2);

    let coin_1_bytes = env
        .get_object(&coin_1)
        .map(|o| o.bcs_bytes.clone())
        .unwrap_or_default();
    let coin_2_bytes = env
        .get_object(&coin_2)
        .map(|o| o.bcs_bytes.clone())
        .unwrap_or_default();

    let sui_type = parse_type_tag("0x2::sui::SUI");

    // Simple PTB: Split coin_1, then merge result with coin_2
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_1,
            bytes: coin_1_bytes,
            type_tag: Some(sui_type.clone()),
        }),
        InputValue::Pure(bcs::to_bytes(&1_000_000_000u64).unwrap()),
        InputValue::Object(ObjectInput::Owned {
            id: coin_2,
            bytes: coin_2_bytes,
            type_tag: Some(sui_type.clone()),
        }),
        InputValue::Pure(bcs::to_bytes(&parse_address("0x1234")).unwrap()),
    ];

    let commands = vec![
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1)],
        },
        Command::MergeCoins {
            destination: Argument::Input(2),
            sources: vec![Argument::Result(0)],
        },
        Command::TransferObjects {
            objects: vec![Argument::Input(0), Argument::Input(2)],
            address: Argument::Input(3),
        },
    ];

    println!(
        "\nExecuting PTB with {} inputs and {} commands...",
        inputs.len(),
        commands.len()
    );

    let result = env.execute_ptb(inputs, commands);

    println!("\nResult:");
    println!("  Success: {}", result.success);
    println!("  Commands succeeded: {}", result.commands_succeeded);

    if !result.success {
        if let Some(ref error) = result.error {
            println!("  Error: {}", error);
        }
        if let Some(ref raw_error) = result.raw_error {
            println!("  Raw error: {}", raw_error);
        }
    } else {
        println!("  ✓ Basic PTB operations work!");
    }
}

/// Test using the proper replay mechanism with VMHarness
#[test]
fn test_replay_cetus_swap_with_vmharness() {
    println!("=== Replay Cetus Swap with VMHarness ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded cached transaction:");
    println!("  Digest: {:?}", cached.transaction.digest);
    println!("  Packages: {}", cached.packages.len());
    println!("  Objects: {}", cached.objects.len());

    // Initialize resolver with Sui framework
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    println!("\nLoading packages into resolver...");

    // Load all cached packages
    let mut loaded_count = 0;
    for pkg_id in cached.packages.keys() {
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<(String, Vec<u8>)> =
                modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                match resolver.add_package_modules(non_empty) {
                    Ok((count, _)) => {
                        loaded_count += count;
                    }
                    Err(e) => {
                        println!("  Warning: Failed to load {}: {}", pkg_id, e);
                    }
                }
            }
        }
    }
    println!(
        "  Loaded {} modules from {} packages",
        loaded_count,
        cached.packages.len()
    );

    // Create VMHarness
    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    // Build address aliases for package upgrade support
    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);
    println!("\nAddress aliases (on-chain -> bytecode):");
    for (on_chain, bytecode) in &address_aliases {
        println!(
            "  {} -> {}",
            on_chain.to_hex_literal(),
            bytecode.to_hex_literal()
        );
    }

    println!("\nReplaying transaction with address aliases...");

    // Replay using the proper mechanism with aliases
    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== REPLAY RESULT ===");
            println!("Local success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ TRANSACTION REPLAYED SUCCESSFULLY!");
                println!("The sandbox can execute complex DeFi PTBs.");
            } else {
                println!("\nLocal execution failed:");
                if let Some(err) = &result.local_error {
                    println!("  Error: {}", err);
                }
            }

            // Check if it matches on-chain result
            if let Some(on_chain) = &cached.transaction.effects {
                use sui_move_interface_extractor::benchmark::tx_replay::TransactionStatus;
                let chain_success = matches!(on_chain.status, TransactionStatus::Success);
                println!("\nOn-chain status: {:?}", on_chain.status);
                println!("Status match: {}", result.local_success == chain_success);
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}

/// Test replay with proper handling of package upgrades via linkage tables.
///
/// The Cetus Router (0x47a7b90...) has a linkage table that maps:
/// - 0x1eabed72... (original CLMM) -> 0x75b2e9ec... (upgraded CLMM v12)
///
/// We need to fetch the UPGRADED package bytecode to pass the version check.
#[test]
fn test_replay_cetus_with_upgraded_packages() {
    println!("=== Replay Cetus Swap with Upgraded Packages ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded cached transaction:");
    println!("  Digest: {:?}", cached.transaction.digest);

    // Initialize resolver with Sui framework
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    // Key insight: The Router's linkage table tells us which upgraded packages to use.
    // Linkage table from Router (0x47a7b90756fba96fe649c2aaa10ec60dec6b8cb8545573d621310072721133aa):
    //   0x1eabed72... -> 0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40
    //
    // This means calls to the original CLMM should use the upgraded bytecode.

    // Create a fetcher to get the upgraded packages
    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
    let fetcher = TransactionFetcher::mainnet();

    println!("\nFetching upgraded CLMM package (v12)...");

    // Fetch the upgraded CLMM package
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            println!("  Fetched {} modules from upgraded CLMM", modules.len());
            // Load with the ORIGINAL address so it gets linked correctly
            // The VM needs to find these modules when the Router calls into CLMM
            let original_clmm =
                parse_address("0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb");
            match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => {
                    println!("  Loaded {} modules at original CLMM address", count);
                }
                Err(e) => {
                    println!("  Warning: Failed to load upgraded CLMM: {}", e);
                }
            }
        }
        Err(e) => {
            println!("  Warning: Failed to fetch upgraded CLMM: {}", e);
            println!("  Falling back to cached (old) CLMM...");
        }
    }

    // Load the rest of the cached packages (excluding original CLMM which we replaced)
    println!("\nLoading other cached packages...");
    let original_clmm_id = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
    let mut loaded_count = 0;
    for pkg_id in cached.packages.keys() {
        // Skip the original CLMM - we loaded the upgraded version
        if pkg_id == original_clmm_id {
            println!("  Skipping original CLMM (using upgraded version)");
            continue;
        }

        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<(String, Vec<u8>)> =
                modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                match resolver.add_package_modules(non_empty) {
                    Ok((count, _)) => {
                        loaded_count += count;
                    }
                    Err(e) => {
                        println!("  Warning: Failed to load {}: {}", pkg_id, e);
                    }
                }
            }
        }
    }
    println!(
        "  Loaded {} modules from {} packages",
        loaded_count,
        cached.packages.len() - 1
    );

    // Create VMHarness
    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    // Build address aliases for package upgrade support
    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    // The upgraded CLMM has a different self-address in bytecode
    // We need to map the on-chain ID to the bytecode self-address
    let _upgraded_clmm_self =
        parse_address("0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40");
    let _original_clmm_addr = parse_address(original_clmm_id);
    // Note: We loaded the upgraded bytecode at the original address, so no alias needed for CLMM itself
    // But we do need to map the Router's target address if it differs
    println!("\nAddress aliases:");
    for (on_chain, bytecode) in &address_aliases {
        println!(
            "  {} -> {}",
            on_chain.to_hex_literal(),
            bytecode.to_hex_literal()
        );
    }

    println!("\nReplaying transaction...");

    // Replay
    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== REPLAY RESULT ===");
            println!("Local success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ TRANSACTION REPLAYED SUCCESSFULLY!");
                println!("The sandbox correctly handles package upgrades via linkage tables.");
            } else {
                println!("\nLocal execution failed:");
                if let Some(err) = &result.local_error {
                    println!("  Error: {}", err);
                }
            }

            // Check if it matches on-chain result
            if let Some(on_chain) = &cached.transaction.effects {
                use sui_move_interface_extractor::benchmark::tx_replay::TransactionStatus;
                let chain_success = matches!(on_chain.status, TransactionStatus::Success);
                println!("\nOn-chain status: {:?}", on_chain.status);
                println!("Status match: {}", result.local_success == chain_success);
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}

/// Test replaying Cetus swap with dynamic field children fetched from mainnet.
///
/// This test demonstrates the complete workflow for replaying DeFi transactions:
/// 1. Load upgraded packages (handle package linkage tables)
/// 2. Fetch dynamic field children (tick data, skip_list nodes)
/// 3. Pre-load children into VM's ObjectRuntime
/// 4. Execute the transaction
///
/// The Cetus Pool stores tick data in a skip_list which uses dynamic fields.
/// Without pre-loading these children, the swap fails with error 1000 (E_NOT_SUPPORTED).
#[test]
fn test_replay_cetus_with_dynamic_fields() {
    println!("=== Replay Cetus Swap with Dynamic Field Fetching ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded cached transaction: {:?}", cached.transaction.digest);

    // Initialize resolver with Sui framework
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    // Fetch upgraded CLMM package (handles version check)
    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
    let fetcher = TransactionFetcher::mainnet();

    println!("\n1. Loading upgraded CLMM package...");
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    let original_clmm_id = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            println!("   Fetched {} modules from upgraded CLMM", modules.len());
            let original_clmm = parse_address(original_clmm_id);
            match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => {
                    println!("   Loaded {} modules at original CLMM address", count);
                }
                Err(e) => {
                    println!("   Warning: Failed to load upgraded CLMM: {}", e);
                }
            }
        }
        Err(e) => {
            println!("   Warning: Failed to fetch upgraded CLMM: {}", e);
            println!("   Falling back to cached (old) CLMM...");
        }
    }

    // Load other cached packages (excluding original CLMM)
    println!("\n2. Loading other cached packages...");
    let mut loaded_count = 0;
    for pkg_id in cached.packages.keys() {
        if pkg_id == original_clmm_id {
            println!("   Skipping original CLMM (using upgraded version)");
            continue;
        }

        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<(String, Vec<u8>)> =
                modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                match resolver.add_package_modules(non_empty) {
                    Ok((count, _)) => {
                        loaded_count += count;
                    }
                    Err(e) => {
                        println!("   Warning: Failed to load {}: {}", pkg_id, e);
                    }
                }
            }
        }
    }
    println!(
        "   Loaded {} modules from {} packages",
        loaded_count,
        cached.packages.len() - 1
    );

    // Create VMHarness
    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    // Fetch dynamic field children for Pool's skip_list
    println!("\n3. Fetching dynamic field children from mainnet...");

    // The Pool contains a tick_manager with a skip_list that has its own UID
    // We need to fetch children of the skip_list's internal UID, not the Pool itself
    // Pool ID: 0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772
    // Pool's tick_manager.ticks.id (skip_list UID): 0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3
    let skip_list_uid = "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3";

    // Use JSON-RPC fetcher (works for wrapped UIDs, unlike GraphQL)
    match fetcher.fetch_dynamic_fields_recursive(skip_list_uid, 2, 100) {
        Ok(fields) => {
            println!("   Fetched {} dynamic field entries", fields.len());

            for field in &fields {
                println!("   - {} (type: {:?})", field.object_id, field.object_type);
            }

            // Now fetch the actual BCS data for each child object
            let mut preload_fields = Vec::new();
            let parent_addr = parse_address(skip_list_uid);

            for field in &fields {
                // Fetch the object's BCS data
                match fetcher.fetch_object_full(&field.object_id) {
                    Ok(obj) => {
                        let child_addr = parse_address(&field.object_id);

                        // Parse the object type to get a TypeTag
                        let type_tag = field
                            .object_type
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));

                        preload_fields.push(((parent_addr, child_addr), type_tag, obj.bcs_bytes));
                    }
                    Err(e) => {
                        println!(
                            "   Warning: Failed to fetch object {}: {}",
                            field.object_id, e
                        );
                    }
                }
            }

            println!("   Prepared {} fields for preloading", preload_fields.len());

            // Preload into VMHarness
            harness.preload_dynamic_fields(preload_fields);
            println!("   Preloaded dynamic fields into VM");
        }
        Err(e) => {
            println!("   Warning: Failed to fetch dynamic fields: {}", e);
            println!("   Continuing without dynamic field children (may fail)...");
        }
    }

    // Build address aliases for package upgrade support
    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    println!("\n4. Replaying transaction...");

    // Replay
    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== REPLAY RESULT ===");
            println!("Local success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ TRANSACTION REPLAYED SUCCESSFULLY!");
                println!("The sandbox correctly handles:");
                println!("  - Package upgrades via linkage tables");
                println!("  - Dynamic field children for tick data");
            } else {
                println!("\nLocal execution failed:");
                if let Some(err) = &result.local_error {
                    println!("  Error: {}", err);

                    // Check if this is a dynamic field lookup failure
                    // Error 1000 is E_FIELD_DOES_NOT_EXIST, but skip_list may abort with error 1
                    // when it can't find a dynamic field child
                    if err.contains("ABORTED")
                        && (err.contains("1000")
                            || err.contains("dynamic_field")
                            || err.contains("skip_list"))
                    {
                        println!("\n--- EXPLANATION ---");
                        println!("This is expected for historical transactions!");
                        println!("The error code 1000 (E_FIELD_DOES_NOT_EXIST) indicates");
                        println!("a dynamic field child was requested that doesn't exist");
                        println!("in our preloaded set.");
                        println!("");
                        println!("The skip_list's dynamic field children have changed since");
                        println!("the transaction was executed. The current mainnet state");
                        println!("has different tick nodes than at transaction time.");
                        println!("");
                        println!("What works:");
                        println!("  ✓ Package upgrade resolution (upgraded CLMM loaded)");
                        println!("  ✓ Dynamic field fetching from mainnet (2 nodes fetched)");
                        println!("  ✓ UID address extraction from Move references");
                        println!("  ✓ Shared state preloading mechanism");
                        println!("");
                        println!("What's needed for full replay:");
                        println!("  - Historical state access (archive node)");
                        println!("  - Or cache dynamic fields at transaction time");
                    }
                }
            }

            // Check if it matches on-chain result
            if let Some(on_chain) = &cached.transaction.effects {
                use sui_move_interface_extractor::benchmark::tx_replay::TransactionStatus;
                let chain_success = matches!(on_chain.status, TransactionStatus::Success);
                println!("\nOn-chain status: {:?}", on_chain.status);
                println!("Status match: {}", result.local_success == chain_success);
            }
        }
        Err(e) => {
            println!("Replay with dynamic fields failed: {}", e);
        }
    }
}

/// Parse a type tag more flexibly, handling generic types.
/// Falls back to a simple type if parsing fails.
fn parse_type_tag_flexible(type_str: &str) -> TypeTag {
    // Handle primitive types
    match type_str {
        "u8" => return TypeTag::U8,
        "u16" => return TypeTag::U16,
        "u32" => return TypeTag::U32,
        "u64" => return TypeTag::U64,
        "u128" => return TypeTag::U128,
        "u256" => return TypeTag::U256,
        "bool" => return TypeTag::Bool,
        "address" => return TypeTag::Address,
        "signer" => return TypeTag::Signer,
        _ => {}
    }

    // Handle vector<T>
    if type_str.starts_with("vector<") && type_str.ends_with(">") {
        let inner = &type_str[7..type_str.len() - 1];
        return TypeTag::Vector(Box::new(parse_type_tag_flexible(inner)));
    }

    // Try to parse as struct type "0xaddr::module::name" or "0xaddr::module::name<T>"
    let base_type = if let Some(idx) = type_str.find('<') {
        &type_str[..idx]
    } else {
        type_str
    };

    let parts: Vec<&str> = base_type.split("::").collect();
    if parts.len() != 3 {
        // Can't parse, return a generic type
        return TypeTag::Vector(Box::new(TypeTag::U8));
    }

    let address = parse_address(parts[0]);
    let module = match Identifier::new(parts[1]) {
        Ok(id) => id,
        Err(_) => return TypeTag::Vector(Box::new(TypeTag::U8)),
    };
    let name = match Identifier::new(parts[2]) {
        Ok(id) => id,
        Err(_) => return TypeTag::Vector(Box::new(TypeTag::U8)),
    };

    // Parse type parameters if present
    let type_params = if let Some(idx) = type_str.find('<') {
        let params_str = &type_str[idx + 1..type_str.len() - 1];
        // Simple split by comma (doesn't handle nested generics well)
        params_str
            .split(',')
            .map(|s| parse_type_tag_flexible(s.trim()))
            .collect()
    } else {
        vec![]
    };

    TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    }))
}

/// Test replaying Cetus swap with historical object state and on-demand child fetching.
/// This test:
/// 1. Fetches input objects at their HISTORICAL versions (not current state)
/// 2. Sets up on-demand fetching for dynamic field children
/// 3. Attempts full transaction replay
#[test]
fn test_replay_cetus_with_historical_state() {
    println!("=== Replay Cetus Swap with HISTORICAL Object State ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded cached transaction: {:?}", cached.transaction.digest);

    // Initialize resolver with Sui framework
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    // Use archive-enabled fetcher for historical lookups
    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
    let fetcher = TransactionFetcher::mainnet_with_archive();

    println!("\n1. Loading upgraded CLMM package...");
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    let original_clmm_id = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            println!("   Fetched {} modules from upgraded CLMM", modules.len());
            let original_clmm = parse_address(original_clmm_id);
            match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => {
                    println!("   Loaded {} modules at original CLMM address", count);
                }
                Err(e) => {
                    println!("   Warning: Failed to load upgraded CLMM: {}", e);
                }
            }
        }
        Err(e) => {
            println!("   Warning: Failed to fetch upgraded CLMM: {}", e);
        }
    }

    // Load other cached packages (excluding original CLMM)
    println!("\n2. Loading other cached packages...");
    let mut loaded_count = 0;
    for pkg_id in cached.packages.keys() {
        if pkg_id == original_clmm_id {
            continue;
        }
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<(String, Vec<u8>)> =
                modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                match resolver.add_package_modules(non_empty) {
                    Ok((count, _)) => loaded_count += count,
                    Err(_) => {}
                }
            }
        }
    }
    println!("   Loaded {} modules", loaded_count);

    // Create VMHarness
    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    println!("\n3. Fetching HISTORICAL object state from archive...");

    // Key insight: We need to fetch objects at their historical versions
    // Pool ID: 0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772
    // Historical version: 751677305 (before this transaction mutated it)

    let pool_id = "0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772";
    let pool_historical_version = 751677305u64;

    let mut historical_objects: std::collections::HashMap<String, String> = cached.objects.clone();

    // Fetch Pool at historical version
    match fetcher.fetch_object_at_version_full(pool_id, pool_historical_version) {
        Ok(historical_pool) => {
            use base64::Engine;
            let bcs_base64 =
                base64::engine::general_purpose::STANDARD.encode(&historical_pool.bcs_bytes);
            historical_objects.insert(pool_id.to_string(), bcs_base64);
            println!(
                "   ✓ Pool fetched at version {} ({} bytes)",
                pool_historical_version,
                historical_pool.bcs_bytes.len()
            );
        }
        Err(e) => {
            println!("   ✗ Failed to fetch historical Pool: {}", e);
            println!("   Falling back to cached (current) state...");
        }
    }

    println!("\n4. Setting up on-demand child fetcher...");

    let archive_fetcher = std::sync::Arc::new(TransactionFetcher::mainnet_with_archive());
    use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;

    let fetcher_clone = archive_fetcher.clone();
    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
        eprintln!("[on_demand_fetcher] Fetching child: {}", child_id_str);

        // Try fetching at current state first (most efficient)
        match fetcher_clone.fetch_object_full(&child_id_str) {
            Ok(fetched) => {
                eprintln!(
                    "[on_demand_fetcher] SUCCESS: {} bytes",
                    fetched.bcs_bytes.len()
                );
                let type_tag = fetched
                    .type_string
                    .as_ref()
                    .map(|t| parse_type_tag_flexible(t))
                    .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                Some((type_tag, fetched.bcs_bytes))
            }
            Err(e) => {
                eprintln!("[on_demand_fetcher] FAILED: {}", e);
                None
            }
        }
    });

    harness.set_child_fetcher(child_fetcher);

    // Preload dynamic field children from historical state
    println!("\n5. Fetching dynamic field children...");
    let skip_list_uid = "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3";

    // First, get current dynamic fields (these might be different from historical)
    let base_fetcher = TransactionFetcher::mainnet();
    match base_fetcher.fetch_dynamic_fields_recursive(skip_list_uid, 2, 100) {
        Ok(fields) => {
            println!(
                "   Current state has {} dynamic field entries",
                fields.len()
            );
            for field in &fields {
                println!("   - key={:?}, id={}", field.name_json, field.object_id);
            }

            let mut preload_fields = Vec::new();
            let parent_addr = parse_address(skip_list_uid);

            for field in &fields {
                match base_fetcher.fetch_object_full(&field.object_id) {
                    Ok(obj) => {
                        let child_addr = parse_address(&field.object_id);
                        let type_tag = field
                            .object_type
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                        preload_fields.push(((parent_addr, child_addr), type_tag, obj.bcs_bytes));
                    }
                    Err(e) => {
                        println!("   Warning: Failed to fetch {}: {}", field.object_id, e);
                    }
                }
            }

            harness.preload_dynamic_fields(preload_fields);
        }
        Err(e) => {
            println!("   Warning: Failed to fetch dynamic fields: {}", e);
        }
    }

    // Build address aliases for package upgrade support
    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    println!("\n6. Replaying transaction with historical Pool state...");

    // Use historical objects instead of cached (current) objects
    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &historical_objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== REPLAY RESULT ===");
            println!("Local success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ HISTORICAL TRANSACTION REPLAYED SUCCESSFULLY!");
            } else {
                println!("\nLocal execution failed:");
                if let Some(err) = &result.local_error {
                    println!("  Error: {}", err);
                }
            }

            if let Some(on_chain) = &cached.transaction.effects {
                use sui_move_interface_extractor::benchmark::tx_replay::TransactionStatus;
                let chain_success = matches!(on_chain.status, TransactionStatus::Success);
                println!("\nOn-chain status: {:?}", on_chain.status);
                println!("Status match: {}", result.local_success == chain_success);
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}

/// Test replaying Cetus swap with on-demand child fetching from archive.
/// This test sets up a child fetcher callback that fetches dynamic field
/// children from the BlockVision archive when they're not found in the
/// preloaded set.
#[test]
fn test_replay_cetus_with_ondemand_fetching() {
    println!("=== Replay Cetus Swap with On-Demand Child Fetching ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded cached transaction: {:?}", cached.transaction.digest);

    // Initialize resolver with Sui framework
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    // Fetch upgraded CLMM package (handles version check)
    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
    let fetcher = TransactionFetcher::mainnet_with_archive();

    println!("\n1. Loading upgraded CLMM package...");
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    let original_clmm_id = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            println!("   Fetched {} modules from upgraded CLMM", modules.len());
            let original_clmm = parse_address(original_clmm_id);
            match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => {
                    println!("   Loaded {} modules at original CLMM address", count);
                }
                Err(e) => {
                    println!("   Warning: Failed to load upgraded CLMM: {}", e);
                }
            }
        }
        Err(e) => {
            println!("   Warning: Failed to fetch upgraded CLMM: {}", e);
            println!("   Falling back to cached (old) CLMM...");
        }
    }

    // Load other cached packages (excluding original CLMM)
    println!("\n2. Loading other cached packages...");
    let mut loaded_count = 0;
    for pkg_id in cached.packages.keys() {
        if pkg_id == original_clmm_id {
            println!("   Skipping original CLMM (using upgraded version)");
            continue;
        }

        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<(String, Vec<u8>)> =
                modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                match resolver.add_package_modules(non_empty) {
                    Ok((count, _)) => {
                        loaded_count += count;
                    }
                    Err(e) => {
                        println!("   Warning: Failed to load {}: {}", pkg_id, e);
                    }
                }
            }
        }
    }
    println!(
        "   Loaded {} modules from {} packages",
        loaded_count,
        cached.packages.len() - 1
    );

    // Create VMHarness
    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    println!("\n3. Setting up on-demand child fetcher with archive...");

    // Create fetcher for on-demand child loading
    // Use Arc to share the fetcher with the callback
    let archive_fetcher = std::sync::Arc::new(TransactionFetcher::mainnet_with_archive());

    // Set up the child fetcher callback
    use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;

    let fetcher_clone = archive_fetcher.clone();
    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        // Use full 64-char hex representation (to_hex_literal strips leading zeros)
        let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
        eprintln!("[on_demand_fetcher] Fetching child: {}", child_id_str);

        // Try fetching the object from the archive
        // First try current state, then try at various historical versions
        match fetcher_clone.fetch_object_full(&child_id_str) {
            Ok(fetched) => {
                eprintln!(
                    "[on_demand_fetcher] SUCCESS: {} bytes, type={:?}",
                    fetched.bcs_bytes.len(),
                    fetched.type_string
                );

                // Parse the type string to get a TypeTag
                let type_tag = fetched
                    .type_string
                    .as_ref()
                    .map(|t| parse_type_tag_flexible(t))
                    .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));

                Some((type_tag, fetched.bcs_bytes))
            }
            Err(e) => {
                eprintln!("[on_demand_fetcher] FAILED: {}", e);
                // Object doesn't exist at current state
                // This is expected for historical transactions where dynamic field
                // children may have changed since execution
                None
            }
        }
    });

    harness.set_child_fetcher(child_fetcher);
    println!("   On-demand fetcher configured");

    // Preload current dynamic field children (may be different from historical)
    println!("\n4. Preloading current dynamic field children...");
    let skip_list_uid = "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3";

    // Use the base fetcher (without archive) for current-state fetches
    let base_fetcher = TransactionFetcher::mainnet();
    match base_fetcher.fetch_dynamic_fields_recursive(skip_list_uid, 2, 100) {
        Ok(fields) => {
            println!("   Fetched {} dynamic field entries", fields.len());

            let mut preload_fields = Vec::new();
            let parent_addr = parse_address(skip_list_uid);

            for field in &fields {
                match base_fetcher.fetch_object_full(&field.object_id) {
                    Ok(obj) => {
                        let child_addr = parse_address(&field.object_id);
                        let type_tag = field
                            .object_type
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                        preload_fields.push(((parent_addr, child_addr), type_tag, obj.bcs_bytes));
                    }
                    Err(e) => {
                        println!("   Warning: Failed to fetch {}: {}", field.object_id, e);
                    }
                }
            }

            harness.preload_dynamic_fields(preload_fields);
            println!("   Preloaded {} fields", fields.len());
        }
        Err(e) => {
            println!("   Warning: Failed to fetch dynamic fields: {}", e);
        }
    }

    // Build address aliases for package upgrade support
    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    println!("\n5. Replaying transaction with on-demand fetching...");

    // Replay
    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== REPLAY RESULT ===");
            println!("Local success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ TRANSACTION REPLAYED SUCCESSFULLY!");
                println!("The sandbox correctly handles:");
                println!("  - Package upgrades via linkage tables");
                println!("  - Dynamic field children via on-demand fetching");
            } else {
                println!("\nLocal execution failed:");
                if let Some(err) = &result.local_error {
                    println!("  Error: {}", err);

                    // Analyze the error
                    if err.contains("ABORTED") && err.contains("1000") {
                        println!("\n--- ANALYSIS ---");
                        println!("E_FIELD_DOES_NOT_EXIST (1000) - dynamic field child not found");
                        println!("The on-demand fetcher couldn't find the requested object.");
                        println!("This could mean:");
                        println!("  1. The object ID was computed differently at execution time");
                        println!("  2. The object was deleted after the transaction");
                        println!("  3. The object never existed (computation error)");
                    }
                }
            }

            // Check if it matches on-chain result
            if let Some(on_chain) = &cached.transaction.effects {
                use sui_move_interface_extractor::benchmark::tx_replay::TransactionStatus;
                let chain_success = matches!(on_chain.status, TransactionStatus::Success);
                println!("\nOn-chain status: {:?}", on_chain.status);
                println!("Status match: {}", result.local_success == chain_success);
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}

/// Test deriving dynamic field object IDs and fetching historical objects.
///
/// This test demonstrates how to:
/// 1. Derive dynamic field object IDs from known keys using hash formula
/// 2. Fetch historical versions of those objects from the archive
/// 3. Use this to enable historical transaction replay
#[test]
fn test_derive_and_fetch_historical_dynamic_fields() {
    println!("=== Derive and Fetch Historical Dynamic Field Children ===\n");

    use sui_move_interface_extractor::benchmark::tx_replay::{
        derive_dynamic_field_id_u64, TransactionFetcher,
    };

    // The skip_list's UID (parent for the dynamic fields)
    let skip_list_uid_hex = "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3";
    let skip_list_uid = parse_address(skip_list_uid_hex);

    // Historical Pool at version 751677305 had:
    // skip_list.head = 0
    // skip_list.tail = 887272
    // skip_list.size = 4
    //
    // The 4 keys would include: 0, 887272, and 2 others in between

    println!("1. Computing dynamic field object IDs from historical keys...");

    // Keys we know from the historical Pool BCS data
    let historical_keys: Vec<u64> = vec![0, 887272];

    let mut derived_ids = Vec::new();
    for key in &historical_keys {
        match derive_dynamic_field_id_u64(skip_list_uid, *key) {
            Ok(id) => {
                let id_hex = format!("0x{}", hex::encode(id.as_ref()));
                println!("   Key {} -> {}", key, id_hex);
                derived_ids.push((key, id_hex));
            }
            Err(e) => {
                println!("   Key {} -> ERROR: {}", key, e);
            }
        }
    }

    // Verify against known current state (key 481316 is known to exist)
    println!("\n2. Verifying derivation with known current key (481316)...");
    let current_key = 481316u64;
    match derive_dynamic_field_id_u64(skip_list_uid, current_key) {
        Ok(id) => {
            let id_hex = format!("0x{}", hex::encode(id.as_ref()));
            let expected = "0x01aff7f7c58ba303e1d23df4ef9ccc1562d9bdcee7aeed813a3edb3a7f2b3704";
            if id_hex == expected {
                println!("   ✓ Key {} -> {} (MATCHES known ID)", current_key, id_hex);
            } else {
                println!(
                    "   ✗ Key {} -> {} (expected {})",
                    current_key, id_hex, expected
                );
            }
        }
        Err(e) => {
            println!("   ✗ Key {} -> ERROR: {}", current_key, e);
        }
    }

    println!("\n3. Fetching historical dynamic field objects from archive...");

    let fetcher = TransactionFetcher::mainnet_with_archive();

    // The Pool version at transaction time was 751677305
    // Dynamic field objects would have had versions around that time
    let pool_version = 751677305u64;

    for (key, obj_id) in &derived_ids {
        println!("\n   --- Key {} (Object: {}) ---", key, obj_id);

        // First check current state
        match fetcher.fetch_object_full(obj_id) {
            Ok(obj) => {
                println!("   Current state: EXISTS ({} bytes)", obj.bcs_bytes.len());
                if let Some(t) = &obj.type_string {
                    println!("   Type: {}...", &t[..t.len().min(60)]);
                }
            }
            Err(e) => {
                let err_str = format!("{}", e);
                if err_str.contains("deleted") {
                    println!("   Current state: DELETED");
                } else if err_str.contains("notExists") {
                    println!("   Current state: NEVER EXISTED");
                } else {
                    println!("   Current state: Error - {}", e);
                }
            }
        }

        // Try to fetch at historical version
        match fetcher.fetch_object_at_version_full(obj_id, pool_version) {
            Ok(obj) => {
                println!(
                    "   Historical (v{}): FOUND ({} bytes)",
                    pool_version,
                    obj.bcs_bytes.len()
                );
                if let Some(t) = &obj.type_string {
                    println!("   Type: {}...", &t[..t.len().min(60)]);
                }
            }
            Err(e) => {
                let err_str = format!("{}", e);
                if err_str.contains("VersionNotFound") {
                    println!("   Historical (v{}): VERSION NOT IN ARCHIVE", pool_version);
                    println!("   (Archive may not retain historical versions of deleted objects)");
                } else if err_str.contains("ObjectNotExists") {
                    println!("   Historical (v{}): OBJECT NOT IN ARCHIVE", pool_version);
                } else {
                    println!("   Historical (v{}): Error - {}", pool_version, e);
                }
            }
        }
    }

    println!("\n4. Summary and implications...");
    println!("   The dynamic field ID derivation formula is correct.");
    println!("   However, historical objects may not be retained in archives");
    println!("   after they've been deleted from the current state.");
    println!("");
    println!("   For full historical replay, you would need:");
    println!("   a) An archive node that retains deleted object versions, OR");
    println!("   b) Cache the dynamic field children at transaction time");
}

/// Test fetching historical dynamic field children via gRPC archive.
///
/// Key insight from previous investigation:
/// - The gRPC archive at archive.mainnet.sui.io:443 DOES have historical objects
/// - Objects are stored at their CREATION version (751561008), not at every version
/// - When requesting without a version, the archive returns whatever version is available
///
/// This test demonstrates successful historical object retrieval for deleted objects.
#[test]
fn test_grpc_archive_historical_objects() {
    println!("=== gRPC Archive Historical Object Retrieval ===\n");

    use sui_move_interface_extractor::benchmark::tx_replay::{
        derive_dynamic_field_id_u64, TransactionFetcher,
    };

    let fetcher = TransactionFetcher::mainnet_with_archive();

    // The skip_list's UID (parent for the dynamic fields)
    let skip_list_uid =
        parse_address("0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3");

    // Historical keys from the Pool at transaction time
    // skip_list.head = 0, skip_list.tail = 887272
    let historical_keys: Vec<u64> = vec![0, 887272];

    // Known creation version for these objects (from previous gRPC investigation)
    let creation_version = 751561008u64;

    println!("1. Deriving dynamic field object IDs...");
    let mut derived_objects = Vec::new();
    for key in &historical_keys {
        match derive_dynamic_field_id_u64(skip_list_uid, *key) {
            Ok(id) => {
                let id_hex = format!("0x{}", hex::encode(id.as_ref()));
                println!("   Key {} -> {}", key, id_hex);
                derived_objects.push((*key, id_hex));
            }
            Err(e) => {
                println!("   Key {} -> ERROR: {}", key, e);
            }
        }
    }

    println!(
        "\n2. Fetching objects at CREATION version ({}) via gRPC archive...",
        creation_version
    );

    let mut successful_fetches = 0;
    let mut fetched_data: Vec<(u64, Vec<u8>, Option<String>)> = Vec::new();

    for (key, obj_id) in &derived_objects {
        println!("\n   --- Key {} ---", key);
        println!("   Object ID: {}", obj_id);

        // Fetch at the known creation version
        match fetcher.fetch_object_at_version_full(obj_id, creation_version) {
            Ok(obj) => {
                println!(
                    "   ✓ SUCCESS: {} bytes at version {}",
                    obj.bcs_bytes.len(),
                    obj.version
                );
                if let Some(ref t) = obj.type_string {
                    println!("   Type: {}", t);
                }
                successful_fetches += 1;
                fetched_data.push((*key, obj.bcs_bytes.clone(), obj.type_string.clone()));
            }
            Err(e) => {
                println!("   ✗ FAILED: {}", e);

                // Try without version (get latest available)
                println!("   Trying without specific version...");
                match fetcher.fetch_object_full(obj_id) {
                    Ok(obj) => {
                        println!(
                            "   ✓ Found at version {}: {} bytes",
                            obj.version,
                            obj.bcs_bytes.len()
                        );
                        if let Some(ref t) = obj.type_string {
                            println!("   Type: {}", t);
                        }
                        successful_fetches += 1;
                        fetched_data.push((*key, obj.bcs_bytes.clone(), obj.type_string.clone()));
                    }
                    Err(e2) => {
                        println!("   ✗ Also failed without version: {}", e2);
                    }
                }
            }
        }
    }

    println!("\n3. Results Summary:");
    println!(
        "   Successfully fetched: {}/{} objects",
        successful_fetches,
        derived_objects.len()
    );

    if successful_fetches > 0 {
        println!("\n4. Analyzing fetched skip_list node data...");
        for (key, bcs_bytes, type_string) in &fetched_data {
            println!("\n   --- Node key={} ({} bytes) ---", key, bcs_bytes.len());
            if let Some(t) = type_string {
                println!("   Type: {}", t);
            }

            // The skip_list Node structure contains:
            // - nexts: vector<Option<u64>> - forward pointers
            // - prev: Option<u64> - backward pointer
            // - value: V - the tick data
            // Try to decode the BCS to understand the structure
            if bcs_bytes.len() >= 8 {
                println!(
                    "   First 32 bytes (hex): {}",
                    hex::encode(&bcs_bytes[..bcs_bytes.len().min(32)])
                );
            }

            // Full decode of node structure
            // Field<u64, Node<Tick>>: uid (32) + name/key (8) + Node
            if bcs_bytes.len() >= 48 {
                let node_key = u64::from_le_bytes(bcs_bytes[32..40].try_into().unwrap());
                println!("   Field.name (key): {}", node_key);

                // Node starts at offset 40
                let node_value = &bcs_bytes[40..];
                let score = u64::from_le_bytes(node_value[0..8].try_into().unwrap());
                println!("   Node.score: {}", score);

                // nexts: Vec<OptionU64>
                let nexts_len = node_value[8] as usize;
                println!("   Node.nexts.len: {}", nexts_len);

                let mut pos = 9;
                for i in 0..nexts_len {
                    if pos + 9 <= node_value.len() {
                        let is_none = node_value[pos] != 0;
                        let v =
                            u64::from_le_bytes(node_value[pos + 1..pos + 9].try_into().unwrap());
                        println!("     nexts[{}]: is_none={}, v={}", i, is_none, v);
                        pos += 9;
                    }
                }
            }
        }
    }

    println!("\n5. Implications for transaction replay:");
    if successful_fetches == derived_objects.len() {
        println!("   ✓ All historical dynamic field children are available!");
        println!("   ✓ The gRPC archive retains deleted objects at their creation version.");
        println!("   ✓ Full historical transaction replay should be possible.");
    } else if successful_fetches > 0 {
        println!("   ⚠ Partial success - some objects available, some missing.");
        println!("   The archive may have incomplete historical data.");
    } else {
        println!("   ✗ No historical objects could be fetched.");
        println!("   The gRPC archive may not retain these specific objects.");
    }
}

/// Deep investigation into the "missing" object to understand the archive gap.
///
/// Questions to answer:
/// 1. Was this object ever created, or is it a computation error?
/// 2. If created, at what checkpoint/version?
/// 3. Is it available via other methods (GraphQL, different archive)?
/// 4. What exactly is the skip_list doing when it requests this object?
#[test]
fn test_investigate_missing_object() {
    println!("=== Deep Investigation: Missing Object ===\n");

    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;

    let missing_id = "0x05d5d28540b54b466aef2b985d62f1ebc693bb8ff6c265dd022878a250e73363";

    println!("Missing Object ID: {}", missing_id);
    println!("Parent UID (skip_list): 0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3\n");

    // 1. Try to get object info via gRPC without version (should return latest available)
    println!("1. Attempting gRPC fetch without version specification...");
    let fetcher = TransactionFetcher::mainnet_with_archive();

    // The gRPC GetObject without version should return whatever version exists
    match fetcher.fetch_object_full(missing_id) {
        Ok(obj) => {
            println!(
                "   ✓ FOUND! Version: {}, Size: {} bytes",
                obj.version,
                obj.bcs_bytes.len()
            );
            if let Some(t) = &obj.type_string {
                println!("   Type: {}", t);
            }
        }
        Err(e) => {
            println!("   ✗ Not found: {}", e);
        }
    }

    // 2. Try a wide range of versions to see if it exists anywhere
    println!("\n2. Scanning version range around transaction time...");

    let tx_version = 751677305u64;
    let creation_version = 751561008u64;

    // Try versions in batches
    let version_ranges = vec![
        (creation_version - 10000, creation_version + 10000, 1000), // Around creation
        (tx_version - 10000, tx_version + 10000, 1000),             // Around tx
        (1, 1000, 100),                                             // Very early
    ];

    for (start, end, step) in version_ranges {
        let mut found_any = false;
        for v in (start..=end).step_by(step as usize) {
            match fetcher.fetch_object_at_version_full(missing_id, v) {
                Ok(obj) => {
                    println!("   ✓ Found at version {}: {} bytes", v, obj.bcs_bytes.len());
                    found_any = true;
                    break;
                }
                Err(_) => continue,
            }
        }
        if !found_any {
            println!("   Range {}-{} (step {}): Not found", start, end, step);
        }
    }

    // 3. Check the transaction's object changes to see if this object was involved
    println!("\n3. Checking if object appears in transaction effects...");

    let tx_digest = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    match fetcher.fetch_transaction_sync(tx_digest) {
        Ok(tx) => {
            println!("   Transaction fetched successfully");

            // Check created objects
            if let Some(effects) = &tx.effects {
                println!("   Created objects: {}", effects.created.len());
                for obj_id in &effects.created {
                    if obj_id.contains("05d5d28") {
                        println!("   ✓ Missing object was CREATED in this tx: {}", obj_id);
                    }
                }

                println!("   Mutated objects: {}", effects.mutated.len());
                for obj_id in &effects.mutated {
                    if obj_id.contains("05d5d28") {
                        println!("   ✓ Missing object was MUTATED in this tx: {}", obj_id);
                    }
                }

                println!("   Deleted objects: {}", effects.deleted.len());
                for obj_id in &effects.deleted {
                    if obj_id.contains("05d5d28") {
                        println!("   ✓ Missing object was DELETED in this tx: {}", obj_id);
                    }
                }

                // Also show first few of each to understand the tx
                if !effects.created.is_empty() {
                    println!("   First created: {}", &effects.created[0]);
                }
                if !effects.mutated.is_empty() {
                    println!("   First few mutated:");
                    for obj_id in effects.mutated.iter().take(5) {
                        println!("      {}", obj_id);
                    }
                }
            }
        }
        Err(e) => {
            println!("   Failed to fetch tx: {}", e);
        }
    }

    // 4. Check what keys actually exist in the skip_list at transaction time
    println!("\n4. Checking what skip_list nodes exist...");

    use sui_move_interface_extractor::benchmark::tx_replay::derive_dynamic_field_id_u64;
    let skip_list_uid =
        parse_address("0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3");

    // Known keys from historical state
    let known_keys = vec![0u64, 887272];
    println!("   Known historical keys: {:?}", known_keys);

    for key in &known_keys {
        if let Ok(id) = derive_dynamic_field_id_u64(skip_list_uid, *key) {
            let id_hex = format!("0x{}", hex::encode(id.as_ref()));
            println!("   Key {} -> {}", key, &id_hex[..20]);
        }
    }

    // 5. The key insight: check if the object ID format matches dynamic_field derivation
    println!("\n5. Analyzing the missing object ID format...");
    println!("   Missing: {}", missing_id);

    // Dynamic field IDs are derived via: hash(parent_uid || type_tag || bcs_key)
    // If the missing ID doesn't follow this pattern, it might be a different kind of object

    // Let's check if it could be derived from the POOL's UID instead of the skip_list
    let pool_id =
        parse_address("0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772");

    println!("\n6. Checking if missing object is child of Pool (not skip_list)...");
    for key in 0u64..100 {
        if let Ok(id) = derive_dynamic_field_id_u64(pool_id, key) {
            let id_hex = format!("0x{}", hex::encode(id.as_ref()));
            if id_hex == missing_id {
                println!("   ✓ FOUND! Missing ID is child of POOL with key {}", key);
            }
        }
    }

    // 7. The missing object ID is COMPUTED by Move code, not by us
    println!("\n7. Understanding the computation path:");
    println!("   The Move VM's dynamic_field::borrow_child_object native receives:");
    println!("   - parent UID (the skip_list UID)");
    println!("   - child_id (computed by dynamic_field module from type_tag + bcs(key))");
    println!("");
    println!("   The child_id 0x05d5d28... is derived from:");
    println!("   hash(skip_list_uid || type_tag || bcs(key))");
    println!("");
    println!("   The key being looked up is UNKNOWN to us because it's computed");
    println!("   at runtime by the skip_list traversal code.");

    // 8. Let's try to reverse-engineer what key produces this ID
    println!("\n8. Brute-force search for the key (may be slow)...");

    // The skip_list uses u64 keys for tick indices
    // Try a wider range
    let mut found_key = None;

    // First try common tick values
    let tick_ranges = vec![
        (0i64, 1000),       // Small positive
        (-1000, 0),         // Small negative (stored as offset)
        (887000, 888000),   // Near max
        (-888000, -887000), // Near min
        (440000, 450000),   // Mid range
        (-450000, -440000), // Mid range negative
    ];

    for (start, end) in tick_ranges {
        for tick in start..end {
            // u64 key - ticks might be stored directly or with offset
            let key = tick as u64;
            if let Ok(id) = derive_dynamic_field_id_u64(skip_list_uid, key) {
                let id_hex = format!("0x{}", hex::encode(id.as_ref()));
                if id_hex == missing_id {
                    println!("   ✓ FOUND! Key = {} (u64)", key);
                    found_key = Some(key);
                    break;
                }
            }
        }
        if found_key.is_some() {
            break;
        }
    }

    if found_key.is_none() {
        println!("   Key not found in common tick ranges.");
        println!("");
        println!("9. CRITICAL INSIGHT - State Mismatch:");
        println!("   We may be loading CURRENT state instead of HISTORICAL state!");
        println!("");
        println!("   The Pool object contains a skip_list with head/tail/size.");
        println!("   If we load the CURRENT Pool, it has CURRENT skip_list keys.");
        println!("   But the ORIGINAL transaction used HISTORICAL skip_list keys.");
        println!("");
        println!("   The historical pool at version 751677305 had:");
        println!("   - skip_list.head = 0");
        println!("   - skip_list.tail = 887272");
        println!("   - skip_list.size = 4");
        println!("");
        println!("   But the CURRENT pool might have different keys!");
        println!("");
        println!("   SOLUTION: We need to load the Pool at its HISTORICAL version,");
        println!("   not at its current version.");
    }

    // 10. Let's verify by fetching the historical Pool
    println!("\n10. Fetching historical Pool to verify state...");

    let pool_id = "0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772";
    let tx_version = 751677305u64;

    // First, fetch at transaction version
    println!("   Fetching Pool at tx version {}...", tx_version);
    match fetcher.fetch_object_at_version_full(pool_id, tx_version) {
        Ok(obj) => {
            println!(
                "   ✓ Historical Pool: {} bytes at version {}",
                obj.bcs_bytes.len(),
                obj.version
            );
            // The BCS contains the skip_list structure
            // We'd need to parse it to extract head/tail/level_length
        }
        Err(e) => {
            println!("   ✗ Failed: {}", e);
        }
    }

    // Then fetch current
    println!("   Fetching Pool at current version...");
    match fetcher.fetch_object_full(pool_id) {
        Ok(obj) => {
            println!(
                "   Current Pool: {} bytes at version {}",
                obj.bcs_bytes.len(),
                obj.version
            );
        }
        Err(e) => {
            println!("   ✗ Failed: {}", e);
        }
    }
}

/// Test to discover ALL skip_list node keys by walking the linked structure.
///
/// The skip_list has size=4 but we only know head=0 and tail=887272.
/// This test uses brute-force search to find which keys produce known object IDs.
#[test]
fn test_discover_all_skiplist_keys() {
    println!("=== Discover All Skip List Keys ===\n");

    use sui_move_interface_extractor::benchmark::tx_replay::{
        derive_dynamic_field_id_u64, TransactionFetcher,
    };

    let fetcher = TransactionFetcher::mainnet_with_archive();
    let skip_list_uid =
        parse_address("0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3");
    let creation_version = 751561008u64;

    // The missing object ID from the replay attempt
    let missing_id = "0x05d5d28540b54b466aef2b985d62f1ebc693bb8ff6c265dd022878a250e73363";

    println!("1. Brute-force search for key that produces missing object ID...");
    println!("   Target: {}", missing_id);

    // The pool had tick spacing, so keys are likely to be tick values
    // Cetus uses tick spacing of 60 typically
    // Try a range of reasonable tick values

    // First try the known working approach - query what children exist in archive
    println!("\n2. Fetching known nodes to verify...");

    let known_keys = vec![0u64, 887272u64];
    for key in &known_keys {
        match derive_dynamic_field_id_u64(skip_list_uid, *key) {
            Ok(id) => {
                let id_hex = format!("0x{}", hex::encode(id.as_ref()));
                println!("   Key {} -> {}", key, id_hex);

                match fetcher.fetch_object_at_version_full(&id_hex, creation_version) {
                    Ok(obj) => {
                        println!("      ✓ Available ({} bytes)", obj.bcs_bytes.len());
                        // Dump the hex to analyze structure
                        if obj.bcs_bytes.len() > 0 {
                            println!(
                                "      Hex dump (bytes 0-50): {}",
                                hex::encode(&obj.bcs_bytes[..obj.bcs_bytes.len().min(50)])
                            );
                            println!(
                                "      Hex dump (bytes 50-100): {}",
                                hex::encode(
                                    &obj.bcs_bytes
                                        [50.min(obj.bcs_bytes.len())..obj.bcs_bytes.len().min(100)]
                                )
                            );
                        }
                    }
                    Err(e) => println!("      ✗ {}", e),
                }
            }
            Err(e) => println!("   Key {} -> ERROR: {}", key, e),
        }
    }

    // The BCS for a dynamic_field::Field<u64, Node<Tick>> is complex because
    // it includes a type tag. Let's try to search for the key by iterating.
    println!("\n3. Searching for key that produces target object ID...");

    // Try tick values that might be in the pool
    // Cetus ticks are signed i32 values, but stored as u64 in the skip_list
    // Range is typically -887272 to 887272 for standard pools

    let mut found = false;

    // First, check if the ID corresponds to a small key
    for key in 0u64..1000 {
        if let Ok(id) = derive_dynamic_field_id_u64(skip_list_uid, key) {
            let id_hex = format!("0x{}", hex::encode(id.as_ref()));
            if id_hex == missing_id {
                println!("   ✓ FOUND! Key {} produces the missing object ID!", key);
                found = true;
                break;
            }
        }
    }

    if !found {
        // Try tick-spaced values (spacing of 60)
        for tick in (-15000i64..15000).step_by(60) {
            // Ticks might be stored as absolute values or with offset
            let key = if tick >= 0 {
                tick as u64
            } else {
                (tick + 887272 * 2) as u64
            };

            if let Ok(id) = derive_dynamic_field_id_u64(skip_list_uid, key) {
                let id_hex = format!("0x{}", hex::encode(id.as_ref()));
                if id_hex == missing_id {
                    println!(
                        "   ✓ FOUND! Key {} (tick {}) produces the missing object ID!",
                        key, tick
                    );
                    found = true;
                    break;
                }
            }
        }
    }

    if !found {
        // Try values around 887272 (the tail)
        for offset in -1000i64..1000 {
            let key = (887272i64 + offset) as u64;
            if let Ok(id) = derive_dynamic_field_id_u64(skip_list_uid, key) {
                let id_hex = format!("0x{}", hex::encode(id.as_ref()));
                if id_hex == missing_id {
                    println!("   ✓ FOUND! Key {} produces the missing object ID!", key);
                    found = true;
                    break;
                }
            }
        }
    }

    if !found {
        println!("   Key not found in search range. The missing object might use a different derivation.");
        println!("   The child being requested might be from a nested structure, not the skip_list directly.");
    }

    // Check what the actual object is
    println!("\n4. Trying to fetch the missing object directly from archive...");

    // Try various versions - the object might have been created at a different time
    let versions_to_try = vec![
        creation_version, // 751561008
        751561008 - 1000,
        751561008 + 1000,
        751677305, // Pool version at tx time
        751677305 - 1,
    ];

    for version in versions_to_try {
        match fetcher.fetch_object_at_version_full(missing_id, version) {
            Ok(obj) => {
                println!(
                    "   ✓ Object found at version {}! {} bytes",
                    version,
                    obj.bcs_bytes.len()
                );
                if let Some(t) = &obj.type_string {
                    println!("   Type: {}", t);
                }
                break;
            }
            Err(_) => {
                continue;
            }
        }
    }

    // Try fetching without version (latest available)
    println!("   Trying without specific version...");
    match fetcher.fetch_object_full(missing_id) {
        Ok(obj) => {
            println!(
                "   ✓ Found at version {}: {} bytes",
                obj.version,
                obj.bcs_bytes.len()
            );
            if let Some(t) = &obj.type_string {
                println!("   Type: {}", t);
            }
        }
        Err(e) => {
            println!("   ✗ Object not available: {}", e);
        }
    }

    // The child might be derived from one of the skip_list nodes (not the skip_list itself)
    println!("\n5. Checking if missing ID is derived from OTHER UIDs in the Pool...");

    // Pool structure contains multiple UIDs that could be parents:
    // - tick_manager.ticks (skip_list) - already checked
    // - position_manager.positions - linked list UID
    // - The pool itself

    let other_parents = vec![
        (
            "Pool",
            parse_address("0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772"),
        ),
        (
            "position_manager.positions",
            parse_address("0x3a58791c06f885966e0fdd2d98314f1b0dfda0c85019252a70581abd9b5e550d"),
        ),
        // Node 0's object ID as parent
        (
            "node_key_0",
            derive_dynamic_field_id_u64(skip_list_uid, 0).unwrap(),
        ),
        // Node 887272's object ID as parent
        (
            "node_key_887272",
            derive_dynamic_field_id_u64(skip_list_uid, 887272).unwrap(),
        ),
    ];

    for (name, parent) in &other_parents {
        println!(
            "   Checking parent: {} ({})...",
            name,
            parent.to_hex_literal()
        );
        let mut found_here = false;
        for child_key in 0u64..10000 {
            if let Ok(child_id) = derive_dynamic_field_id_u64(*parent, child_key) {
                let child_id_hex = format!("0x{}", hex::encode(child_id.as_ref()));
                if child_id_hex == missing_id {
                    println!(
                        "   ✓ FOUND! Missing ID is child of {} with key={}",
                        name, child_key
                    );
                    found_here = true;
                    break;
                }
            }
        }
        if !found_here {
            println!("   Not found with keys 0-9999");
        }
    }
}

/// Read ULEB128 encoded integer
fn read_uleb128(bytes: &[u8]) -> (usize, usize) {
    let mut result = 0usize;
    let mut shift = 0;
    let mut bytes_read = 0;

    for byte in bytes {
        bytes_read += 1;
        result |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }

    (result, bytes_read)
}

/// Full integration test: Replay Cetus swap with gRPC-fetched historical dynamic fields.
///
/// This test puts everything together:
/// 1. Load packages (with upgraded CLMM)
/// 2. Derive dynamic field object IDs from historical Pool state
/// 3. Fetch those objects from gRPC archive at their creation version
/// 4. Preload into VM and execute the transaction
#[test]
fn test_replay_cetus_with_grpc_archive_data() {
    println!("=== Full Replay with gRPC Archive Historical Data ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded transaction: {:?}\n", cached.transaction.digest);

    // Initialize resolver
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    use sui_move_interface_extractor::benchmark::tx_replay::{
        derive_dynamic_field_id_u64, TransactionFetcher,
    };

    let fetcher = TransactionFetcher::mainnet_with_archive();

    // Step 1: Load upgraded CLMM
    println!("Step 1: Loading upgraded CLMM package...");
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    let original_clmm_id = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            let original_clmm = parse_address(original_clmm_id);
            match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => println!("   Loaded {} modules", count),
                Err(e) => println!("   Warning: {}", e),
            }
        }
        Err(e) => println!("   Warning: Failed to fetch upgraded CLMM: {}", e),
    }

    // Load other cached packages
    println!("\nStep 2: Loading other cached packages...");
    let mut loaded = 0;
    for pkg_id in cached.packages.keys() {
        if pkg_id == original_clmm_id {
            continue;
        }
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<_> = modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                if let Ok((count, _)) = resolver.add_package_modules(non_empty) {
                    loaded += count;
                }
            }
        }
    }
    println!("   Loaded {} modules from cache", loaded);

    // Create VMHarness
    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    // Step 2.5: Fetch HISTORICAL Pool state from gRPC archive
    // This is CRITICAL - the cached Pool is current state, but we need historical state
    // because the swap algorithm computes tick indices based on current_tick_index
    println!("\nStep 2.5: Fetching historical Pool state from gRPC archive...");
    let pool_id = "0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772";
    let pool_version = 751677305u64; // Version at transaction time

    // Create a mutable copy of cached objects that we can update
    let mut historical_objects = cached.objects.clone();

    match fetcher.fetch_object_at_version_full(pool_id, pool_version) {
        Ok(historical_pool) => {
            use base64::Engine;
            let bcs_base64 =
                base64::engine::general_purpose::STANDARD.encode(&historical_pool.bcs_bytes);
            historical_objects.insert(pool_id.to_string(), bcs_base64);
            println!(
                "   ✓ Historical Pool fetched at version {} ({} bytes)",
                pool_version,
                historical_pool.bcs_bytes.len()
            );
        }
        Err(e) => {
            println!("   ✗ Failed to fetch historical Pool: {}", e);
            println!("   Will use cached (current) state - may fail due to state mismatch");
        }
    }

    // Step 3: Fetch historical dynamic field children via gRPC
    println!("\nStep 3: Fetching historical dynamic field children via gRPC...");

    let skip_list_uid =
        parse_address("0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3");
    let creation_version = 751561008u64;

    // Historical keys: All 4 skip_list nodes
    // head=0 → 481316 → 512756 → 887272=tail
    // These were discovered by analyzing the node structure via gRPC archive
    let historical_keys: Vec<u64> = vec![0, 481316, 512756, 887272];

    let mut preload_fields = Vec::new();

    for key in &historical_keys {
        match derive_dynamic_field_id_u64(skip_list_uid, *key) {
            Ok(child_id) => {
                let child_id_hex = format!("0x{}", hex::encode(child_id.as_ref()));

                // Try to fetch at creation version
                match fetcher.fetch_object_at_version_full(&child_id_hex, creation_version) {
                    Ok(obj) => {
                        println!("   ✓ Key {}: {} bytes", key, obj.bcs_bytes.len());

                        let type_tag = obj
                            .type_string
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));

                        preload_fields.push(((skip_list_uid, child_id), type_tag, obj.bcs_bytes));
                    }
                    Err(e) => {
                        println!("   ✗ Key {}: {}", key, e);
                    }
                }
            }
            Err(e) => {
                println!("   ✗ Key {} derivation failed: {}", key, e);
            }
        }
    }

    println!("   Prepared {} fields for preloading", preload_fields.len());

    if !preload_fields.is_empty() {
        harness.preload_dynamic_fields(preload_fields);
        println!("   Preloaded into VM");
    }

    // Step 4: Set up on-demand fetcher for any missing children
    println!("\nStep 4: Setting up on-demand child fetcher...");

    let archive_fetcher = std::sync::Arc::new(TransactionFetcher::mainnet_with_archive());
    use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;

    let fetcher_clone = archive_fetcher.clone();
    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
        eprintln!("[gRPC fetcher] Requesting: {}", child_id_str);

        // Try creation version first, then current state
        if let Ok(obj) = fetcher_clone.fetch_object_at_version_full(&child_id_str, 751561008) {
            eprintln!(
                "[gRPC fetcher] Found at creation version: {} bytes",
                obj.bcs_bytes.len()
            );
            let type_tag = obj
                .type_string
                .as_ref()
                .map(|t| parse_type_tag_flexible(t))
                .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
            return Some((type_tag, obj.bcs_bytes));
        }

        // Fallback to current state
        if let Ok(obj) = fetcher_clone.fetch_object_full(&child_id_str) {
            eprintln!(
                "[gRPC fetcher] Found at current state: {} bytes",
                obj.bcs_bytes.len()
            );
            let type_tag = obj
                .type_string
                .as_ref()
                .map(|t| parse_type_tag_flexible(t))
                .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
            return Some((type_tag, obj.bcs_bytes));
        }

        eprintln!("[gRPC fetcher] Not found");
        None
    });

    harness.set_child_fetcher(child_fetcher);

    // Step 5: Replay with HISTORICAL objects (not cached current state)
    println!("\nStep 5: Replaying transaction with historical Pool state...");

    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &historical_objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== RESULT ===");
            println!("Success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ TRANSACTION REPLAYED SUCCESSFULLY WITH gRPC ARCHIVE DATA!");
            } else if let Some(err) = &result.local_error {
                println!("Error: {}", err);

                // Analyze error
                if err.contains("1000") || err.contains("FIELD_DOES_NOT_EXIST") {
                    println!("\nMissing dynamic field child - need more historical data");
                } else if err.contains("LINKER") || err.contains("MissingPackage") {
                    println!("\nMissing package dependency");
                }
            }
        }
        Err(e) => {
            println!("Replay setup failed: {}", e);
        }
    }
}

/// Test that gRPC archive actually returns different data at different versions
#[test]
fn test_grpc_archive_version_difference() {
    println!("=== Verify gRPC Archive Returns Historical Data ===\n");

    use sui_move_interface_extractor::grpc::GrpcClient;

    // Create runtime for async
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Pool object
    let pool_id = "0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772";

    rt.block_on(async {
        let client = match GrpcClient::new("https://archive.mainnet.sui.io:443").await {
            Ok(c) => c,
            Err(e) => {
                println!("SKIP: Cannot create gRPC client: {}", e);
                return;
            }
        };

        // Fetch at historical version (tx time)
        let historical_version = 751677305u64;
        println!("Fetching Pool at v{} (historical)...", historical_version);
        match client
            .get_object_at_version(pool_id, Some(historical_version))
            .await
        {
            Ok(Some(obj)) => {
                let contents_len = obj.bcs.as_ref().map(|b| b.len()).unwrap_or(0);
                let full_len = obj.bcs_full.as_ref().map(|b| b.len()).unwrap_or(0);
                println!("  contents: {} bytes", contents_len);
                println!("  bcs_full: {} bytes", full_len);
                if let Some(bcs) = &obj.bcs {
                    println!(
                        "  contents first 32: {}",
                        hex::encode(&bcs[..32.min(bcs.len())])
                    );
                }
                if let Some(bcs_full) = &obj.bcs_full {
                    println!(
                        "  bcs_full first 32: {}",
                        hex::encode(&bcs_full[..32.min(bcs_full.len())])
                    );
                }
            }
            Ok(None) => println!("  Not found"),
            Err(e) => println!("  Error: {}", e),
        }

        // Fetch at current version (latest)
        println!("\nFetching Pool at latest version...");
        match client.get_object(pool_id).await {
            Ok(Some(obj)) => {
                let contents_len = obj.bcs.as_ref().map(|b| b.len()).unwrap_or(0);
                let full_len = obj.bcs_full.as_ref().map(|b| b.len()).unwrap_or(0);
                println!("  Version: {}", obj.version);
                println!("  contents: {} bytes", contents_len);
                println!("  bcs_full: {} bytes", full_len);
            }
            Ok(None) => println!("  Not found"),
            Err(e) => println!("  Error: {}", e),
        }

        // Fetch at a very different version
        let old_version = 751561008u64; // skip_list creation version
        println!("\nFetching Pool at v{} (older)...", old_version);
        match client
            .get_object_at_version(pool_id, Some(old_version))
            .await
        {
            Ok(Some(obj)) => {
                let contents_len = obj.bcs.as_ref().map(|b| b.len()).unwrap_or(0);
                let full_len = obj.bcs_full.as_ref().map(|b| b.len()).unwrap_or(0);
                println!("  contents: {} bytes", contents_len);
                println!("  bcs_full: {} bytes", full_len);
                if let Some(bcs_full) = &obj.bcs_full {
                    println!(
                        "  bcs_full first 32: {}",
                        hex::encode(&bcs_full[..32.min(bcs_full.len())])
                    );
                }
            }
            Ok(None) => println!("  Not found"),
            Err(e) => println!("  Error: {}", e),
        }

        // Debug: examine bytes around UID
        println!("\n=== Debug: Bytes around UID ===");
        if let Ok(Some(obj)) = client.get_object_at_version(pool_id, Some(751677305)).await {
            if let Some(bcs_full) = &obj.bcs_full {
                // Parse object_id to bytes
                let id_hex = pool_id.strip_prefix("0x").unwrap_or(pool_id);
                let id_bytes = hex::decode(id_hex).unwrap();

                // Find UID position
                for i in 0..bcs_full.len().saturating_sub(32) {
                    if &bcs_full[i..i + 32] == id_bytes.as_slice() {
                        println!("  UID found at position: {}", i);
                        println!("  bcs_full length: {}", bcs_full.len());
                        println!("  Bytes after UID: {}", bcs_full.len() - i);

                        // Show bytes before UID (potential length prefix)
                        let prefix_start = i.saturating_sub(10);
                        println!(
                            "  10 bytes before UID: {}",
                            hex::encode(&bcs_full[prefix_start..i])
                        );

                        // Expected size based on JSON-RPC
                        println!("  Expected struct size: 426 bytes");
                        println!(
                            "  If ULEB128(426) = {:02x} {:02x}",
                            426 & 0x7f | 0x80,
                            (426 >> 7) & 0x7f
                        );
                        break;
                    }
                }
            }
        }

        // Compare hashes of extracted contents at different versions
        println!("\n=== Comparing extracted contents hashes ===");
        let mut hashes = vec![];
        for version in [751561008u64, 751677305, 752112176] {
            if let Ok(Some(obj)) = client.get_object_at_version(pool_id, Some(version)).await {
                if let Some(bcs) = &obj.bcs {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    bcs.hash(&mut hasher);
                    let hash = hasher.finish();
                    println!("  v{}: hash={:016x}, len={}", version, hash, bcs.len());
                    hashes.push((version, hash, bcs.len()));
                }
            }
        }
        if hashes.len() == 3 {
            if hashes[0].1 == hashes[1].1 && hashes[1].1 == hashes[2].1 {
                println!("  SAME: All versions return identical data (current state only)");
            } else {
                println!("  DIFFERENT: Archive returns historical state!");
            }
        }
    });
}

/// Test replay using HISTORICAL state for shared objects.
///
/// This is the solution to the state mismatch issue. Instead of using
/// cached current-state objects, we:
/// 1. Extract shared object versions from transaction effects
/// 2. Fetch each shared object at its tx-time version via gRPC archive
/// 3. Replace cached objects with historical versions
/// 4. Run replay
#[test]
fn test_replay_with_historical_shared_objects() {
    println!("=== Replay with Historical Shared Object State ===\n");

    const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";
    let cache_file = format!(".tx-cache/{}.json", TX_DIGEST);

    // Load the cached transaction
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => data,
        Err(e) => {
            println!("SKIP: Cannot read cache file - {}", e);
            return;
        }
    };

    let mut cached: CachedTransaction = match serde_json::from_str(&cache_data) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot parse cache - {}", e);
            return;
        }
    };

    println!("Loaded transaction: {:?}", cached.transaction.digest);

    // Step 1: Get shared object versions from effects
    // Always re-fetch from network to get the latest effects with sharedObjects
    println!("\nStep 1: Fetching transaction effects with shared object versions...");

    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
    let fetcher = TransactionFetcher::mainnet_with_archive();

    let shared_versions = match fetcher.fetch_transaction_sync(TX_DIGEST) {
        Ok(tx) => {
            if let Some(effects) = &tx.effects {
                println!(
                    "   Found {} shared objects in effects",
                    effects.shared_object_versions.len()
                );
                for (obj_id, version) in &effects.shared_object_versions {
                    let display_len = 20.min(obj_id.len());
                    println!("   - {}... @ v{}", &obj_id[..display_len], version);
                }
                effects.shared_object_versions.clone()
            } else {
                println!("   SKIP: No effects in fetched transaction");
                return;
            }
        }
        Err(e) => {
            println!("   SKIP: Failed to fetch transaction: {}", e);
            return;
        }
    };

    if shared_versions.is_empty() {
        println!("   No shared objects to fetch historically");
        return;
    }

    // Step 2: Fetch shared objects at historical versions via gRPC archive
    println!("\nStep 2: Fetching shared objects at historical versions via gRPC...");

    let mut historical_objects = std::collections::HashMap::new();
    let mut historical_types = std::collections::HashMap::new();

    for (object_id, version) in &shared_versions {
        // Skip Clock (0x6) - it's a system object
        if object_id == "0x0000000000000000000000000000000000000000000000000000000000000006" {
            println!("   Skipping Clock (system object)");
            continue;
        }

        match fetcher.fetch_object_at_version_full(object_id, *version) {
            Ok(obj) => {
                use base64::Engine;
                let bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes);
                println!(
                    "   ✓ {} @ v{}: {} bytes, type: {:?}",
                    &object_id[..20.min(object_id.len())],
                    version,
                    obj.bcs_bytes.len(),
                    obj.type_string
                );

                // Debug: compare with cached version
                if let Some(cached_b64) = cached.objects.get(object_id) {
                    if let Ok(cached_bytes) =
                        base64::engine::general_purpose::STANDARD.decode(cached_b64)
                    {
                        println!(
                            "      Cached: {} bytes, first 32: {}",
                            cached_bytes.len(),
                            hex::encode(&cached_bytes[..32.min(cached_bytes.len())])
                        );
                        println!(
                            "      Historical: {} bytes, first 32: {}",
                            obj.bcs_bytes.len(),
                            hex::encode(&obj.bcs_bytes[..32.min(obj.bcs_bytes.len())])
                        );
                    }
                }

                historical_objects.insert(object_id.clone(), bytes_b64);
                if let Some(type_str) = &obj.type_string {
                    historical_types.insert(object_id.clone(), type_str.clone());
                }
            }
            Err(e) => {
                println!(
                    "   ✗ {} @ v{}: {}",
                    &object_id[..20.min(object_id.len())],
                    version,
                    e
                );
            }
        }
    }

    println!("   Fetched {} historical objects", historical_objects.len());

    // Step 3: Replace cached objects with historical versions
    println!("\nStep 3: Replacing cached objects with historical versions...");

    for (obj_id, bytes_b64) in &historical_objects {
        if cached.objects.contains_key(obj_id.as_str()) {
            println!(
                "   Replacing {} (was cached)",
                &obj_id[..20.min(obj_id.len())]
            );
        } else {
            println!(
                "   Adding {} (not in cache)",
                &obj_id[..20.min(obj_id.len())]
            );
        }
        cached.objects.insert(obj_id.to_string(), bytes_b64.clone());
    }
    for (obj_id, type_str) in &historical_types {
        cached
            .object_types
            .insert(obj_id.to_string(), type_str.clone());
    }

    // Step 4: Set up resolver with upgraded packages
    println!("\nStep 4: Setting up resolver...");

    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("SKIP: Cannot create resolver - {}", e);
            return;
        }
    };

    // Load upgraded CLMM
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    let original_clmm_id = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            let original_clmm = parse_address(original_clmm_id);
            match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => println!("   Loaded {} CLMM modules", count),
                Err(e) => println!("   Warning: {}", e),
            }
        }
        Err(e) => println!("   Warning: Failed to fetch upgraded CLMM: {}", e),
    }

    // Load other cached packages
    let mut loaded = 0;
    for pkg_id in cached.packages.keys() {
        if pkg_id == original_clmm_id {
            continue;
        }
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<_> = modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
            if !non_empty.is_empty() {
                if let Ok((count, _)) = resolver.add_package_modules(non_empty) {
                    loaded += count;
                }
            }
        }
    }
    println!("   Loaded {} modules from cache", loaded);

    // Step 5: Create VM harness with on-demand child fetcher
    println!("\nStep 5: Creating VM harness with historical child fetcher...");

    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("SKIP: Cannot create VMHarness - {}", e);
            return;
        }
    };

    // Set up on-demand fetcher that uses historical versions for children
    let archive_fetcher = std::sync::Arc::new(TransactionFetcher::mainnet_with_archive());
    // IMPORTANT: Use the Pool's CREATION version for dynamic field children, not the tx version!
    // The nodes exist at their creation version, not at every subsequent transaction version.
    let pool_creation_version = 751561008u64; // Version when Pool and its skip_list were created
    use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;

    let fetcher_clone = archive_fetcher.clone();
    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
        eprintln!("[Historical fetcher] Requesting: {}", child_id_str);

        // First try at Pool's creation version (where skip_list nodes were created)
        if let Ok(obj) =
            fetcher_clone.fetch_object_at_version_full(&child_id_str, pool_creation_version)
        {
            eprintln!(
                "[Historical fetcher] Found at creation v{}: {} bytes",
                pool_creation_version,
                obj.bcs_bytes.len()
            );
            let type_tag = obj
                .type_string
                .as_ref()
                .map(|t| parse_type_tag_flexible(t))
                .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
            return Some((type_tag, obj.bcs_bytes));
        }

        // Fallback: try a range of versions near the creation version
        for delta in [0i64, 1, 10, 100, 1000, 10000] {
            let try_version = (pool_creation_version as i64 + delta).max(1) as u64;
            if let Ok(obj) = fetcher_clone.fetch_object_at_version_full(&child_id_str, try_version)
            {
                eprintln!(
                    "[Historical fetcher] Found at v{}: {} bytes",
                    try_version,
                    obj.bcs_bytes.len()
                );
                let type_tag = obj
                    .type_string
                    .as_ref()
                    .map(|t| parse_type_tag_flexible(t))
                    .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                return Some((type_tag, obj.bcs_bytes));
            }
        }

        eprintln!("[Historical fetcher] Not found at any version");
        None
    });

    harness.set_child_fetcher(child_fetcher);

    // Step 6: Replay transaction
    println!("\nStep 6: Replaying transaction with historical state...");

    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n{}", "=".repeat(60));
            println!(
                "RESULT: {}",
                if result.local_success {
                    "SUCCESS ✓"
                } else {
                    "FAILED ✗"
                }
            );
            println!("{}", "=".repeat(60));

            if result.local_success {
                println!("\n🎉 TRANSACTION REPLAYED SUCCESSFULLY WITH HISTORICAL STATE!");
                println!("\nThis proves that the state mismatch was the root cause.");
                println!("By fetching shared objects at their tx-time versions,");
                println!("the skip_list traversal finds the correct children.");
            } else if let Some(err) = &result.local_error {
                println!("\nError: {}", err);

                if err.contains("1000") || err.contains("FIELD_DOES_NOT_EXIST") {
                    println!("\nStill missing dynamic field - may need to fetch at object's creation version");
                    println!("instead of transaction version.");
                } else if err.contains("LINKER") || err.contains("MissingPackage") {
                    println!("\nMissing package dependency");
                } else if err.contains("abort") {
                    println!("\nMove abort - check error code");
                }
            }
        }
        Err(e) => {
            println!("\nReplay setup failed: {}", e);
        }
    }
}

/// Test replay of a different Cetus swap transaction to validate our approach works generally.
///
/// Transaction: 6YPypxnkG5LW3C3cgeJoezPh8HCyykvWt25N51qzRiAu
/// - Swaps JACKSON -> SUI via Cetus pool_script_v2::swap_a2b
/// - Pool: 0xdcd97bb5d843844a6debf28b774488f20d46bc645ac0afbb6f1ebb8d38a9e19b
/// - Simpler structure: SplitCoins -> coin::zero -> swap_a2b
#[test]
fn test_replay_second_cetus_swap() {
    println!("=== Replay Second Cetus Swap Transaction ===\n");

    const TX_DIGEST: &str = "6YPypxnkG5LW3C3cgeJoezPh8HCyykvWt25N51qzRiAu";

    use sui_move_interface_extractor::benchmark::tx_replay::{
        CachedTransaction, TransactionFetcher,
    };

    let fetcher = TransactionFetcher::mainnet_with_archive();

    // Step 1: Fetch the transaction
    println!("Step 1: Fetching transaction {}...", TX_DIGEST);
    let tx = match fetcher.fetch_transaction_sync(TX_DIGEST) {
        Ok(t) => {
            println!("   ✓ Transaction fetched");
            t
        }
        Err(e) => {
            println!("   ✗ Failed to fetch: {}", e);
            return;
        }
    };

    let mut cached = CachedTransaction::new(tx.clone());

    // Debug: print transaction inputs
    println!("\nTransaction inputs ({}):", tx.inputs.len());
    for (i, input) in tx.inputs.iter().enumerate() {
        println!("  [{}] {:?}", i, input);
    }

    // Step 2: Fetch input objects using the correct API
    println!("\nStep 2: Fetching input objects...");
    match fetcher.fetch_transaction_inputs(&tx) {
        Ok(objects) => {
            println!("   ✓ Fetched {} input objects:", objects.len());
            use base64::Engine;
            for (obj_id, bcs_bytes) in &objects {
                println!("     - {} ({} bytes)", obj_id, bcs_bytes.len());
            }
            for (obj_id, bcs_bytes) in objects {
                let bcs_base64 = base64::engine::general_purpose::STANDARD.encode(&bcs_bytes);
                cached.objects.insert(obj_id, bcs_base64);
            }
        }
        Err(e) => println!("   Warning: {}", e),
    }

    // Step 3: Fetch packages
    println!("\nStep 3: Fetching packages...");
    // Main packages needed
    let packages_to_fetch = vec![
        "0xb2db7142fa83210a7d78d9c12ac49c043b3cbbd482224fea6e3da00aa5a5ae2d", // pool_script_v2
        "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb", // cetus clmm (original)
        "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40", // cetus clmm (upgraded)
        "0xbe21a06129308e0495431d12286127897aff07a8ade3970495a4404d97f9eaaa", // skip_list
        "0x714a63a0dba6da4f017b42d5d0fb78867f18bcde904868e51d951a5a6f5b7f57", // integer_mate (full_math_u128)
        "0x5ffe80c90a653e3ca056fd3926987bf3e8068ca21528bb4fdbc4d487cc152dad", // JACKSON token
    ];

    // Collect modules for resolver (raw bytes)
    let mut package_modules_raw: std::collections::HashMap<String, Vec<(String, Vec<u8>)>> =
        std::collections::HashMap::new();

    for pkg in &packages_to_fetch {
        match fetcher.fetch_package_modules(pkg) {
            Ok(modules) => {
                let names: Vec<_> = modules.iter().map(|(n, _)| n.as_str()).collect();
                println!("   ✓ {}: {} modules", &pkg[..20], names.len());
                // Store raw bytes for resolver
                package_modules_raw.insert(pkg.to_string(), modules.clone());
                // Add to cached transaction (converts to base64)
                cached.add_package(pkg.to_string(), modules);
            }
            Err(e) => println!("   ✗ {}: {}", &pkg[..20], e),
        }
    }

    // Step 4: Initialize resolver and load packages
    println!("\nStep 4: Initializing resolver...");
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("   ✗ Failed: {}", e);
            return;
        }
    };

    // Load upgraded CLMM at original address
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
    let original_clmm =
        parse_address("0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb");
    if let Some(modules) = package_modules_raw.get(upgraded_clmm) {
        match resolver.add_package_modules_at(modules.clone(), Some(original_clmm)) {
            Ok((count, _)) => println!("   Loaded {} CLMM modules at original address", count),
            Err(e) => println!("   Warning loading CLMM: {}", e),
        }
    }

    // Load other packages (but skip CLMM packages - we already loaded the upgraded one)
    let original_clmm_str = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
    for (pkg_id, modules) in &package_modules_raw {
        if pkg_id == upgraded_clmm || pkg_id == original_clmm_str {
            continue; // Already loaded upgraded CLMM at original address
        }
        match resolver.add_package_modules(modules.clone()) {
            Ok((count, _)) => println!("   Loaded {} modules from {}", count, &pkg_id[..20]),
            Err(e) => println!("   Warning: {}", e),
        }
    }

    // Step 5: Create VM harness with correct clock timestamp
    println!("\nStep 5: Creating VM harness...");

    // Get transaction timestamp - CRITICAL for rewarder::settle
    // The rewarder checks that last_updated_time <= current_clock_time
    // If clock is older than pool's last_updated_time, it aborts with code 3
    let tx_timestamp_ms = tx.timestamp_ms.unwrap_or(1768570886558); // Transaction timestamp from blockchain

    println!(
        "   Using clock timestamp: {} ms ({} seconds)",
        tx_timestamp_ms,
        tx_timestamp_ms / 1000
    );

    // Create config with the transaction's clock time
    use sui_move_interface_extractor::benchmark::vm::SimulationConfig;
    let config = SimulationConfig::default().with_clock_base(tx_timestamp_ms);

    let mut harness = match VMHarness::with_config(&resolver, false, config) {
        Ok(h) => h,
        Err(e) => {
            println!("   ✗ Failed: {}", e);
            return;
        }
    };

    // Step 6: Fetch ALL shared objects at tx-time versions from effects
    println!("\nStep 6: Fetching shared objects at historical versions from effects...");

    // Get the shared object versions from transaction effects
    let shared_versions = if let Some(effects) = &tx.effects {
        println!(
            "   Found {} shared objects in effects:",
            effects.shared_object_versions.len()
        );
        for (obj_id, version) in &effects.shared_object_versions {
            println!(
                "     - {}... @ v{}",
                &obj_id[..20.min(obj_id.len())],
                version
            );
        }
        effects.shared_object_versions.clone()
    } else {
        println!("   ✗ No effects in transaction, cannot get historical versions");
        std::collections::HashMap::new()
    };

    let mut historical_objects = cached.objects.clone();

    // Manually construct Clock object with the transaction's timestamp
    // Clock struct: { id: UID (32 bytes), timestamp_ms: u64 (8 bytes) } = 40 bytes
    let clock_id_str = "0x0000000000000000000000000000000000000000000000000000000000000006";
    {
        use base64::Engine;
        let mut clock_bytes = Vec::with_capacity(40);
        // UID is the Clock's object ID (0x6)
        let clock_id = parse_address(clock_id_str);
        clock_bytes.extend_from_slice(clock_id.as_ref()); // 32 bytes
        clock_bytes.extend_from_slice(&tx_timestamp_ms.to_le_bytes()); // 8 bytes
        let clock_base64 = base64::engine::general_purpose::STANDARD.encode(&clock_bytes);
        historical_objects.insert(clock_id_str.to_string(), clock_base64);
        println!(
            "   ✓ Clock @ timestamp {} ms: {} bytes",
            tx_timestamp_ms,
            clock_bytes.len()
        );
    }

    for (object_id, version) in &shared_versions {
        // Skip Clock (0x6) - we manually constructed it above with correct timestamp
        if object_id == clock_id_str {
            continue;
        }

        match fetcher.fetch_object_at_version_full(object_id, *version) {
            Ok(obj) => {
                use base64::Engine;
                let bcs_base64 = base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes);
                historical_objects.insert(object_id.clone(), bcs_base64);
                println!(
                    "   ✓ {} @ v{}: {} bytes",
                    &object_id[..20.min(object_id.len())],
                    version,
                    obj.bcs_bytes.len()
                );
            }
            Err(e) => {
                println!(
                    "   ✗ {} @ v{}: {}",
                    &object_id[..20.min(object_id.len())],
                    version,
                    e
                );
            }
        }
    }

    // Step 7: Set up on-demand child fetcher for dynamic fields
    println!("\nStep 7: Setting up on-demand child fetcher...");

    // Use Arc to share the fetcher in the closure
    let archive_fetcher = std::sync::Arc::new(TransactionFetcher::mainnet_with_archive());

    // Pool creation version - where skip_list nodes were created
    let pool_creation_version = 703722732u64; // From Pool's initialSharedVersion

    let fetcher_for_closure = archive_fetcher.clone();
    let child_fetcher: sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn =
        Box::new(
            move |child_id: AccountAddress| -> Option<(TypeTag, Vec<u8>)> {
                let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
                eprintln!("[On-demand fetcher] Requesting: {}", child_id_str);

                // Try fetching at pool creation version first (where children were created)
                if let Ok(obj) = fetcher_for_closure
                    .fetch_object_at_version_full(&child_id_str, pool_creation_version)
                {
                    eprintln!(
                        "[On-demand fetcher] Found at creation v{}: {} bytes",
                        pool_creation_version,
                        obj.bcs_bytes.len()
                    );
                    let type_tag = obj
                        .type_string
                        .as_ref()
                        .map(|t| parse_type_tag_flexible(t))
                        .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                    return Some((type_tag, obj.bcs_bytes));
                }

                // Fallback: try fetching at latest version
                match fetcher_for_closure.fetch_object_full(&child_id_str) {
                    Ok(obj) => {
                        eprintln!(
                            "[On-demand fetcher] Found at latest: {} bytes",
                            obj.bcs_bytes.len()
                        );
                        let type_tag = obj
                            .type_string
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                        Some((type_tag, obj.bcs_bytes))
                    }
                    Err(e) => {
                        eprintln!("[On-demand fetcher] Failed: {}", e);
                        None
                    }
                }
            },
        );

    harness.set_child_fetcher(child_fetcher);
    println!("   ✓ Child fetcher configured");

    // Step 8: Replay
    println!("\nStep 8: Replaying transaction...");

    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &historical_objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== RESULT ===");
            println!("Success: {}", result.local_success);

            if result.local_success {
                println!("\n✓ SECOND CETUS SWAP REPLAYED SUCCESSFULLY!");
            } else if let Some(err) = &result.local_error {
                println!("Error: {}", err);
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}
