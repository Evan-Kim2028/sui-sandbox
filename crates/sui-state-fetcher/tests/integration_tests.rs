//! Integration tests for sui-state-fetcher.
//!
//! These tests require network access and are marked with #[ignore].
//! Run with: cargo test -p sui-state-fetcher --test integration_tests -- --ignored

use sui_state_fetcher::{HistoricalStateProvider, ReplayState};

/// A known successful transaction on mainnet (Cetus swap)
const KNOWN_SUCCESS_TX: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";

/// A known failed transaction on mainnet (DeepBook flash loan arb)
const KNOWN_FAILURE_TX: &str = "D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva";

#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_fetch_replay_state_success_tx() {
    let provider = HistoricalStateProvider::mainnet()
        .await
        .expect("Failed to create provider");

    let state: ReplayState = provider
        .fetch_replay_state(KNOWN_SUCCESS_TX)
        .await
        .expect("Failed to fetch replay state");

    // Verify transaction data
    assert!(
        !state.transaction.commands.is_empty(),
        "Transaction should have commands"
    );
    assert!(
        !state.transaction.inputs.is_empty(),
        "Transaction should have inputs"
    );

    // Verify objects were fetched
    assert!(
        !state.objects.is_empty(),
        "Should have fetched objects for replay"
    );

    // Verify packages were fetched
    assert!(
        !state.packages.is_empty(),
        "Should have fetched packages for replay"
    );

    // Verify the checkpoint was captured
    assert!(state.checkpoint.is_some(), "Should have checkpoint number");

    println!(
        "Transaction: {} commands, {} inputs",
        state.transaction.commands.len(),
        state.transaction.inputs.len()
    );
    println!("Objects: {}", state.objects.len());
    println!("Packages: {}", state.packages.len());
    println!("Checkpoint: {:?}", state.checkpoint);
}

#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_fetch_replay_state_failure_tx() {
    let provider = HistoricalStateProvider::mainnet()
        .await
        .expect("Failed to create provider");

    let state: ReplayState = provider
        .fetch_replay_state(KNOWN_FAILURE_TX)
        .await
        .expect("Failed to fetch replay state");

    // Even failed transactions should have full state
    assert!(
        !state.transaction.commands.is_empty(),
        "Transaction should have commands"
    );
    assert!(!state.objects.is_empty(), "Should have objects");
    assert!(!state.packages.is_empty(), "Should have packages");

    println!(
        "Failed TX fetched: {} objects, {} packages",
        state.objects.len(),
        state.packages.len()
    );
}

#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_objects_have_correct_versions() {
    let provider = HistoricalStateProvider::mainnet()
        .await
        .expect("Failed to create provider");

    let state = provider
        .fetch_replay_state(KNOWN_SUCCESS_TX)
        .await
        .expect("Failed to fetch replay state");

    // All objects should have non-zero versions (version 0 doesn't exist in Sui)
    for (id, obj) in &state.objects {
        assert!(
            obj.version > 0,
            "Object {} should have version > 0, got {}",
            hex::encode(id.as_ref()),
            obj.version
        );

        // Objects should have BCS bytes
        assert!(
            !obj.bcs_bytes.is_empty(),
            "Object {} should have BCS bytes",
            hex::encode(id.as_ref())
        );
    }
}

#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_packages_have_modules() {
    let provider = HistoricalStateProvider::mainnet()
        .await
        .expect("Failed to create provider");

    let state = provider
        .fetch_replay_state(KNOWN_SUCCESS_TX)
        .await
        .expect("Failed to fetch replay state");

    // All packages should have at least one module
    for (addr, pkg) in &state.packages {
        assert!(
            !pkg.modules.is_empty(),
            "Package {} should have modules",
            hex::encode(addr.as_ref())
        );

        // Each module should have bytecode
        for (name, bytecode) in &pkg.modules {
            assert!(
                !bytecode.is_empty(),
                "Module {} in package {} should have bytecode",
                name,
                hex::encode(addr.as_ref())
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_cache_works() {
    let provider = HistoricalStateProvider::mainnet()
        .await
        .expect("Failed to create provider");

    // First fetch - should hit network and populate cache
    let state1 = provider
        .fetch_replay_state(KNOWN_SUCCESS_TX)
        .await
        .expect("Failed to fetch replay state");

    let cache = provider.cache();
    let objects_after_first = cache.object_count();
    let packages_after_first = cache.package_count();

    assert!(
        objects_after_first > 0,
        "Cache should have objects after first fetch"
    );
    assert!(
        packages_after_first > 0,
        "Cache should have packages after first fetch"
    );

    // Second fetch - should use cache for objects already fetched
    let state2 = provider
        .fetch_replay_state(KNOWN_SUCCESS_TX)
        .await
        .expect("Failed to fetch replay state second time");

    // Results should be the same
    assert_eq!(
        state1.objects.len(),
        state2.objects.len(),
        "Object counts should match"
    );
    assert_eq!(
        state1.packages.len(),
        state2.packages.len(),
        "Package counts should match"
    );

    // Cache should not have grown much (same transaction)
    let objects_after_second = cache.object_count();
    let packages_after_second = cache.package_count();

    // Allow for minor growth due to dynamic field discovery, but should be similar
    assert!(
        objects_after_second <= objects_after_first * 2,
        "Cache should not grow dramatically on second fetch"
    );

    println!(
        "Cache after first: {} objects, {} packages",
        objects_after_first, packages_after_first
    );
    println!(
        "Cache after second: {} objects, {} packages",
        objects_after_second, packages_after_second
    );
}

#[tokio::test]
#[ignore = "requires network access to Sui mainnet"]
async fn test_replay_data_conversion() {
    use sui_state_fetcher::{get_historical_versions, to_replay_data};

    let provider = HistoricalStateProvider::mainnet()
        .await
        .expect("Failed to create provider");

    let state = provider
        .fetch_replay_state(KNOWN_SUCCESS_TX)
        .await
        .expect("Failed to fetch replay state");

    // Test conversion to replay data format
    let replay_data = to_replay_data(&state);

    assert_eq!(
        replay_data.packages.len(),
        state.packages.len(),
        "Package count should match"
    );
    assert_eq!(
        replay_data.objects.len(),
        state.objects.len(),
        "Object count should match"
    );

    // Test historical versions extraction
    let versions = get_historical_versions(&state);
    assert_eq!(
        versions.len(),
        state.objects.len(),
        "Version count should match object count"
    );

    // All objects should appear in versions map
    for id in state.objects.keys() {
        let id_str = format!("0x{}", hex::encode(id.as_ref()));
        assert!(
            versions.contains_key(&id_str),
            "Object {} should be in versions map",
            id_str
        );
    }
}
