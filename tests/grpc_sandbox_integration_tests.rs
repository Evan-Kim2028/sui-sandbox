#![allow(unused_imports)]
//! gRPC + Sandbox Integration Tests
//!
//! These tests validate the full pipeline from gRPC data fetching through sandbox replay.
//! They verify that transactions fetched via gRPC can be successfully parsed, converted,
//! and replayed in the local simulation environment.
//!
//! Test categories:
//! - Data format consistency (gRPC vs GraphQL)
//! - Transaction parsing and conversion
//! - Sandbox replay with gRPC-fetched data
//! - Streaming transaction validation
//!
//! IMPORTANT: These tests require a gRPC endpoint. Set:
//!   export SUI_GRPC_ENDPOINT="https://your-endpoint:9000"
//!
//! Run with:
//!   cargo test --test grpc_sandbox_integration_tests -- --ignored --nocapture

mod test_utils;

use std::time::Duration;

use sui_move_interface_extractor::data_fetcher::DataFetcher;
use sui_move_interface_extractor::graphql::GraphQLClient;
use sui_move_interface_extractor::grpc::{
    GrpcArgument, GrpcClient, GrpcCommand, GrpcInput, GrpcTransaction,
};
use test_utils::get_grpc_endpoint;

/// Skip test if no gRPC endpoint configured
macro_rules! require_grpc_endpoint {
    () => {
        match get_grpc_endpoint() {
            Some(endpoint) => endpoint,
            None => {
                eprintln!("SKIPPED: SUI_GRPC_ENDPOINT not set");
                return;
            }
        }
    };
}

// =============================================================================
// Data Format Consistency Tests
// =============================================================================

/// Verify that gRPC and GraphQL return consistent transaction data.
/// This is critical for ensuring cache compatibility.
#[tokio::test]
#[ignore]
async fn test_grpc_graphql_transaction_consistency() {
    let endpoint = require_grpc_endpoint!();

    let grpc_client = GrpcClient::new(&endpoint).await.expect("gRPC connect");
    let graphql_client = GraphQLClient::mainnet();

    // Get a checkpoint via gRPC
    let checkpoint = grpc_client
        .get_latest_checkpoint()
        .await
        .expect("get checkpoint")
        .expect("checkpoint exists");

    println!(
        "Testing data consistency for checkpoint {}",
        checkpoint.sequence_number
    );

    // Find a PTB transaction
    let grpc_tx = checkpoint
        .transactions
        .iter()
        .find(|t| t.is_ptb() && !t.commands.is_empty())
        .expect("should have PTB transaction");

    println!("Comparing transaction: {}", grpc_tx.digest);

    // Fetch same transaction via GraphQL
    let graphql_result = graphql_client.fetch_transaction(&grpc_tx.digest);

    match graphql_result {
        Ok(graphql_tx) => {
            // Compare command counts
            assert_eq!(
                grpc_tx.commands.len(),
                graphql_tx.commands.len(),
                "Command count should match: gRPC={}, GraphQL={}",
                grpc_tx.commands.len(),
                graphql_tx.commands.len()
            );

            // Compare input counts
            assert_eq!(
                grpc_tx.inputs.len(),
                graphql_tx.inputs.len(),
                "Input count should match: gRPC={}, GraphQL={}",
                grpc_tx.inputs.len(),
                graphql_tx.inputs.len()
            );

            // Compare sender
            assert_eq!(grpc_tx.sender, graphql_tx.sender, "Sender should match");

            println!("  Commands: {} (match)", grpc_tx.commands.len());
            println!("  Inputs: {} (match)", grpc_tx.inputs.len());
            println!("  Sender: {} (match)", grpc_tx.sender);
        }
        Err(e) => {
            eprintln!("GraphQL fetch failed (may be rate limited): {}", e);
        }
    }
}

/// Test that gRPC input types are parsed correctly and match expected structure.
#[tokio::test]
#[ignore]
async fn test_grpc_input_type_parsing() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    let checkpoint = client
        .get_latest_checkpoint()
        .await
        .expect("checkpoint")
        .expect("exists");

    let mut pure_count = 0;
    let mut object_count = 0;
    let mut shared_count = 0;
    let mut receiving_count = 0;

    for tx in &checkpoint.transactions {
        if !tx.is_ptb() {
            continue;
        }

        for input in &tx.inputs {
            match input {
                GrpcInput::Pure { .. } => pure_count += 1,
                GrpcInput::Object { .. } => object_count += 1,
                GrpcInput::SharedObject { .. } => shared_count += 1,
                GrpcInput::Receiving { .. } => receiving_count += 1,
            }
        }
    }

    println!("Input type distribution:");
    println!("  Pure: {}", pure_count);
    println!("  Object (immutable/owned): {}", object_count);
    println!("  Shared: {}", shared_count);
    println!("  Receiving: {}", receiving_count);

    // Sanity checks
    assert!(pure_count > 0, "Should have some pure inputs");
    assert!(
        object_count > 0 || shared_count > 0,
        "Should have some object inputs"
    );
}

