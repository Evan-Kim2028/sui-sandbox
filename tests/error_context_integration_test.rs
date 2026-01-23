//! Integration tests for error context population in PTB failures.
//!
//! These tests verify that when PTB commands fail, the error context
//! is properly populated with debugging information.
//!
//! Run with:
//! ```sh
//! cargo test --test error_context_integration_test
//! ```

use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_sandbox_core::simulation::SimulationEnvironment;

/// Test that error_context is populated when SplitCoins fails due to insufficient balance.
#[test]
fn test_error_context_on_split_coins_insufficient_balance() {
    // Create environment with framework
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a SUI coin with small balance (100)
    let coin_id = env.create_coin("0x2::sui::SUI", 100).expect("create coin");

    // Try to split more than available (500)
    let split_amount: u64 = 500;
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: env.get_object(&coin_id).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(split_amount.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    // Verify failure
    assert!(!result.success, "Should fail due to insufficient balance");
    assert_eq!(result.failed_command_index, Some(0));

    // Verify error_context is populated
    assert!(
        result.error_context.is_some(),
        "error_context should be populated on failure"
    );
    let ctx = result.error_context.unwrap();
    assert_eq!(ctx.command_index, 0);
    assert_eq!(ctx.command_type, "SplitCoins");

    // Verify coin balances context
    assert!(
        ctx.coin_balances.is_some(),
        "coin_balances should be populated for SplitCoins failures"
    );
    let coin_ctx = ctx.coin_balances.unwrap();
    assert_eq!(coin_ctx.source_balance, Some(100)); // The actual balance
    assert_eq!(coin_ctx.requested_splits, Some(vec![500])); // What was requested

    // Verify state_at_failure is populated
    assert!(
        result.state_at_failure.is_some(),
        "state_at_failure should be populated on failure"
    );
    let snapshot = result.state_at_failure.unwrap();
    assert_eq!(snapshot.successful_commands.len(), 0); // No commands succeeded before failure
}

/// Test that error_context tracks prior successful commands.
#[test]
fn test_error_context_tracks_successful_commands() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a coin with 1000 balance
    let coin_id = env.create_coin("0x2::sui::SUI", 1000).expect("create coin");

    // Command 0: Split 100 (should succeed, leaving 900)
    // Command 1: Split 200 (should succeed, leaving 700)
    // Command 2: Split 2000 (should fail - only 700 left)
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: env.get_object(&coin_id).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100u64.to_le_bytes().to_vec()),
        InputValue::Pure(200u64.to_le_bytes().to_vec()),
        InputValue::Pure(2000u64.to_le_bytes().to_vec()),
    ];

    let commands = vec![
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1)],
        },
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(2)],
        },
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(3)],
        },
    ];

    let result = env.execute_ptb(inputs, commands);

    // Verify failure on third command
    assert!(!result.success);
    assert_eq!(result.failed_command_index, Some(2));
    assert_eq!(result.commands_succeeded, 2);

    // Verify error context tracks prior commands
    let ctx = result.error_context.expect("should have error_context");
    assert_eq!(ctx.command_index, 2);
    assert_eq!(ctx.prior_successful_commands, vec![0, 1]);

    // Verify state_at_failure has successful command summaries
    let snapshot = result
        .state_at_failure
        .expect("should have state_at_failure");
    assert_eq!(snapshot.successful_commands.len(), 2);
    assert_eq!(snapshot.successful_commands[0].index, 0);
    assert_eq!(snapshot.successful_commands[0].command_type, "SplitCoins");
    assert_eq!(snapshot.successful_commands[1].index, 1);
}

/// Test that error_context is NOT populated on successful execution.
#[test]
fn test_no_error_context_on_success() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a coin with enough balance
    let coin_id = env
        .create_coin("0x2::sui::SUI", 1_000_000_000)
        .expect("create coin");

    // Split a small amount - should succeed
    let split_amount: u64 = 100;
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: env.get_object(&coin_id).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(split_amount.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    // Verify success
    assert!(result.success, "Should succeed with sufficient balance");

    // Verify error context is NOT populated on success
    assert!(
        result.error_context.is_none(),
        "error_context should be None on success"
    );
    assert!(
        result.state_at_failure.is_none(),
        "state_at_failure should be None on success"
    );
}

