#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
//! Complex Transaction Replay Test
//!
//! This test samples recent mainnet transactions, filters out simple Sui framework
//! transactions, and attempts to replay the more complex ones (DeFi, DEX, etc.)
//!
//! Run with:
//!   cargo test --test complex_tx_replay -- --ignored --nocapture

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use sui_move_interface_extractor::benchmark::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;
use sui_move_interface_extractor::cache::CacheManager;
use sui_move_interface_extractor::data_fetcher::DataFetcher;
use sui_move_interface_extractor::grpc::{
    GrpcArgument, GrpcClient, GrpcCommand, GrpcInput, GrpcObject, GrpcTransaction,
};

/// Packages that are considered "simple" Sui framework transactions
const SIMPLE_PACKAGES: &[&str] = &[
    "0x0000000000000000000000000000000000000000000000000000000000000001", // Move stdlib
    "0x0000000000000000000000000000000000000000000000000000000000000002", // Sui framework
    "0x0000000000000000000000000000000000000000000000000000000000000003", // Sui system
];

/// Simple function patterns to exclude (framework operations)
const SIMPLE_FUNCTIONS: &[(&str, &str)] = &[
    ("coin", "split"),
    ("coin", "join"),
    ("coin", "merge"),
    ("pay", "split"),
    ("pay", "split_vec"),
    ("pay", "join"),
    ("pay", "join_vec"),
    ("transfer", "public_transfer"),
    ("transfer", "transfer"),
];

/// Check if a transaction is "simple" (only uses basic framework operations)
fn is_simple_transaction(tx: &GrpcTransaction) -> bool {
    if tx.commands.is_empty() {
        return true;
    }

    // Check all commands
    for cmd in &tx.commands {
        match cmd {
            GrpcCommand::MoveCall {
                package,
                module,
                function,
                ..
            } => {
                // Normalize package address
                let pkg_normalized = normalize_address(package);

                // If it's not a simple package, the tx is complex
                if !SIMPLE_PACKAGES.contains(&pkg_normalized.as_str()) {
                    return false;
                }

                // Check if it's a simple function
                let is_simple_func = SIMPLE_FUNCTIONS
                    .iter()
                    .any(|(m, f)| module == *m && function == *f);

                if !is_simple_func {
                    // Complex function in framework package
                    return false;
                }
            }
            GrpcCommand::Publish { .. } | GrpcCommand::Upgrade { .. } => {
                // Publish/Upgrade are interesting, not simple
                return false;
            }
            // SplitCoins, MergeCoins, TransferObjects, MakeMoveVec are simple
            _ => {}
        }
    }

    true
}

/// Normalize address to full 0x-prefixed 64-char hex
fn normalize_address(addr: &str) -> String {
    let hex = addr.strip_prefix("0x").unwrap_or(addr);
    format!("0x{:0>64}", hex)
}

/// Categorize a transaction by its primary protocol/usage
fn categorize_transaction(tx: &GrpcTransaction) -> String {
    let mut protocols: HashSet<String> = HashSet::new();

    for cmd in &tx.commands {
        if let GrpcCommand::MoveCall {
            package,
            module,
            function,
            ..
        } = cmd
        {
            let pkg = normalize_address(package);

            // Known DeFi protocols
            let protocol = match pkg.as_str() {
                // Cetus
                p if p.contains("1eabed72") || p.contains("714a63a0") => "Cetus",
                // DeepBook
                p if p.contains("dee9") => "DeepBook",
                // Scallop
                p if p.contains("efe8b36d") => "Scallop",
                // NAVI
                p if p.contains("48271d39") => "NAVI",
                // Turbos
                p if p.contains("91bfbc386a41afcfd") => "Turbos",
                // FlowX
                p if p.contains("ba153a0b") => "FlowX",
                // Aftermath
                p if p.contains("7f6ce7a") => "Aftermath",
                // Kriya
                p if p.contains("a0eba10b") => "Kriya",
                // Bucket
                p if p.contains("9e3dab13") => "Bucket",
                // Suilend
                p if p.contains("f95b06141") => "Suilend",
                // Bluefin
                p if p.contains("3a253") => "Bluefin",
                _ => {
                    // Check module/function patterns
                    if module.contains("swap") || function.contains("swap") {
                        "DEX (unknown)"
                    } else if module.contains("pool") || function.contains("pool") {
                        "DeFi (pool)"
                    } else if module.contains("stake") || function.contains("stake") {
                        "Staking"
                    } else if module.contains("borrow") || function.contains("borrow") {
                        "Lending"
                    } else if module.contains("nft") || function.contains("mint") {
                        "NFT"
                    } else {
                        "Other"
                    }
                }
            };

            protocols.insert(protocol.to_string());
        }
    }

    if protocols.is_empty() {
        "Unknown".to_string()
    } else {
        protocols.into_iter().collect::<Vec<_>>().join(", ")
    }
}

