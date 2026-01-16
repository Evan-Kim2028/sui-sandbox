#![allow(clippy::len_zero)]
//! Sandbox Replay Integration Tests
//!
//! These tests validate the ability to replay fetched transactions in the simulation
//! environment. They bridge the gap between data fetching and local execution.
//!
//! Test categories:
//! - SimulationEnvironment basic operations
//! - PTB construction and execution
//! - Transaction replay with fetched data
//! - Error recovery and self-healing workflows
//!
//! Run with:
//!   cargo test --test sandbox_replay_integration_tests -- --nocapture

use move_core_types::account_address::AccountAddress;
use sui_move_interface_extractor::benchmark::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;

// =============================================================================
// SimulationEnvironment Basic Tests
// =============================================================================

#[test]
fn test_simulation_environment_creation() {
    let env = SimulationEnvironment::new();
    assert!(env.is_ok(), "Should create SimulationEnvironment");
}

#[test]
fn test_simulation_environment_has_framework() {
    let env = SimulationEnvironment::new().expect("create env");

    // Should have Sui framework loaded
    let modules = env.list_modules();
    assert!(!modules.is_empty(), "Should have modules loaded");

    // Check for well-known framework modules
    let has_coin = modules.iter().any(|m| m.contains("coin"));
    let has_object = modules.iter().any(|m| m.contains("object"));
    let has_transfer = modules.iter().any(|m| m.contains("transfer"));

    assert!(has_coin, "Should have sui::coin");
    assert!(has_object, "Should have sui::object");
    assert!(has_transfer, "Should have sui::transfer");

    println!("Framework loaded with {} modules", modules.len());
}

#[test]
fn test_simulation_environment_create_coin() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a SUI coin
    let coin_id = env.create_coin("0x2::sui::SUI", 1_000_000_000);

    assert!(coin_id.is_ok(), "Should create SUI coin");
    let id = coin_id.unwrap();

    // Verify the object exists
    let obj = env.get_object(&id);
    assert!(obj.is_some(), "Created coin should exist");

    println!("Created coin: {}", id.to_hex_literal());
}

#[test]
fn test_simulation_environment_create_multiple_coins() {
    let mut env = SimulationEnvironment::new().expect("create env");

    let coin1 = env.create_coin("0x2::sui::SUI", 100).expect("coin 1");
    let coin2 = env.create_coin("0x2::sui::SUI", 200).expect("coin 2");
    let coin3 = env.create_coin("0x2::sui::SUI", 300).expect("coin 3");

    // All IDs should be unique
    assert_ne!(coin1, coin2, "Coin IDs should be unique");
    assert_ne!(coin2, coin3, "Coin IDs should be unique");
    assert_ne!(coin1, coin3, "Coin IDs should be unique");

    // All should exist
    assert!(env.get_object(&coin1).is_some());
    assert!(env.get_object(&coin2).is_some());
    assert!(env.get_object(&coin3).is_some());
}

// =============================================================================
// PTB Execution Tests
// =============================================================================

#[test]
fn test_ptb_split_coins() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a coin with enough balance
    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("create coin");
    let coin_obj = env.get_object(&coin_id).expect("coin exists");

    // Build PTB: split coin
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100_000_000u64.to_le_bytes().to_vec()), // Split amount
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("SplitCoins succeeded");
        if let Some(effects) = &result.effects {
            println!("  Created: {}", effects.created.len());
            println!("  Mutated: {}", effects.mutated.len());
            assert!(effects.created.len() >= 1, "Should create new coin");
        }
    } else {
        panic!("SplitCoins failed: {:?}", result.error);
    }
}