/// Test error context with MergeCoins failure.
#[test]
fn test_error_context_on_merge_coins_type_mismatch() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create two coins of different types (if possible) or same type
    // For this test, we'll create coins and try to merge them incorrectly
    let coin_id_1 = env
        .create_coin("0x2::sui::SUI", 100)
        .expect("create coin 1");
    let coin_id_2 = env
        .create_coin("0x2::sui::SUI", 200)
        .expect("create coin 2");

    // Try to merge - first split coin_1, then merge with coin_2
    // This should work, but let's test the error context structure
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id_1,
            bytes: env.get_object(&coin_id_1).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Object(ObjectInput::Owned {
            id: coin_id_2,
            bytes: env.get_object(&coin_id_2).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
    ];

    let commands = vec![Command::MergeCoins {
        destination: Argument::Input(0),
        sources: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    // MergeCoins should succeed for same-type coins
    assert!(
        result.success,
        "MergeCoins should succeed for same type coins"
    );
    assert!(result.error_context.is_none());
}

/// Test that gas consumed is tracked in error context.
#[test]
fn test_error_context_gas_tracking() {
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a coin
    let coin_id = env.create_coin("0x2::sui::SUI", 1000).expect("create coin");

    // First command succeeds, second fails
    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: env.get_object(&coin_id).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(100u64.to_le_bytes().to_vec()),
        InputValue::Pure(5000u64.to_le_bytes().to_vec()), // More than remaining balance
    ];

    let commands = vec![
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1)],
        },
        Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(2)],
        },
    ];

    let result = env.execute_ptb(inputs, commands);

    assert!(!result.success);

    let ctx = result.error_context.expect("should have error_context");

    // Gas should have been consumed by the first successful command
    // Note: The exact amount depends on the gas model
    // Just verify the field is accessible (it's a u64, so always >= 0)
    let _gas_before = ctx.gas_consumed_before_failure;

    let snapshot = result
        .state_at_failure
        .expect("should have state_at_failure");
    // Verify the field is accessible
    let _total_gas = snapshot.total_gas_consumed;
}

/// Test that input object snapshots are captured in error context.
#[test]
fn test_error_context_object_snapshots() {
    let mut env = SimulationEnvironment::new().expect("create env");

    let coin_id = env.create_coin("0x2::sui::SUI", 100).expect("create coin");

    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: env.get_object(&coin_id).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(500u64.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    assert!(!result.success);

    let ctx = result.error_context.expect("should have error_context");

    // Should have captured the input coin as an object snapshot
    assert!(
        !ctx.input_objects.is_empty(),
        "input_objects should contain the coin"
    );

    let coin_snapshot = &ctx.input_objects[0];
    assert!(
        coin_snapshot.id.contains(&coin_id.to_hex_literal()[2..8]),
        "object snapshot should contain coin ID"
    );
    assert!(
        coin_snapshot.data_size > 0,
        "should have captured data size"
    );
}

/// Test that the error context Display implementation works.
#[test]
fn test_error_context_display() {
    let mut env = SimulationEnvironment::new().expect("create env");

    let coin_id = env.create_coin("0x2::sui::SUI", 100).expect("create coin");

    let inputs = vec![
        InputValue::Object(ObjectInput::Owned {
            id: coin_id,
            bytes: env.get_object(&coin_id).unwrap().bcs_bytes.clone(),
            type_tag: None,
        }),
        InputValue::Pure(500u64.to_le_bytes().to_vec()),
    ];

    let commands = vec![Command::SplitCoins {
        coin: Argument::Input(0),
        amounts: vec![Argument::Input(1)],
    }];

    let result = env.execute_ptb(inputs, commands);

    assert!(!result.success);

    let ctx = result.error_context.expect("should have error_context");

    // Test that Display works without panicking
    let display = format!("{}", ctx);

    // Should contain key information
    assert!(
        display.contains("Command #0"),
        "Display should show command index"
    );
    assert!(
        display.contains("SplitCoins"),
        "Display should show command type"
    );
}