/// Summary of a transaction for display
struct TxSummary {
    digest: String,
    sender: String,
    category: String,
    commands: Vec<String>,
    status: String,
}

impl TxSummary {
    fn from_tx(tx: &GrpcTransaction) -> Self {
        let commands: Vec<String> = tx
            .commands
            .iter()
            .map(|cmd| match cmd {
                GrpcCommand::MoveCall {
                    package,
                    module,
                    function,
                    ..
                } => {
                    let short_pkg = &package[..std::cmp::min(10, package.len())];
                    format!("{}::{}::{}", short_pkg, module, function)
                }
                GrpcCommand::SplitCoins { .. } => "SplitCoins".to_string(),
                GrpcCommand::MergeCoins { .. } => "MergeCoins".to_string(),
                GrpcCommand::TransferObjects { .. } => "TransferObjects".to_string(),
                GrpcCommand::Publish { .. } => "Publish".to_string(),
                GrpcCommand::Upgrade { .. } => "Upgrade".to_string(),
                GrpcCommand::MakeMoveVec { .. } => "MakeMoveVec".to_string(),
            })
            .collect();

        Self {
            digest: tx.digest.clone(),
            sender: tx.sender.clone(),
            category: categorize_transaction(tx),
            commands,
            status: tx.status.clone().unwrap_or_else(|| "unknown".to_string()),
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_sample_and_filter_complex_transactions() {
    println!("=== Complex Transaction Sampling ===\n");

    // Connect to mainnet gRPC
    let client = match GrpcClient::mainnet().await {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot connect to mainnet gRPC: {}", e);
            return;
        }
    };

    println!("Connected to: {}\n", client.endpoint());

    // Collect transactions until we have 50+ total
    let target_total = 50;
    let mut all_txs: Vec<GrpcTransaction> = Vec::new();
    let mut checkpoints_scanned = 0;

    println!(
        "Collecting {} transactions from recent checkpoints...\n",
        target_total
    );

    // Subscribe and collect from live stream
    let mut stream = match client.subscribe_checkpoints().await {
        Ok(s) => s,
        Err(e) => {
            println!("SKIP: Cannot subscribe to checkpoints: {}", e);
            return;
        }
    };

    let start = std::time::Instant::now();
    let max_duration = Duration::from_secs(30);

    while all_txs.len() < target_total && start.elapsed() < max_duration {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(checkpoint))) => {
                checkpoints_scanned += 1;
                let ptb_txs: Vec<_> = checkpoint
                    .transactions
                    .into_iter()
                    .filter(|t| t.is_ptb())
                    .collect();

                println!(
                    "Checkpoint {}: {} PTB transactions",
                    checkpoint.sequence_number,
                    ptb_txs.len()
                );

                all_txs.extend(ptb_txs);
            }
            Ok(Some(Err(e))) => {
                println!("Stream error: {}", e);
                break;
            }
            Ok(None) => {
                println!("Stream ended");
                break;
            }
            Err(_) => {
                println!("Timeout waiting for checkpoint");
                break;
            }
        }
    }

    println!(
        "\nCollected {} transactions from {} checkpoints\n",
        all_txs.len(),
        checkpoints_scanned
    );

    // Separate simple vs complex
    let (simple, complex): (Vec<_>, Vec<_>) =
        all_txs.iter().partition(|tx| is_simple_transaction(tx));

    println!("=== Transaction Classification ===");
    println!("Simple (framework only): {}", simple.len());
    println!("Complex (DeFi/other):    {}", complex.len());
    println!();

    // Categorize complex transactions
    let mut category_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for tx in &complex {
        let cat = categorize_transaction(tx);
        *category_counts.entry(cat).or_insert(0) += 1;
    }

    println!("=== Complex Transaction Categories ===");
    let mut sorted: Vec<_> = category_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (cat, count) in &sorted {
        println!("  {}: {}", cat, count);
    }
    println!();

    // Show details of first 10 complex transactions
    println!("=== Sample Complex Transactions ===\n");
    for (i, tx) in complex.iter().take(10).enumerate() {
        let summary = TxSummary::from_tx(tx);
        println!("{}. {} [{}]", i + 1, summary.digest, summary.status);
        println!("   Category: {}", summary.category);
        println!("   Commands: {}", summary.commands.len());
        for (j, cmd) in summary.commands.iter().take(5).enumerate() {
            println!("     {}: {}", j, cmd);
        }
        if summary.commands.len() > 5 {
            println!("     ... and {} more", summary.commands.len() - 5);
        }
        println!();
    }

    // Store complex transaction digests for replay testing
    let complex_digests: Vec<String> = complex.iter().map(|t| t.digest.clone()).collect();
    println!(
        "Complex transaction digests for replay testing: {:?}",
        &complex_digests[..std::cmp::min(10, complex_digests.len())]
    );
}

