//! gRPC Integration Tests
//!
//! These tests validate the gRPC client against a Sui gRPC endpoint.
//!
//! ## Requirements
//!
//! IMPORTANT: Sui's public fullnodes don't expose gRPC. To run these tests,
//! you need a gRPC endpoint from a provider (QuickNode, Dwellir, etc.) or
//! a self-hosted fullnode with gRPC enabled.
//!
//! ## Running Tests
//!
//! Set the environment variable before running:
//! ```sh
//! export SUI_GRPC_ENDPOINT="https://your-endpoint.sui-mainnet.quiknode.pro:9000"
//! cargo test --test grpc_tests -- --ignored --nocapture
//! ```
//!
//! ## Test Organization
//!
//! - Unit tests (no network): Run without `--ignored`
//! - Integration tests (network required): Run with `--ignored` and SUI_GRPC_ENDPOINT set
//!
//! Note: These tests use `panic!()` for failures because they're explicitly
//! testing network connectivity when the environment is properly configured.

use std::time::Duration;
use sui_move_interface_extractor::data_fetcher::DataFetcher;
use sui_move_interface_extractor::grpc::{GrpcClient, GrpcCommand};

/// Helper to get gRPC endpoint from env, or skip test
fn get_grpc_endpoint() -> Option<String> {
    std::env::var("SUI_GRPC_ENDPOINT").ok()
}

/// Helper macro to skip test if no endpoint configured
macro_rules! require_grpc_endpoint {
    () => {
        match get_grpc_endpoint() {
            Some(endpoint) => endpoint,
            None => {
                eprintln!("SKIPPED: SUI_GRPC_ENDPOINT not set");
                eprintln!("To run gRPC tests, set the environment variable:");
                eprintln!("  export SUI_GRPC_ENDPOINT=\"https://your-endpoint:9000\"");
                return;
            }
        }
    };
}

// =============================================================================
// GrpcClient Unit Tests (no network required)
// =============================================================================

#[test]
fn test_grpc_client_creation_doesnt_panic() {
    // These should not panic even without network
    // (connection happens lazily or on first request)
    let _mainnet = GrpcClient::mainnet();
    let _testnet = GrpcClient::testnet();
}

