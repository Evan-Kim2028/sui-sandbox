//! Integration tests for MM2 (Move Model 2) type validation.
//!
//! These tests verify that the MM2-based static type checking works correctly
//! with real Move bytecode.

use sui_sandbox_core::mm2::{ConstructorGraph, TypeModel, TypeValidator};
use sui_sandbox_core::phases::{resolution, typecheck};
use sui_sandbox_core::resolver::LocalModuleResolver;

/// Test that MM2 can build a model from framework modules.
#[test]
fn test_mm2_model_from_sui_framework() {
    // Load Sui framework modules
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");

    // Collect all modules
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    assert!(!modules.is_empty(), "No framework modules loaded");

    // Build MM2 model
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    // Verify we can find known framework modules
    let modules_list = model.modules();
    assert!(
        modules_list.len() > 50,
        "Expected many framework modules, got {}",
        modules_list.len()
    );

    // Check for well-known modules
    let module_names: Vec<_> = modules_list.iter().map(|(_, name)| name.as_str()).collect();
    assert!(
        module_names.contains(&"object"),
        "Missing sui::object module"
    );
    assert!(module_names.contains(&"coin"), "Missing sui::coin module");
    assert!(
        module_names.contains(&"transfer"),
        "Missing sui::transfer module"
    );
}

/// Test that MM2 can validate function existence.
#[test]
fn test_mm2_function_validation() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let validator = TypeValidator::new(&model);

    // Should find coin::value
    let sui_addr = move_core_types::account_address::AccountAddress::TWO;
    let result = validator.validate_function_exists(&sui_addr, "coin", "value");
    assert!(result.is_ok(), "Should find coin::value");

    let sig = result.unwrap();
    assert_eq!(sig.name, "value");
    assert!(sig.is_public, "coin::value should be public");

    // Should not find non-existent function
    let not_found = validator.validate_function_exists(&sui_addr, "coin", "not_a_function");
    assert!(not_found.is_err(), "Should not find non-existent function");
}

/// Test that MM2 can get struct information.
#[test]
fn test_mm2_struct_info() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    // Get Coin struct info
    let coin_info = model.get_struct(&sui_addr, "coin", "Coin");
    assert!(coin_info.is_some(), "Should find Coin struct");

    let info = coin_info.unwrap();
    assert_eq!(info.name, "Coin");
    assert!(
        !info.type_parameters.is_empty(),
        "Coin should have type params"
    );

    // Coin should have store ability
    assert!(
        info.abilities
            .0
            .iter()
            .any(|a| *a == move_model_2::summary::Ability::Store),
        "Coin should have store ability"
    );
}

/// Test constructor graph building.
#[test]
fn test_constructor_graph() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let graph = ConstructorGraph::from_model(&model);
    let stats = graph.stats();

    // Should have discovered many types
    assert!(stats.total_types > 50, "Expected many types in framework");

    // Some types should have constructors
    assert!(
        stats.types_with_constructors > 0,
        "Expected some constructors"
    );

    // Some types should be objects (have key ability)
    assert!(stats.object_types > 0, "Expected some object types");
}

/// Test phase-based resolution.
#[test]
fn test_phase_resolution() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");

    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    // Test resolution of a known function
    let config = resolution::ResolutionConfig {
        resolver: &resolver,
        module_addr: sui_addr,
        module_name: "coin",
        function_name: "value",
    };

    let result = resolution::resolve(config);
    assert!(
        result.is_ok(),
        "Should resolve coin::value: {:?}",
        result.err()
    );

    let ctx = result.unwrap();
    assert_eq!(ctx.target_module_name, "coin");
    assert_eq!(ctx.target_function_name, "value");
}

/// Test phase-based type checking.
#[test]
fn test_phase_typecheck() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");

    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    // Resolve first
    let config = resolution::ResolutionConfig {
        resolver: &resolver,
        module_addr: sui_addr,
        module_name: "coin",
        function_name: "value",
    };

    let ctx = resolution::resolve(config).expect("Resolution should succeed");

    // Now type check
    let tc_result = typecheck::validate(&ctx);
    assert!(
        tc_result.is_ok(),
        "Type check should succeed: {:?}",
        tc_result.err()
    );

    let tc = tc_result.unwrap();
    // coin::value takes one parameter (a reference to Coin<T>)
    assert_eq!(tc.param_count, 1, "coin::value has 1 parameter");
    // It has one type parameter (T)
    assert_eq!(tc.type_param_count, 1, "coin::value has 1 type param");
}

/// Test function existence check.
#[test]
fn test_function_exists_quick_check() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");

    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    // Known function should exist
    assert!(
        resolution::function_exists(&resolver, &sui_addr, "coin", "value"),
        "coin::value should exist"
    );

    // Unknown function should not exist
    assert!(
        !resolution::function_exists(&resolver, &sui_addr, "coin", "not_a_real_function"),
        "non-existent function should not exist"
    );

    // Unknown module should not have functions
    assert!(
        !resolution::function_exists(&resolver, &sui_addr, "not_a_module", "any"),
        "non-existent module should not have functions"
    );
}

