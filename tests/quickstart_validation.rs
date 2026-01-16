//! # Quickstart Validation Test
//!
//! This test validates that a new user can successfully run the Cetus DEX swap replay
//! example from the README. It's designed to be the first test a user runs after cloning
//! the repository to verify their setup works correctly.
//!
//! ## What this test validates:
//! 1. The cached transaction data exists and loads correctly
//! 2. Historical object state (from cache or gRPC archive)
//! 3. Package loading and address aliasing for upgraded packages
//! 4. Dynamic field resolution (skip_list nodes for tick management)
//! 5. Full PTB execution with the real Move VM
//!
//! ## Data Sources (in priority order):
//! 1. **Cache**: Pre-fetched data in `.tx-cache/` (historical_objects, dynamic_field_children)
//! 2. **Network**: gRPC archive at `archive.mainnet.sui.io:443` (fallback)
//!
//! ## Usage:
//! ```bash
//! cargo test --test quickstart_validation -- --nocapture
//! ```

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::tx_replay::CachedTransaction;
use sui_move_interface_extractor::benchmark::vm::VMHarness;

/// The Cetus LEIA/SUI swap transaction used in the quickstart guide
const CETUS_TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";

/// Pool object ID
const POOL_ID: &str = "0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772";

/// Skip_list UID (parent of dynamic field nodes)
const SKIP_LIST_UID: &str = "0x6dd50d2538eb0977065755d430067c2177a93a048016270d3e56abd4c9e679b3";

/// Original CLMM package address (transaction references this)
const ORIGINAL_CLMM_ID: &str = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

/// Upgraded CLMM package address (contains the actual bytecode)
const UPGRADED_CLMM_ID: &str = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";

/// Default Pool version at transaction time (used if not in cache)
const DEFAULT_POOL_VERSION: u64 = 751677305;

/// Default skip_list node version (used if not in cache)
const DEFAULT_SKIPLIST_VERSION: u64 = 751561008;

/// Skip_list keys that need to be pre-loaded for the swap
const SKIPLIST_KEYS: [u64; 4] = [0, 481316, 512756, 887272];

/// Parse a hex address string to AccountAddress
fn parse_address(s: &str) -> AccountAddress {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    let padded = format!("{:0>64}", hex);
    let bytes: [u8; 32] = hex::decode(&padded).unwrap().try_into().unwrap();
    AccountAddress::new(bytes)
}