// =============================================================================
// GrpcClient Integration Tests (require SUI_GRPC_ENDPOINT)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_grpc_connection() {
    let endpoint = require_grpc_endpoint!();

    println!("Connecting to gRPC endpoint: {}", endpoint);

    match GrpcClient::new(&endpoint).await {
        Ok(client) => {
            println!("SUCCESS: Connected to gRPC endpoint");

            // Try to get service info
            match client.get_service_info().await {
                Ok(info) => {
                    println!("Service Info:");
                    println!("  Chain: {}", info.chain);
                    println!("  Epoch: {}", info.epoch);
                    println!("  Checkpoint: {}", info.checkpoint_height);
                    println!("  Lowest checkpoint: {}", info.lowest_available_checkpoint);

                    assert!(!info.chain.is_empty(), "Chain should not be empty");
                    assert!(info.checkpoint_height > 0, "Should have checkpoint height");
                }
                Err(e) => {
                    panic!("Failed to get service info: {}", e);
                }
            }
        }
        Err(e) => {
            panic!("Failed to connect: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_grpc_get_latest_checkpoint() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("Failed to connect");

    println!("Fetching latest checkpoint...");

    match client.get_latest_checkpoint().await {
        Ok(Some(checkpoint)) => {
            println!("Latest Checkpoint:");
            println!("  Sequence: {}", checkpoint.sequence_number);
            println!("  Digest: {}", checkpoint.digest);
            println!("  Timestamp: {:?}", checkpoint.timestamp_ms);
            println!("  Transactions: {}", checkpoint.transactions.len());

            assert!(checkpoint.sequence_number > 0);
            assert!(!checkpoint.digest.is_empty());

            // Print first few transactions
            for (i, tx) in checkpoint.transactions.iter().take(3).enumerate() {
                println!("  TX {}: {} ({} commands)", i, tx.digest, tx.commands.len());
            }
        }
        Ok(None) => {
            panic!("No checkpoint returned");
        }
        Err(e) => {
            panic!("Failed to get checkpoint: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_grpc_get_specific_checkpoint() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("Failed to connect");

    // Get latest first to know a valid checkpoint number
    let latest = client
        .get_latest_checkpoint()
        .await
        .expect("Failed to get latest")
        .expect("No latest checkpoint");

    // Fetch a recent checkpoint (latest - 10 to ensure it exists)
    let target = latest.sequence_number.saturating_sub(10);
    println!("Fetching checkpoint {}...", target);

    match client.get_checkpoint(target).await {
        Ok(Some(checkpoint)) => {
            assert_eq!(checkpoint.sequence_number, target);
            println!(
                "Checkpoint {}: {} transactions",
                checkpoint.sequence_number,
                checkpoint.transactions.len()
            );
        }
        Ok(None) => {
            println!("Checkpoint {} not found (may be pruned)", target);
        }
        Err(e) => {
            panic!("Failed to get checkpoint: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_grpc_checkpoint_stream() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("Failed to connect");

    println!("Starting checkpoint subscription...");
    println!("Will collect checkpoints for 10 seconds\n");

    let mut stream = client
        .subscribe_checkpoints()
        .await
        .expect("Failed to subscribe");

    let start = std::time::Instant::now();
    let duration = Duration::from_secs(10);
    let mut count = 0;
    let mut total_txs = 0;

    while start.elapsed() < duration {
        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(checkpoint)) => {
                        count += 1;
                        total_txs += checkpoint.transactions.len();
                        println!(
                            "[{:>3}] Checkpoint {}: {} txs",
                            count,
                            checkpoint.sequence_number,
                            checkpoint.transactions.len()
                        );
                    }
                    Some(Err(e)) => {
                        eprintln!("Stream error: {}", e);
                        break;
                    }
                    None => {
                        println!("Stream ended");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // Continue waiting
            }
        }
    }

    println!("\nStream Summary:");
    println!("  Duration: {:?}", start.elapsed());
    println!("  Checkpoints received: {}", count);
    println!("  Total transactions: {}", total_txs);
    println!(
        "  Avg txs/checkpoint: {:.1}",
        if count > 0 {
            total_txs as f64 / count as f64
        } else {
            0.0
        }
    );

    assert!(count > 0, "Should have received at least one checkpoint");
}

#[tokio::test]
#[ignore]
async fn test_grpc_transaction_parsing() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("Failed to connect");

    // Get a checkpoint with transactions
    let checkpoint = client
        .get_latest_checkpoint()
        .await
        .expect("Failed to get checkpoint")
        .expect("No checkpoint");

    println!(
        "Analyzing {} transactions from checkpoint {}",
        checkpoint.transactions.len(),
        checkpoint.sequence_number
    );

    let mut ptb_count = 0;
    let mut system_count = 0;
    let mut command_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for tx in &checkpoint.transactions {
        if tx.is_ptb() {
            ptb_count += 1;

            for cmd in &tx.commands {
                let cmd_type = match cmd {
                    GrpcCommand::MoveCall { .. } => "MoveCall",
                    GrpcCommand::SplitCoins { .. } => "SplitCoins",
                    GrpcCommand::MergeCoins { .. } => "MergeCoins",
                    GrpcCommand::TransferObjects { .. } => "TransferObjects",
                    GrpcCommand::MakeMoveVec { .. } => "MakeMoveVec",
                    GrpcCommand::Publish { .. } => "Publish",
                    GrpcCommand::Upgrade { .. } => "Upgrade",
                };
                *command_counts.entry(cmd_type.to_string()).or_insert(0) += 1;
            }
        } else {
            system_count += 1;
        }
    }

    println!("\nTransaction Types:");
    println!("  PTB transactions: {}", ptb_count);
    println!("  System transactions: {}", system_count);

    println!("\nCommand Distribution:");
    let mut sorted: Vec<_> = command_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (cmd, count) in sorted {
        println!("  {}: {}", cmd, count);
    }
}

#[tokio::test]
#[ignore]
async fn test_grpc_move_call_details() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("Failed to connect");

    let checkpoint = client
        .get_latest_checkpoint()
        .await
        .expect("Failed to get checkpoint")
        .expect("No checkpoint");

    println!("Looking for MoveCall commands...\n");

    let mut found = 0;
    for tx in &checkpoint.transactions {
        if !tx.is_ptb() {
            continue;
        }

        for cmd in &tx.commands {
            if let GrpcCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } = cmd
            {
                found += 1;
                println!("MoveCall #{}", found);
                println!("  Function: {}::{}::{}", package, module, function);
                if !type_arguments.is_empty() {
                    println!("  Type args: {:?}", type_arguments);
                }
                println!("  Arguments: {:?}", arguments);
                println!();

                if found >= 5 {
                    break;
                }
            }
        }
        if found >= 5 {
            break;
        }
    }

    println!("Found {} MoveCall commands (showing first 5)", found);
}

// =============================================================================
// DataFetcher gRPC Integration Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_datafetcher_with_grpc() {
    let endpoint = require_grpc_endpoint!();

    println!("Creating DataFetcher with gRPC endpoint...");

    let fetcher = DataFetcher::mainnet()
        .with_grpc_endpoint(&endpoint)
        .await
        .expect("Failed to create fetcher with gRPC");

    assert!(fetcher.has_grpc(), "Should have gRPC enabled");

    // Test service info
    let info = fetcher
        .get_service_info()
        .await
        .expect("Failed to get service info");
    println!(
        "Connected to {} at checkpoint {}",
        info.chain, info.checkpoint_height
    );

    // Test checkpoint fetch
    let checkpoint = fetcher
        .get_latest_checkpoint_grpc()
        .await
        .expect("Failed to get checkpoint");
    println!(
        "Latest checkpoint {}: {} transactions",
        checkpoint.sequence_number,
        checkpoint.transactions.len()
    );

    // GraphQL should still work
    let pkg = fetcher
        .fetch_package("0x2")
        .expect("Failed to fetch package via GraphQL");
    println!(
        "Also fetched package 0x2 via GraphQL: {} modules",
        pkg.modules.len()
    );
}

#[tokio::test]
#[ignore]
async fn test_datafetcher_grpc_stream() {
    let endpoint = require_grpc_endpoint!();

    let fetcher = DataFetcher::mainnet()
        .with_grpc_endpoint(&endpoint)
        .await
        .expect("Failed to create fetcher");

    println!("Streaming checkpoints for 5 seconds...\n");

    let mut stream = fetcher
        .subscribe_checkpoints()
        .await
        .expect("Failed to subscribe");

    let start = std::time::Instant::now();
    let duration = Duration::from_secs(5);
    let mut count = 0;

    while start.elapsed() < duration {
        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(cp)) => {
                        count += 1;
                        let ptb_count = cp.transactions.iter().filter(|t| t.is_ptb()).count();
                        println!(
                            "Checkpoint {}: {} total, {} PTB",
                            cp.sequence_number,
                            cp.transactions.len(),
                            ptb_count
                        );
                    }
                    Some(Err(e)) => {
                        eprintln!("Error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    println!("\nReceived {} checkpoints", count);
    assert!(count > 0);
}

// =============================================================================
// gRPC + GraphQL Hybrid Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_hybrid_grpc_graphql_workflow() {
    let endpoint = require_grpc_endpoint!();

    let fetcher = DataFetcher::mainnet()
        .with_grpc_endpoint(&endpoint)
        .await
        .expect("Failed to create fetcher");

    println!("=== Hybrid gRPC + GraphQL Workflow ===\n");

    // 1. Get latest checkpoint via gRPC (faster)
    let checkpoint = fetcher
        .get_latest_checkpoint_grpc()
        .await
        .expect("gRPC checkpoint fetch failed");
    println!(
        "1. Got checkpoint {} via gRPC ({} txs)",
        checkpoint.sequence_number,
        checkpoint.transactions.len()
    );

    // 2. Find a PTB transaction
    let ptb_tx = checkpoint.transactions.iter().find(|t| t.is_ptb());

    if let Some(tx) = ptb_tx {
        println!("2. Found PTB transaction: {}", tx.digest);

        // 3. If we need more details, we could fetch via GraphQL
        // (In practice, gRPC already has full data, but this shows the hybrid approach)
        match fetcher.fetch_transaction(&tx.digest) {
            Ok(graphql_tx) => {
                println!(
                    "3. Verified via GraphQL: {} commands",
                    graphql_tx.commands.len()
                );

                // Compare command counts
                assert_eq!(
                    tx.commands.len(),
                    graphql_tx.commands.len(),
                    "Command count should match between gRPC and GraphQL"
                );
                println!("   Command count matches between gRPC and GraphQL");
            }
            Err(e) => {
                println!(
                    "3. Note: GraphQL fetch failed ({}), but gRPC data is complete",
                    e
                );
            }
        }
    } else {
        println!("2. No PTB transactions in this checkpoint (all system txs)");
    }

    // 4. Fetch a package via GraphQL (gRPC doesn't have full bytecode)
    let pkg = fetcher
        .fetch_package("0x2")
        .expect("GraphQL package fetch failed");
    println!(
        "4. Fetched Sui framework via GraphQL: {} modules",
        pkg.modules.len()
    );

    println!("\n=== Hybrid workflow complete ===");
}

// =============================================================================
// Performance/Load Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_grpc_one_minute_collection() {
    let endpoint = require_grpc_endpoint!();
    let client = GrpcClient::new(&endpoint).await.expect("Failed to connect");

    println!("=== 1-Minute gRPC Collection Test ===\n");
    println!("Collecting all checkpoints for 60 seconds...\n");

    let mut stream = client
        .subscribe_checkpoints()
        .await
        .expect("Failed to subscribe");

    let start = std::time::Instant::now();
    let duration = Duration::from_secs(60);

    let mut checkpoint_count = 0;
    let mut total_txs = 0;
    let mut ptb_txs = 0;
    let mut first_checkpoint: Option<u64> = None;
    let mut last_checkpoint: Option<u64> = None;

    while start.elapsed() < duration {
        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(checkpoint)) => {
                        checkpoint_count += 1;
                        total_txs += checkpoint.transactions.len();
                        ptb_txs += checkpoint.transactions.iter().filter(|t| t.is_ptb()).count();

                        if first_checkpoint.is_none() {
                            first_checkpoint = Some(checkpoint.sequence_number);
                        }
                        last_checkpoint = Some(checkpoint.sequence_number);

                        // Progress every 10 checkpoints
                        if checkpoint_count % 10 == 0 {
                            println!(
                                "[{:>3}s] {} checkpoints, {} txs ({} PTB)",
                                start.elapsed().as_secs(),
                                checkpoint_count,
                                total_txs,
                                ptb_txs
                            );
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("Stream error: {}", e);
                        break;
                    }
                    None => {
                        println!("Stream ended unexpectedly");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    let elapsed = start.elapsed();

    println!("\n=== Collection Summary ===");
    println!("Duration: {:.1}s", elapsed.as_secs_f64());
    println!("Checkpoints: {}", checkpoint_count);
    println!(
        "Checkpoint range: {} - {}",
        first_checkpoint.unwrap_or(0),
        last_checkpoint.unwrap_or(0)
    );
    println!("Total transactions: {}", total_txs);
    println!("PTB transactions: {}", ptb_txs);
    println!("System transactions: {}", total_txs.saturating_sub(ptb_txs));
    println!(
        "Avg checkpoints/sec: {:.2}",
        checkpoint_count as f64 / elapsed.as_secs_f64()
    );
    println!(
        "Avg transactions/sec: {:.2}",
        total_txs as f64 / elapsed.as_secs_f64()
    );

    // Sanity checks
    assert!(checkpoint_count > 0, "Should have received checkpoints");
    assert!(total_txs > 0, "Should have received transactions");
}