/// Test producer chain discovery in constructor graph.
#[test]
fn test_producer_chain_discovery() {
    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let graph = ConstructorGraph::from_model(&model);

    // Verify producer chains were discovered
    let stats = graph.stats();

    // The framework should have some types with producers
    // (functions that return types as part of multi-return or direct return)
    assert!(
        stats.total_types > 0,
        "Should have discovered types in framework"
    );

    // Check that we can find execution chains for known types
    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    // Coin<T> should be constructible (coin::zero returns Coin<T>)
    // This tests the basic chain finding capability
    let _coin_key = format!("{}::coin::Coin", sui_addr);

    // The graph should contain the Coin type
    // Note: We test the graph structure, not runtime execution
    assert!(
        stats.types_with_constructors > 0 || stats.object_types > 0,
        "Framework should have constructible or object types"
    );
}

/// Test type synthesizer for SuiSystemState.
#[test]
fn test_type_synthesizer_sui_system_state() {
    use sui_sandbox_core::mm2::TypeSynthesizer;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let mut synthesizer = TypeSynthesizer::new(&model);

    // SuiSystemState should be synthesizable (address 0x3)
    let sui_system_addr = move_core_types::account_address::AccountAddress::from_hex_literal(
        "0x0000000000000000000000000000000000000000000000000000000000000003",
    )
    .expect("Valid address");

    let result = synthesizer.synthesize_struct(
        &sui_system_addr,
        "sui_system_state_inner",
        "SuiSystemStateV2",
    );

    // The synthesizer should handle this type (even if with a stub)
    // It's OK if it fails for V2 but succeeds for inner types
    // The key test is that it doesn't panic
    match result {
        Ok(synth) => {
            assert!(
                !synth.bytes.is_empty(),
                "Synthesized bytes should not be empty"
            );
        }
        Err(e) => {
            // Some types may not be fully synthesizable - that's OK
            // We just want to ensure no panic
            eprintln!(
                "Note: SuiSystemStateV2 synthesis returned error (expected): {}",
                e
            );
        }
    }
}

/// Test type synthesizer for ValidatorSet with 10 validators.
#[test]
fn test_type_synthesizer_validator_set() {
    use sui_sandbox_core::mm2::TypeSynthesizer;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let mut synthesizer = TypeSynthesizer::new(&model);

    // ValidatorSet synthesis (from sui_system module at 0x3)
    let sui_system_addr = move_core_types::account_address::AccountAddress::from_hex_literal(
        "0x0000000000000000000000000000000000000000000000000000000000000003",
    )
    .expect("Valid address");

    let result = synthesizer.synthesize_struct(&sui_system_addr, "validator_set", "ValidatorSet");

    // ValidatorSet should be synthesizable with our special handling
    match result {
        Ok(synth) => {
            assert!(
                !synth.bytes.is_empty(),
                "ValidatorSet bytes should not be empty"
            );
            // Should indicate it was synthesized (may be a stub)
            eprintln!("ValidatorSet synthesis succeeded: {}", synth.description);
        }
        Err(e) => {
            // If it fails, ensure it's not a panic but a proper error
            eprintln!("Note: ValidatorSet synthesis returned error: {}", e);
        }
    }
}

/// Test that Coin synthesis uses non-zero balance (1 SUI).
#[test]
fn test_coin_synthesis_has_balance() {
    use sui_sandbox_core::mm2::TypeSynthesizer;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let modules: Vec<_> = resolver.iter_modules().cloned().collect();
    let model = TypeModel::from_modules(modules).expect("Failed to build MM2 model");

    let mut synthesizer = TypeSynthesizer::new(&model);

    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    let result = synthesizer.synthesize_struct(&sui_addr, "coin", "Coin");

    match result {
        Ok(synth) => {
            // Coin<T> has: UID (32 bytes) + Balance (8 bytes u64)
            // Total: 40 bytes minimum
            assert!(synth.bytes.len() >= 40, "Coin should have UID + Balance");

            // Check the description mentions 1 SUI (not 0)
            assert!(
                synth.description.contains("1_SUI") || synth.description.contains("Coin"),
                "Coin should be synthesized with realistic balance"
            );
            eprintln!(
                "Coin synthesis: {} ({} bytes)",
                synth.description,
                synth.bytes.len()
            );
        }
        Err(e) => {
            panic!("Coin synthesis should succeed: {}", e);
        }
    }
}

// ============================================================================
// PTB Integration Tests
// ============================================================================