#[tokio::test]
#[ignore]
async fn test_replay_complex_transactions() {
    println!("=== Complex Transaction Replay Test ===\n");

    // Connect to mainnet for live transactions
    let client = match GrpcClient::mainnet().await {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot connect to mainnet gRPC: {}", e);
            return;
        }
    };

    // Connect to archive for historical object fetching
    let archive = match GrpcClient::archive().await {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot connect to archive gRPC: {}", e);
            return;
        }
    };

    println!("Live endpoint: {}", client.endpoint());
    println!("Archive endpoint: {}", archive.endpoint());
    println!();

    // Collect complex transactions
    let mut complex_txs: Vec<GrpcTransaction> = Vec::new();
    let target_complex = 20;

    println!("Collecting {} complex transactions...\n", target_complex);

    let mut stream = match client.subscribe_checkpoints().await {
        Ok(s) => s,
        Err(e) => {
            println!("SKIP: Cannot subscribe: {}", e);
            return;
        }
    };

    let start = std::time::Instant::now();
    let max_duration = Duration::from_secs(60);

    while complex_txs.len() < target_complex && start.elapsed() < max_duration {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(checkpoint))) => {
                for tx in checkpoint.transactions {
                    if tx.is_ptb() && !is_simple_transaction(&tx) {
                        println!(
                            "Found complex tx: {} ({})",
                            tx.digest,
                            categorize_transaction(&tx)
                        );
                        complex_txs.push(tx);
                        if complex_txs.len() >= target_complex {
                            break;
                        }
                    }
                }
            }
            Ok(Some(Err(e))) => {
                println!("Stream error: {}", e);
                break;
            }
            Ok(None) | Err(_) => break,
        }
    }

    println!("\nCollected {} complex transactions\n", complex_txs.len());

    // Now attempt replay on each
    println!("=== Replay Results ===\n");

    let mut success_count = 0;
    let mut fail_count = 0;
    let mut skip_count = 0;

    for (i, tx) in complex_txs.iter().enumerate() {
        print!("{}. {} ... ", i + 1, tx.digest);

        // For now, we just validate we can fetch all input objects
        let mut all_inputs_available = true;
        let mut missing_objects: Vec<String> = Vec::new();

        for input in &tx.inputs {
            match input {
                sui_move_interface_extractor::grpc::GrpcInput::Object {
                    object_id,
                    version,
                    ..
                } => {
                    match archive
                        .get_object_at_version(object_id, Some(*version))
                        .await
                    {
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            all_inputs_available = false;
                            missing_objects.push(format!("{}@{}", object_id, version));
                        }
                        Err(e) => {
                            all_inputs_available = false;
                            missing_objects.push(format!("{}@{} (err: {})", object_id, version, e));
                        }
                    }
                }
                sui_move_interface_extractor::grpc::GrpcInput::SharedObject {
                    object_id,
                    initial_version,
                    ..
                } => {
                    // For shared objects, we might need the version at tx time
                    // For now, just check it exists at initial version
                    match archive
                        .get_object_at_version(object_id, Some(*initial_version))
                        .await
                    {
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            // Try latest version
                            match archive.get_object(object_id).await {
                                Ok(Some(_)) => {}
                                _ => {
                                    all_inputs_available = false;
                                    missing_objects.push(format!("{} (shared)", object_id));
                                }
                            }
                        }
                        Err(_) => {
                            // Try latest
                            match archive.get_object(object_id).await {
                                Ok(Some(_)) => {}
                                _ => {
                                    all_inputs_available = false;
                                    missing_objects.push(format!("{} (shared)", object_id));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if all_inputs_available {
            println!("✓ inputs available ({})", categorize_transaction(tx));
            success_count += 1;
        } else {
            println!("✗ missing inputs:");
            for obj in &missing_objects[..std::cmp::min(3, missing_objects.len())] {
                println!("     - {}", obj);
            }
            if missing_objects.len() > 3 {
                println!("     ... and {} more", missing_objects.len() - 3);
            }
            fail_count += 1;
        }
    }

    println!("\n=== Summary ===");
    println!("Inputs available: {}", success_count);
    println!("Missing inputs:   {}", fail_count);
    println!("Skipped:          {}", skip_count);
    println!(
        "Success rate:     {:.1}%",
        100.0 * success_count as f64 / (success_count + fail_count + skip_count) as f64
    );
}

/// Parse a hex address string to AccountAddress
fn parse_address(s: &str) -> AccountAddress {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    let padded = format!("{:0>64}", hex);
    let bytes: [u8; 32] = hex::decode(&padded).unwrap().try_into().unwrap();
    AccountAddress::new(bytes)
}

/// Parse a type string like "0xaddr::module::Name<T1, T2>" into a TypeTag
fn parse_type_string(type_str: &str) -> Option<TypeTag> {
    // Handle primitive types
    match type_str {
        "u8" => return Some(TypeTag::U8),
        "u16" => return Some(TypeTag::U16),
        "u32" => return Some(TypeTag::U32),
        "u64" => return Some(TypeTag::U64),
        "u128" => return Some(TypeTag::U128),
        "u256" => return Some(TypeTag::U256),
        "bool" => return Some(TypeTag::Bool),
        "address" => return Some(TypeTag::Address),
        "signer" => return Some(TypeTag::Signer),
        _ => {}
    }

    // Handle vector types
    if type_str.starts_with("vector<") && type_str.ends_with(">") {
        let inner = &type_str[7..type_str.len() - 1];
        return parse_type_string(inner).map(|t| TypeTag::Vector(Box::new(t)));
    }

    // Parse struct types: "0xaddr::module::Name" or "0xaddr::module::Name<T1, T2>"
    let (base, type_params) = if let Some(idx) = type_str.find('<') {
        let base = &type_str[..idx];
        let params_str = &type_str[idx + 1..type_str.len() - 1];
        // Simple param splitting (doesn't handle nested generics well)
        let params: Vec<TypeTag> = params_str
            .split(',')
            .filter_map(|s| parse_type_string(s.trim()))
            .collect();
        (base, params)
    } else {
        (type_str, vec![])
    };

    let parts: Vec<&str> = base.split("::").collect();
    if parts.len() != 3 {
        return None;
    }

    let address = parse_address(parts[0]);
    let module = Identifier::new(parts[1]).ok()?;
    let name = Identifier::new(parts[2]).ok()?;

    Some(TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    })))
}

/// Convert GrpcArgument to PTB Argument
fn convert_argument(arg: &GrpcArgument) -> Option<Argument> {
    match arg {
        GrpcArgument::Input(idx) => Some(Argument::Input(*idx as u16)),
        GrpcArgument::Result(idx) => Some(Argument::Result(*idx as u16)),
        GrpcArgument::NestedResult(cmd_idx, result_idx) => {
            Some(Argument::NestedResult(*cmd_idx as u16, *result_idx as u16))
        }
        GrpcArgument::GasCoin => None, // GasCoin not supported in our PTB args - skip
    }
}

/// Convert GrpcCommand to PTB Command
fn convert_command(cmd: &GrpcCommand) -> Option<Command> {
    match cmd {
        GrpcCommand::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => {
            let type_args: Vec<TypeTag> = type_arguments
                .iter()
                .filter_map(|s| parse_type_string(s))
                .collect();

            // Convert arguments, filtering out GasCoin references
            let args: Vec<Argument> = arguments.iter().filter_map(convert_argument).collect();

            Some(Command::MoveCall {
                package: parse_address(package),
                module: Identifier::new(module.as_str()).ok()?,
                function: Identifier::new(function.as_str()).ok()?,
                type_args,
                args,
            })
        }
        GrpcCommand::SplitCoins { coin, amounts } => {
            let coin_arg = convert_argument(coin)?;
            let amount_args: Vec<Argument> = amounts.iter().filter_map(convert_argument).collect();
            Some(Command::SplitCoins {
                coin: coin_arg,
                amounts: amount_args,
            })
        }
        GrpcCommand::MergeCoins { coin, sources } => {
            let dest_arg = convert_argument(coin)?;
            let source_args: Vec<Argument> = sources.iter().filter_map(convert_argument).collect();
            Some(Command::MergeCoins {
                destination: dest_arg,
                sources: source_args,
            })
        }
        GrpcCommand::TransferObjects { objects, address } => {
            let obj_args: Vec<Argument> = objects.iter().filter_map(convert_argument).collect();
            let addr_arg = convert_argument(address)?;
            Some(Command::TransferObjects {
                objects: obj_args,
                address: addr_arg,
            })
        }
        GrpcCommand::MakeMoveVec {
            element_type,
            elements,
        } => {
            let type_tag = element_type.as_ref().and_then(|s| parse_type_string(s));
            let elem_args: Vec<Argument> = elements.iter().filter_map(convert_argument).collect();
            Some(Command::MakeMoveVec {
                type_tag,
                elements: elem_args,
            })
        }
        GrpcCommand::Publish { .. } | GrpcCommand::Upgrade { .. } => {
            // Skip publish/upgrade for now
            None
        }
    }
}

/// Replay result for a single transaction
#[derive(Debug)]
struct ReplayResult {
    digest: String,
    category: String,
    original_status: String,
    replay_status: ReplayStatus,
    error_message: Option<String>,
    commands_executed: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum ReplayStatus {
    Success,
    Failed,
    Skipped,
    PackageMissing,
    ObjectMissing,
}

impl std::fmt::Display for ReplayStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayStatus::Success => write!(f, "✓ Success"),
            ReplayStatus::Failed => write!(f, "✗ Failed"),
            ReplayStatus::Skipped => write!(f, "⊘ Skipped"),
            ReplayStatus::PackageMissing => write!(f, "⊘ Package missing"),
            ReplayStatus::ObjectMissing => write!(f, "⊘ Object missing"),
        }
    }
}

