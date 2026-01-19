//! Cached PTB Replay Test
//!
//! This test loads cached mainnet PTBs and validates that:
//! 1. Cached transactions can be parsed correctly
//! 2. Cached packages have valid bytecode
//! 3. The SimulationEnvironment can be created and has framework loaded
//!
//! Run with:
//!   cargo test --test cached_ptb_replay_test -- --nocapture

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;

// =============================================================================
// Cached Transaction Format
// =============================================================================

#[derive(Debug, Deserialize)]
struct CachedTransaction {
    transaction: TransactionData,
    packages: HashMap<String, Vec<(String, String)>>, // package_id -> [(module_name, base64_bytes)]
    objects: HashMap<String, String>,                 // object_id -> base64_bytes
}

#[derive(Debug, Deserialize)]
struct TransactionData {
    digest: String,
    sender: String,
    commands: Vec<serde_json::Value>, // Keep as raw JSON for flexibility
    inputs: Vec<serde_json::Value>,
    effects: TransactionEffects,
}

#[derive(Debug, Deserialize)]
struct TransactionEffects {
    status: String,
}

// =============================================================================
// Test: Load and Parse Cached Transactions
// =============================================================================

#[test]
#[ignore] // Requires .tx-cache with valid mainnet transaction data
fn test_load_cached_transactions() {
    let cache_dir = Path::new(".tx-cache");
    if !cache_dir.exists() {
        println!("No .tx-cache directory found, skipping test");
        return;
    }

    let mut loaded = 0;
    let mut with_commands = 0;
    let mut with_packages = 0;
    let mut with_objects = 0;

    for entry in fs::read_dir(cache_dir).expect("read cache dir") {
        let path = entry.expect("entry").path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let cached: CachedTransaction = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                println!("Failed to parse {}: {}", path.display(), e);
                continue;
            }
        };

        loaded += 1;

        if !cached.transaction.commands.is_empty() {
            with_commands += 1;
        }
        if !cached.packages.is_empty() {
            with_packages += 1;
        }
        if !cached.objects.is_empty() {
            with_objects += 1;
        }
    }

    println!("\n=== Cached Transaction Summary ===");
    println!("Total loaded: {}", loaded);
    println!("With commands: {}", with_commands);
    println!("With packages: {}", with_packages);
    println!("With objects: {}", with_objects);

    assert!(loaded > 0, "Should have loaded some transactions");
}

// =============================================================================
// Test: Validate Cached Package Bytecode
// =============================================================================

#[test]
fn test_validate_cached_package_bytecode() {
    use base64::Engine;
    use move_binary_format::CompiledModule;

    let cache_dir = Path::new(".tx-cache");
    if !cache_dir.exists() {
        println!("No .tx-cache directory found");
        return;
    }

    let mut total_modules = 0;
    let mut valid_modules = 0;
    let mut invalid_modules = 0;

    for entry in fs::read_dir(cache_dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let cached: CachedTransaction = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (pkg_id, modules) in &cached.packages {
            for (module_name, base64_bytes) in modules {
                total_modules += 1;

                let bytes = match base64::engine::general_purpose::STANDARD.decode(base64_bytes) {
                    Ok(b) => b,
                    Err(e) => {
                        println!("  Failed to decode {}::{}: {}", pkg_id, module_name, e);
                        invalid_modules += 1;
                        continue;
                    }
                };

                // Try to deserialize as CompiledModule
                match CompiledModule::deserialize_with_defaults(&bytes) {
                    Ok(module) => {
                        valid_modules += 1;
                        // First 5 only
                        if valid_modules <= 5 {
                            let module_id = module.self_id();
                            let name = module_id.name().as_str();
                            println!(
                                "  ✓ Valid module: {}::{} (bytecode name: {})",
                                pkg_id, module_name, name
                            );
                        }
                    }
                    Err(e) => {
                        invalid_modules += 1;
                        println!("  ✗ Invalid bytecode {}::{}: {}", pkg_id, module_name, e);
                    }
                }
            }
        }
    }

    println!("\n=== Bytecode Validation Summary ===");
    println!("Total modules: {}", total_modules);
    println!("Valid: {}", valid_modules);
    println!("Invalid: {}", invalid_modules);

    // All modules should be valid
    assert_eq!(
        invalid_modules, 0,
        "All cached modules should have valid bytecode"
    );
}

