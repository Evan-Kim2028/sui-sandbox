//! Edge case and error handling tests
//!
//! Test coverage areas:
//! - Boundary conditions (empty inputs, max values, overflow)
//! - Malformed inputs (invalid bytecode, corrupt data)
//! - Error message quality and actionability
//! - Stress testing (many operations, concurrent access)

use std::path::Path;
use std::sync::Arc;
use std::thread;

use move_core_types::account_address::AccountAddress;
use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{ModuleId, StructTag, TypeTag};
use move_core_types::u256::U256;

use sui_move_interface_extractor::benchmark::natives::{MockClock, MockNativeState, MockRandom};
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::validator::Validator;
use sui_move_interface_extractor::benchmark::vm::{SimulationConfig, VMHarness};

// =============================================================================
// Empty/Null Input Tests
// =============================================================================

mod empty_input_tests {
    use super::*;

    #[test]
    fn test_empty_resolver() {
        let resolver = LocalModuleResolver::new();
        assert_eq!(resolver.module_count(), 0);
        assert!(resolver.list_packages().is_empty());
        assert!(resolver.list_modules().is_empty());
    }

    #[test]
    fn test_validator_with_empty_resolver() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let result = validator.validate_target(AccountAddress::ZERO, "module", "func");
        assert!(result.is_err());
    }

    #[test]
    fn test_harness_with_empty_resolver() {
        let resolver = LocalModuleResolver::new();
        let result = VMHarness::new(&resolver, true);

        // Should succeed - empty resolver is valid for harness creation
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_function_name() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let module = resolver.iter_modules().next().unwrap();

        // Empty function name
        let result = validator.validate_target(
            *module.self_id().address(),
            &module.self_id().name().to_string(),
            "",
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_empty_module_name() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        // Empty module name should fail
        let result = validator.validate_target(AccountAddress::ZERO, "", "func");

        assert!(result.is_err());
    }

    #[test]
    fn test_execute_with_empty_args() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).unwrap();

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .unwrap();

        // simple_func requires one u64 arg, empty args should fail
        let result = harness.execute_function(&module.self_id(), "simple_func", vec![], vec![]);

        assert!(result.is_err());
    }

    #[test]
    fn test_bcs_roundtrip_empty_vector() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8));
        let value = MoveValue::Vector(vec![]);
        let bytes = value.simple_serialize().unwrap();

        let result = validator.validate_bcs_roundtrip(&layout, &bytes);
        assert!(result.is_ok(), "empty vector should roundtrip");
    }

    #[test]
    fn test_bcs_roundtrip_empty_bytes() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;
        let result = validator.validate_bcs_roundtrip(&layout, &[]);

        assert!(result.is_err(), "empty bytes for u64 should fail");
    }
}

// =============================================================================
// Boundary Value Tests
// =============================================================================

mod boundary_value_tests {
    use super::*;

    #[test]
    fn test_u8_boundaries() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U8;