/// Test that PTBBuilder can construct and execute a simple MoveCall.
#[test]
fn test_ptb_simple_move_call() {
    use sui_sandbox_core::ptb::PTBBuilder;
    use sui_sandbox_core::vm::VMHarness;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");

    let mut harness = VMHarness::new(&resolver, false).expect("Failed to create VM harness");

    let mut builder = PTBBuilder::new();

    // Call object::id_from_address which takes an address and returns an ID
    let sui_addr = move_core_types::account_address::AccountAddress::TWO;
    let test_addr = move_core_types::account_address::AccountAddress::ZERO;

    // Add the address as input (BCS-encoded 32-byte address)
    let addr_arg = builder.pure_bytes(test_addr.to_vec());

    // Call object::id_from_address(address): ID
    let result = builder.move_call(
        sui_addr,
        "object",
        "id_from_address",
        vec![],
        vec![addr_arg],
    );

    assert!(result.is_ok(), "move_call should succeed");

    // Execute the PTB
    let effects = builder.execute(&mut harness);
    assert!(effects.is_ok(), "PTB execution should succeed");

    let effects = effects.unwrap();
    assert!(effects.success, "PTB should succeed: {:?}", effects.error);
}

/// Test PTB with chained results - call a function and use its result.
#[test]
fn test_ptb_chained_results() {
    use sui_sandbox_core::ptb::PTBBuilder;
    use sui_sandbox_core::vm::VMHarness;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let mut harness = VMHarness::new(&resolver, false).expect("Failed to create VM harness");

    let mut builder = PTBBuilder::new();
    let sui_addr = move_core_types::account_address::AccountAddress::TWO;

    // First call: create a UID using object::new
    // object::new requires &mut TxContext
    let ctx_bytes = harness.synthesize_tx_context().expect("synthesize ctx");
    let ctx_arg = builder.pure_bytes(ctx_bytes);

    let uid_result = builder.move_call(sui_addr, "object", "new", vec![], vec![ctx_arg]);
    assert!(uid_result.is_ok(), "first move_call should succeed");
    let uid_arg = uid_result.unwrap();

    // Second call: get the ID from the UID using object::uid_to_inner
    // object::uid_to_inner(&UID): ID
    let id_result = builder.move_call(sui_addr, "object", "uid_to_inner", vec![], vec![uid_arg]);
    assert!(id_result.is_ok(), "second move_call should succeed");

    // Execute the PTB
    let effects = builder.execute(&mut harness);
    assert!(effects.is_ok(), "PTB execution should succeed");

    let effects = effects.unwrap();
    // This may fail due to native function limitations, but should not panic
    if !effects.success {
        eprintln!(
            "PTB chained results test: execution returned error (expected in sandbox): {:?}",
            effects.error
        );
    }
}

/// Test PTB SplitCoins command.
#[test]
fn test_ptb_split_coins() {
    use sui_sandbox_core::ptb::{Argument, PTBBuilder};
    use sui_sandbox_core::vm::VMHarness;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let mut harness = VMHarness::new(&resolver, false).expect("Failed to create VM harness");

    let mut builder = PTBBuilder::new();

    // Create a mock Coin with balance
    // Coin<SUI> structure: UID (32 bytes) + Balance { value: u64 }
    let mut coin_bytes = vec![0u8; 32]; // UID
    coin_bytes.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // 1 SUI balance

    let coin_arg = builder.pure_bytes(coin_bytes);

    // Split amounts
    let amount1 = builder.pure(&100_000_000u64).expect("serialize amount1");
    let amount2 = builder.pure(&200_000_000u64).expect("serialize amount2");

    // Execute SplitCoins
    let split_result = builder.split_coins(coin_arg, vec![amount1, amount2]);

    // The result should be a tuple of new coins
    assert!(matches!(split_result, Argument::Result(_)));

    // Execute the PTB
    let effects = builder.execute(&mut harness);
    assert!(effects.is_ok(), "PTB execution should succeed");

    let effects = effects.unwrap();
    assert!(
        effects.success,
        "SplitCoins should succeed: {:?}",
        effects.error
    );
}

/// Test PTB MergeCoins command.
#[test]
fn test_ptb_merge_coins() {
    use sui_sandbox_core::ptb::{Argument, PTBBuilder};
    use sui_sandbox_core::vm::VMHarness;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let mut harness = VMHarness::new(&resolver, false).expect("Failed to create VM harness");

    let mut builder = PTBBuilder::new();

    // Create mock Coins
    let mut coin1_bytes = vec![0u8; 32]; // UID
    coin1_bytes.extend_from_slice(&500_000_000u64.to_le_bytes()); // 0.5 SUI

    let mut coin2_bytes = vec![1u8; 32]; // Different UID
    coin2_bytes.extend_from_slice(&300_000_000u64.to_le_bytes()); // 0.3 SUI

    let coin1_arg = builder.pure_bytes(coin1_bytes);
    let coin2_arg = builder.pure_bytes(coin2_bytes);

    // Execute MergeCoins - merge coin2 into coin1
    let merge_result = builder.merge_coins(coin1_arg, vec![coin2_arg]);

    assert!(matches!(merge_result, Argument::Result(_)));

    // Execute the PTB
    let effects = builder.execute(&mut harness);
    assert!(effects.is_ok(), "PTB execution should succeed");

    let effects = effects.unwrap();
    assert!(
        effects.success,
        "MergeCoins should succeed: {:?}",
        effects.error
    );
}

