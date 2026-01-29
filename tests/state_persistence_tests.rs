//! State Persistence Integration Tests
//!
//! Tests for the "save game" functionality - persisting and restoring simulation state.
//!
//! Test categories:
//! - Basic save/load round-trip
//! - Dynamic fields (Table/Bag) persistence
//! - SimulationConfig persistence
//! - Multi-sender workflows
//! - Version compatibility
//! - Metadata handling
//!
//! Run with:
//!   cargo test --test state_persistence_tests -- --nocapture

use move_core_types::account_address::AccountAddress;
use std::path::PathBuf;
use sui_sandbox_core::session::SimulationSession;
use sui_sandbox_core::simulation::{PersistentState, SimulationEnvironment};
use tempfile::TempDir;

// =============================================================================
// Helper Functions
// =============================================================================

fn create_temp_state_file() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("create temp dir");
    let path = dir.path().join("test-state.json");
    (dir, path)
}

// =============================================================================
// Basic Save/Load Tests
// =============================================================================

#[test]
fn test_save_state_creates_file() {
    let mut env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Create some state
    let _coin = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("create coin");

    // Save state
    let result = env.save_state(&path);
    assert!(result.is_ok(), "Should save state: {:?}", result.err());
    assert!(path.exists(), "State file should exist");

    // Verify it's valid JSON
    let content = std::fs::read_to_string(&path).expect("read file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse JSON");
    assert!(parsed.get("version").is_some(), "Should have version field");
}

#[test]
fn test_save_load_roundtrip_objects() {
    let mut env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Create some coins
    let coin1 = env
        .create_coin("0x2::sui::SUI", 100_000_000)
        .expect("coin 1");
    let coin2 = env
        .create_coin("0x2::sui::SUI", 200_000_000)
        .expect("coin 2");

    // Save state
    env.save_state(&path).expect("save state");

    // Create fresh environment and load
    let mut env2 = SimulationEnvironment::new().expect("create env2");
    env2.load_state(&path).expect("load state");

    // Verify objects exist
    assert!(
        env2.get_object(&coin1).is_some(),
        "Coin 1 should exist after load"
    );
    assert!(
        env2.get_object(&coin2).is_some(),
        "Coin 2 should exist after load"
    );

    // Verify object data matches
    let obj1_original = env.get_object(&coin1).unwrap();
    let obj1_loaded = env2.get_object(&coin1).unwrap();
    assert_eq!(
        obj1_original.bcs_bytes, obj1_loaded.bcs_bytes,
        "Object data should match"
    );
}

#[test]
fn test_save_load_roundtrip_coin_registry() {
    let mut env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Register a custom coin
    env.register_coin("0xabc::my_coin::MYCOIN", 6, "MYCOIN", "My Custom Coin");

    // Save state
    env.save_state(&path).expect("save state");

    // Load into fresh env
    let mut env2 = SimulationEnvironment::new().expect("create env2");
    env2.load_state(&path).expect("load state");

    // Verify coin metadata
    let metadata = env2.get_coin_metadata("0xabc::my_coin::MYCOIN");
    assert!(metadata.is_some(), "Custom coin should be registered");
    let meta = metadata.unwrap();
    assert_eq!(meta.decimals, 6);
    assert_eq!(meta.symbol, "MYCOIN");
}

#[test]
fn test_from_state_file_factory() {
    let mut env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Create state and save
    let coin = env.create_coin("0x2::sui::SUI", 500_000_000).expect("coin");
    env.save_state(&path).expect("save");

    // Use factory method
    let env2 = SimulationEnvironment::from_state_file(&path).expect("from_state_file");
    assert!(env2.get_object(&coin).is_some(), "Object should exist");
}

// =============================================================================
// SimulationConfig Persistence Tests
// =============================================================================

#[test]
fn test_config_persistence_epoch() {
    let env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Verify config is included in export
    let state = env.export_state();
    assert!(state.config.is_some(), "Config should be exported");

    // Save and reload
    env.save_state(&path).expect("save");
    let mut env2 = SimulationEnvironment::new().expect("env2");
    env2.load_state(&path).expect("load");

    // Config should be restored (verify via export)
    let state2 = env2.export_state();
    assert!(state2.config.is_some(), "Config should be loaded");
}