        for val in [0u8, 1, 127, 128, 254, 255] {
            let bytes = vec![val];
            let result = validator.validate_bcs_roundtrip(&layout, &bytes);
            assert!(result.is_ok(), "u8 value {} should roundtrip", val);
        }
    }

    #[test]
    fn test_u64_boundaries() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;

        for val in [0u64, 1, u64::MAX / 2, u64::MAX - 1, u64::MAX] {
            let bytes = val.to_le_bytes().to_vec();
            let result = validator.validate_bcs_roundtrip(&layout, &bytes);
            assert!(result.is_ok(), "u64 value {} should roundtrip", val);
        }
    }

    #[test]
    fn test_u128_boundaries() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U128;

        for val in [0u128, 1, u128::MAX / 2, u128::MAX - 1, u128::MAX] {
            let bytes = val.to_le_bytes().to_vec();
            let result = validator.validate_bcs_roundtrip(&layout, &bytes);
            assert!(result.is_ok(), "u128 value {} should roundtrip", val);
        }
    }

    #[test]
    fn test_u256_boundaries() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U256;

        let values = [
            U256::zero(),
            U256::one(),
            U256::max_value(),
        ];

        for val in values {
            let move_val = MoveValue::U256(val);
            let bytes = move_val.simple_serialize().unwrap();
            let result = validator.validate_bcs_roundtrip(&layout, &bytes);
            assert!(result.is_ok(), "u256 should roundtrip");
        }
    }

    #[test]
    fn test_epoch_boundaries() {
        // Test epoch at various boundary values
        let configs = [
            SimulationConfig::default().with_epoch(0),
            SimulationConfig::default().with_epoch(1),
            SimulationConfig::default().with_epoch(u64::MAX / 2),
            SimulationConfig::default().with_epoch(u64::MAX - 1),
            SimulationConfig::default().with_epoch(u64::MAX),
        ];

        for config in configs {
            let resolver = LocalModuleResolver::new();
            let result = VMHarness::with_config(&resolver, true, config);
            assert!(result.is_ok(), "should create harness with any epoch");
        }
    }

    #[test]
    fn test_gas_budget_boundaries() {
        let configs = [
            SimulationConfig::default().with_gas_budget(None),
            SimulationConfig::default().with_gas_budget(Some(0)),
            SimulationConfig::default().with_gas_budget(Some(1)),
            SimulationConfig::default().with_gas_budget(Some(u64::MAX / 2)),
            SimulationConfig::default().with_gas_budget(Some(u64::MAX)),
        ];

        for config in configs {
            let resolver = LocalModuleResolver::new();
            let result = VMHarness::with_config(&resolver, true, config);
            assert!(result.is_ok(), "should create harness with any gas budget");
        }
    }

    #[test]
    fn test_clock_base_boundaries() {
        for base in [0u64, 1, u64::MAX / 2, u64::MAX - 10000] {
            let clock = MockClock::with_base(base);
            let ts = clock.timestamp_ms();
            assert!(ts >= base, "timestamp should be at least base");
        }
    }
}

// =============================================================================
// Malformed Input Tests
// =============================================================================

mod malformed_input_tests {
    use super::*;

    #[test]
    fn test_invalid_bytecode() {
        let mut resolver = LocalModuleResolver::new();

        // Random bytes that aren't valid bytecode
        let invalid_bytecode = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03];
        let result = resolver.add_module_bytes(invalid_bytecode);

        assert!(result.is_err());
    }

    #[test]
    fn test_truncated_bytecode() {
        let mut resolver = LocalModuleResolver::new();

        // A truncated module (just a few bytes)
        let truncated = vec![0x00];
        let result = resolver.add_module_bytes(truncated);

        assert!(result.is_err());
    }

    #[test]
    fn test_bcs_wrong_layout() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        // Serialize a u64 but try to deserialize as bool
        let u64_bytes = 42u64.to_le_bytes().to_vec();
        let bool_layout = MoveTypeLayout::Bool;

        // This should either fail or produce different bytes on roundtrip
        let result = validator.validate_bcs_roundtrip(&bool_layout, &u64_bytes);
        // Either error or mismatch is acceptable
        let _ = result;
    }

    #[test]
    fn test_bcs_short_bytes() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        // u64 needs 8 bytes, provide less
        for len in 0..8 {
            let bytes = vec![0u8; len];
            let result = validator.validate_bcs_roundtrip(&MoveTypeLayout::U64, &bytes);
            assert!(result.is_err(), "length {} should fail for u64", len);
        }
    }

    #[test]
    fn test_bcs_invalid_bool() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        // Bool must be 0 or 1
        for invalid in [2u8, 3, 127, 255] {
            let result = validator.validate_bcs_roundtrip(&MoveTypeLayout::Bool, &[invalid]);
            assert!(result.is_err(), "bool value {} should fail", invalid);
        }
    }

    #[test]
    fn test_bcs_truncated_vector() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U64));
        // Length says 5 elements (0x05), but no data follows
        let bytes = vec![5u8];

        let result = validator.validate_bcs_roundtrip(&layout, &bytes);
        assert!(result.is_err(), "truncated vector should fail");
    }

    #[test]
    fn test_invalid_address_format() {
        // Invalid hex string
        let result = AccountAddress::from_hex_literal("not_hex");
        assert!(result.is_err());

        // Too short
        let result = AccountAddress::from_hex_literal("0x1234");
        assert!(result.is_ok()); // Actually this is valid (pads with zeros)

        // Invalid characters
        let result = AccountAddress::from_hex_literal("0xGGGG");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_identifier() {
        // Empty identifier
        let result = Identifier::new("");
        assert!(result.is_err());

        // Identifier starting with number
        let result = Identifier::new("123abc");
        assert!(result.is_err());

        // Valid identifiers
        assert!(Identifier::new("valid").is_ok());
        assert!(Identifier::new("_valid").is_ok());
        assert!(Identifier::new("valid123").is_ok());
    }
}

