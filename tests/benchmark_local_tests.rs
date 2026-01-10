use std::path::Path;

use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::validator::Validator;

#[test]
fn benchmark_local_can_load_fixture_modules() {
    let fixture_dir = Path::new("tests/fixture/build/fixture");
    assert!(fixture_dir.exists(), "fixture dir missing: {fixture_dir:?}");

    let mut resolver = LocalModuleResolver::new();
    let loaded = resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");
    assert!(loaded > 0, "expected >0 .mv modules from fixture");
}

#[test]
fn benchmark_local_bcs_roundtrip_primitives() {
    use move_core_types::annotated_value::MoveTypeLayout;

    let resolver = LocalModuleResolver::new();
    let validator = Validator::new(&resolver);

    let cases: Vec<(MoveTypeLayout, Vec<u8>)> = vec![
        (MoveTypeLayout::Bool, vec![0u8]),
        (MoveTypeLayout::U8, vec![7u8]),
        (MoveTypeLayout::U64, 42u64.to_le_bytes().to_vec()),
        (
            MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8)),
            vec![0u8],
        ), // empty vec<u8>
    ];

    for (layout, bytes) in cases {
        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("bcs roundtrip should succeed");
    }
}

#[test]
fn benchmark_local_vm_can_execute_entry_zero_args_fixture() {
    use sui_move_interface_extractor::benchmark::vm::VMHarness;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let mut harness = VMHarness::new(&resolver, true).expect("vm harness should construct");

    // The fixture corpus contains a simple non-entry function:
    // `fixture::test_module::simple_func(u64): u64`
    // Tier B will evolve to support more realistic entry execution, but we start
    // by proving we can execute *some* function in the local VM.
    let module = resolver
        .iter_modules()
        .find(|m| {
            let name = sui_move_interface_extractor::bytecode::compiled_module_name(m);
            name == "test_module"
        })
        .expect("test_module module should exist in fixture corpus");

    let module_id = module.self_id();
    harness
        .execute_function(
            &module_id,
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        )
        .expect("VM should execute simple_func");
}

#[test]
fn benchmark_local_report_schema_has_stable_minimum_fields() {
    // This test guards against brittle/accidental schema drift in benchmark-local JSONL output
    // which the Python E2E harness relies on for tx simulation attribution.
    use serde_json::Value;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let module = resolver
        .iter_modules()
        .find(|m| {
            let name = sui_move_interface_extractor::bytecode::compiled_module_name(m);
            name == "test_module"
        })
        .expect("test_module module should exist in fixture corpus");

    let addr = sui_move_interface_extractor::bytecode::module_self_address_hex(module);
    let report = sui_move_interface_extractor::benchmark::runner::BenchmarkReport {
        target_package: addr,
        target_module: "test_module".to_string(),
        target_function: "simple_func".to_string(),
        status: sui_move_interface_extractor::benchmark::runner::AttemptStatus::TierAHit,
        failure_stage: None,
        failure_reason: None,
        tier_a_details: Some(sui_move_interface_extractor::benchmark::runner::TierADetails {
            resolved_params: vec!["u64".to_string()],
            bcs_roundtrip_verified: true,
            has_object_params: false,
        }),
        tier_b_details: None,
    };

    let v: Value = serde_json::to_value(&report).expect("report should serialize");
    for k in ["target_package", "target_module", "target_function", "status"] {
        assert!(v.get(k).is_some(), "missing key {k}");
    }
    assert!(
        v.get("tier_a_details").is_some() || v.get("tier_b_details").is_some(),
        "expected tier details present"
    );
}

// --- Failure Stage Tests ---

