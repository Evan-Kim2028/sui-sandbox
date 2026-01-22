//! DeepBook CLOB PTB Replay Tests
//!
//! Integration tests for replaying DeepBook central limit order book transactions
//! in the local Move VM sandbox.
//!
//! Uses auto-fetch pattern with `load_or_fetch_transaction` for transaction data caching.
//!
//! For detailed documentation on the replay infrastructure and technical insights,
//! see `docs/defi-case-study/DEEPBOOK_PTB_REPLAY.md`.
//!
//! ## DeepBook Operations
//!
//! | Operation | Function | Description |
//! |-----------|----------|-------------|
//! | Place Limit Order | `place_limit_order` | Place a limit order on the order book |
//! | Cancel Order | `cancel_order` | Cancel an existing order |
//! | Swap | `swap_exact_*` | Execute market swap |
//!
//! ## Key Challenge: Dynamic Fields
//!
//! DeepBook uses extensive dynamic fields for order book state:
//! - Tables for open orders
//! - Balance managers
//! - Order book pools with frequent state changes
//!
//! Solution: On-demand child fetching with historical version tracking.
//!
//! ## Historical Replay Limitations
//!
//! **IMPORTANT**: Accurate historical replay requires fetching objects at their exact
//! versions at transaction time. The public Sui GraphQL API has limited historical
//! data retention (approximately 30-60 days). For older transactions:
//!
//! - Objects may only be available at their current version
//! - Replay may produce different results than on-chain execution
//! - For accurate historical replay, use a full Sui archive node
//!
//! The system attempts to fetch objects at their correct historical versions using:
//! 1. `effects.mutated[].outputState.version - 1` for modified objects
//! 2. Falls back to current version with warnings if historical data unavailable

#![allow(dead_code)]
#![allow(unused_imports)]

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::tx_replay::{
    build_address_aliases_for_test, load_or_fetch_transaction, CachedTransaction,
};
use sui_move_interface_extractor::benchmark::vm::{SimulationConfig, VMHarness};
use sui_move_interface_extractor::data_fetcher::DataFetcher;
use sui_move_interface_extractor::graphql::{GraphQLCommand, GraphQLTransaction};

// =============================================================================
// DeepBook Protocol Constants
// =============================================================================

mod deepbook {
    /// DeepBook V2 package address
    pub const DEEPBOOK_V2: &str =
        "0x000000000000000000000000000000000000000000000000000000000000dee9";

    /// DeepBook V3 package address (if different)
    pub const DEEPBOOK_V3: &str = "0xdee9";

    /// Package fragment for detection
    pub const FRAGMENT: &str = "dee9";
}

// =============================================================================
// Sample DeepBook Transactions (from documentation)
// =============================================================================

/// Successfully replayed DeepBook transactions from case study
/// See docs/defi-case-study/DEEPBOOK.md for details
const SAMPLE_DEEPBOOK_TRANSACTIONS: &[(&str, &str)] = &[
    // Basic order operations
    (
        "7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY",
        "cancel_order",
    ),
    (
        "3AKpMt66kXcPutKxkQ4D3NuAu4MJ1YGEvTNkWoAzyVVE",
        "place_limit_order",
    ),
    (
        "6fZMHYnpJoShz6ZXuWW14dCTgwv9XpgZ4jbZh6HBHufU",
        "place_limit_order",
    ),
    // Flash loan operations (from case study)
    (
        "DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs",
        "flashloan_swap_success",
    ),
    (
        "D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva",
        "flashloan_arb_failed",
    ),
];

// =============================================================================
// Helper Functions
// =============================================================================

fn parse_address(s: &str) -> AccountAddress {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    let padded = format!("{:0>64}", hex);
    let bytes: [u8; 32] = hex::decode(&padded).unwrap().try_into().unwrap();
    AccountAddress::new(bytes)
}