// =============================================================================
// Large Input Tests
// =============================================================================

mod large_input_tests {
    use super::*;

    #[test]
    fn test_large_vector() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8));

        // Create a large vector
        let value = MoveValue::Vector((0..10000).map(|i| MoveValue::U8(i as u8)).collect());
        let bytes = value.simple_serialize().unwrap();

        let result = validator.validate_bcs_roundtrip(&layout, &bytes);
        assert!(result.is_ok(), "large vector should roundtrip");
    }

    #[test]
    fn test_deeply_nested_vector() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        // vector<vector<vector<u8>>>
        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::Vector(Box::new(
            MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8)),
        ))));

        let value = MoveValue::Vector(vec![MoveValue::Vector(vec![MoveValue::Vector(vec![
            MoveValue::U8(42),
        ])])]);
        let bytes = value.simple_serialize().unwrap();

        let result = validator.validate_bcs_roundtrip(&layout, &bytes);
        assert!(result.is_ok(), "nested vector should roundtrip");
    }

    #[test]
    fn test_many_type_resolutions() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        // Resolve many primitive types
        let types = vec![
            TypeTag::Bool,
            TypeTag::U8,
            TypeTag::U16,
            TypeTag::U32,
            TypeTag::U64,
            TypeTag::U128,
            TypeTag::U256,
            TypeTag::Address,
            TypeTag::Signer,
            TypeTag::Vector(Box::new(TypeTag::U8)),
            TypeTag::Vector(Box::new(TypeTag::Address)),
            TypeTag::Vector(Box::new(TypeTag::Vector(Box::new(TypeTag::U64)))),
        ];

        for type_tag in &types {
            let result = validator.resolve_type_layout(type_tag);
            assert!(result.is_ok(), "should resolve {:?}", type_tag);
        }
    }
}

// =============================================================================
// Concurrent Access Tests
// =============================================================================

mod concurrent_access_tests {
    use super::*;

    #[test]
    fn test_concurrent_mock_clock() {
        let clock = Arc::new(MockClock::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let clock_clone = clock.clone();
            handles.push(thread::spawn(move || {
                let mut timestamps = vec![];
                for _ in 0..1000 {
                    timestamps.push(clock_clone.timestamp_ms());
                }
                timestamps
            }));
        }

        let mut all_timestamps = vec![];
        for handle in handles {
            let ts = handle.join().unwrap();
            all_timestamps.extend(ts);
        }

        // All timestamps should be unique (no races)
        all_timestamps.sort();
        for i in 1..all_timestamps.len() {
            assert!(
                all_timestamps[i] > all_timestamps[i - 1],
                "timestamps should be strictly increasing"
            );
        }
    }

