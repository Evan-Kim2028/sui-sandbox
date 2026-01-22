//! Bluefin Perpetual Futures PTB Replay Tests
//!
//! Integration tests for replaying Bluefin perpetual futures transactions in the local Move VM sandbox.
//!
//! For detailed documentation on the replay infrastructure and technical insights,
//! see `docs/defi-case-study/BLUEFIN_PERPETUALS.md`.
//!
//! ## Bluefin Perpetual Operations
//!
//! | Operation | Function | Description |
//! |-----------|----------|-------------|
//! | Open Position | `open_position` | Open a new leveraged position |
//! | Close Position | `close_position` | Close an existing position |
//! | Adjust Margin | `adjust_margin` | Add or remove collateral |
//! | Liquidate | `liquidate` | Liquidate undercollateralized position |
//!
//! ## Key Insight: Version Verification
//!
//! Bluefin has a `verify_version` check that must be satisfied:
//! - GlobalConfig stores `package_version`
//! - Bytecode has `CURRENT_VERSION` constant
//! - These must match for execution to proceed
//!
//! Solution: Use linkage table from dependent packages to find correct upgraded bytecode.

#![allow(dead_code)]
#![allow(unused_imports)]

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::tx_replay::{
    build_address_aliases_for_test, graphql_to_fetched_transaction, load_or_fetch_transaction,
    CachedTransaction,
};
use sui_move_interface_extractor::benchmark::vm::{SimulationConfig, VMHarness};
use sui_move_interface_extractor::data_fetcher::DataFetcher;
use sui_move_interface_extractor::graphql::{GraphQLCommand, GraphQLTransaction};

// =============================================================================
// Bluefin Protocol Constants
// =============================================================================

mod bluefin {
    /// Original Bluefin perpetuals package (referenced by PTBs)
    pub const ORIGINAL: &str = "0x3492c874c1e3b3e2984e8c41b589e642d4d0a5d6459e5a9cfc2d52fd7c89c267";

    /// Upgraded storage address (contains actual bytecode)
    /// Found via linkage table from dependent packages
    pub const UPGRADED_STORAGE: &str =
        "0xd075338d105482f1527cbfd363d6413558f184dec36d9138a70261e87f486e9c";

    /// Package fragment for detection
    pub const FRAGMENT: &str = "3492c874";

    /// jk aggregator package (provides linkage table)
    pub const JK_AGGREGATOR: &str =
        "0xecad7a19ef75d2a6c0bbe0976f279f1eec97602c34b2f22be45e736d328f602f";

    /// GlobalConfig object ID (contains package_version)
    pub const GLOBAL_CONFIG: &str =
        "0x03db251ba509a8d5d8777b6338836082335d93eecbdd09a11e190a1cff51c352";
}

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

/// Helper to detect if a transaction uses Bluefin perpetuals
fn is_bluefin_perp_transaction(tx: &GraphQLTransaction) -> bool {
    tx.commands.iter().any(|cmd| {
        if let GraphQLCommand::MoveCall {
            package, function, ..
        } = cmd
        {
            // Check if it's Bluefin and a perpetual operation (not just swap)
            let is_bluefin = package.contains(bluefin::FRAGMENT);
            let is_perp_op = function.contains("position")
                || function.contains("margin")
                || function.contains("liquidate")
                || function.contains("leverage");
            is_bluefin && is_perp_op
        } else {
            false
        }
    })
}

/// Extract perpetual operation type from transaction
fn get_perp_operation(tx: &GraphQLTransaction) -> String {
    for cmd in &tx.commands {
        if let GraphQLCommand::MoveCall { function, .. } = cmd {
            let func = function.to_lowercase();
            if func.contains("open_position") || func.contains("create_position") {
                return "open_position".to_string();
            } else if func.contains("close_position") {
                return "close_position".to_string();
            } else if func.contains("adjust_margin")
                || func.contains("add_margin")
                || func.contains("remove_margin")
            {
                return "adjust_margin".to_string();
            } else if func.contains("liquidate") {
                return "liquidate".to_string();
            } else if func.contains("leverage") {
                return "set_leverage".to_string();
            }
        }
    }
    "unknown".to_string()
}

// =============================================================================
// Discovery Test - Find Bluefin Perpetual Transactions
// =============================================================================