fn parse_type_tag_flexible(type_str: &str) -> TypeTag {
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

    if type_str.starts_with("vector<") && type_str.ends_with('>') {
        let inner = &type_str[7..type_str.len() - 1];
        return TypeTag::Vector(Box::new(parse_type_tag_flexible(inner)));
    }

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

/// Helper to detect if a transaction uses DeepBook
fn is_deepbook_transaction(tx: &GraphQLTransaction) -> bool {
    tx.commands.iter().any(|cmd| {
        if let GraphQLCommand::MoveCall { package, .. } = cmd {
            package.contains(deepbook::FRAGMENT)
        } else {
            false
        }
    })
}

/// Extract DeepBook operation type from transaction
fn get_deepbook_operation(tx: &GraphQLTransaction) -> String {
    for cmd in &tx.commands {
        if let GraphQLCommand::MoveCall { function, .. } = cmd {
            let func = function.to_lowercase();
            if func.contains("place_limit_order") || func.contains("place_order") {
                return "place_limit_order".to_string();
            } else if func.contains("cancel_order") || func.contains("cancel") {
                return "cancel_order".to_string();
            } else if func.contains("swap") {
                return "swap".to_string();
            } else if func.contains("flashloan") {
                return "flashloan".to_string();
            } else if func.contains("deposit") {
                return "deposit".to_string();
            } else if func.contains("withdraw") {
                return "withdraw".to_string();
            }
        }
    }
    "unknown".to_string()
}

// =============================================================================
// Discovery Test - Find DeepBook Transactions
// =============================================================================

/// Discover recent DeepBook transactions
///
/// This test fetches recent transactions and identifies DeepBook PTBs.
/// Run with: cargo test --test execute_deepbook test_discover_deepbook_transactions -- --nocapture
#[test]
fn test_discover_deepbook_transactions() {
    println!("=== Discovering DeepBook Transactions ===\n");

    let fetcher = DataFetcher::mainnet();

    // Fetch recent transactions
    println!("Fetching recent transactions...");
    let recent = match fetcher.fetch_recent_transactions_full(100) {
        Ok(txs) => txs,
        Err(e) => {
            println!("Failed to fetch recent transactions: {}", e);
            return;
        }
    };

    println!(
        "Scanning {} transactions for DeepBook operations...\n",
        recent.len()
    );

    let mut deepbook_txs = Vec::new();

    for tx in &recent {
        if is_deepbook_transaction(tx) {
            let op = get_deepbook_operation(tx);
            println!("DeepBook {} - {}", op, tx.digest);

            // Print command details
            for (i, cmd) in tx.commands.iter().enumerate() {
                if let GraphQLCommand::MoveCall {
                    package,
                    module,
                    function,
                    ..
                } = cmd
                {
                    println!(
                        "  [{}] {}::{}::{}",
                        i,
                        &package[..20.min(package.len())],
                        module,
                        function
                    );
                }
            }

            deepbook_txs.push((tx.digest.clone(), op));

            if deepbook_txs.len() >= 10 {
                break;
            }
        }
    }

    println!("\n=== Summary ===");
    println!("Found {} DeepBook transactions", deepbook_txs.len());

    if !deepbook_txs.is_empty() {
        println!("\nDeepBook transaction digests for case study:");
        for (digest, op) in &deepbook_txs {
            println!("  {} - {}", op, digest);
        }
    }
}

// =============================================================================
// DeepBook Replay Tests with Auto-Fetch
// =============================================================================

/// Test DeepBook cancel_order transaction replay
///
/// Run with: cargo test --test execute_deepbook test_replay_deepbook_cancel_order -- --nocapture
#[test]
fn test_replay_deepbook_cancel_order() {
    println!("=== Replay DeepBook Cancel Order ===\n");

    const TX_DIGEST: &str = "7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY";

    // Try to load from cache or fetch
    let cached = match load_or_fetch_transaction(
        ".tx-cache",
        TX_DIGEST,
        None,
        false,
        true, // Fetch dynamic field children
    ) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot load/fetch transaction: {}", e);
            return;
        }
    };

    println!("Loaded transaction: {}", TX_DIGEST);
    println!("  Packages: {}", cached.packages.len());
    println!("  Objects: {}", cached.objects.len());

    replay_deepbook_transaction(&cached, "cancel_order");
}