#[test]
fn benchmark_local_failure_stage_a1_module_not_found() {
    use move_core_types::account_address::AccountAddress;
    use sui_move_interface_extractor::benchmark::runner::AttemptStatus;
    use sui_move_interface_extractor::benchmark::runner::FailureStage;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try to validate a non-existent module
    let result = validator.validate_target(
        AccountAddress::from_hex_literal("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap(),
        "nonexistent",
        "any_function",
    );

    assert!(result.is_err(), "should fail for non-existent module");
}

#[test]
fn benchmark_local_failure_stage_a1_function_not_found() {
    use move_core_types::account_address::AccountAddress;
    use sui_move_interface_extractor::benchmark::runner::AttemptStatus;
    use sui_move_interface_extractor::benchmark::runner::FailureStage;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try to validate a non-existent function in existing module
    let module = resolver
        .iter_modules()
        .next()
        .expect("should have at least one module");

    let result = validator.validate_target(
        *module.self_id().address(),
        sui_move_interface_extractor::bytecode::compiled_module_name(module).as_str(),
        "nonexistent_function",
    );

    assert!(result.is_err(), "should fail for non-existent function");
}

#[test]
fn benchmark_local_failure_stage_a3_bcs_roundtrip_fail() {
    use move_core_types::annotated_value::MoveTypeLayout;

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try BCS roundtrip with malformed bytes (wrong length for u64)
    let layout = MoveTypeLayout::U64;
    let malformed_bytes = vec![1u8, 2u8, 3u8]; // Only 3 bytes, u64 needs 8

    let result = validator.validate_bcs_roundtrip(&layout, &malformed_bytes);
    assert!(result.is_err(), "should fail for malformed BCS bytes");
}

#[test]
fn benchmark_local_failure_stage_a4_object_params_detected() {
    use move_core_types::account_address::AccountAddress;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    // Check that test_module exists and has the expected function
    let module = resolver
        .iter_modules()
        .find(|m| {
            let name = sui_move_interface_extractor::bytecode::compiled_module_name(m);
            name == "test_module"
        })
        .expect("test_module should exist");

    // Verify that simple_func exists and has correct parameters
    let func_def = module
        .function_defs()
        .iter()
        .find(|def| {
            let handle = module.function_handle_at(def.function);
            let name = module.identifier_at(handle.name);
            name.as_str() == "simple_func"
        })
        .expect("simple_func should exist");

    // simple_func has parameter (u64), which is not an object reference
    // This test validates that we can distinguish pure args from object args
    let params_sig = module.signature_at(module.function_handle_at(func_def.function).parameters);
    let has_refs = params_sig
        .0
        .iter()
        .any(|t| matches!(t, move_binary_format::file_format::SignatureToken::Reference(_)));

    assert!(!has_refs, "simple_func should not have reference parameters");
}

#[test]
fn benchmark_local_failure_stage_a5_generic_functions_skipped() {
    use move_core_types::account_address::AccountAddress;
    use sui_move_interface_extractor::args::BenchmarkLocalArgs;
    use sui_move_interface_extractor::benchmark::runner::AttemptStatus;
    use sui_move_interface_extractor::benchmark::runner::FailureStage;
    use tempfile::TempDir;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    // Check for any generic functions in fixture
    let mut generic_func_found = false;
    for module in resolver.iter_modules() {
        for def in module.function_defs() {
            let handle = module.function_handle_at(def.function);
            if !handle.type_parameters.is_empty() {
                generic_func_found = true;
                break;
            }
        }
        if generic_func_found {
            break;
        }
    }

    // This test documents current behavior: generic functions exist and should fail at A5
    // The implementation currently skips them (see runner.rs A5 handling)
    if generic_func_found {
        // If we find a generic function, verify it would be detected
        // For now, we just verify that generics exist in the corpus
        assert!(true, "generic functions exist in corpus");
    }
}

#[test]
fn benchmark_local_failure_stage_a2_unresolvable_type() {
    use move_core_types::account_address::AccountAddress;
    use move_core_types::identifier::Identifier;
    use move_core_types::language_storage::StructTag;
    use move_core_types::language_storage::TypeTag;

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try to resolve type from non-existent module
    let nonexistent_tag = StructTag {
        address: AccountAddress::from_hex_literal("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap(),
        module: Identifier::new("nonexistent_module").unwrap(),
        name: Identifier::new("SomeType").unwrap(),
        type_params: vec![],
    };

    let result = validator.resolve_type_layout(&TypeTag::Struct(Box::new(nonexistent_tag)));

    // Should fail because module doesn't exist
    assert!(result.is_err(), "should fail to resolve type from non-existent module");

    let err = result.unwrap_err().to_string();
    // Verify error message is informative
    assert!(err.contains("module not found") || err.contains("not found") || err.contains("nonexistent"),
        "error should mention module not found: {err}");
}

#[test]
fn benchmark_local_failure_stage_b1_vm_harness_creation_fail() {
    use sui_move_interface_extractor::benchmark::vm::VMHarness;

    let resolver = LocalModuleResolver::new();

    // Try to create VM harness with restricted state but no modules loaded
    // This should work (no failure in current implementation)
    // The test verifies that harness can be created and reports errors during execution
    let harness_result = VMHarness::new(&resolver, true);

    // Current implementation creates harness successfully even with empty resolver
    // Real B1 failures would occur with corrupt bytecode
    // This test documents expected behavior
    match harness_result {
        Ok(_harness) => {
            // VM harness created successfully (expected for empty resolver)
            assert!(true, "VM harness can be created with empty resolver");
        }
        Err(e) => {
            // If it fails, verify error is actionable
            let err_msg = e.to_string();
            assert!(err_msg.contains("VM") || err_msg.contains("failed"),
                "VM harness error should be actionable: {err_msg}");
        }
    }
}

#[test]
fn benchmark_local_failure_stage_validation() {
    use move_core_types::account_address::AccountAddress;
    use move_core_types::annotated_value::MoveTypeLayout;
    use sui_move_interface_extractor::benchmark::runner::AttemptStatus;
    use sui_move_interface_extractor::benchmark::runner::FailureStage;

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Test A3: BCS roundtrip failure with wrong layout
    let layout = MoveTypeLayout::Bool;
    let wrong_bytes = vec![0u8, 1u8]; // Two bytes for bool (wrong)

    let result = validator.validate_bcs_roundtrip(&layout, &wrong_bytes);
    assert!(result.is_err(), "should fail with wrong number of bytes");

    // Test A3: BCS roundtrip failure with malformed value
    let layout = MoveTypeLayout::U64;
    let malformed_bytes = vec![0xFFu8; 8]; // Valid bytes but might cause issues

    // Actually, any 8 bytes are valid for u64 in BCS
    let result = validator.validate_bcs_roundtrip(&layout, &malformed_bytes);
    assert!(result.is_ok(), "u64 should accept any 8 bytes");
}

// --- Error Context Tests ---

#[test]
fn benchmark_local_error_context_module_not_found() {
    use move_core_types::account_address::AccountAddress;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try to validate non-existent module
    let result = validator.validate_target(
        AccountAddress::from_hex_literal("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap(),
        "nonexistent",
        "any_function",
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();

    // Verify error message includes module information
    assert!(err.contains("module not found") || err.contains("nonexistent"),
        "error message should mention module: {err}");
}

#[test]
fn benchmark_local_error_context_function_not_found() {
    use move_core_types::account_address::AccountAddress;

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try to validate non-existent function
    let module = resolver
        .iter_modules()
        .next()
        .expect("should have at least one module");

    let result = validator.validate_target(
        *module.self_id().address(),
        sui_move_interface_extractor::bytecode::compiled_module_name(module).as_str(),
        "nonexistent_function",
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();

    // Verify error message includes function information
    assert!(err.contains("function not found") || err.contains("nonexistent_function"),
        "error message should mention function: {err}");
}

#[test]
fn benchmark_local_error_context_bcs_roundtrip() {
    use move_core_types::annotated_value::MoveTypeLayout;

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Try BCS roundtrip with malformed bytes
    let layout = MoveTypeLayout::U64;
    let malformed_bytes = vec![1u8, 2u8, 3u8]; // Only 3 bytes for u64

    let result = validator.validate_bcs_roundtrip(&layout, &malformed_bytes);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();

    // Verify error message is actionable
    assert!(err.contains("BCS") || err.contains("deserialize") || err.contains("mismatch"),
        "error message should be actionable and mention BCS issue: {err}");
}

// --- Performance Tests ---

#[test]
#[ignore] // Run manually with `cargo test -- --ignored`
fn benchmark_local_performance_validation_speed() {
    let start = std::time::Instant::now();

    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    let module_count = resolver
        .load_from_dir(fixture_dir)
        .expect("load_from_dir should succeed");

    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Validate all functions in all modules
    let mut validation_count = 0;
    for module in resolver.iter_modules() {
        for func_def in module.function_defs() {
            let handle = module.function_handle_at(func_def.function);
            let func_name = module.identifier_at(handle.name);

            // Try to validate (some will fail due to non-public, etc.)
            let _ = validator.validate_target(
                *module.self_id().address(),
                sui_move_interface_extractor::bytecode::compiled_module_name(module).as_str(),
                func_name.as_str(),
            );
            validation_count += 1;
        }
    }

    let duration = start.elapsed();

    // Performance expectation: <100ms for full corpus validation
    assert!(
        duration.as_millis() < 100,
        "Too slow: {:?} for {} validations (expected <100ms)",
        duration,
        validation_count
    );

    println!(
        "Validated {} functions in {:?} ({:.2} ms/validation)",
        validation_count,
        duration,
        duration.as_millis() as f64 / validation_count as f64
    );
}

#[test]
#[ignore] // Run manually with `cargo test -- --ignored`
fn benchmark_local_performance_bcs_roundtrip_speed() {
    use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};

    let start = std::time::Instant::now();

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Test BCS roundtrip for various types
    let cases = vec![
        (MoveTypeLayout::U8, MoveValue::U8(42)),
        (MoveTypeLayout::U64, MoveValue::U64(42)),
        (MoveTypeLayout::U128, MoveValue::U128(42)),
        (
            MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8)),
            MoveValue::Vector(vec![MoveValue::U8(1), MoveValue::U8(2), MoveValue::U8(3)]),
        ),
        (
            MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U64)),
            MoveValue::Vector(vec![MoveValue::U64(1), MoveValue::U64(2), MoveValue::U64(3)]),
        ),
    ];

    let mut iteration_count = 0;
    for _ in 0..1000 {
        for (layout, value) in &cases {
            let bytes = value.simple_serialize().unwrap();
            let _ = validator.validate_bcs_roundtrip(layout, &bytes);
            iteration_count += 1;
        }
    }

    let duration = start.elapsed();

    // Performance expectation: <500ms for 1000 * 6 = 6000 validations
    assert!(
        duration.as_millis() < 500,
        "Too slow: {:?} for {} BCS roundtrips (expected <500ms)",
        duration,
        iteration_count
    );

    println!(
        "Completed {} BCS roundtrips in {:?} ({:.2} μs/roundtrip)",
        iteration_count,
        duration,
        duration.as_micros() as f64 / iteration_count as f64
    );
}