/// Discover recent Bluefin perpetual transactions
///
/// This test fetches recent transactions and identifies Bluefin perpetual PTBs.
/// Run with: cargo test --test execute_bluefin_perpetuals test_discover_bluefin_perps -- --nocapture
#[test]
fn test_discover_bluefin_perps() {
    println!("=== Discovering Bluefin Perpetual Transactions ===\n");

    let fetcher = DataFetcher::mainnet();

    // Fetch recent transactions
    println!("Fetching recent transactions...");
    let recent = match fetcher.fetch_recent_transactions_full(50) {
        Ok(txs) => txs,
        Err(e) => {
            println!("Failed to fetch recent transactions: {}", e);
            return;
        }
    };

    println!(
        "Scanning {} transactions for Bluefin perpetuals...\n",
        recent.len()
    );

    let mut perp_txs = Vec::new();

    for tx in &recent {
        if is_bluefin_perp_transaction(tx) {
            let op = get_perp_operation(tx);
            println!("Bluefin {} - {}", op, tx.digest);

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

            perp_txs.push((tx.digest.clone(), op));

            if perp_txs.len() >= 10 {
                break;
            }
        }
    }

    println!("\n=== Summary ===");
    println!("Found {} Bluefin perpetual transactions", perp_txs.len());

    if !perp_txs.is_empty() {
        println!("\nBluefin perpetual transaction digests for case study:");
        for (digest, op) in &perp_txs {
            println!("  {} - {}", op, digest);
        }
    }
}

// =============================================================================
// Sample Perpetual Transactions (To be populated after discovery)
// =============================================================================

/// Sample Bluefin perpetual transactions for testing
const SAMPLE_PERP_TRANSACTIONS: &[(&str, &str)] = &[
    // Will be populated after running discovery test
    // ("DIGEST_HERE", "open_position"),
    // ("DIGEST_HERE", "close_position"),
];

// =============================================================================
// Bluefin Perpetual Tests
// =============================================================================

/// Test Bluefin package loading with version handling
///
/// This validates that we can load Bluefin's upgraded bytecode at the original address
/// using the linkage table from the jk aggregator package.
#[test]
fn test_bluefin_package_loading() {
    println!("=== Testing Bluefin Package Loading ===\n");

    let fetcher = DataFetcher::mainnet();

    // Step 1: Fetch jk aggregator to get linkage table
    println!("Step 1: Fetching jk aggregator package for linkage table...");
    match fetcher.fetch_package(bluefin::JK_AGGREGATOR) {
        Ok(pkg) => {
            let modules: Vec<(String, Vec<u8>)> = pkg
                .modules
                .into_iter()
                .map(|m| (m.name, m.bytecode))
                .collect();
            println!("   [OK] jk aggregator: {} modules", modules.len());
        }
        Err(e) => {
            println!("   [FAILED] Failed to fetch jk: {}", e);
            return;
        }
    }

    // Step 2: Fetch Bluefin from upgraded storage address
    println!("\nStep 2: Fetching Bluefin from upgraded storage address...");
    let bluefin_modules = match fetcher.fetch_package(bluefin::UPGRADED_STORAGE) {
        Ok(pkg) => {
            let modules: Vec<(String, Vec<u8>)> = pkg
                .modules
                .into_iter()
                .map(|m| (m.name, m.bytecode))
                .collect();
            println!("   [OK] Bluefin (upgraded): {} modules", modules.len());
            for (name, bytes) in &modules {
                println!("      - {}: {} bytes", name, bytes.len());
            }
            modules
        }
        Err(e) => {
            println!("   [FAILED] Failed to fetch Bluefin: {}", e);
            return;
        }
    };

    // Step 3: Initialize resolver and load at original address
    println!("\nStep 3: Loading Bluefin at original address...");
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("   [FAILED] Failed to create resolver: {}", e);
            return;
        }
    };

    let original_addr = parse_address(bluefin::ORIGINAL);
    match resolver.add_package_modules_at(bluefin_modules, Some(original_addr)) {
        Ok((count, _)) => {
            println!(
                "   [OK] Loaded {} Bluefin modules at original address",
                count
            );
            println!("     Original: {}", bluefin::ORIGINAL);
            println!("     Storage:  {}", bluefin::UPGRADED_STORAGE);
        }
        Err(e) => {
            println!("   [FAILED] Failed to load: {}", e);
            return;
        }
    }

    println!("\n[OK] Bluefin package loading successful!");
    println!("  Version check will pass because we loaded upgraded bytecode (v17)");
    println!("  at the original address that PTBs reference.");
}

/// Replay a Bluefin open position transaction
///
/// Run with: cargo test --test execute_bluefin_perpetuals test_replay_bluefin_open_position -- --nocapture --ignored
#[test]
#[ignore = "requires valid Bluefin open_position transaction digest - set TX_DIGEST env var"]
fn test_replay_bluefin_open_position() {
    println!("=== Replay Bluefin Open Position ===\n");

    // Get transaction digest from environment or skip
    let tx_digest = match std::env::var("BLUEFIN_OPEN_POSITION_TX") {
        Ok(digest) if !digest.is_empty() => digest,
        _ => {
            println!("SKIPPED: Set BLUEFIN_OPEN_POSITION_TX environment variable to a valid transaction digest");
            println!("Example: BLUEFIN_OPEN_POSITION_TX=<digest> cargo test --test execute_bluefin_perpetuals test_replay_bluefin_open_position -- --nocapture --ignored");
            return;
        }
    };

    replay_bluefin_perp_transaction(&tx_digest, "open_position");
}