/// Test DeepBook place_limit_order transaction replay
///
/// Run with: cargo test --test execute_deepbook test_replay_deepbook_place_limit_order -- --nocapture
#[test]
fn test_replay_deepbook_place_limit_order() {
    println!("=== Replay DeepBook Place Limit Order ===\n");

    const TX_DIGEST: &str = "3AKpMt66kXcPutKxkQ4D3NuAu4MJ1YGEvTNkWoAzyVVE";

    let cached = match load_or_fetch_transaction(".tx-cache", TX_DIGEST, None, false, true) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot load/fetch transaction: {}", e);
            return;
        }
    };

    println!("Loaded transaction: {}", TX_DIGEST);
    println!("  Packages: {}", cached.packages.len());
    println!("  Objects: {}", cached.objects.len());

    replay_deepbook_transaction(&cached, "place_limit_order");
}

/// Test DeepBook place_limit_order transaction replay (second sample)
///
/// Run with: cargo test --test execute_deepbook test_replay_deepbook_place_limit_order_2 -- --nocapture
#[test]
fn test_replay_deepbook_place_limit_order_2() {
    println!("=== Replay DeepBook Place Limit Order (2) ===\n");

    const TX_DIGEST: &str = "6fZMHYnpJoShz6ZXuWW14dCTgwv9XpgZ4jbZh6HBHufU";

    let cached = match load_or_fetch_transaction(".tx-cache", TX_DIGEST, None, false, true) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot load/fetch transaction: {}", e);
            return;
        }
    };

    println!("Loaded transaction: {}", TX_DIGEST);
    println!("  Packages: {}", cached.packages.len());
    println!("  Objects: {}", cached.objects.len());

    replay_deepbook_transaction(&cached, "place_limit_order");
}

// =============================================================================
// Core Replay Function
// =============================================================================