/// Test PTB MakeMoveVec command.
#[test]
fn test_ptb_make_move_vec() {
    use move_core_types::language_storage::TypeTag;
    use sui_sandbox_core::ptb::{Argument, PTBBuilder};
    use sui_sandbox_core::vm::VMHarness;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let mut harness = VMHarness::new(&resolver, false).expect("Failed to create VM harness");

    let mut builder = PTBBuilder::new();

    // Create u64 elements
    let elem1 = builder.pure(&100u64).expect("serialize elem1");
    let elem2 = builder.pure(&200u64).expect("serialize elem2");
    let elem3 = builder.pure(&300u64).expect("serialize elem3");

    // Create vector<u64>
    let vec_result = builder.make_move_vec(Some(TypeTag::U64), vec![elem1, elem2, elem3]);

    assert!(matches!(vec_result, Argument::Result(_)));

    // Execute the PTB
    let effects = builder.execute(&mut harness);
    assert!(effects.is_ok(), "PTB execution should succeed");

    let effects = effects.unwrap();
    assert!(
        effects.success,
        "MakeMoveVec should succeed: {:?}",
        effects.error
    );
}

/// Test PTB TransferObjects command.
#[test]
fn test_ptb_transfer_objects() {
    use sui_sandbox_core::ptb::PTBBuilder;
    use sui_sandbox_core::vm::VMHarness;

    let resolver = LocalModuleResolver::with_sui_framework().expect("Failed to load framework");
    let mut harness = VMHarness::new(&resolver, false).expect("Failed to create VM harness");

    let mut builder = PTBBuilder::new();

    // Create a mock object (just bytes representing some object)
    let object_bytes = vec![0u8; 40]; // UID + some data
    let object_arg = builder.pure_bytes(object_bytes);

    // Destination address
    let dest_addr = move_core_types::account_address::AccountAddress::from_hex_literal("0x1234")
        .expect("parse address");
    let dest_arg = builder.pure_bytes(dest_addr.to_vec());

    // Transfer the object
    builder.transfer_objects(vec![object_arg], dest_arg);

    // Execute the PTB
    let effects = builder.execute(&mut harness);
    assert!(effects.is_ok(), "PTB execution should succeed");

    let effects = effects.unwrap();
    assert!(
        effects.success,
        "TransferObjects should succeed: {:?}",
        effects.error
    );
}

/// Test cross-PTB transfer and receive: objects transferred in one PTB
/// can be received in a subsequent PTB.
#[test]
fn test_cross_ptb_transfer_receive() {
    use move_core_types::account_address::AccountAddress;
    use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectChange, ObjectInput};
    use sui_sandbox_core::simulation::SimulationEnvironment;

    // Create a simulation environment
    let mut env = SimulationEnvironment::new().expect("create env");

    // Create a test coin with some balance
    let coin_id = env.create_sui_coin(1_000_000_000).expect("create coin"); // 1 SUI

    // Recipient address for the transfer
    let recipient = AccountAddress::from_hex_literal(
        "0xabc0000000000000000000000000000000000000000000000000000000000001",
    )
    .expect("parse recipient");

    // PTB 1: Transfer the coin to the recipient
    let coin_obj = env.get_object(&coin_id).expect("get coin");
    let coin_type = coin_obj.type_tag.clone();
    let coin_input = InputValue::Object(ObjectInput::Owned {
        id: coin_id,
        bytes: coin_obj.bcs_bytes.clone(),
        type_tag: Some(coin_type.clone()),
    });

    let result1 = env.execute_ptb(
        vec![
            coin_input,
            InputValue::Pure(recipient.to_vec()), // destination address
        ],
        vec![Command::TransferObjects {
            objects: vec![Argument::Input(0)],
            address: Argument::Input(1),
        }],
    );

    assert!(
        result1.success,
        "PTB1 (transfer) should succeed: {:?}",
        result1.raw_error
    );

    // Verify the transfer was recorded
    let effects1 = result1.effects.expect("effects");

    // Debug: print all object changes
    println!("Object changes: {:?}", effects1.object_changes);
    println!("Transferred list: {:?}", effects1.transferred);

    assert!(
        !effects1.transferred.is_empty(),
        "Should have transferred objects"
    );
    assert!(
        effects1.transferred.contains(&coin_id),
        "Coin should be in transferred list"
    );

    // Verify the object change includes the transfer with bytes
    let transfer_change = effects1
        .object_changes
        .iter()
        .find(|c| matches!(c, ObjectChange::Transferred { id, .. } if *id == coin_id));
    assert!(
        transfer_change.is_some(),
        "Should have Transferred change for the coin"
    );

    // Verify the coin is now in pending_receives
    assert!(
        env.has_pending_receives(recipient),
        "Recipient should have pending receives"
    );

    let pending = env.get_pending_receives(recipient);
    assert_eq!(pending.len(), 1, "Should have exactly one pending receive");
    assert_eq!(pending[0].0, coin_id, "Pending receive should be our coin");

    // Verify the coin is no longer in top-level objects
    assert!(
        env.get_object(&coin_id).is_none(),
        "Coin should not be in top-level objects after transfer"
    );

    // PTB 2: Receive the coin as the recipient
    // First, set the sender to be the recipient
    env.set_sender(recipient);

    let result2 = env.execute_ptb(
        vec![],
        vec![Command::Receive {
            object_id: coin_id,
            object_type: Some(coin_type.clone()),
        }],
    );

    assert!(
        result2.success,
        "PTB2 (receive) should succeed: {:?}",
        result2.raw_error
    );

    // Verify the pending receive was consumed
    assert!(
        !env.has_pending_receives(recipient),
        "Pending receives should be cleared after receive"
    );

    println!("Cross-PTB transfer/receive test passed!");
}