// =============================================================================
// Multi-Sender Tests
// =============================================================================

#[test]
fn test_set_sender() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Default sender should be zero
    let initial = env.sender();

    // Set new sender
    let new_sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )
    .expect("parse address");

    env.set_sender(new_sender);
    assert_eq!(env.sender(), new_sender, "Sender should be updated");
    assert_ne!(env.sender(), initial, "Sender should differ from initial");
}

#[test]
fn test_sender_persistence() {
    let mut env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Set custom sender
    let custom_sender = AccountAddress::from_hex_literal(
        "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    )
    .expect("parse");
    env.set_sender(custom_sender);

    // Save and reload
    env.save_state(&path).expect("save");
    let mut env2 = SimulationEnvironment::new().expect("env2");
    env2.load_state(&path).expect("load");

    // Sender should be restored
    assert_eq!(env2.sender(), custom_sender, "Sender should be persisted");
}

// =============================================================================
// Metadata Tests
// =============================================================================

#[test]
fn test_metadata_in_export() {
    let env = SimulationEnvironment::new().expect("create env");

    let state = env.export_state();
    assert!(state.metadata.is_some(), "Metadata should be present");

    let metadata = state.metadata.unwrap();
    assert!(
        metadata.created_at.is_some(),
        "Should have created_at timestamp"
    );
    assert!(
        metadata.modified_at.is_some(),
        "Should have modified_at timestamp"
    );
}

#[test]
fn test_save_with_metadata() {
    let env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Save with custom metadata
    env.save_state_with_metadata(
        &path,
        Some("Test simulation for Cetus swap".to_string()),
        vec!["defi".to_string(), "cetus".to_string(), "test".to_string()],
    )
    .expect("save with metadata");

    // Read and verify
    let content = std::fs::read_to_string(&path).expect("read");
    let state: PersistentState = serde_json::from_str(&content).expect("parse");

    assert!(state.metadata.is_some());
    let meta = state.metadata.unwrap();
    assert_eq!(
        meta.description,
        Some("Test simulation for Cetus swap".to_string())
    );
    assert_eq!(meta.tags, vec!["defi", "cetus", "test"]);
}

// =============================================================================
// Fetcher Config Persistence Tests (v4+)
// =============================================================================

#[test]
fn test_fetcher_config_persisted_when_enabled() {
    // Create env with mainnet fetching
    let session = SimulationSession::new()
        .expect("create session")
        .with_mainnet_fetching();

    // Export state
    let state = session.export_state();

    // Verify fetcher config is persisted
    assert!(
        state.fetcher_config.is_some(),
        "fetcher_config should be persisted"
    );
    let fc = state.fetcher_config.unwrap();
    assert!(fc.enabled, "fetcher should be enabled");
    assert_eq!(fc.network, Some("mainnet".to_string()));
    assert!(!fc.use_archive);
}

#[test]
fn test_fetcher_config_not_persisted_when_disabled() {
    // Create env without fetching (default)
    let session = SimulationSession::new().expect("create session");

    // Export state
    let state = session.export_state();

    // Verify fetcher config is not persisted when disabled
    assert!(
        state.fetcher_config.is_none(),
        "fetcher_config should be None when disabled"
    );
}

#[test]
fn test_fetcher_config_archive_mode_persisted() {
    // Create env with archive fetching
    let session = SimulationSession::new()
        .expect("create session")
        .with_mainnet_archive_fetching();

    // Export state
    let state = session.export_state();

    // Verify archive mode is captured
    assert!(state.fetcher_config.is_some());
    let fc = state.fetcher_config.unwrap();
    assert!(fc.enabled);
    assert!(fc.use_archive, "archive mode should be persisted");
    assert_eq!(fc.network, Some("mainnet".to_string()));
}

#[test]
fn test_fetcher_config_round_trip_mainnet() {
    let (_dir, path) = create_temp_state_file();

    // Create and save env with mainnet fetching
    {
        let session = SimulationSession::new()
            .expect("create session")
            .with_mainnet_fetching();
        session.save_state(&path).expect("save");
    }

    // Load into a fresh env and verify fetcher is reconnected
    let mut session2 = SimulationSession::new().expect("create session");
    session2.load_state(&path).expect("load");

    assert!(
        session2.is_fetching_enabled(),
        "fetcher should be auto-reconnected"
    );
    let fc = session2.fetcher_config();
    assert!(fc.enabled);
    assert_eq!(fc.network, Some("mainnet".to_string()));
}

#[test]
fn test_fetcher_config_round_trip_with_custom_config() {
    use sui_sandbox_core::simulation::FetcherConfig;

    let (_dir, path) = create_temp_state_file();

    // Create and save env with custom fetcher config
    {
        let config = FetcherConfig {
            enabled: true,
            network: Some("testnet".to_string()),
            endpoint: None,
            use_archive: false,
        };
        let session = SimulationSession::new()
            .expect("create session")
            .with_fetcher_config(config);
        session.save_state(&path).expect("save");
    }

    // Load and verify
    let mut session2 = SimulationSession::new().expect("create session");
    session2.load_state(&path).expect("load");

    assert!(session2.is_fetching_enabled());
    let fc = session2.fetcher_config();
    assert_eq!(fc.network, Some("testnet".to_string()));
}

#[test]
fn test_fetcher_config_from_state_file() {
    let (_dir, path) = create_temp_state_file();

    // Create state file with mainnet fetching
    {
        let session = SimulationSession::new()
            .expect("create session")
            .with_mainnet_fetching();
        session.save_state(&path).expect("save");
    }

    // Use from_state_file and verify fetcher is restored
    let session = SimulationSession::from_state_file(&path).expect("load");
    assert!(
        session.is_fetching_enabled(),
        "fetcher should be restored from state file"
    );
}

#[test]
fn test_load_v3_state_without_fetcher_config() {
    // V3 state has no fetcher_config field - should load fine with default
    let v3_state = r#"{
        "version": 3,
        "objects": [],
        "modules": [],
        "coin_registry": {},
        "sender": "0x0",
        "id_counter": 0,
        "timestamp_ms": null,
        "dynamic_fields": [],
        "pending_receives": [],
        "config": null,
        "metadata": null
    }"#;

    let (_dir, path) = create_temp_state_file();
    std::fs::write(&path, v3_state).expect("write v3 state");

    let mut env = SimulationEnvironment::new().expect("create env");
    let result = env.load_state(&path);
    assert!(result.is_ok(), "Should load v3 state: {:?}", result.err());

    // Fetcher should remain disabled
    assert!(
        !env.is_fetching_enabled(),
        "fetcher should not be enabled from v3 state"
    );
}