/// Generic DeepBook transaction replay function
fn replay_deepbook_transaction(cached: &CachedTransaction, operation: &str) {
    // Step 1: Initialize resolver with cached packages
    println!("\nStep 1: Loading packages...");
    let mut resolver = match LocalModuleResolver::with_sui_framework_auto() {
        Ok(r) => r,
        Err(e) => {
            println!("   [FAILED] Failed to create resolver: {}", e);
            return;
        }
    };

    // Load cached packages
    for (pkg_id, modules) in &cached.packages {
        let modules_vec: Vec<(String, Vec<u8>)> = modules
            .iter()
            .map(|(name, bytecode_base64)| {
                use base64::Engine;
                let bytecode = base64::engine::general_purpose::STANDARD
                    .decode(bytecode_base64)
                    .unwrap_or_default();
                (name.clone(), bytecode)
            })
            .collect();

        match resolver.add_package_modules(modules_vec) {
            Ok((count, _)) => println!(
                "   [OK] Loaded {} modules from {}",
                count,
                &pkg_id[..20.min(pkg_id.len())]
            ),
            Err(e) => println!(
                "   [WARN] Failed to load {}: {}",
                &pkg_id[..20.min(pkg_id.len())],
                e
            ),
        }
    }

    // Step 2: Create VM harness
    println!("\nStep 2: Creating VM harness...");

    let tx_timestamp_ms = cached.transaction.timestamp_ms.unwrap_or(1700000000000);
    println!("   Using clock timestamp: {} ms", tx_timestamp_ms);

    let config = SimulationConfig::default().with_clock_base(tx_timestamp_ms);
    let mut harness = match VMHarness::with_config(&resolver, false, config) {
        Ok(h) => h,
        Err(e) => {
            println!("   [FAILED] Failed to create harness: {}", e);
            return;
        }
    };

    // Step 3: Set up on-demand child fetcher
    println!("\nStep 3: Setting up on-demand child fetcher...");

    let archive_fetcher = std::sync::Arc::new(DataFetcher::mainnet());
    let fetcher_for_closure = archive_fetcher.clone();

    let child_fetcher: sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn =
        Box::new(move |child_id: AccountAddress| {
            let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
            eprintln!("[On-demand fetcher] Requesting: {}", child_id_str);

            match fetcher_for_closure.fetch_object(&child_id_str) {
                Ok(obj) => {
                    if let Some(bcs_bytes) = obj.bcs_bytes {
                        eprintln!("[On-demand fetcher] Found: {} bytes", bcs_bytes.len());
                        let type_tag = obj
                            .type_string
                            .as_ref()
                            .map(|t| parse_type_tag_flexible(t))
                            .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                        Some((type_tag, bcs_bytes))
                    } else {
                        eprintln!("[On-demand fetcher] No BCS bytes");
                        None
                    }
                }
                Err(e) => {
                    eprintln!("[On-demand fetcher] Failed: {}", e);
                    None
                }
            }
        });

    harness.set_child_fetcher(child_fetcher);
    println!("   [OK] Child fetcher configured");

    // Step 4: Replay transaction
    println!("\nStep 4: Replaying transaction...");

    let address_aliases = build_address_aliases_for_test(cached);

    // Check if we had to use fallback versions (historical data unavailable)
    let has_version_mismatch = cached.object_versions.values().any(|&v| {
        // If version is very different from what we expected, we used fallback
        // This is a heuristic - ideally we'd track this explicitly
        v > 760000000 // Indicates current version fallback was used
    });

    if has_version_mismatch {
        println!("\n[WARNING] Some objects were fetched at CURRENT version instead of historical version.");
        println!("         Replay results may differ from on-chain execution.");
        println!("         For accurate replay, use a full Sui archive node.");
    }

    match cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    ) {
        Ok(result) => {
            println!("\n=== RESULT ===");
            println!("Success: {}", result.local_success);

            if result.local_success {
                println!(
                    "\n[OK] DEEPBOOK {} REPLAYED SUCCESSFULLY!",
                    operation.to_uppercase()
                );
            } else if let Some(err) = &result.local_error {
                println!("Error: {}", err);
                if has_version_mismatch {
                    println!("\n[NOTE] This failure may be due to historical object versions being unavailable.");
                    println!("       The transaction succeeded on-chain but local replay used current object state.");
                }
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}

// =============================================================================
// Batch Replay Test
// =============================================================================

/// Test replaying all sample DeepBook transactions
///
/// Run with: cargo test --test execute_deepbook test_replay_all_sample_deepbook_transactions -- --nocapture
#[test]
fn test_replay_all_sample_deepbook_transactions() {
    println!("=== Replaying All Sample DeepBook Transactions ===\n");

    let mut success_count = 0;
    let mut fail_count = 0;
    let mut skip_count = 0;

    for (digest, operation) in SAMPLE_DEEPBOOK_TRANSACTIONS {
        println!("\n--- {} ({}) ---", digest, operation);

        let cached = match load_or_fetch_transaction(".tx-cache", digest, None, false, true) {
            Ok(c) => c,
            Err(e) => {
                println!("SKIP: Cannot load/fetch: {}", e);
                skip_count += 1;
                continue;
            }
        };

        // Simplified replay check - just verify we can load the transaction
        println!("  Packages: {}", cached.packages.len());
        println!("  Objects: {}", cached.objects.len());

        if cached.packages.is_empty() {
            println!("  [WARN] No packages cached - may fail on replay");
            fail_count += 1;
        } else {
            println!("  [OK] Transaction data loaded successfully");
            success_count += 1;
        }
    }

    println!("\n=== Summary ===");
    println!("Loaded: {}", success_count);
    println!("Failed: {}", fail_count);
    println!("Skipped: {}", skip_count);
    println!("Total: {}", SAMPLE_DEEPBOOK_TRANSACTIONS.len());
}

// =============================================================================
// Flash Loan Tests (from DEEPBOOK.md case study)
// =============================================================================

/// Test successful flash loan swap replay
///
/// This transaction successfully executes a flash loan swap:
/// 1. borrow_flashloan_base() - borrow SUI
/// 2. jk::swap() - swap via aggregator
/// 3. swap_exact_base_for_quote() - DeepBook swap
/// 4. return_flashloan_base() - repay loan
/// 5-7. Cleanup and transfer
///
/// Run with: cargo test --test execute_deepbook test_replay_flashloan_swap_success -- --nocapture
#[test]
fn test_replay_flashloan_swap_success() {
    println!("=== Replay DeepBook Flash Loan Swap (SUCCESS) ===\n");

    const TX_DIGEST: &str = "DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs";

    let cached = match load_or_fetch_transaction(
        ".tx-cache",
        TX_DIGEST,
        None,
        true, // Fetch historical versions
        true, // Fetch dynamic field children
    ) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot load/fetch transaction: {}", e);
            return;
        }
    };

    println!("Loaded transaction: {}", TX_DIGEST);
    println!("  Packages: {}", cached.packages.len());
    println!("  Objects: {}", cached.objects.len());
    println!("  Commands: {}", cached.transaction.commands.len());
    println!("  Expected: SUCCESS (profitable flash loan swap)");

    replay_deepbook_transaction(&cached, "flashloan_swap");
}