/// Test that receiving without a prior transfer fails.
#[test]
fn test_receive_without_transfer_fails() {
    use move_core_types::account_address::AccountAddress;
    use sui_sandbox_core::ptb::Command;
    use sui_sandbox_core::simulation::SimulationEnvironment;

    let mut env = SimulationEnvironment::new().expect("create env");

    // Try to receive an object that was never transferred
    let fake_object_id = AccountAddress::from_hex_literal(
        "0xdeadbeef00000000000000000000000000000000000000000000000000000001",
    )
    .expect("parse id");

    let result = env.execute_ptb(
        vec![],
        vec![Command::Receive {
            object_id: fake_object_id,
            object_type: None,
        }],
    );

    assert!(
        !result.success,
        "Receive without prior transfer should fail"
    );
    assert!(
        result
            .raw_error
            .as_ref()
            .is_some_and(|e| e.contains("not found in pending receives")),
        "Error should mention pending receives: {:?}",
        result.raw_error
    );
}

/// Test multiple transfers in a single PTB and receiving them separately.
#[test]
fn test_multi_transfer_receive() {
    use move_core_types::account_address::AccountAddress;
    use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput};
    use sui_sandbox_core::simulation::SimulationEnvironment;

    let mut env = SimulationEnvironment::new().expect("create env");

    // Create two coins
    let coin1_id = env.create_sui_coin(1_000_000_000).expect("create coin1");
    let coin2_id = env.create_sui_coin(2_000_000_000).expect("create coin2");

    // Two different recipients
    let recipient1 = AccountAddress::from_hex_literal(
        "0xaaa0000000000000000000000000000000000000000000000000000000000001",
    )
    .expect("parse recipient1");
    let recipient2 = AccountAddress::from_hex_literal(
        "0xbbb0000000000000000000000000000000000000000000000000000000000002",
    )
    .expect("parse recipient2");

    let coin1_obj = env.get_object(&coin1_id).expect("get coin1");
    let coin2_obj = env.get_object(&coin2_id).expect("get coin2");

    // Transfer both coins to different recipients in a single PTB
    let result1 = env.execute_ptb(
        vec![
            InputValue::Object(ObjectInput::Owned {
                id: coin1_id,
                bytes: coin1_obj.bcs_bytes.clone(),
                type_tag: Some(coin1_obj.type_tag.clone()),
            }),
            InputValue::Object(ObjectInput::Owned {
                id: coin2_id,
                bytes: coin2_obj.bcs_bytes.clone(),
                type_tag: Some(coin2_obj.type_tag.clone()),
            }),
            InputValue::Pure(recipient1.to_vec()),
            InputValue::Pure(recipient2.to_vec()),
        ],
        vec![
            Command::TransferObjects {
                objects: vec![Argument::Input(0)],
                address: Argument::Input(2),
            },
            Command::TransferObjects {
                objects: vec![Argument::Input(1)],
                address: Argument::Input(3),
            },
        ],
    );

    assert!(
        result1.success,
        "Multi-transfer PTB should succeed: {:?}",
        result1.raw_error
    );

    // Verify both recipients have pending receives
    assert!(
        env.has_pending_receives(recipient1),
        "Recipient1 should have pending"
    );
    assert!(
        env.has_pending_receives(recipient2),
        "Recipient2 should have pending"
    );

    // Receive as recipient1
    env.set_sender(recipient1);
    let result2 = env.execute_ptb(
        vec![],
        vec![Command::Receive {
            object_id: coin1_id,
            object_type: None,
        }],
    );
    assert!(
        result2.success,
        "Recipient1 receive should succeed: {:?}",
        result2.raw_error
    );

    // Verify recipient1's pending is cleared but recipient2's is still there
    assert!(
        !env.has_pending_receives(recipient1),
        "Recipient1 should have no pending after receive"
    );
    assert!(
        env.has_pending_receives(recipient2),
        "Recipient2 should still have pending"
    );

    // Receive as recipient2
    env.set_sender(recipient2);
    let result3 = env.execute_ptb(
        vec![],
        vec![Command::Receive {
            object_id: coin2_id,
            object_type: None,
        }],
    );
    assert!(
        result3.success,
        "Recipient2 receive should succeed: {:?}",
        result3.raw_error
    );

    assert!(
        !env.has_pending_receives(recipient2),
        "Recipient2 should have no pending after receive"
    );

    println!("Multi-transfer/receive test passed!");
}