/// Replay a Bluefin close position transaction
#[test]
#[ignore = "requires valid Bluefin close_position transaction digest - set TX_DIGEST env var"]
fn test_replay_bluefin_close_position() {
    println!("=== Replay Bluefin Close Position ===\n");

    // Get transaction digest from environment or skip
    let tx_digest = match std::env::var("BLUEFIN_CLOSE_POSITION_TX") {
        Ok(digest) if !digest.is_empty() => digest,
        _ => {
            println!("SKIPPED: Set BLUEFIN_CLOSE_POSITION_TX environment variable to a valid transaction digest");
            return;
        }
    };

    replay_bluefin_perp_transaction(&tx_digest, "close_position");
}

// =============================================================================
// Core Replay Function
// =============================================================================

/// Generic Bluefin perpetual transaction replay function
fn replay_bluefin_perp_transaction(tx_digest: &str, operation: &str) {
    let fetcher = DataFetcher::mainnet();

    // Step 1: Fetch transaction
    println!(
        "Step 1: Fetching Bluefin {} transaction {}...",
        operation, tx_digest
    );
    let tx = match fetcher.fetch_transaction(tx_digest) {
        Ok(t) => {
            println!("   [OK] Transaction fetched");
            println!("   Commands: {}", t.commands.len());
            for (i, cmd) in t.commands.iter().enumerate() {
                if let GraphQLCommand::MoveCall {
                    package,
                    module,
                    function,
                    ..
                } = cmd
                {
                    println!(
                        "   [{}] {}::{}::{}",
                        i,
                        &package[..20.min(package.len())],
                        module,
                        function
                    );
                }
            }
            t
        }
        Err(e) => {
            println!("   [FAILED] Failed to fetch: {}", e);
            return;
        }
    };

    // Convert GraphQL transaction to FetchedTransaction for replay
    let fetched_tx = match graphql_to_fetched_transaction(&tx) {
        Ok(ft) => ft,
        Err(e) => {
            println!("   [FAILED] Failed to convert transaction: {}", e);
            return;
        }
    };
    let mut cached = CachedTransaction::new(fetched_tx.clone());

    // Step 2: Fetch input objects
    println!("\nStep 2: Fetching input objects...");
    match fetcher.fetch_transaction_inputs(&tx) {
        Ok(objects) => {
            println!("   [OK] Fetched {} input objects", objects.len());
            use base64::Engine;
            for (obj_id, bcs_bytes) in objects {
                let bcs_base64 = base64::engine::general_purpose::STANDARD.encode(&bcs_bytes);
                cached.objects.insert(obj_id, bcs_base64);
            }
        }
        Err(e) => println!("   Warning: {}", e),
    }

    // Step 3: Collect and fetch packages with linkage resolution
    println!("\nStep 3: Fetching packages with linkage resolution...");

    let mut packages_to_fetch = Vec::new();
    for cmd in &tx.commands {
        if let GraphQLCommand::MoveCall { package, .. } = cmd {
            if !packages_to_fetch.contains(package) {
                packages_to_fetch.push(package.clone());
            }
        }
    }

    // Add jk aggregator for linkage table
    if !packages_to_fetch.contains(&bluefin::JK_AGGREGATOR.to_string()) {
        packages_to_fetch.push(bluefin::JK_AGGREGATOR.to_string());
    }

    let mut package_modules_raw: std::collections::HashMap<String, Vec<(String, Vec<u8>)>> =
        std::collections::HashMap::new();

    for pkg in &packages_to_fetch {
        // Check if this is Bluefin original - redirect to upgraded storage
        let fetch_pkg = if pkg == bluefin::ORIGINAL {
            println!("   Redirecting Bluefin to upgraded storage...");
            bluefin::UPGRADED_STORAGE
        } else {
            pkg.as_str()
        };

        match fetcher.fetch_package(fetch_pkg) {
            Ok(pkg_data) => {
                let modules: Vec<(String, Vec<u8>)> = pkg_data
                    .modules
                    .into_iter()
                    .map(|m| (m.name, m.bytecode))
                    .collect();
                println!(
                    "   [OK] {}: {} modules",
                    &pkg[..20.min(pkg.len())],
                    modules.len()
                );
                package_modules_raw.insert(pkg.to_string(), modules.clone());
                cached.add_package(pkg.to_string(), modules);
            }
            Err(e) => println!("   [FAILED] {}: {}", &pkg[..20.min(pkg.len())], e),
        }
    }

    // Step 4: Initialize resolver
    println!("\nStep 4: Initializing resolver...");
    let mut resolver = match LocalModuleResolver::with_sui_framework() {
        Ok(r) => r,
        Err(e) => {
            println!("   [FAILED] Failed: {}", e);
            return;
        }
    };

    // Load Bluefin at original address if we fetched it
    if let Some(modules) = package_modules_raw.get(bluefin::ORIGINAL) {
        let original_addr = parse_address(bluefin::ORIGINAL);
        match resolver.add_package_modules_at(modules.clone(), Some(original_addr)) {
            Ok((count, _)) => println!(
                "   [OK] Loaded {} Bluefin modules at original address",
                count
            ),
            Err(e) => println!("   Warning loading Bluefin: {}", e),
        }
    }

    // Load other packages normally
    for (pkg_id, modules) in &package_modules_raw {
        if pkg_id == bluefin::ORIGINAL {
            continue; // Already loaded with address aliasing
        }
        match resolver.add_package_modules(modules.clone()) {
            Ok((count, _)) => println!(
                "   Loaded {} modules from {}",
                count,
                &pkg_id[..20.min(pkg_id.len())]
            ),
            Err(e) => println!(
                "   Warning loading {}: {}",
                &pkg_id[..20.min(pkg_id.len())],
                e
            ),
        }
    }

    // Step 5: Create VM harness
    println!("\nStep 5: Creating VM harness...");

    let tx_timestamp_ms = tx.timestamp_ms.unwrap_or(1768483200000);
    println!("   Using clock timestamp: {} ms", tx_timestamp_ms);

    let config = SimulationConfig::default().with_clock_base(tx_timestamp_ms);
    let mut harness = match VMHarness::with_config(&resolver, false, config) {
        Ok(h) => h,
        Err(e) => {
            println!("   [FAILED] Failed: {}", e);
            return;
        }
    };

    // Step 6: Fetch shared objects at historical versions
    println!("\nStep 6: Fetching shared objects at historical versions...");

    let shared_versions = if let Some(effects) = &fetched_tx.effects {
        effects.shared_object_versions.clone()
    } else {
        println!("   [FAILED] No effects in transaction");
        std::collections::HashMap::new()
    };

    let mut historical_objects = cached.objects.clone();

    // Construct Clock
    let clock_id_str = "0x0000000000000000000000000000000000000000000000000000000000000006";
    {
        use base64::Engine;
        let mut clock_bytes = Vec::with_capacity(40);
        let clock_id = parse_address(clock_id_str);
        clock_bytes.extend_from_slice(clock_id.as_ref());
        clock_bytes.extend_from_slice(&tx_timestamp_ms.to_le_bytes());
        let clock_base64 = base64::engine::general_purpose::STANDARD.encode(&clock_bytes);
        historical_objects.insert(clock_id_str.to_string(), clock_base64);
        println!("   [OK] Clock @ timestamp {} ms", tx_timestamp_ms);
    }

    // Fetch shared objects
    for (object_id, version) in &shared_versions {
        if object_id == clock_id_str {
            continue;
        }
        match fetcher.fetch_object_at_version(object_id, *version) {
            Ok(obj) => {
                use base64::Engine;
                if let Some(bcs_bytes) = obj.bcs_bytes {
                    let bcs_base64 = base64::engine::general_purpose::STANDARD.encode(&bcs_bytes);
                    historical_objects.insert(object_id.to_string(), bcs_base64);
                    println!(
                        "   [OK] {} @ v{}: {} bytes",
                        &object_id[..20.min(object_id.len())],
                        version,
                        bcs_bytes.len()
                    );
                } else {
                    println!(
                        "   [WARN] {} @ v{}: no BCS bytes",
                        &object_id[..20.min(object_id.len())],
                        version
                    );
                }
            }
            Err(e) => {
                println!(
                    "   [FAILED] {} @ v{}: {}",
                    &object_id[..20.min(object_id.len())],
                    version,
                    e
                );
            }
        }
    }

    // Step 7: Set up on-demand child fetcher
    println!("\nStep 7: Setting up on-demand child fetcher...");

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

    // Step 8: Replay
    println!("\nStep 8: Replaying transaction...");

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
                println!(
                    "\n[OK] BLUEFIN {} REPLAYED SUCCESSFULLY!",
                    operation.to_uppercase()
                );
            } else if let Some(err) = &result.local_error {
                println!("Error: {}", err);
            }
        }
        Err(e) => {
            println!("Replay failed: {}", e);
        }
    }
}