#[test]
fn test_version_in_state_file() {
    let session = SimulationSession::new()
        .expect("create session")
        .with_mainnet_fetching();

    let state = session.export_state();
    assert_eq!(
        state.version,
        PersistentState::CURRENT_VERSION,
        "State version should match CURRENT_VERSION with fetcher config"
    );
}

// =============================================================================
// Version Compatibility Tests
// =============================================================================

#[test]
fn test_load_v1_state() {
    // V1 state format (no dynamic_fields, pending_receives, config, metadata)
    let v1_state = r#"{
        "version": 1,
        "objects": [],
        "modules": [],
        "coin_registry": {},
        "sender": "0x0",
        "id_counter": 0,
        "timestamp_ms": null
    }"#;

    let (_dir, path) = create_temp_state_file();
    std::fs::write(&path, v1_state).expect("write v1 state");

    let mut env = SimulationEnvironment::new().expect("env");
    let result = env.load_state(&path);
    assert!(result.is_ok(), "Should load v1 state: {:?}", result.err());
}

#[test]
fn test_load_v2_state() {
    // V2 state format (has dynamic_fields, pending_receives, but no config/metadata)
    let v2_state = r#"{
        "version": 2,
        "objects": [],
        "modules": [],
        "coin_registry": {},
        "sender": "0x0",
        "id_counter": 0,
        "timestamp_ms": null,
        "dynamic_fields": [],
        "pending_receives": []
    }"#;

    let (_dir, path) = create_temp_state_file();
    std::fs::write(&path, v2_state).expect("write v2 state");

    let mut env = SimulationEnvironment::new().expect("env");
    let result = env.load_state(&path);
    assert!(result.is_ok(), "Should load v2 state: {:?}", result.err());
}