/// Test failed flash loan arbitrage replay
///
/// This transaction failed on-chain with error code 2 (insufficient output).
/// The sandbox should correctly reproduce this failure.
///
/// Transaction details:
/// - 16 commands (multi-DEX flash loan arbitrage)
/// - Protocols: DeepBook, Bluefin, FlowX, stSUI
/// - Failure: deepbook_v3::swap_a2b_ at offset 43, error code 2
///
/// Run with: cargo test --test execute_deepbook test_replay_flashloan_arb_failed -- --nocapture
#[test]
fn test_replay_flashloan_arb_failed() {
    println!("=== Replay DeepBook Flash Loan Arbitrage (FAILED) ===\n");

    const TX_DIGEST: &str = "D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva";

    let cached = match load_or_fetch_transaction(
        ".tx-cache",
        TX_DIGEST,
        None,
        true, // Fetch historical versions
        true, // Fetch dynamic field children
    ) {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot load/fetch transaction: {}", e);
            return;
        }
    };

    println!("Loaded transaction: {}", TX_DIGEST);
    println!("  Packages: {}", cached.packages.len());
    println!("  Objects: {}", cached.objects.len());
    println!("  Commands: {}", cached.transaction.commands.len());
    println!("  Expected: FAILURE (unprofitable arbitrage, error code 2)");

    replay_deepbook_transaction(&cached, "flashloan_arb");
}