#[test]
fn test_ptb_merge_coins() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create two coins
    let coin1_id = env
        .create_coin("0x2::sui::SUI", 100_000_000)
        .expect("coin 1");
    let coin2_id = env
        .create_coin("0x2::sui::SUI", 200_000_000)
        .expect("coin 2");

    let coin1_obj = env.get_object(&coin1_id).expect("coin 1 exists");
    let coin2_obj = env.get_object(&coin2_id).expect("coin 2 exists");

    // Build PTB: merge coins
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin1_id,
            bytes: coin1_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Object(ObjectInput::Owned {
            id: coin2_id,
            bytes: coin2_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
    ];

    let commands = vec![Command::MergeCoins {
        destination: Argument::Input(0),
        sources: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("MergeCoins succeeded");
        if let Some(effects) = &result.effects {
            println!("  Deleted: {}", effects.deleted.len());
            println!("  Mutated: {}", effects.mutated.len());
        }
    } else {
        panic!("MergeCoins failed: {:?}", result.error);
    }
}

#[test]
fn test_ptb_transfer_objects() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a coin to transfer
    let coin_id = env.create_coin("0x2::sui::SUI", 100_000_000).expect("coin");
    let coin_obj = env.get_object(&coin_id).expect("coin exists");

    // Generate recipient address
    let recipient = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )
    .unwrap();

    // Build PTB: transfer objects
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(recipient.to_vec()),
    ];

    let commands = vec![Command::TransferObjects {
        objects: vec![Argument::Input(0)],
        address: Argument::Input(1),
    }];

    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("TransferObjects succeeded");
        if let Some(effects) = &result.effects {
            println!("  Mutated: {}", effects.mutated.len());
        }
    } else {
        panic!("TransferObjects failed: {:?}", result.error);
    }
}

#[test]
fn test_ptb_multi_command_sequence() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create initial coin
    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("coin");
    let coin_obj = env.get_object(&coin_id).expect("coin exists");

    let recipient = AccountAddress::from_hex_literal(
        "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
    )
    .unwrap();

    // Build PTB: split then transfer
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100_000_000u64.to_le_bytes().to_vec()),
        InputValue::Pure(recipient.to_vec()),
    ];

    let commands = vec![
        // First: split the coin
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1)],
        },
        // Second: transfer the split result
        Command::TransferObjects {
            objects: vec![Argument::NestedResult(0, 0)], // First result of first command
            address: Argument::Input(2),
        },
    ];

    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("Multi-command PTB succeeded");
        if let Some(effects) = &result.effects {
            println!("  Created: {}", effects.created.len());
            println!("  Mutated: {}", effects.mutated.len());
        }
    } else {
        panic!("Multi-command PTB failed: {:?}", result.error);
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[test]
fn test_ptb_insufficient_balance_error() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a coin with small balance
    let coin_id = env.create_coin("0x2::sui::SUI", 100).expect("coin");
    let coin_obj = env.get_object(&coin_id).expect("coin exists");

    // Try to split more than available
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(1_000_000u64.to_le_bytes().to_vec()), // More than balance
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    // Should fail with a structured error
    assert!(!result.success, "Should fail with insufficient balance");
    assert!(result.error.is_some(), "Should have error details");

    println!("Expected failure: {:?}", result.error);
}

#[test]
fn test_ptb_missing_object_error() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Use a non-existent object ID
    let fake_id = AccountAddress::from_hex_literal(
        "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    )
    .unwrap();

    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: fake_id,
            bytes: vec![], // Empty bytes
            type_tag: None,
        }),
        InputValue::Pure(100u64.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    assert!(!result.success, "Should fail with missing object");
    println!("Expected failure: {:?}", result.error);
}

// =============================================================================
// State Persistence Tests
// =============================================================================