#[test]
fn test_current_version_is_5() {
    assert_eq!(
        PersistentState::CURRENT_VERSION,
        5,
        "Current version should be 5"
    );
}

#[test]
fn test_reject_future_version() {
    let future_state = r#"{
        "version": 999,
        "objects": [],
        "modules": [],
        "coin_registry": {},
        "sender": "0x0",
        "id_counter": 0,
        "timestamp_ms": null
    }"#;

    let (_dir, path) = create_temp_state_file();
    std::fs::write(&path, future_state).expect("write future state");

    let mut env = SimulationEnvironment::new().expect("env");
    let result = env.load_state(&path);
    assert!(result.is_err(), "Should reject future version");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("newer than supported"),
        "Error should mention version incompatibility"
    );
}

// =============================================================================
// ID Counter Tests
// =============================================================================

#[test]
fn test_id_counter_persistence() {
    let mut env = SimulationEnvironment::new().expect("create env");
    let (_dir, path) = create_temp_state_file();

    // Create several objects to advance ID counter
    for _ in 0..10 {
        env.create_coin("0x2::sui::SUI", 1000).expect("create coin");
    }

    // Save state
    env.save_state(&path).expect("save");

    // Load into fresh env
    let mut env2 = SimulationEnvironment::new().expect("env2");
    env2.load_state(&path).expect("load");

    // Create more coins - IDs should not collide
    let coin_after_load = env2
        .create_coin("0x2::sui::SUI", 1000)
        .expect("coin after load");

    // The new coin ID should be unique from all previous IDs
    // (This is implicitly tested by the fact that we can create it without collision)
    assert!(
        env2.get_object(&coin_after_load).is_some(),
        "New coin should be created with unique ID"
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_load_nonexistent_file() {
    let mut env = SimulationEnvironment::new().expect("env");
    let result = env.load_state(&PathBuf::from("/nonexistent/path/state.json"));
    assert!(result.is_err(), "Should fail for nonexistent file");
}

#[test]
fn test_load_invalid_json() {
    let (_dir, path) = create_temp_state_file();
    std::fs::write(&path, "not valid json {{{").expect("write invalid");

    let mut env = SimulationEnvironment::new().expect("env");
    let result = env.load_state(&path);
    assert!(result.is_err(), "Should fail for invalid JSON");
}

#[test]
fn test_save_to_readonly_fails_gracefully() {
    // This test is platform-dependent; skip if we can't make read-only
    let (_dir, path) = create_temp_state_file();

    // Create the file first
    std::fs::write(&path, "{}").expect("create file");

    // Make it read-only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&path, perms).expect("set permissions");

        let env = SimulationEnvironment::new().expect("env");
        let result = env.save_state(&path);

        // Restore permissions for cleanup
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).expect("restore permissions");

        assert!(result.is_err(), "Should fail for read-only file");
    }
}

// =============================================================================
// Export State Tests
// =============================================================================

#[test]
fn test_export_state_structure() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create some state
    env.create_coin("0x2::sui::SUI", 1_000_000).expect("coin");
    env.register_coin("0xtest::token::TOKEN", 8, "TKN", "Test Token");

    let state = env.export_state();

    // Verify structure
    assert_eq!(state.version, PersistentState::CURRENT_VERSION);
    assert!(!state.objects.is_empty(), "Should have objects");
    assert!(state.config.is_some(), "Should have config");
    assert!(state.metadata.is_some(), "Should have metadata");
}

#[test]
fn test_export_excludes_framework_modules() {
    let env = SimulationEnvironment::new().expect("create env");

    let state = env.export_state();

    // Framework modules (0x1, 0x2, 0x3) should NOT be in exported modules
    for module in &state.modules {
        assert!(
            !module.id.starts_with("0x1::"),
            "Should not export 0x1 framework modules"
        );
        assert!(
            !module.id.starts_with("0x2::"),
            "Should not export 0x2 framework modules"
        );
        assert!(
            !module.id.starts_with("0x3::"),
            "Should not export 0x3 framework modules"
        );
    }
}