    #[test]
    fn test_concurrent_mock_random() {
        let random = Arc::new(MockRandom::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let random_clone = random.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    let _ = random_clone.next_bytes(32);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
        // Just verify no panics
    }

    #[test]
    fn test_concurrent_fresh_id() {
        let state = Arc::new(MockNativeState::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let state_clone = state.clone();
            handles.push(thread::spawn(move || {
                let mut ids = vec![];
                for _ in 0..1000 {
                    ids.push(state_clone.fresh_id());
                }
                ids
            }));
        }

        let mut all_ids = vec![];
        for handle in handles {
            all_ids.extend(handle.join().unwrap());
        }

        // All IDs should be unique
        all_ids.sort();
        all_ids.dedup();
        assert_eq!(all_ids.len(), 10000, "all IDs should be unique");
    }
}

// =============================================================================
// Error Message Quality Tests
// =============================================================================

mod error_quality_tests {
    use super::*;

    #[test]
    fn test_module_not_found_error_contains_context() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let result = validator.validate_target(
            AccountAddress::from_hex_literal("0x9999").unwrap(),
            "totally_fake_module",
            "some_func",
        );

        let err = result.unwrap_err().to_string();
        // Error should help user identify what went wrong
        assert!(
            err.to_lowercase().contains("module")
                || err.to_lowercase().contains("not found")
                || err.contains("9999")
                || err.contains("totally_fake_module"),
            "error should be helpful: {err}"
        );
    }

    #[test]
    fn test_function_not_found_error_contains_context() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .unwrap();

        let result = validator.validate_target(
            *module.self_id().address(),
            "test_module",
            "nonexistent_function_name_xyz",
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("function")
                || err.to_lowercase().contains("not found")
                || err.contains("nonexistent_function_name_xyz"),
            "error should be helpful: {err}"
        );
    }

    #[test]
    fn test_execution_error_is_informative() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).unwrap();

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .unwrap();

        // Wrong args
        let result = harness.execute_function(
            &module.self_id(),
            "simple_func",
            vec![],
            vec![vec![1, 2]], // Wrong size for u64
        );

        let err = result.unwrap_err().to_string();
        // Error should be informative
        assert!(err.len() > 5, "error should have content: {err}");
    }

    #[test]
    fn test_bcs_error_is_informative() {
        let resolver = LocalModuleResolver::new();
        let validator = Validator::new(&resolver);

        let result = validator.validate_bcs_roundtrip(&MoveTypeLayout::U64, &[1, 2, 3]);

        let err = result.unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("bcs")
                || err.to_lowercase().contains("deserialize")
                || err.to_lowercase().contains("failed"),
            "error should mention BCS: {err}"
        );
    }
}

// =============================================================================
// Special Character and Unicode Tests
// =============================================================================

mod special_character_tests {
    use super::*;

    #[test]
    fn test_address_various_formats() {
        // These should all work
        assert!(AccountAddress::from_hex_literal("0x0").is_ok());
        assert!(AccountAddress::from_hex_literal("0x1").is_ok());
        assert!(AccountAddress::from_hex_literal("0x123").is_ok());
        assert!(AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        )
        .is_ok());

        // With or without 0x prefix - from_hex_literal requires it
        assert!(AccountAddress::from_hex_literal("0xabcdef").is_ok());
    }

    #[test]
    fn test_resolver_search_special_patterns() {
        let resolver = load_fixture_resolver();

        // Empty pattern
        let results = resolver.search_functions("", false);
        // Should return all functions
        assert!(!results.is_empty());

        // Wildcard pattern
        let results = resolver.search_functions("*", false);
        assert!(!results.is_empty());

        // Partial wildcard
        let results = resolver.search_functions("*simple*", false);
        // May or may not find results depending on what's loaded
        let _ = results;
    }
}

// =============================================================================
// Helper
// =============================================================================

fn load_fixture_resolver() -> LocalModuleResolver {
    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("fixture should load");
    resolver
}