/// Flexible type tag parser for gRPC responses
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
    if type_str.starts_with("vector<") && type_str.ends_with('>') {
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

    let type_params = if let Some(idx) = type_str.find('<') {
        let params_str = &type_str[idx + 1..type_str.len() - 1];
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

/// Main quickstart validation test.
///
/// This test replicates what a new user would experience when following
/// the "Getting Started: Cetus DEX Swap Replay" section in the README.
#[test]
fn test_quickstart_cetus_swap_replay() {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║       QUICKSTART VALIDATION: Cetus DEX Swap Replay             ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 1: Verify cached transaction data exists
    // ═══════════════════════════════════════════════════════════════════════════
    println!("Step 1/5: Loading cached transaction data...");

    let cache_file = format!(".tx-cache/{}.json", CETUS_TX_DIGEST);
    let cache_data = match std::fs::read_to_string(&cache_file) {
        Ok(data) => {
            println!("   ✓ Found cached transaction: {}", CETUS_TX_DIGEST);
            data
        }
        Err(e) => {
            println!("   ✗ FAILED: Cannot read cache file");
            println!("     Error: {}", e);
            println!("\n   The .tx-cache/ directory should be included in the repository.");
            println!("   Please ensure you have the complete repository clone.");
            panic!("Quickstart validation failed: missing cache file");
        }
    };

    let cached: CachedTransaction = match serde_json::from_str::<CachedTransaction>(&cache_data) {
        Ok(c) => {
            println!(
                "   ✓ Parsed transaction with {} packages, {} objects",
                c.packages.len(),
                c.objects.len()
            );
            // Report cache status
            if !c.historical_objects.is_empty() {
                println!(
                    "   ✓ Found {} pre-cached historical objects",
                    c.historical_objects.len()
                );
            }
            if !c.dynamic_field_children.is_empty() {
                println!(
                    "   ✓ Found {} pre-cached dynamic field children",
                    c.dynamic_field_children.len()
                );
            }
            c
        }
        Err(e) => {
            println!("   ✗ FAILED: Cannot parse cached transaction");
            println!("     Error: {}", e);
            panic!("Quickstart validation failed: invalid cache format");
        }
    };

    // Get version from cache or use default
    let pool_version = cached
        .object_versions
        .get(POOL_ID)
        .copied()
        .unwrap_or(DEFAULT_POOL_VERSION);

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 2: Initialize resolver and load packages
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 2/5: Loading Move packages...");

    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => {
            println!("   ✓ Initialized resolver with Sui framework");
            r
        }
        Err(e) => {
            println!("   ✗ FAILED: Cannot create resolver");
            println!("     Error: {}", e);
            panic!("Quickstart validation failed: resolver initialization");
        }
    };

    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
    let fetcher = TransactionFetcher::mainnet_with_archive();

    // Check if upgraded CLMM is in cache via package_upgrades mapping
    let upgraded_clmm_addr = cached
        .package_upgrades
        .get(ORIGINAL_CLMM_ID)
        .map(|s| s.as_str())
        .unwrap_or(UPGRADED_CLMM_ID);

    // Try to load upgraded CLMM - first from cache, then from network
    let original_clmm = parse_address(ORIGINAL_CLMM_ID);
    let clmm_loaded = if let Some(modules) = cached.get_package_modules(upgraded_clmm_addr) {
        // Use cached upgraded package
        let non_empty: Vec<(String, Vec<u8>)> =
            modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
        if !non_empty.is_empty() {
            match resolver.add_package_modules_at(non_empty, Some(original_clmm)) {
                Ok((count, _)) => {
                    println!(
                        "   ✓ Loaded upgraded CLMM from cache: {} modules (at original address)",
                        count
                    );
                    true
                }
                Err(e) => {
                    println!("   ⚠ Warning loading cached CLMM: {}", e);
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // Fall back to network fetch if not in cache
    if !clmm_loaded {
        match fetcher.fetch_package_modules(UPGRADED_CLMM_ID) {
            Ok(modules) => match resolver.add_package_modules_at(modules, Some(original_clmm)) {
                Ok((count, _)) => println!(
                    "   ✓ Loaded upgraded CLMM from network: {} modules (at original address)",
                    count
                ),
                Err(e) => println!("   ⚠ Warning loading CLMM: {}", e),
            },
            Err(e) => println!("   ⚠ Could not fetch upgraded CLMM: {}", e),
        }
    }

    // Load other cached packages
    let mut loaded_count = 0;
    for pkg_id in cached.packages.keys() {
        if pkg_id == ORIGINAL_CLMM_ID || pkg_id == UPGRADED_CLMM_ID || pkg_id == upgraded_clmm_addr
        {
            continue;
        }
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            let non_empty: Vec<(String, Vec<u8>)> = modules
                .into_iter()
                .filter(|(_, b): &(String, Vec<u8>)| !b.is_empty())
                .collect();
            if !non_empty.is_empty() {
                if let Ok((count, _)) = resolver.add_package_modules(non_empty) {
                    loaded_count += count;
                }
            }
        }
    }
    println!("   ✓ Loaded {} additional modules from cache", loaded_count);

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 3: Get historical state (cache first, then network)
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 3/5: Loading historical state...");

    let mut historical_objects = cached.objects.clone();
    let mut used_network = false;

    // Try to get historical Pool from cache first
    if let Some(historical_pool_bcs) = cached.historical_objects.get(POOL_ID) {
        historical_objects.insert(POOL_ID.to_string(), historical_pool_bcs.clone());
        println!(
            "   ✓ Using cached historical Pool at version {}",
            pool_version
        );
    } else {
        // Fall back to network fetch
        println!("   → Historical Pool not in cache, fetching from gRPC archive...");
        match fetcher.fetch_object_at_version_full(POOL_ID, pool_version) {
            Ok(historical_pool) => {
                use base64::Engine;
                let bcs_base64 =
                    base64::engine::general_purpose::STANDARD.encode(&historical_pool.bcs_bytes);
                historical_objects.insert(POOL_ID.to_string(), bcs_base64);
                println!(
                    "   ✓ Fetched historical Pool at version {} ({} bytes)",
                    pool_version,
                    historical_pool.bcs_bytes.len()
                );
                used_network = true;
            }
            Err(e) => {
                println!("   ✗ FAILED: Cannot fetch historical Pool state");
                println!("     Error: {}", e);
                println!("\n   This requires network access to archive.mainnet.sui.io:443");
                println!("   Please check your network connectivity.");
                panic!("Quickstart validation failed: gRPC archive access");
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 4: Pre-load dynamic field children (skip_list nodes)
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 4/5: Loading dynamic field children...");

    let mut harness = match VMHarness::new(&resolver, false) {
        Ok(h) => h,
        Err(e) => {
            println!("   ✗ FAILED: Cannot create VM harness");
            println!("     Error: {}", e);
            panic!("Quickstart validation failed: VM initialization");
        }
    };

    use sui_move_interface_extractor::benchmark::tx_replay::derive_dynamic_field_id_u64;

    let skip_list_uid = parse_address(SKIP_LIST_UID);
    let mut preload_fields = Vec::new();
    let mut loaded_from_cache = 0;
    let mut loaded_from_network = 0;
    let mut failed_keys = Vec::new();

    for key in &SKIPLIST_KEYS {
        match derive_dynamic_field_id_u64(skip_list_uid, *key) {
            Ok(child_id) => {
                let child_id_hex = format!("0x{}", hex::encode(child_id.as_ref()));

                // Try cache first
                if let Some((_parent_id, type_string, bytes, _version)) =
                    cached.get_dynamic_field_child(&child_id_hex)
                {
                    let type_tag = parse_type_tag_flexible(&type_string);
                    preload_fields.push(((skip_list_uid, child_id), type_tag, bytes));
                    loaded_from_cache += 1;
                    continue;
                }

                // Fall back to network
                let version = DEFAULT_SKIPLIST_VERSION;
                match fetcher.fetch_object_at_version_full(&child_id_hex, version) {
                    Ok(obj) => {
                        let type_tag = obj
                            .type_string
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));

                        preload_fields.push(((skip_list_uid, child_id), type_tag, obj.bcs_bytes));
                        loaded_from_network += 1;
                        used_network = true;
                    }
                    Err(e) => {
                        println!("   ⚠ Failed to load key {}: {}", key, e);
                        failed_keys.push(*key);
                    }
                }
            }
            Err(e) => {
                println!("   ⚠ Key {} derivation failed: {}", key, e);
                failed_keys.push(*key);
            }
        }
    }

    // Report loading results
    if loaded_from_cache > 0 {
        println!(
            "   ✓ Loaded {} skip_list nodes from cache",
            loaded_from_cache
        );
    }
    if loaded_from_network > 0 {
        println!(
            "   ✓ Loaded {} skip_list nodes from network",
            loaded_from_network
        );
    }

    let total_loaded = loaded_from_cache + loaded_from_network;

    // Assert we loaded all required nodes
    assert_eq!(
        total_loaded,
        SKIPLIST_KEYS.len(),
        "Expected to load all {} skip_list nodes, but only loaded {}. Failed keys: {:?}",
        SKIPLIST_KEYS.len(),
        total_loaded,
        failed_keys
    );

    if !preload_fields.is_empty() {
        harness.preload_dynamic_fields(preload_fields);
    }
    println!("   ✓ Pre-loaded {} skip_list nodes total", total_loaded);

    // Set up on-demand fetcher for any missing children
    let archive_fetcher = std::sync::Arc::new(TransactionFetcher::mainnet_with_archive());
    use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;

    let fetcher_clone = archive_fetcher.clone();
    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));

        if let Ok(obj) =
            fetcher_clone.fetch_object_at_version_full(&child_id_str, DEFAULT_SKIPLIST_VERSION)
        {
            let type_tag = obj
                .type_string
                .as_ref()
                .map(|t| parse_type_tag_flexible(t))
                .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
            return Some((type_tag, obj.bcs_bytes));
        }

        if let Ok(obj) = fetcher_clone.fetch_object_full(&child_id_str) {
            let type_tag = obj
                .type_string
                .as_ref()
                .map(|t| parse_type_tag_flexible(t))
                .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
            return Some((type_tag, obj.bcs_bytes));
        }

        None
    });

    harness.set_child_fetcher(child_fetcher);
    println!("   ✓ Configured on-demand child fetcher");

    // ═══════════════════════════════════════════════════════════════════════════
    // STEP 5: Execute the transaction replay
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 5/5: Executing transaction replay...");

    use sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test;
    let address_aliases = build_address_aliases_for_test(&cached);

    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &historical_objects,
        &address_aliases,
    ) {
        Ok(result) => {
            if result.local_success {
                println!("   ✓ Transaction executed successfully!\n");

                // Report data source summary
                if used_network {
                    println!("   Note: Some data was fetched from network (gRPC archive).");
                    println!("   For fully offline operation, pre-cache historical data.\n");
                } else {
                    println!("   All data loaded from cache (fully offline).\n");
                }

                println!("╔════════════════════════════════════════════════════════════════╗");
                println!("║                    QUICKSTART VALIDATION PASSED                ║");
                println!("║                                                                ║");
                println!("║  Your setup is working correctly. You can now:                 ║");
                println!("║  - Replay other mainnet transactions                           ║");
                println!("║  - Use the sandbox for transaction simulation                  ║");
                println!("║  - Integrate with LLM-based transaction building               ║");
                println!("╚════════════════════════════════════════════════════════════════╝");
            } else {
                println!("   ✗ Transaction execution failed");
                if let Some(err) = &result.local_error {
                    println!("     Error: {}", err);
                }
                panic!("Quickstart validation failed: transaction execution error");
            }
        }
        Err(e) => {
            println!("   ✗ FAILED: Replay setup error");
            println!("     Error: {}", e);
            panic!("Quickstart validation failed: replay setup error");
        }
    }
}