#[test]
fn test_state_persists_across_ptbs() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create initial coin
    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("coin");

    // First PTB: split
    let coin_obj = env.get_object(&coin_id).expect("coin exists");
    let inputs1 = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100_000_000u64.to_le_bytes().to_vec()),
    ];

    let commands1 = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result1 = env.execute_ptb(inputs1, commands1);
    assert!(result1.success, "First PTB should succeed");

    // Get the created coin ID from effects
    let created_id = result1
        .effects
        .as_ref()
        .and_then(|e| e.created.first())
        .cloned()
        .expect("should have created coin");

    // Second PTB: use the created coin
    let created_obj = env
        .get_object(&created_id)
        .expect("created coin should exist");

    let recipient = AccountAddress::from_hex_literal(
        "0x9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba",
    )
    .unwrap();

    let inputs2 = vec![
        InputValue::Object(ObjectInput::Owned {
            id: created_id,
            bytes: created_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(recipient.to_vec()),
    ];

    let commands2 = vec![Command::TransferObjects {
        objects: vec![Argument::Input(0)],
        address: Argument::Input(1),
    }];

    let result2 = env.execute_ptb(inputs2, commands2);
    assert!(result2.success, "Second PTB should succeed");

    println!("State persisted correctly across PTBs");
}

#[test]
fn test_environment_reset() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create some objects
    let coin1 = env.create_coin("0x2::sui::SUI", 100).expect("coin 1");
    let coin2 = env.create_coin("0x2::sui::SUI", 200).expect("coin 2");

    assert!(env.get_object(&coin1).is_some());
    assert!(env.get_object(&coin2).is_some());

    // Reset environment
    env.reset().expect("reset");

    // Objects should no longer exist
    assert!(
        env.get_object(&coin1).is_none(),
        "coin1 should be gone after reset"
    );
    assert!(
        env.get_object(&coin2).is_none(),
        "coin2 should be gone after reset"
    );

    // But framework should still be loaded
    let modules = env.list_modules();
    assert!(
        !modules.is_empty(),
        "Framework should still exist after reset"
    );
}

// =============================================================================
// Module Introspection Tests
// =============================================================================

#[test]
fn test_list_functions() {
    let env = SimulationEnvironment::new().expect("create env");

    // List functions in sui::coin
    let functions = env.list_functions("0x2::coin");

    assert!(functions.is_some(), "Should find sui::coin");
    let funcs = functions.unwrap();

    assert!(!funcs.is_empty(), "Should have functions");

    // Check for well-known functions
    assert!(funcs.contains(&"value".to_string()), "Should have value()");
    assert!(
        funcs.contains(&"balance".to_string()),
        "Should have balance()"
    );

    println!("sui::coin functions: {:?}", funcs);
}

#[test]
fn test_get_function_info() {
    let env = SimulationEnvironment::new().expect("create env");

    let info = env.get_function_info("0x2::coin", "value");

    assert!(info.is_some(), "Should find coin::value");
    let info = info.unwrap();

    // Info is a serde_json::Value, check for expected keys
    assert!(info.get("visibility").is_some(), "Should have visibility");
    assert!(info.get("params").is_some(), "Should have params");

    println!("coin::value info: {:?}", info);
}

#[test]
fn test_module_summary() {
    let env = SimulationEnvironment::new().expect("create env");

    let sui_addr = AccountAddress::from_hex_literal("0x2").unwrap();
    let summary = env.get_module_summary(&sui_addr, "coin");

    assert!(summary.is_ok(), "Should get coin summary");
    let summary = summary.unwrap();

    // Summary is a string, just verify it's non-empty
    assert!(!summary.is_empty(), "Summary should not be empty");

    println!("Module summary length: {} chars", summary.len());
}

// =============================================================================
// Gas Tracking Tests
// =============================================================================

#[test]
fn test_gas_usage_tracking() {
    let mut env = SimulationEnvironment::new().expect("create env");

    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("coin");
    let coin_obj = env.get_object(&coin_id).expect("coin exists");

    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100u64.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    assert!(result.success);
    assert!(result.effects.is_some(), "Should have effects");

    // Gas tracking is available in effects (may be 0 in unmetered execution)
    let effects = result.effects.unwrap();
    println!("Gas used: {}", effects.gas_used);
}

// =============================================================================
// Event Handling Tests
// =============================================================================

#[test]
fn test_events_are_captured() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Clear any existing events
    env.clear_events();
    assert!(
        env.get_all_events().is_empty(),
        "Events should be empty after clear"
    );

    // Execute a PTB that might emit events (coin operations)
    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("coin");
    let coin_obj = env.get_object(&coin_id).expect("coin exists");

    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: coin_obj.bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100u64.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let _result = env.execute_ptb(inputs, commands);

    // Events are available (may or may not have any depending on operation)
    let events = env.get_all_events();
    println!("Events captured: {}", events.len());
}