/// Full replay test that attempts actual VM execution
#[tokio::test]
#[ignore]
async fn test_full_vm_replay() {
    println!("=== Full VM Transaction Replay Test ===\n");

    // Connect to mainnet gRPC for live transactions
    let client = match GrpcClient::mainnet().await {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot connect to mainnet gRPC: {}", e);
            return;
        }
    };

    // Connect to archive for historical objects
    let archive = match GrpcClient::archive().await {
        Ok(c) => c,
        Err(e) => {
            println!("SKIP: Cannot connect to archive gRPC: {}", e);
            return;
        }
    };

    // Create data fetcher for packages
    let fetcher = DataFetcher::mainnet();

    println!("Connected to:");
    println!("  Live: {}", client.endpoint());
    println!("  Archive: {}", archive.endpoint());
    println!();

    // Collect complex transactions
    let target_count = 10;
    let mut complex_txs: Vec<GrpcTransaction> = Vec::new();

    println!("Collecting {} complex transactions...\n", target_count);

    let mut stream = match client.subscribe_checkpoints().await {
        Ok(s) => s,
        Err(e) => {
            println!("SKIP: Cannot subscribe: {}", e);
            return;
        }
    };

    let start = std::time::Instant::now();
    let max_duration = Duration::from_secs(30);

    while complex_txs.len() < target_count && start.elapsed() < max_duration {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(checkpoint))) => {
                for tx in checkpoint.transactions {
                    if tx.is_ptb() && !is_simple_transaction(&tx) {
                        // Skip publish/upgrade transactions
                        let has_publish = tx.commands.iter().any(|c| {
                            matches!(c, GrpcCommand::Publish { .. } | GrpcCommand::Upgrade { .. })
                        });
                        if !has_publish {
                            complex_txs.push(tx);
                            if complex_txs.len() >= target_count {
                                break;
                            }
                        }
                    }
                }
            }
            Ok(Some(Err(e))) => {
                println!("Stream error: {}", e);
                break;
            }
            Ok(None) | Err(_) => break,
        }
    }

    println!("Collected {} complex transactions\n", complex_txs.len());

    // Attempt replay on each
    let mut results: Vec<ReplayResult> = Vec::new();

    for (i, tx) in complex_txs.iter().enumerate() {
        println!(
            "--- Transaction {}/{}: {} ---",
            i + 1,
            complex_txs.len(),
            tx.digest
        );
        println!("Category: {}", categorize_transaction(tx));
        println!("Commands: {}", tx.commands.len());

        let result = attempt_replay(&tx, &archive, &fetcher).await;
        println!("Result: {}", result.replay_status);
        if let Some(ref err) = result.error_message {
            println!("  Error: {}", err);
        }
        println!();

        results.push(result);
    }

    // Summary
    println!("=== Replay Summary ===\n");

    let success = results
        .iter()
        .filter(|r| r.replay_status == ReplayStatus::Success)
        .count();
    let failed = results
        .iter()
        .filter(|r| r.replay_status == ReplayStatus::Failed)
        .count();
    let pkg_missing = results
        .iter()
        .filter(|r| r.replay_status == ReplayStatus::PackageMissing)
        .count();
    let obj_missing = results
        .iter()
        .filter(|r| r.replay_status == ReplayStatus::ObjectMissing)
        .count();
    let skipped = results
        .iter()
        .filter(|r| r.replay_status == ReplayStatus::Skipped)
        .count();

    println!("Total:           {}", results.len());
    println!("Success:         {}", success);
    println!("Failed:          {}", failed);
    println!("Package missing: {}", pkg_missing);
    println!("Object missing:  {}", obj_missing);
    println!("Skipped:         {}", skipped);

    if !results.is_empty() {
        println!(
            "\nSuccess rate: {:.1}%",
            100.0 * success as f64 / results.len() as f64
        );
    }

    // Show failed transactions
    let failures: Vec<_> = results
        .iter()
        .filter(|r| r.replay_status == ReplayStatus::Failed)
        .collect();
    if !failures.is_empty() {
        println!("\n=== Failed Transactions ===\n");
        for r in failures.iter().take(5) {
            println!("- {} ({})", r.digest, r.category);
            if let Some(ref err) = r.error_message {
                println!("  Error: {}", err);
            }
        }
    }
}