/// Test that the cache file exists - a basic sanity check
#[test]
fn test_cache_file_exists() {
    let cache_file = format!(".tx-cache/{}.json", CETUS_TX_DIGEST);
    assert!(
        std::path::Path::new(&cache_file).exists(),
        "Cache file for Cetus swap transaction should exist at {}",
        cache_file
    );
}

/// Test that we can parse the cached transaction
#[test]
fn test_cache_parses_correctly() {
    let cache_file = format!(".tx-cache/{}.json", CETUS_TX_DIGEST);
    let cache_data =
        std::fs::read_to_string(&cache_file).expect("Should be able to read cache file");

    let cached: CachedTransaction = serde_json::from_str(&cache_data)
        .expect("Should be able to parse cache file as CachedTransaction");

    // Verify basic structure
    assert!(!cached.packages.is_empty(), "Should have packages");
    assert!(!cached.objects.is_empty(), "Should have objects");
}

/// Test gRPC archive connectivity (separate from main test for isolation)
#[test]
fn test_grpc_archive_connectivity() {
    use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;

    let fetcher = TransactionFetcher::mainnet_with_archive();

    match fetcher.fetch_object_at_version_full(POOL_ID, DEFAULT_POOL_VERSION) {
        Ok(obj) => {
            assert!(
                !obj.bcs_bytes.is_empty(),
                "Should fetch non-empty Pool object"
            );
            println!(
                "gRPC archive connectivity: OK ({} bytes)",
                obj.bcs_bytes.len()
            );
        }
        Err(e) => {
            println!("gRPC archive connectivity: FAILED - {}", e);
            println!("This test requires network access to archive.mainnet.sui.io:443");
            // Don't panic here - the main test will handle this more gracefully
        }
    }
}