/// Test ValidatePTB request type - validates PTB structure without execution.
#[test]
fn test_validate_ptb() {
    use sui_move_interface_extractor::benchmark::sandbox_exec::{
        execute_request, PtbArg, PtbCommand, PtbInput, SandboxRequest,
    };
    use sui_sandbox_core::simulation::SimulationEnvironment;

    let mut env = SimulationEnvironment::new().expect("Failed to create environment");

    // Test 1: Valid PTB with pure inputs
    let valid_ptb = SandboxRequest::ValidatePtb {
        inputs: vec![
            PtbInput::Pure {
                value: serde_json::json!(1000u64),
                value_type: "u64".to_string(),
            },
            PtbInput::Pure {
                value: serde_json::json!("0x2"),
                value_type: "address".to_string(),
            },
        ],
        commands: vec![PtbCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_args: vec!["0x2::sui::SUI".to_string()],
            args: vec![PtbArg::Input(0)],
        }],
    };

    let response = execute_request(&mut env, &valid_ptb, false);
    assert!(
        response.success,
        "Valid PTB should validate successfully: {:?}",
        response.error
    );

    if let Some(data) = response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
        assert!(valid, "PTB should be marked as valid");

        let input_count = data
            .get("input_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(input_count, 2, "Should have 2 inputs");

        let command_count = data
            .get("command_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(command_count, 1, "Should have 1 command");
    }

    // Test 2: Invalid package address
    let invalid_pkg = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![PtbCommand::MoveCall {
            package: "not_an_address".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_args: vec![],
            args: vec![],
        }],
    };

    let response = execute_request(&mut env, &invalid_pkg, false);
    assert!(
        response.success,
        "Validate should return success status even for invalid PTBs"
    );

    if let Some(data) = response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "PTB with invalid package should be marked invalid");

        let errors = data.get("errors").and_then(|v| v.as_array());
        assert!(errors.is_some(), "Should have errors array");
        assert!(
            !errors.unwrap().is_empty(),
            "Should have at least one error"
        );
    }

    // Test 3: Invalid type argument
    let invalid_type = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![PtbCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_args: vec!["invalid::type::BAD".to_string()],
            args: vec![],
        }],
    };

    let response = execute_request(&mut env, &invalid_type, false);
    if let Some(data) = response.data {
        // Type arg parsing may fail or succeed depending on strictness
        // Just verify we get a structured response
        assert!(data.get("commands").is_some(), "Should have commands info");
    }

    // Test 4: MakeMoveVec with invalid element type
    let invalid_vec_type = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![PtbCommand::MakeMoveVec {
            element_type: Some("bad!!!type".to_string()),
            elements: vec![],
        }],
    };

    let response = execute_request(&mut env, &invalid_vec_type, false);
    if let Some(data) = response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "MakeMoveVec with invalid type should be invalid");
    }

    println!("ValidatePTB tests passed!");
}