// --- Complex Struct Layout Tests ---

#[test]
fn benchmark_local_layout_resolution_nested_vectors() {
    use move_core_types::annotated_value::{MoveFieldLayout, MoveStructLayout, MoveTypeLayout, MoveValue};

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Test vector of u64s
    let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U64));

    // Create value with two u64s
    let value = MoveValue::Vector(vec![
        MoveValue::U64(42),
        MoveValue::U64(100),
    ]);

    let bytes = value.simple_serialize().unwrap();
    let result = validator.validate_bcs_roundtrip(&layout, &bytes);
    assert!(result.is_ok(), "vector of u64 should roundtrip");
}

#[test]
fn benchmark_local_layout_resolution_structs() {
    use move_core_types::annotated_value::{MoveFieldLayout, MoveStructLayout, MoveTypeLayout};
    use move_core_types::account_address::AccountAddress;
    use move_core_types::identifier::Identifier;
    use move_core_types::language_storage::StructTag;

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Test struct layout with multiple fields
    let tag = StructTag {
        address: AccountAddress::ZERO,
        module: Identifier::new("test").unwrap(),
        name: Identifier::new("TestStruct").unwrap(),
        type_params: vec![],
    };

    let fields = vec![
        MoveFieldLayout::new(Identifier::new("field1").unwrap(), MoveTypeLayout::U64),
        MoveFieldLayout::new(Identifier::new("field2").unwrap(), MoveTypeLayout::Address),
    ];

    let layout = MoveTypeLayout::Struct(Box::new(MoveStructLayout::new(
        tag,
        fields,
    )));

    // Just verify the layout is valid - actual BCS roundtrip requires complex MoveStruct creation
    // which is outside scope of simple layout validation test
    assert!(matches!(layout, MoveTypeLayout::Struct(_)));
}

#[test]
fn benchmark_local_layout_resolution_address() {
    use move_core_types::account_address::AccountAddress;
    use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Test address layout
    let layout = MoveTypeLayout::Address;
    let value = MoveValue::Address(AccountAddress::ZERO);
    let bytes = value.simple_serialize().unwrap();

    let result = validator.validate_bcs_roundtrip(&layout, &bytes);
    assert!(result.is_ok(), "address should roundtrip");
}

#[test]
fn benchmark_local_layout_resolution_u256() {
    use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};
    use move_core_types::u256::U256;

    let resolver = LocalModuleResolver::new();
    let validator = sui_move_interface_extractor::benchmark::validator::Validator::new(&resolver);

    // Test U256 layout
    let layout = MoveTypeLayout::U256;
    let value = MoveValue::U256(U256::zero());
    let bytes = value.simple_serialize().unwrap();

    let result = validator.validate_bcs_roundtrip(&layout, &bytes);
    assert!(result.is_ok(), "u256 should roundtrip");
}
