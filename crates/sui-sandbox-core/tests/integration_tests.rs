//! Integration tests for sui-sandbox-core.
//!
//! These tests verify end-to-end behavior across multiple modules.

use move_core_types::account_address::AccountAddress;
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};

/// Test that we can create a basic VM harness with framework loaded.
#[test]
fn test_vm_harness_creation_with_framework() {
    // Create resolver and load framework
    let mut resolver = LocalModuleResolver::new();
    resolver.load_sui_framework().expect("load framework");

    // Create VM harness with default config
    let config = SimulationConfig::default();
    let harness = VMHarness::with_config(&resolver, false, config);

    assert!(harness.is_ok(), "VMHarness should be created successfully");
}

/// Test that SimulationConfig produces unique tx hashes.
#[test]
fn test_simulation_config_unique_tx_hashes() {
    let configs: Vec<_> = (0..100).map(|_| SimulationConfig::default()).collect();

    // All tx_hashes should be unique
    let mut hashes: Vec<_> = configs.iter().map(|c| c.tx_hash).collect();
    hashes.sort();
    hashes.dedup();

    assert_eq!(
        hashes.len(),
        100,
        "All 100 configs should have unique tx_hashes"
    );
}

/// Test resolver can load and retrieve modules.
#[test]
fn test_resolver_module_loading() {
    use move_core_types::resolver::ModuleResolver;

    let mut resolver = LocalModuleResolver::new();
    resolver.load_sui_framework().expect("load framework");

    // Check that 0x2::coin module is available
    let coin_module_id = move_core_types::language_storage::ModuleId::new(
        AccountAddress::from_hex_literal("0x2").unwrap(),
        move_core_types::identifier::Identifier::new("coin").unwrap(),
    );

    let module = resolver.get_module(&coin_module_id).expect("get module");
    assert!(module.is_some(), "0x2::coin module should be loaded");
}

/// Test that strict config differs from default config.
#[test]
fn test_strict_vs_default_config() {
    let default = SimulationConfig::default();
    let strict = SimulationConfig::strict();

    // Strict should have stricter settings
    assert!(default.mock_crypto_pass);
    assert!(!strict.mock_crypto_pass);

    assert!(default.permissive_ownership);
    assert!(!strict.permissive_ownership);

    assert!(!default.enforce_immutability);
    assert!(strict.enforce_immutability);
}

/// Test config serialization round-trip.
#[test]
fn test_config_json_round_trip() {
    let config = SimulationConfig::default()
        .with_epoch(500)
        .with_gas_budget(Some(10_000_000_000))
        .with_protocol_version(73);

    let json = serde_json::to_string(&config).expect("serialize");
    let restored: SimulationConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored.epoch, 500);
    assert_eq!(restored.gas_budget, Some(10_000_000_000));
    assert_eq!(restored.protocol_version, 73);
}