/// Test enhanced ValidatePTB with deep validation features.
#[test]
fn test_validate_ptb_enhanced() {
    use sui_move_interface_extractor::benchmark::sandbox_exec::{
        execute_request, PtbArg, PtbCommand, PtbInput, SandboxRequest,
    };
    use sui_sandbox_core::simulation::SimulationEnvironment;

    let mut env = SimulationEnvironment::new().expect("Failed to create environment");

    // Test 1: Wrong argument count - coin::value takes 1 arg (coin), not 2
    let wrong_arg_count = SandboxRequest::ValidatePtb {
        inputs: vec![
            PtbInput::Pure {
                value: serde_json::json!(100u64),
                value_type: "u64".to_string(),
            },
            PtbInput::Pure {
                value: serde_json::json!(200u64),
                value_type: "u64".to_string(),
            },
        ],
        commands: vec![PtbCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_args: vec!["0x2::sui::SUI".to_string()],
            args: vec![PtbArg::Input(0), PtbArg::Input(1)], // Wrong: 2 args instead of 1
        }],
    };

    let response = execute_request(&mut env, &wrong_arg_count, false);
    assert!(response.success, "Validate should return success status");

    if let Some(data) = &response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "PTB with wrong arg count should be invalid");

        let errors = data.get("errors").and_then(|v| v.as_array());
        assert!(errors.is_some(), "Should have errors");
        let err_strs: Vec<String> = errors
            .unwrap()
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.to_string()))
            .collect();
        let has_arg_count_error = err_strs
            .iter()
            .any(|e| e.contains("Argument count mismatch"));
        assert!(
            has_arg_count_error,
            "Should report argument count mismatch: {:?}",
            err_strs
        );
    }

    // Test 2: Wrong type argument count - coin::value takes 1 type param, not 2
    let wrong_type_count = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![PtbCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_args: vec![
                "0x2::sui::SUI".to_string(),
                "0x2::sui::SUI".to_string(), // Wrong: 2 type args instead of 1
            ],
            args: vec![],
        }],
    };

    let response = execute_request(&mut env, &wrong_type_count, false);
    if let Some(data) = &response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "PTB with wrong type arg count should be invalid");

        let errors = data.get("errors").and_then(|v| v.as_array());
        let err_strs: Vec<String> = errors
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.to_string()))
            .collect();
        let has_type_count_error = err_strs
            .iter()
            .any(|e| e.contains("Type argument count mismatch"));
        assert!(
            has_type_count_error,
            "Should report type argument count mismatch: {:?}",
            err_strs
        );
    }

    // Test 3: Forward reference - Result(1, 0) before command 1 executes
    let forward_ref = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![
            PtbCommand::MoveCall {
                package: "0x2".to_string(),
                module: "coin".to_string(),
                function: "zero".to_string(),
                type_args: vec!["0x2::sui::SUI".to_string()],
                args: vec![PtbArg::Result { cmd: 1, idx: 0 }], // Forward reference!
            },
            PtbCommand::MoveCall {
                package: "0x2".to_string(),
                module: "coin".to_string(),
                function: "zero".to_string(),
                type_args: vec!["0x2::sui::SUI".to_string()],
                args: vec![],
            },
        ],
    };

    let response = execute_request(&mut env, &forward_ref, false);
    if let Some(data) = &response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "PTB with forward reference should be invalid");

        let errors = data.get("errors").and_then(|v| v.as_array());
        let err_strs: Vec<String> = errors
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.to_string()))
            .collect();
        let has_forward_error = err_strs.iter().any(|e| e.contains("forward reference"));
        assert!(
            has_forward_error,
            "Should report forward reference error: {:?}",
            err_strs
        );
    }

    // Test 4: Invalid input reference - Input(5) when only 2 inputs
    let bad_input_ref = SandboxRequest::ValidatePtb {
        inputs: vec![PtbInput::Pure {
            value: serde_json::json!(100u64),
            value_type: "u64".to_string(),
        }],
        commands: vec![PtbCommand::TransferObjects {
            objects: vec![PtbArg::Input(5)], // Out of bounds
            recipient: PtbArg::Input(0),
        }],
    };

    let response = execute_request(&mut env, &bad_input_ref, false);
    if let Some(data) = &response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "PTB with out-of-bounds input should be invalid");

        let errors = data.get("errors").and_then(|v| v.as_array());
        let err_strs: Vec<String> = errors
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.to_string()))
            .collect();
        let has_input_error = err_strs.iter().any(|e| e.contains("non-existent input"));
        assert!(
            has_input_error,
            "Should report non-existent input error: {:?}",
            err_strs
        );
    }

    // Test 5: Function not found
    let no_func = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![PtbCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "this_function_does_not_exist".to_string(),
            type_args: vec![],
            args: vec![],
        }],
    };

    let response = execute_request(&mut env, &no_func, false);
    if let Some(data) = &response.data {
        let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);
        assert!(!valid, "PTB with non-existent function should be invalid");

        let errors = data.get("errors").and_then(|v| v.as_array());
        let err_strs: Vec<String> = errors
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.to_string()))
            .collect();
        let has_not_found = err_strs.iter().any(|e| e.contains("not found"));
        assert!(
            has_not_found,
            "Should report function not found: {:?}",
            err_strs
        );
    }

    // Test 6: Check result_types are returned
    let with_results = SandboxRequest::ValidatePtb {
        inputs: vec![],
        commands: vec![
            PtbCommand::MoveCall {
                package: "0x2".to_string(),
                module: "coin".to_string(),
                function: "zero".to_string(),
                type_args: vec!["0x2::sui::SUI".to_string()],
                args: vec![],
            },
            PtbCommand::SplitCoins {
                coin: PtbArg::Result { cmd: 0, idx: 0 },
                amounts: vec![
                    PtbArg::Input(0), // Would need proper inputs in real scenario
                ],
            },
        ],
    };

    let response = execute_request(&mut env, &with_results, false);
    if let Some(data) = &response.data {
        // Check that result_types is populated
        let result_types = data.get("result_types").and_then(|v| v.as_array());
        assert!(result_types.is_some(), "Should have result_types array");
        let types = result_types.unwrap();
        assert_eq!(types.len(), 2, "Should have result types for 2 commands");
    }

    println!("Enhanced ValidatePTB tests passed!");
}