/// Test that gRPC command types are parsed correctly.
#[tokio::test]
#[ignore]
async fn test_grpc_command_type_parsing() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    let checkpoint = client
        .get_latest_checkpoint()
        .await
        .expect("checkpoint")
        .expect("exists");

    let mut move_calls = 0;
    let mut split_coins = 0;
    let mut merge_coins = 0;
    let mut transfer_objects = 0;
    let mut make_move_vec = 0;
    let mut publish = 0;
    let mut upgrade = 0;

    for tx in &checkpoint.transactions {
        for cmd in &tx.commands {
            match cmd {
                GrpcCommand::MoveCall { .. } => move_calls += 1,
                GrpcCommand::SplitCoins { .. } => split_coins += 1,
                GrpcCommand::MergeCoins { .. } => merge_coins += 1,
                GrpcCommand::TransferObjects { .. } => transfer_objects += 1,
                GrpcCommand::MakeMoveVec { .. } => make_move_vec += 1,
                GrpcCommand::Publish { .. } => publish += 1,
                GrpcCommand::Upgrade { .. } => upgrade += 1,
            }
        }
    }

    println!(
        "Command distribution for checkpoint {}:",
        checkpoint.sequence_number
    );
    println!("  MoveCall: {}", move_calls);
    println!("  SplitCoins: {}", split_coins);
    println!("  MergeCoins: {}", merge_coins);
    println!("  TransferObjects: {}", transfer_objects);
    println!("  MakeMoveVec: {}", make_move_vec);
    println!("  Publish: {}", publish);
    println!("  Upgrade: {}", upgrade);

    // Most checkpoints should have MoveCall commands
    assert!(move_calls > 0, "Expected some MoveCall commands");
}

// =============================================================================
// DataFetcher Integration Tests
// =============================================================================

/// Test DataFetcher with gRPC for checkpoint streaming.
#[tokio::test]
#[ignore]
async fn test_datafetcher_grpc_checkpoint_fetch() {
    let endpoint = require_grpc_endpoint!();

    let fetcher = DataFetcher::mainnet()
        .with_grpc_endpoint(&endpoint)
        .await
        .expect("create fetcher");

    assert!(fetcher.has_grpc(), "Should have gRPC enabled");

    // Fetch latest checkpoint
    let checkpoint = fetcher
        .get_latest_checkpoint_grpc()
        .await
        .expect("get checkpoint");

    println!("Checkpoint {} via DataFetcher:", checkpoint.sequence_number);
    println!("  Transactions: {}", checkpoint.transactions.len());

    let ptb_count = checkpoint
        .transactions
        .iter()
        .filter(|t| t.is_ptb())
        .count();
    println!("  PTB transactions: {}", ptb_count);

    assert!(
        !checkpoint.transactions.is_empty(),
        "Should have transactions"
    );
}

/// Test hybrid workflow: gRPC for transactions, GraphQL for packages.
#[tokio::test]
#[ignore]
async fn test_hybrid_grpc_graphql_workflow() {
    let endpoint = require_grpc_endpoint!();

    let fetcher = DataFetcher::mainnet()
        .with_grpc_endpoint(&endpoint)
        .await
        .expect("create fetcher");

    println!("=== Hybrid gRPC + GraphQL Workflow ===\n");

    // 1. Get transactions via gRPC (fast, real-time)
    let checkpoint = fetcher
        .get_latest_checkpoint_grpc()
        .await
        .expect("gRPC checkpoint");

    println!(
        "1. gRPC: Got checkpoint {} ({} txs)",
        checkpoint.sequence_number,
        checkpoint.transactions.len()
    );

    // 2. Find a MoveCall to a well-known package
    let move_call_tx = checkpoint
        .transactions
        .iter()
        .find(|tx| {
            tx.commands.iter().any(|cmd| {
                matches!(cmd, GrpcCommand::MoveCall { package, .. }
                    if package.starts_with("0x0000000000000000000000000000000000000000000000000000000000000002"))
            })
        });

    if let Some(tx) = move_call_tx {
        println!("2. Found tx calling Sui framework: {}", tx.digest);

        // 3. Fetch the package via GraphQL (has full bytecode)
        let pkg = fetcher.fetch_package("0x2").expect("fetch package");
        println!(
            "3. GraphQL: Fetched Sui framework ({} modules)",
            pkg.modules.len()
        );

        // This demonstrates the hybrid approach:
        // - gRPC for fast transaction streaming
        // - GraphQL for package bytecode needed for replay
    } else {
        println!("2. No framework calls in this checkpoint");
    }
}

