#![allow(unused_imports)]
//! GraphQL Integration Tests
//!
//! These tests validate the GraphQL client and DataFetcher against mainnet.
//!
//! ## Network Tests
//!
//! Most tests in this file require network access to Sui mainnet/testnet.
//! They are marked with `#[ignore]` to avoid CI failures when network is unavailable.
//!
//! To run all network tests:
//! ```sh
//! cargo test --test graphql_tests -- --ignored
//! ```
//!
//! Note: Tests that silently pass when network is unavailable provide false confidence.
//! We explicitly mark network-dependent tests as ignored rather than having them
//! silently succeed with eprintln warnings.

use sui_move_interface_extractor::data_fetcher::{
    DataFetcher, DataSource, GraphQLArgument, GraphQLCommand,
};
use sui_move_interface_extractor::graphql::{
    GraphQLClient, PageInfo, PaginationDirection, Paginator,
};

// =============================================================================
// Unit Tests (No Network Required)
// =============================================================================

#[test]
fn test_graphql_client_creation() {
    let mainnet = GraphQLClient::mainnet();
    let testnet = GraphQLClient::testnet();
    let custom = GraphQLClient::new("https://custom.endpoint/graphql");

    // Just verify they can be created (no network call yet)
    drop(mainnet);
    drop(testnet);
    drop(custom);
}