/// Attempt to replay a single transaction
async fn attempt_replay(
    tx: &GrpcTransaction,
    archive: &GrpcClient,
    fetcher: &DataFetcher,
) -> ReplayResult {
    let category = categorize_transaction(tx);
    let original_status = tx.status.clone().unwrap_or_else(|| "unknown".to_string());

    // Create simulation environment
    let mut env = match SimulationEnvironment::new() {
        Ok(e) => e,
        Err(e) => {
            return ReplayResult {
                digest: tx.digest.clone(),
                category,
                original_status,
                replay_status: ReplayStatus::Failed,
                error_message: Some(format!("Cannot create SimulationEnvironment: {}", e)),
                commands_executed: 0,
            };
        }
    };

    // Collect packages needed
    let mut packages_needed: HashSet<String> = HashSet::new();
    for cmd in &tx.commands {
        if let GrpcCommand::MoveCall { package, .. } = cmd {
            let pkg = normalize_address(package);
            // Skip framework packages
            if !SIMPLE_PACKAGES.contains(&pkg.as_str()) {
                packages_needed.insert(pkg);
            }
        }
    }

    // Fetch and deploy packages
    for pkg_addr in &packages_needed {
        match fetcher.fetch_package(pkg_addr) {
            Ok(pkg) => {
                // Convert FetchedModuleData to (String, Vec<u8>) tuples
                let modules: Vec<(String, Vec<u8>)> = pkg
                    .modules
                    .iter()
                    .map(|m| (m.name.clone(), m.bytecode.clone()))
                    .collect();

                if let Err(_e) = env.deploy_package(modules) {
                    // Package deploy failed - might be OK if dependencies are missing
                    // Continue and see if execution works
                }
            }
            Err(e) => {
                return ReplayResult {
                    digest: tx.digest.clone(),
                    category,
                    original_status,
                    replay_status: ReplayStatus::PackageMissing,
                    error_message: Some(format!("Cannot fetch package {}: {}", pkg_addr, e)),
                    commands_executed: 0,
                };
            }
        }
    }

    // Fetch input objects and build inputs
    let mut inputs: Vec<InputValue> = Vec::new();
    let mut object_bytes: HashMap<String, Vec<u8>> = HashMap::new();

    for input in &tx.inputs {
        match input {
            GrpcInput::Pure { bytes } => {
                inputs.push(InputValue::Pure(bytes.clone()));
            }
            GrpcInput::Object {
                object_id, version, ..
            } => {
                match archive
                    .get_object_at_version(object_id, Some(*version))
                    .await
                {
                    Ok(Some(obj)) => {
                        let bytes = obj.bcs.clone().unwrap_or_default();
                        object_bytes.insert(object_id.clone(), bytes.clone());
                        inputs.push(InputValue::Object(ObjectInput::Owned {
                            id: parse_address(object_id),
                            bytes,
                            type_tag: None,
                        }));
                    }
                    _ => {
                        return ReplayResult {
                            digest: tx.digest.clone(),
                            category,
                            original_status,
                            replay_status: ReplayStatus::ObjectMissing,
                            error_message: Some(format!(
                                "Cannot fetch object {}@{}",
                                object_id, version
                            )),
                            commands_executed: 0,
                        };
                    }
                }
            }
            GrpcInput::SharedObject { object_id, .. } => {
                // For shared objects, try to get at the transaction time version
                // We'll try latest if that fails
                let obj_result = archive.get_object(object_id).await;
                match obj_result {
                    Ok(Some(obj)) => {
                        let bytes = obj.bcs.clone().unwrap_or_default();
                        object_bytes.insert(object_id.clone(), bytes.clone());
                        inputs.push(InputValue::Object(ObjectInput::Shared {
                            id: parse_address(object_id),
                            bytes,
                            type_tag: None,
                        }));
                    }
                    _ => {
                        return ReplayResult {
                            digest: tx.digest.clone(),
                            category,
                            original_status,
                            replay_status: ReplayStatus::ObjectMissing,
                            error_message: Some(format!(
                                "Cannot fetch shared object {}",
                                object_id
                            )),
                            commands_executed: 0,
                        };
                    }
                }
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                // Treat receiving objects as owned for now
                match archive
                    .get_object_at_version(object_id, Some(*version))
                    .await
                {
                    Ok(Some(obj)) => {
                        let bytes = obj.bcs.clone().unwrap_or_default();
                        inputs.push(InputValue::Object(ObjectInput::Owned {
                            id: parse_address(object_id),
                            bytes,
                            type_tag: None,
                        }));
                    }
                    _ => {
                        return ReplayResult {
                            digest: tx.digest.clone(),
                            category,
                            original_status,
                            replay_status: ReplayStatus::ObjectMissing,
                            error_message: Some(format!(
                                "Cannot fetch receiving object {}",
                                object_id
                            )),
                            commands_executed: 0,
                        };
                    }
                }
            }
        }
    }

    // Convert commands
    let commands: Vec<Command> = tx.commands.iter().filter_map(convert_command).collect();

    if commands.len() != tx.commands.len() {
        return ReplayResult {
            digest: tx.digest.clone(),
            category,
            original_status,
            replay_status: ReplayStatus::Skipped,
            error_message: Some("Some commands could not be converted".to_string()),
            commands_executed: 0,
        };
    }

    // Execute PTB
    let result = env.execute_ptb(inputs, commands.clone());

    if result.success {
        ReplayResult {
            digest: tx.digest.clone(),
            category,
            original_status,
            replay_status: ReplayStatus::Success,
            error_message: None,
            commands_executed: commands.len(),
        }
    } else {
        // Extract error message from SimulationError or raw_error
        let error_msg = result
            .raw_error
            .or_else(|| result.error.map(|e| format!("{:?}", e)));

        ReplayResult {
            digest: tx.digest.clone(),
            category,
            original_status,
            replay_status: ReplayStatus::Failed,
            error_message: error_msg,
            commands_executed: commands.len(),
        }
    }
}