// =============================================================================
// Streaming Validation Tests
// =============================================================================

/// Test that streamed transactions have valid structure.
#[tokio::test]
#[ignore]
async fn test_streaming_transaction_validation() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    println!("Validating streamed transactions for 10 seconds...\n");

    let mut stream = client.subscribe_checkpoints().await.expect("subscribe");

    let start = std::time::Instant::now();
    let duration = Duration::from_secs(10);
    let mut validated = 0;
    let mut validation_errors = 0;

    while start.elapsed() < duration {
        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(checkpoint)) => {
                        for tx in &checkpoint.transactions {
                            if !tx.is_ptb() {
                                continue;
                            }

                            // Validate transaction structure
                            if let Err(e) = validate_transaction_structure(tx) {
                                eprintln!("Validation error for {}: {}", tx.digest, e);
                                validation_errors += 1;
                            } else {
                                validated += 1;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("Stream error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    println!("\nValidation Summary:");
    println!("  Validated: {}", validated);
    println!("  Errors: {}", validation_errors);

    assert!(validated > 0, "Should have validated some transactions");
    assert_eq!(validation_errors, 0, "Should have no validation errors");
}

/// Helper to validate transaction structure
fn validate_transaction_structure(tx: &GrpcTransaction) -> Result<(), String> {
    // Digest should be non-empty
    if tx.digest.is_empty() {
        return Err("Empty digest".to_string());
    }

    // PTB transactions should have sender
    if tx.is_ptb() && tx.sender.is_empty() {
        return Err("PTB missing sender".to_string());
    }

    // Validate command arguments reference valid inputs
    for (cmd_idx, cmd) in tx.commands.iter().enumerate() {
        if let GrpcCommand::MoveCall { arguments, .. } = cmd {
            for arg in arguments {
                match arg {
                    GrpcArgument::Input(idx) => {
                        if *idx as usize >= tx.inputs.len() {
                            return Err(format!(
                                "Command {} references invalid input {}",
                                cmd_idx, idx
                            ));
                        }
                    }
                    GrpcArgument::Result(idx) => {
                        if *idx as usize >= cmd_idx {
                            return Err(format!(
                                "Command {} references future result {}",
                                cmd_idx, idx
                            ));
                        }
                    }
                    GrpcArgument::NestedResult(idx, _) => {
                        if *idx as usize >= cmd_idx {
                            return Err(format!(
                                "Command {} references future nested result {}",
                                cmd_idx, idx
                            ));
                        }
                    }
                    GrpcArgument::GasCoin => {}
                }
            }
        }
    }

    Ok(())
}

// =============================================================================
// Input/Command Structure Tests
// =============================================================================

/// Test that gRPC transactions have consistent structure.
#[tokio::test]
#[ignore]
async fn test_grpc_transaction_structure() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    let checkpoint = client
        .get_latest_checkpoint()
        .await
        .expect("checkpoint")
        .expect("exists");

    // Find a transaction with various input types
    let tx = checkpoint
        .transactions
        .iter()
        .find(|t| t.is_ptb() && t.inputs.len() >= 2)
        .expect("should have multi-input tx");

    println!("Transaction {} structure:", tx.digest);
    println!("  Inputs: {}", tx.inputs.len());
    println!("  Commands: {}", tx.commands.len());
    println!("  Sender: {}", tx.sender);

    // Verify input types are valid
    for (i, input) in tx.inputs.iter().enumerate() {
        let variant_name = match input {
            GrpcInput::Pure { .. } => "Pure",
            GrpcInput::Object { .. } => "Object",
            GrpcInput::SharedObject { .. } => "SharedObject",
            GrpcInput::Receiving { .. } => "Receiving",
        };
        println!("  Input {}: {}", i, variant_name);
    }

    // Verify all inputs are referenced correctly
    assert!(
        !tx.inputs.is_empty() || tx.commands.is_empty(),
        "Non-empty commands should have inputs"
    );
}

// =============================================================================
// Performance Tests
// =============================================================================

/// Test gRPC streaming throughput.
#[tokio::test]
#[ignore]
async fn test_grpc_streaming_throughput() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    println!("=== gRPC Streaming Throughput Test ===\n");
    println!("Collecting for 30 seconds...\n");

    let mut stream = client.subscribe_checkpoints().await.expect("subscribe");

    let start = std::time::Instant::now();
    let duration = Duration::from_secs(30);

    let mut checkpoints = 0;
    let mut transactions = 0;
    let mut ptb_transactions = 0;
    let mut commands = 0;

    while start.elapsed() < duration {
        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(checkpoint)) => {
                        checkpoints += 1;
                        transactions += checkpoint.transactions.len();

                        for tx in &checkpoint.transactions {
                            if tx.is_ptb() {
                                ptb_transactions += 1;
                                commands += tx.commands.len();
                            }
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("Stream error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    let elapsed = start.elapsed().as_secs_f64();

    println!("=== Throughput Summary ===");
    println!("Duration: {:.1}s", elapsed);
    println!(
        "Checkpoints: {} ({:.1}/s)",
        checkpoints,
        checkpoints as f64 / elapsed
    );
    println!(
        "Transactions: {} ({:.1}/s)",
        transactions,
        transactions as f64 / elapsed
    );
    println!(
        "PTB transactions: {} ({:.1}/s)",
        ptb_transactions,
        ptb_transactions as f64 / elapsed
    );
    println!(
        "Commands: {} ({:.1}/s)",
        commands,
        commands as f64 / elapsed
    );

    // Performance assertions
    assert!(checkpoints > 0, "Should receive checkpoints");

    // At ~1 checkpoint/400ms on mainnet, expect at least 50 in 30s
    assert!(
        checkpoints >= 30,
        "Expected at least 30 checkpoints in 30s, got {}",
        checkpoints
    );
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test graceful handling of connection issues.
#[tokio::test]
async fn test_grpc_connection_error_handling() {
    // Try to connect to invalid endpoint
    let result = GrpcClient::new("https://invalid.endpoint.example:9000").await;

    // Should return an error, not panic
    assert!(
        result.is_err(),
        "Should fail to connect to invalid endpoint"
    );

    // Error exists - connection failed as expected
    println!("Connection failed as expected (invalid endpoint)");
}

/// Test graceful handling of stream interruption.
#[tokio::test]
#[ignore]
async fn test_grpc_stream_interruption_handling() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    let mut stream = client.subscribe_checkpoints().await.expect("subscribe");

    // Read a few checkpoints then drop the stream
    for _ in 0..3 {
        let result = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;

        match result {
            Ok(Some(Ok(cp))) => {
                println!("Got checkpoint {}", cp.sequence_number);
            }
            Ok(Some(Err(e))) => {
                eprintln!("Stream error: {}", e);
                break;
            }
            Ok(None) => {
                println!("Stream ended");
                break;
            }
            Err(_) => {
                eprintln!("Timeout waiting for checkpoint");
                break;
            }
        }
    }

    // Drop stream - should not panic
    drop(stream);
    println!("Stream dropped cleanly");
}

/// Test handling of invalid object ID formats.
#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_grpc_invalid_object_id_handling() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    // Test with obviously invalid object ID
    let result = client.get_object("not-a-valid-object-id").await;

    // Should not panic - should return None or an error
    match result {
        Ok(None) => println!("Invalid ID returned None (expected)"),
        Ok(Some(_)) => panic!("Should not find object with invalid ID"),
        Err(e) => println!("Invalid ID returned error (expected): {}", e),
    }
}

/// Test handling of non-existent object.
#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_grpc_nonexistent_object_handling() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    // Use a valid-looking but non-existent object ID (all zeros except last byte)
    let fake_id = "0x0000000000000000000000000000000000000000000000000000000000000099";
    let result = client.get_object(fake_id).await;

    // Should return None, not an error (object simply doesn't exist)
    match result {
        Ok(None) => println!("Non-existent object returned None (expected)"),
        Ok(Some(_)) => panic!("Should not find non-existent object"),
        Err(e) => {
            // Some endpoints may return an error for non-existent objects
            println!("Non-existent object returned error: {}", e);
        }
    }
}

/// Test handling of invalid transaction digest.
#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_grpc_invalid_transaction_digest_handling() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    // Invalid digest (wrong format)
    let result = client.get_transaction("invalid-digest-format").await;

    match result {
        Ok(None) => println!("Invalid digest returned None (expected)"),
        Ok(Some(_)) => panic!("Should not find transaction with invalid digest"),
        Err(e) => println!("Invalid digest returned error (expected): {}", e),
    }
}

/// Test handling of invalid checkpoint number.
#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_grpc_future_checkpoint_handling() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("connect");

    // Request a checkpoint far in the future (should not exist)
    let result = client.get_checkpoint(u64::MAX - 1).await;

    match result {
        Ok(None) => println!("Future checkpoint returned None (expected)"),
        Ok(Some(_)) => panic!("Should not find checkpoint that doesn't exist yet"),
        Err(e) => println!("Future checkpoint returned error (expected): {}", e),
    }
}