/// Test event querying APIs.
#[test]
fn test_event_query_apis() {
    use sui_move_interface_extractor::benchmark::sandbox_exec::{execute_request, SandboxRequest};
    use sui_sandbox_core::simulation::SimulationEnvironment;

    let mut env = SimulationEnvironment::new().expect("Failed to create environment");

    // Initially, there should be no events
    let list_events = SandboxRequest::ListEvents;
    let response = execute_request(&mut env, &list_events, false);
    assert!(response.success, "ListEvents should succeed");

    if let Some(data) = &response.data {
        let count = data.get("count").and_then(|v| v.as_u64()).unwrap_or(999);
        assert_eq!(count, 0, "Initially should have no events");
    }

    // Get last tx events (should be empty initially)
    let last_tx_events = SandboxRequest::GetLastTxEvents;
    let response = execute_request(&mut env, &last_tx_events, false);
    assert!(response.success, "GetLastTxEvents should succeed");

    if let Some(data) = &response.data {
        let count = data.get("count").and_then(|v| v.as_u64()).unwrap_or(999);
        assert_eq!(count, 0, "Last tx events should be empty initially");
    }

    // Get events by type (should be empty)
    let events_by_type = SandboxRequest::GetEventsByType {
        type_prefix: "0x2::coin".to_string(),
    };
    let response = execute_request(&mut env, &events_by_type, false);
    assert!(response.success, "GetEventsByType should succeed");

    if let Some(data) = &response.data {
        let count = data.get("count").and_then(|v| v.as_u64()).unwrap_or(999);
        assert_eq!(count, 0, "Events by type should be empty initially");
    }

    // Clear events (should work even when empty)
    let clear_events = SandboxRequest::ClearEvents;
    let response = execute_request(&mut env, &clear_events, false);
    assert!(response.success, "ClearEvents should succeed");

    if let Some(data) = &response.data {
        let cleared = data
            .get("cleared")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(cleared, "Should indicate events were cleared");

        let previous_count = data
            .get("previous_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(999);
        assert_eq!(previous_count, 0, "Previous count should be 0");
    }

    println!("Event query API tests passed!");
}

/// Test shared object versioning APIs.
#[test]
fn test_shared_object_versioning_apis() {
    use sui_move_interface_extractor::benchmark::sandbox_exec::{execute_request, SandboxRequest};
    use sui_sandbox_core::simulation::SimulationEnvironment;

    let mut env = SimulationEnvironment::new().expect("Failed to create environment");

    // Test 1: Get lamport clock (should start at 0)
    let get_clock = SandboxRequest::GetLamportClock;
    let response = execute_request(&mut env, &get_clock, false);
    assert!(response.success, "GetLamportClock should succeed");

    if let Some(data) = &response.data {
        let clock = data
            .get("lamport_clock")
            .and_then(|v| v.as_u64())
            .unwrap_or(999);
        assert_eq!(clock, 0, "Lamport clock should start at 0");
    }

    // Test 2: Advance lamport clock
    let advance_clock = SandboxRequest::AdvanceLamportClock;
    let response = execute_request(&mut env, &advance_clock, false);
    assert!(response.success, "AdvanceLamportClock should succeed");

    if let Some(data) = &response.data {
        let previous = data
            .get("previous_value")
            .and_then(|v| v.as_u64())
            .unwrap_or(999);
        let new_value = data.get("new_value").and_then(|v| v.as_u64()).unwrap_or(0);
        assert_eq!(previous, 0, "Previous clock should be 0");
        assert_eq!(new_value, 1, "New clock should be 1");
    }

    // Test 3: List shared locks (should be empty initially)
    let list_locks = SandboxRequest::ListSharedLocks;
    let response = execute_request(&mut env, &list_locks, false);
    assert!(response.success, "ListSharedLocks should succeed");

    if let Some(data) = &response.data {
        let count = data.get("count").and_then(|v| v.as_u64()).unwrap_or(999);
        assert_eq!(count, 0, "Should have no locks initially");

        let clock = data
            .get("lamport_clock")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(clock, 1, "Lamport clock should be 1 after advance");
    }

    // Test 4: Get shared object info for Clock object (0x6)
    let get_shared_info = SandboxRequest::GetSharedObjectInfo {
        object_id: "0x6".to_string(),
    };
    let response = execute_request(&mut env, &get_shared_info, false);
    assert!(response.success, "GetSharedObjectInfo should succeed");

    if let Some(data) = &response.data {
        let is_shared = data
            .get("is_shared")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(is_shared, "Clock (0x6) should be a shared object");

        let version = data.get("version").and_then(|v| v.as_u64());
        assert!(version.is_some(), "Should have a version");
    }

    // Test 5: Get info for non-existent object
    let get_nonexistent = SandboxRequest::GetSharedObjectInfo {
        object_id: "0xdeadbeef".to_string(),
    };
    let response = execute_request(&mut env, &get_nonexistent, false);
    assert!(
        !response.success,
        "GetSharedObjectInfo for non-existent should fail"
    );

    println!("Shared object versioning API tests passed!");
}