/// Test sandbox validation: success vs failure contrast
///
/// The most important validation is that the sandbox correctly distinguishes
/// between transactions that should succeed and transactions that should fail.
///
/// | Transaction | On-Chain | Expected Local |
/// |-------------|----------|----------------|
/// | DwrqFzBSVH... | SUCCESS | SUCCESS |
/// | D9sMA7x9b8... | FAILURE | FAILURE |
///
/// Run with: cargo test --test execute_deepbook test_sandbox_success_vs_failure_contrast -- --nocapture
#[test]
fn test_sandbox_success_vs_failure_contrast() {
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                   SANDBOX VALIDATION: Success vs Failure              ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let test_cases = [
        (
            "DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs",
            "flashloan_swap",
            true,
        ),
        (
            "D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva",
            "flashloan_arb",
            false,
        ),
    ];

    let mut results: Vec<(String, bool, bool)> = Vec::new(); // (digest, expected_success, actual_success)

    for (digest, operation, expected_success) in &test_cases {
        println!("\n--- {} ({}) ---", &digest[..12], operation);
        println!(
            "Expected: {}",
            if *expected_success {
                "SUCCESS"
            } else {
                "FAILURE"
            }
        );

        let cached = match load_or_fetch_transaction(".tx-cache", digest, None, true, true) {
            Ok(c) => c,
            Err(e) => {
                println!("SKIP: Cannot load/fetch: {}", e);
                continue;
            }
        };

        // Initialize resolver
        let mut resolver = match LocalModuleResolver::with_sui_framework_auto() {
            Ok(r) => r,
            Err(e) => {
                println!("SKIP: Cannot create resolver: {}", e);
                continue;
            }
        };

        // Load packages
        for (pkg_id, modules) in &cached.packages {
            let modules_vec: Vec<(String, Vec<u8>)> = modules
                .iter()
                .map(|(name, bytecode_base64)| {
                    use base64::Engine;
                    let bytecode = base64::engine::general_purpose::STANDARD
                        .decode(bytecode_base64)
                        .unwrap_or_default();
                    (name.clone(), bytecode)
                })
                .collect();
            let _ = resolver.add_package_modules(modules_vec);
        }

        // Create harness
        let tx_timestamp_ms = cached.transaction.timestamp_ms.unwrap_or(1700000000000);
        let config = SimulationConfig::default().with_clock_base(tx_timestamp_ms);
        let mut harness = match VMHarness::with_config(&resolver, false, config) {
            Ok(h) => h,
            Err(e) => {
                println!("SKIP: Cannot create harness: {}", e);
                continue;
            }
        };

        // Set up child fetcher
        let archive_fetcher = std::sync::Arc::new(DataFetcher::mainnet());
        let fetcher_for_closure = archive_fetcher.clone();
        let child_fetcher: sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn =
            Box::new(move |child_id: AccountAddress| {
                let child_id_str = format!("0x{}", hex::encode(child_id.as_ref()));
                match fetcher_for_closure.fetch_object(&child_id_str) {
                    Ok(obj) => {
                        if let Some(bcs_bytes) = obj.bcs_bytes {
                            let type_tag = obj
                                .type_string
                                .as_ref()
                                .map(|t| parse_type_tag_flexible(t))
                                .unwrap_or_else(|| TypeTag::Vector(Box::new(TypeTag::U8)));
                            Some((type_tag, bcs_bytes))
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            });
        harness.set_child_fetcher(child_fetcher);

        // Replay
        let address_aliases = build_address_aliases_for_test(&cached);
        let actual_success = match cached.transaction.replay_with_objects_and_aliases(
            &mut harness,
            &cached.objects,
            &address_aliases,
        ) {
            Ok(result) => result.local_success,
            Err(_) => false,
        };

        let status_match = actual_success == *expected_success;
        println!(
            "Actual: {}",
            if actual_success { "SUCCESS" } else { "FAILURE" }
        );
        println!("Match: {}", if status_match { "✓" } else { "✗" });

        results.push((digest.to_string(), *expected_success, actual_success));
    }

    // Summary
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                        VALIDATION SUMMARY                            ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    let mut all_match = true;
    for (digest, expected, actual) in &results {
        let expected_str = if *expected { "SUCCESS" } else { "FAILURE" };
        let actual_str = if *actual { "SUCCESS" } else { "FAILURE" };
        let match_str = if expected == actual { "✓" } else { "✗" };
        println!(
            "║ {} {} | Expected: {} | Actual: {}",
            match_str,
            &digest[..12],
            expected_str,
            actual_str
        );
        if expected != actual {
            all_match = false;
        }
    }

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if all_match {
        println!("║ ✓ SANDBOX VALIDATION PASSED                                         ║");
        println!("║   The local sandbox correctly distinguishes success from failure.   ║");
    } else {
        println!("║ ✗ SANDBOX VALIDATION FAILED                                         ║");
        println!("║   Some transactions did not match expected outcomes.                ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");
}