// =============================================================================
// Test: SimulationEnvironment with Framework
// =============================================================================

#[test]
fn test_simulation_environment_with_framework() {
    let env = SimulationEnvironment::new().expect("create env");

    // Verify framework is loaded
    let modules = env.list_modules();
    println!("Framework modules loaded: {}", modules.len());

    // Check for essential framework modules
    let has_coin = modules.iter().any(|m| m.contains("coin"));
    let has_object = modules.iter().any(|m| m.contains("object"));
    let has_transfer = modules.iter().any(|m| m.contains("transfer"));

    assert!(has_coin, "Should have sui::coin");
    assert!(has_object, "Should have sui::object");
    assert!(has_transfer, "Should have sui::transfer");

    // List some functions from coin module
    if let Some(funcs) = env.list_functions("0x2::coin") {
        println!("\nSample sui::coin functions:");
        for func in funcs.iter().take(10) {
            println!("  - {}", func);
        }
    }
}

// =============================================================================
// Test: Load Cached Transaction Details
// =============================================================================

#[test]
fn test_print_cached_transaction_details() {
    let cache_dir = Path::new(".tx-cache");
    if !cache_dir.exists() {
        println!("No .tx-cache directory found");
        return;
    }

    let mut printed = 0;
    let max_to_print = 3;

    for entry in fs::read_dir(cache_dir).expect("read dir") {
        if printed >= max_to_print {
            break;
        }

        let path = entry.expect("entry").path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let cached: CachedTransaction = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Skip empty transactions
        if cached.transaction.commands.is_empty() {
            continue;
        }

        println!("\n=== Transaction: {} ===", cached.transaction.digest);
        println!("Sender: 0x{}", cached.transaction.sender);
        println!("Status: {}", cached.transaction.effects.status);
        println!("Commands: {}", cached.transaction.commands.len());
        println!("Inputs: {}", cached.transaction.inputs.len());
        println!("Cached packages: {}", cached.packages.len());
        println!("Cached objects: {}", cached.objects.len());

        // Print command types
        for (i, cmd) in cached.transaction.commands.iter().enumerate() {
            if let Some(cmd_type) = cmd.get("type").and_then(|v| v.as_str()) {
                let details = match cmd_type {
                    "MoveCall" => {
                        let pkg = cmd.get("package").and_then(|v| v.as_str()).unwrap_or("?");
                        let module = cmd.get("module").and_then(|v| v.as_str()).unwrap_or("?");
                        let func = cmd.get("function").and_then(|v| v.as_str()).unwrap_or("?");
                        format!("{}::{}::{}", pkg, module, func)
                    }
                    _ => String::new(),
                };
                println!("  Command {}: {} {}", i, cmd_type, details);
            }
        }

        // Print input types
        for (i, input) in cached.transaction.inputs.iter().enumerate() {
            if let Some(input_type) = input.get("type").and_then(|v| v.as_str()) {
                let details = match input_type {
                    "Object" | "SharedObject" => input
                        .get("object_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string(),
                    "Pure" => {
                        let bytes = input.get("bytes").and_then(|v| v.as_str()).unwrap_or("");
                        format!("{} bytes (base64)", bytes.len())
                    }
                    _ => String::new(),
                };
                println!("  Input {}: {} {}", i, input_type, details);
            }
        }

        // Print cached package info
        for (pkg_id, modules) in &cached.packages {
            let module_names: Vec<&str> = modules.iter().map(|(n, _)| n.as_str()).collect();
            println!("  Package {}: {:?}", pkg_id, module_names);
        }

        printed += 1;
    }
}

// =============================================================================
// Test: Create Coin and Execute Simple PTB
// =============================================================================

#[test]
fn test_create_coin_and_split() {
    use sui_move_interface_extractor::benchmark::ptb::{
        Argument, Command, InputValue, ObjectInput,
    };

    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a SUI coin
    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("create coin");
    println!("Created coin: {}", coin_id.to_hex_literal());

    // Get the coin object
    let coin_obj = env.get_object(&coin_id).expect("coin exists");
    println!(
        "Coin balance (from object): {} bytes",
        coin_obj.bcs_bytes.len()
    );

    // Build a simple SplitCoins PTB
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100_000_000u64.to_le_bytes().to_vec()), // 0.1 SUI
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    println!("\nExecuting SplitCoins PTB...");
    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("✓ PTB succeeded!");
        if let Some(ref effects) = result.effects {
            println!("  Created: {} objects", effects.created.len());
            println!("  Mutated: {} objects", effects.mutated.len());

            // Verify we created a new coin
            assert!(
                !effects.created.is_empty(),
                "Should have created a new coin"
            );

            // Get the new coin
            if let Some(new_coin_id) = effects.created.first() {
                if let Some(new_coin) = env.get_object(new_coin_id) {
                    println!("  New coin ID: {}", new_coin_id.to_hex_literal());
                    println!("  New coin bytes: {}", new_coin.bcs_bytes.len());
                }
            }
        }
    } else {
        println!("✗ PTB failed: {:?}", result.error);
        panic!("SplitCoins should succeed");
    }
}