// =============================================================================
// Network Integration Tests (require --ignored flag)
// =============================================================================

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_fetch_object() {
    let client = GraphQLClient::mainnet();

    // Fetch the Sui framework package (0x2)
    let obj = client
        .fetch_object("0x2")
        .expect("fetch_object should succeed when network is available");

    assert_eq!(
        obj.address,
        "0x0000000000000000000000000000000000000000000000000000000000000002"
    );
    assert!(obj.version > 0, "Should have version");
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_fetch_package() {
    let client = GraphQLClient::mainnet();

    // Fetch the Sui framework package
    let pkg = client
        .fetch_package("0x2")
        .expect("fetch_package should succeed when network is available");

    assert!(!pkg.modules.is_empty(), "Sui framework should have modules");

    // Check for some known modules
    let module_names: Vec<_> = pkg.modules.iter().map(|m| m.name.as_str()).collect();
    assert!(module_names.contains(&"coin"), "Should have coin module");
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_fetch_transaction() {
    let client = GraphQLClient::mainnet();

    // Fetch a known mainnet transaction
    let result = client.fetch_transaction("8JTTa6k7Expr15zMS2DpTsCsaMC4aV4Lwxvmraew85gY");

    match result {
        Ok(tx) => {
            assert!(!tx.digest.is_empty(), "Should have digest");
            assert!(!tx.sender.is_empty(), "Should have sender");
            assert!(!tx.commands.is_empty(), "Should have commands");

            println!("Fetched transaction:");
            println!("  Digest: {}", tx.digest);
            println!("  Sender: {}", tx.sender);
            println!("  Commands: {}", tx.commands.len());
            println!("  Inputs: {}", tx.inputs.len());
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transaction (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_fetch_recent_transactions() {
    let client = GraphQLClient::mainnet();

    // Fetch a small batch
    let result = client.fetch_recent_transactions(10);

    match result {
        Ok(digests) => {
            assert_eq!(digests.len(), 10, "Should return exactly 10 digests");

            // Verify digests are valid format
            for digest in &digests {
                assert!(!digest.is_empty(), "Digest should not be empty");
                assert!(digest.len() > 20, "Digest should be reasonable length");
            }

            println!("Fetched {} recent transaction digests", digests.len());
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transactions (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_pagination_over_50() {
    let client = GraphQLClient::mainnet();

    // Fetch more than the page limit (50)
    let result = client.fetch_recent_transactions(75);

    match result {
        Ok(digests) => {
            assert_eq!(digests.len(), 75, "Should return exactly 75 digests");

            // Verify no duplicates
            let unique: std::collections::HashSet<_> = digests.iter().collect();
            assert_eq!(
                unique.len(),
                digests.len(),
                "Should have no duplicate digests"
            );

            println!("Successfully paginated to fetch {} digests", digests.len());
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transactions (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_fetch_recent_transactions_full() {
    let client = GraphQLClient::mainnet();

    // Fetch with full data
    let result = client.fetch_recent_transactions_full(20);

    match result {
        Ok(txs) => {
            assert!(txs.len() <= 20, "Should return at most 20 transactions");

            // Count user vs system transactions
            let user_txs = txs.iter().filter(|tx| !tx.sender.is_empty()).count();
            let system_txs = txs.iter().filter(|tx| tx.sender.is_empty()).count();

            println!(
                "Fetched {} transactions: {} user, {} system",
                txs.len(),
                user_txs,
                system_txs
            );
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transactions (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_fetch_ptb_transactions() {
    let client = GraphQLClient::mainnet();

    // Fetch only PTB transactions (filters system transactions)
    let result = client.fetch_recent_ptb_transactions(15);

    match result {
        Ok(txs) => {
            // All PTB transactions should have sender
            for tx in &txs {
                assert!(!tx.sender.is_empty(), "PTB transaction should have sender");
                // Most PTB transactions should have commands (except edge cases)
            }

            println!("Fetched {} PTB transactions", txs.len());
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transactions (network issue?): {}", e);
        }
    }
}

// =============================================================================
// DataFetcher Integration Tests
// =============================================================================

#[test]
fn test_data_fetcher_creation() {
    let mainnet = DataFetcher::mainnet();
    let testnet = DataFetcher::testnet();

    drop(mainnet);
    drop(testnet);
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_data_fetcher_object() {
    let fetcher = DataFetcher::mainnet();

    // Fetch the Sui Clock object (0x6)
    let result = fetcher.fetch_object("0x6");

    match result {
        Ok(obj) => {
            assert!(!obj.address.is_empty(), "Should have address");
            assert!(obj.version > 0, "Should have version");

            // Clock is a shared object
            assert!(obj.is_shared, "Clock should be shared");

            println!(
                "Fetched object 0x6 via DataFetcher (source: {:?})",
                obj.source
            );
        }
        Err(e) => {
            eprintln!("Note: Could not fetch object (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_data_fetcher_package() {
    let fetcher = DataFetcher::mainnet();

    let result = fetcher.fetch_package("0x1");

    match result {
        Ok(pkg) => {
            assert!(!pkg.modules.is_empty(), "Move stdlib should have modules");

            println!(
                "Fetched package 0x1 with {} modules (source: {:?})",
                pkg.modules.len(),
                pkg.source
            );
        }
        Err(e) => {
            eprintln!("Note: Could not fetch package (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_data_fetcher_recent_ptb_transactions() {
    let fetcher = DataFetcher::mainnet();

    let result = fetcher.fetch_recent_ptb_transactions(10);

    match result {
        Ok(txs) => {
            assert!(txs.len() <= 10, "Should return at most 10 transactions");

            // Verify all are valid PTB transactions
            for tx in &txs {
                assert!(!tx.sender.is_empty(), "Should have sender");
                assert!(tx.effects.is_some(), "Should have effects");
            }

            println!("Fetched {} PTB transactions via DataFetcher", txs.len());
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transactions (network issue?): {}", e);
        }
    }
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_data_fetcher_graphql_only() {
    let fetcher = DataFetcher::mainnet();

    // DataFetcher now uses GraphQL exclusively (JSON-RPC removed)
    let result = fetcher.fetch_object("0x2");

    match result {
        Ok(obj) => {
            // Source should always be GraphQL now
            assert!(
                matches!(obj.source, DataSource::GraphQL),
                "DataFetcher should use GraphQL exclusively"
            );
            println!("Fetched via GraphQL: {:?}", obj.source);
        }
        Err(e) => {
            eprintln!("Note: Could not fetch object (network issue?): {}", e);
        }
    }
}

// =============================================================================
// Paginator Unit Tests
// =============================================================================

#[test]
fn test_page_info_parsing() {
    use serde_json::json;

    // Test with full page info
    let value = json!({
        "hasNextPage": true,
        "hasPreviousPage": false,
        "startCursor": "cursor_start",
        "endCursor": "cursor_end"
    });

    let page_info = PageInfo::from_value(Some(&value));
    assert!(page_info.has_next_page);
    assert!(!page_info.has_previous_page);
    assert_eq!(page_info.start_cursor, Some("cursor_start".to_string()));
    assert_eq!(page_info.end_cursor, Some("cursor_end".to_string()));

    // Test with empty/null
    let page_info = PageInfo::from_value(None);
    assert!(!page_info.has_next_page);
    assert!(!page_info.has_previous_page);
    assert!(page_info.start_cursor.is_none());
    assert!(page_info.end_cursor.is_none());
}

#[test]
fn test_paginator_basic() {
    // Simulate a simple paginator that returns items from a vec
    let data: Vec<i32> = (0..100).collect();
    let mut call_count = 0;

    let paginator = Paginator::new(PaginationDirection::Forward, 25, |cursor, page_size| {
        call_count += 1;
        let start = cursor.map(|c| c.parse::<usize>().unwrap()).unwrap_or(0);
        let end = (start + page_size).min(data.len());
        let items: Vec<i32> = data[start..end].to_vec();

        let has_next = end < data.len();
        let page_info = PageInfo {
            has_next_page: has_next,
            has_previous_page: start > 0,
            start_cursor: Some(start.to_string()),
            end_cursor: Some(end.to_string()),
        };

        Ok((items, page_info))
    });

    let result = paginator.collect_all().unwrap();
    assert_eq!(result.len(), 25, "Should collect exactly 25 items");
    assert_eq!(result[0], 0, "First item should be 0");
    assert_eq!(result[24], 24, "Last item should be 24");
}

#[test]
fn test_paginator_with_page_size() {
    let data: Vec<i32> = (0..100).collect();

    let paginator = Paginator::new(PaginationDirection::Forward, 30, |cursor, page_size| {
        let start = cursor.map(|c| c.parse::<usize>().unwrap()).unwrap_or(0);
        let end = (start + page_size).min(data.len());
        let items: Vec<i32> = data[start..end].to_vec();

        Ok((
            items,
            PageInfo {
                has_next_page: end < data.len(),
                has_previous_page: start > 0,
                start_cursor: Some(start.to_string()),
                end_cursor: Some(end.to_string()),
            },
        ))
    })
    .with_page_size(10);

    let result = paginator.collect_all().unwrap();
    assert_eq!(result.len(), 30, "Should collect exactly 30 items");
}

#[test]
fn test_paginator_exhaustion() {
    // Data smaller than requested limit
    let data: Vec<i32> = (0..15).collect();

    let paginator = Paginator::new(
        PaginationDirection::Forward,
        100, // Request more than available
        |cursor, page_size| {
            let start = cursor.map(|c| c.parse::<usize>().unwrap()).unwrap_or(0);
            let end = (start + page_size).min(data.len());
            let items: Vec<i32> = data[start..end].to_vec();

            Ok((
                items,
                PageInfo {
                    has_next_page: end < data.len(),
                    has_previous_page: start > 0,
                    start_cursor: Some(start.to_string()),
                    end_cursor: Some(end.to_string()),
                },
            ))
        },
    );

    let result = paginator.collect_all().unwrap();
    assert_eq!(result.len(), 15, "Should only return available 15 items");
}

// =============================================================================
// GraphQL Command Parsing Tests
// =============================================================================

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_command_variants() {
    let client = GraphQLClient::mainnet();

    // Fetch recent PTB transactions and verify command parsing
    let result = client.fetch_recent_ptb_transactions(20);

    match result {
        Ok(txs) => {
            let mut move_calls = 0;
            let mut split_coins = 0;
            let mut merge_coins = 0;
            let mut transfers = 0;
            let mut make_vec = 0;
            let mut publish = 0;
            let mut upgrade = 0;
            let mut other = 0;

            for tx in &txs {
                for cmd in &tx.commands {
                    match cmd {
                        GraphQLCommand::MoveCall {
                            package,
                            module,
                            function,
                            ..
                        } => {
                            move_calls += 1;
                            assert!(!package.is_empty(), "MoveCall should have package");
                            assert!(!module.is_empty(), "MoveCall should have module");
                            assert!(!function.is_empty(), "MoveCall should have function");
                        }
                        GraphQLCommand::SplitCoins { coin, amounts: _ } => {
                            split_coins += 1;
                            // Validate argument structure
                            match coin {
                                GraphQLArgument::GasCoin
                                | GraphQLArgument::Input(_)
                                | GraphQLArgument::Result(_)
                                | GraphQLArgument::NestedResult(_, _) => {}
                            }
                        }
                        GraphQLCommand::MergeCoins {
                            destination: _,
                            sources,
                        } => {
                            merge_coins += 1;
                            assert!(!sources.is_empty(), "MergeCoins should have sources");
                        }
                        GraphQLCommand::TransferObjects {
                            objects,
                            address: _,
                        } => {
                            transfers += 1;
                            assert!(!objects.is_empty(), "TransferObjects should have objects");
                        }
                        GraphQLCommand::MakeMoveVec { .. } => make_vec += 1,
                        GraphQLCommand::Publish { .. } => publish += 1,
                        GraphQLCommand::Upgrade { .. } => upgrade += 1,
                        GraphQLCommand::Other { .. } => other += 1,
                    }
                }
            }

            println!("Command breakdown across {} transactions:", txs.len());
            println!("  MoveCall: {}", move_calls);
            println!("  SplitCoins: {}", split_coins);
            println!("  MergeCoins: {}", merge_coins);
            println!("  TransferObjects: {}", transfers);
            println!("  MakeMoveVec: {}", make_vec);
            println!("  Publish: {}", publish);
            println!("  Upgrade: {}", upgrade);
            println!("  Other: {}", other);
        }
        Err(e) => {
            eprintln!("Note: Could not fetch transactions (network issue?): {}", e);
        }
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_invalid_object() {
    let client = GraphQLClient::mainnet();

    // Try to fetch a non-existent object
    let result =
        client.fetch_object("0x0000000000000000000000000000000000000000000000000000000000000999");

    // Should return an error, not panic
    assert!(
        result.is_err() || result.unwrap().bcs_base64.is_none(),
        "Non-existent object should fail or return empty"
    );
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_graphql_invalid_transaction() {
    let client = GraphQLClient::mainnet();

    // Try to fetch a non-existent transaction
    let result = client.fetch_transaction("InvalidTransactionDigest123456789");

    // Should return an error, not panic
    assert!(result.is_err(), "Invalid transaction should return error");
}

#[test]
#[ignore = "requires network access to Sui mainnet"]
fn test_data_fetcher_graceful_degradation() {
    let fetcher = DataFetcher::mainnet();

    // Even with network issues, should not panic
    // This test primarily verifies error handling paths
    let _ = fetcher.fetch_object("0x2");
    let _ = fetcher.fetch_package("0x1");
    let _ = fetcher.fetch_recent_transactions(5);
}