// =============================================================================
// Test: Analyze Cached Transaction Patterns
// =============================================================================

#[test]
fn test_analyze_cached_transaction_patterns() {
    let cache_dir = Path::new(".tx-cache");
    if !cache_dir.exists() {
        println!("No .tx-cache directory found");
        return;
    }

    let mut command_counts: HashMap<String, usize> = HashMap::new();
    let mut input_counts: HashMap<String, usize> = HashMap::new();
    let mut unique_packages: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in fs::read_dir(cache_dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let cached: CachedTransaction = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Count command types
        for cmd in &cached.transaction.commands {
            if let Some(cmd_type) = cmd.get("type").and_then(|v| v.as_str()) {
                *command_counts.entry(cmd_type.to_string()).or_insert(0) += 1;

                // Track unique packages for MoveCall
                if cmd_type == "MoveCall" {
                    if let Some(pkg) = cmd.get("package").and_then(|v| v.as_str()) {
                        unique_packages.insert(pkg.to_string());
                    }
                }
            }
        }

        // Count input types
        for input in &cached.transaction.inputs {
            if let Some(input_type) = input.get("type").and_then(|v| v.as_str()) {
                *input_counts.entry(input_type.to_string()).or_insert(0) += 1;
            }
        }
    }

    println!("\n=== Cached Transaction Pattern Analysis ===\n");

    println!("Command type distribution:");
    let mut cmd_vec: Vec<_> = command_counts.iter().collect();
    cmd_vec.sort_by(|a, b| b.1.cmp(a.1));
    for (cmd_type, count) in cmd_vec {
        println!("  {}: {}", cmd_type, count);
    }

    println!("\nInput type distribution:");
    let mut input_vec: Vec<_> = input_counts.iter().collect();
    input_vec.sort_by(|a, b| b.1.cmp(a.1));
    for (input_type, count) in input_vec {
        println!("  {}: {}", input_type, count);
    }

    println!("\nUnique packages called: {}", unique_packages.len());

    // Show some package examples
    let framework_pkgs: Vec<_> = unique_packages
        .iter()
        .filter(|p| {
            p.starts_with("0x0000000000000000000000000000000000000000000000000000000000000002")
        })
        .take(3)
        .collect();
    let user_pkgs: Vec<_> = unique_packages
        .iter()
        .filter(|p| {
            !p.starts_with("0x0000000000000000000000000000000000000000000000000000000000000002")
                && !p.starts_with(
                    "0x0000000000000000000000000000000000000000000000000000000000000001",
                )
        })
        .take(5)
        .collect();

    if !framework_pkgs.is_empty() {
        println!("\nFramework packages:");
        for pkg in framework_pkgs {
            println!("  {}", pkg);
        }
    }

    if !user_pkgs.is_empty() {
        println!("\nUser packages (sample):");
        for pkg in user_pkgs {
            println!("  {}", pkg);
        }
    }
}
