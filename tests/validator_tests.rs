//! Comprehensive tests for validator.rs - BCS validation, type resolution, error handling
//!
//! Test coverage areas:
//! - validate_target: module resolution, function resolution, visibility checks
//! - resolve_type_layout: primitive types, vectors, structs, nested types
//! - resolve_token_to_tag: all signature token variants
//! - validate_bcs_roundtrip: valid/invalid inputs, edge cases, malformed data
//! - Error propagation: actionable error messages

mod common;

use common::{empty_resolver, find_test_module, load_fixture_resolver};
use move_core_types::account_address::AccountAddress;
use move_core_types::annotated_value::{MoveTypeLayout, MoveValue};
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use move_core_types::u256::U256;
use sui_sandbox_core::validator::Validator;

// =============================================================================
// validate_target Tests
// =============================================================================

mod validate_target_tests {
    use super::*;

    #[test]
    fn test_validate_existing_module_and_function() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let module = find_test_module(&resolver).expect("test_module should exist");

        let result =
            validator.validate_target(*module.self_id().address(), "test_module", "simple_func");

        result.expect("should validate existing public function");
    }

    #[test]
    fn test_validate_nonexistent_module() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let result = validator.validate_target(
            AccountAddress::from_hex_literal(
                "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
            "nonexistent_module",
            "any_function",
        );

        let err = result
            .expect_err("should fail for nonexistent module")
            .to_string();
        assert!(
            err.contains("module not found") || err.contains("nonexistent"),
            "error should mention module not found: {err}"
        );
    }

    #[test]
    fn test_validate_nonexistent_function() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let module = resolver.iter_modules().next().expect("should have module");
        let module_name = sui_sandbox::bytecode::compiled_module_name(module);

        let result = validator.validate_target(
            *module.self_id().address(),
            &module_name,
            "nonexistent_function_xyz",
        );

        let err = result
            .expect_err("should fail for nonexistent function")
            .to_string();
        assert!(
            err.contains("function not found") || err.contains("nonexistent_function_xyz"),
            "error should mention function not found: {err}"
        );
    }

    #[test]
    fn test_validate_with_empty_resolver() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let result = validator.validate_target(AccountAddress::ZERO, "any_module", "any_function");

        result.expect_err("should fail with empty resolver");
    }

    #[test]
    fn test_validate_with_invalid_function_name() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let module = resolver.iter_modules().next().expect("should have module");
        let module_name = sui_sandbox::bytecode::compiled_module_name(module);

        // Empty function name should fail during Identifier creation
        let result = validator.validate_target(
            *module.self_id().address(),
            &module_name,
            "", // Empty function name
        );

        result.expect_err("should fail with empty function name");
    }
}

// =============================================================================
// resolve_type_layout Tests - Primitive Types
// =============================================================================

mod resolve_type_layout_primitives {
    use super::*;

    #[test]
    fn test_resolve_bool() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::Bool)
            .expect("should resolve Bool type");
        assert!(matches!(layout, MoveTypeLayout::Bool));
    }

    #[test]
    fn test_resolve_u8() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::U8)
            .expect("should resolve U8 type");
        assert!(matches!(layout, MoveTypeLayout::U8));
    }

    #[test]
    fn test_resolve_u16() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::U16)
            .expect("should resolve U16 type");
        assert!(matches!(layout, MoveTypeLayout::U16));
    }

    #[test]
    fn test_resolve_u32() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::U32)
            .expect("should resolve U32 type");
        assert!(matches!(layout, MoveTypeLayout::U32));
    }

    #[test]
    fn test_resolve_u64() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::U64)
            .expect("should resolve U64 type");
        assert!(matches!(layout, MoveTypeLayout::U64));
    }

    #[test]
    fn test_resolve_u128() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::U128)
            .expect("should resolve U128 type");
        assert!(matches!(layout, MoveTypeLayout::U128));
    }

    #[test]
    fn test_resolve_u256() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::U256)
            .expect("should resolve U256 type");
        assert!(matches!(layout, MoveTypeLayout::U256));
    }

    #[test]
    fn test_resolve_address() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::Address)
            .expect("should resolve Address type");
        assert!(matches!(layout, MoveTypeLayout::Address));
    }

    #[test]
    fn test_resolve_signer() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = validator
            .resolve_type_layout(&TypeTag::Signer)
            .expect("should resolve Signer type");
        assert!(matches!(layout, MoveTypeLayout::Signer));
    }
}

// =============================================================================
// resolve_type_layout Tests - Vectors
// =============================================================================

mod resolve_type_layout_vectors {
    use super::*;

    #[test]
    fn test_resolve_vector_u8() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let tag = TypeTag::Vector(Box::new(TypeTag::U8));
        let layout = validator
            .resolve_type_layout(&tag)
            .expect("should resolve vector<u8> type");

        match layout {
            MoveTypeLayout::Vector(inner) => {
                assert!(matches!(*inner, MoveTypeLayout::U8));
            }
            _ => panic!("expected Vector layout"),
        }
    }

    #[test]
    fn test_resolve_vector_address() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let tag = TypeTag::Vector(Box::new(TypeTag::Address));
        let layout = validator
            .resolve_type_layout(&tag)
            .expect("should resolve vector<address> type");

        match layout {
            MoveTypeLayout::Vector(inner) => {
                assert!(matches!(*inner, MoveTypeLayout::Address));
            }
            _ => panic!("expected Vector layout"),
        }
    }

    #[test]
    fn test_resolve_nested_vector() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        // vector<vector<u64>>
        let tag = TypeTag::Vector(Box::new(TypeTag::Vector(Box::new(TypeTag::U64))));
        let layout = validator
            .resolve_type_layout(&tag)
            .expect("should resolve vector<vector<u64>> type");

        match layout {
            MoveTypeLayout::Vector(inner) => match *inner {
                MoveTypeLayout::Vector(inner2) => {
                    assert!(matches!(*inner2, MoveTypeLayout::U64));
                }
                _ => panic!("expected nested Vector"),
            },
            _ => panic!("expected Vector layout"),
        }
    }

    #[test]
    fn test_resolve_deeply_nested_vector() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        // vector<vector<vector<bool>>>
        let tag = TypeTag::Vector(Box::new(TypeTag::Vector(Box::new(TypeTag::Vector(
            Box::new(TypeTag::Bool),
        )))));

        validator
            .resolve_type_layout(&tag)
            .expect("should handle deeply nested vectors");
    }
}

// =============================================================================
// resolve_type_layout Tests - Structs
// =============================================================================

mod resolve_type_layout_structs {
    use super::*;

    #[test]
    fn test_resolve_nonexistent_struct() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let struct_tag = StructTag {
            address: AccountAddress::from_hex_literal(
                "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
            module: Identifier::new("nonexistent").unwrap(),
            name: Identifier::new("SomeStruct").unwrap(),
            type_params: vec![],
        };

        let result = validator.resolve_type_layout(&TypeTag::Struct(Box::new(struct_tag)));

        let err = result
            .expect_err("should fail for nonexistent struct")
            .to_string();
        assert!(
            err.contains("module not found") || err.contains("nonexistent"),
            "error should mention module not found: {err}"
        );
    }

    #[test]
    fn test_resolve_struct_with_generic_params() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        // Try to resolve a struct type with type parameters
        // This will fail if the module doesn't exist, which is expected for this test
        let struct_tag = StructTag {
            address: AccountAddress::ZERO,
            module: Identifier::new("test").unwrap(),
            name: Identifier::new("Generic").unwrap(),
            type_params: vec![TypeTag::U64],
        };

        // We expect this to fail since the struct doesn't exist
        validator
            .resolve_type_layout(&TypeTag::Struct(Box::new(struct_tag)))
            .expect_err("should fail for nonexistent generic struct");
    }
}

// =============================================================================
// validate_bcs_roundtrip Tests - Valid Cases
// =============================================================================

mod validate_bcs_roundtrip_valid {
    use super::*;

    #[test]
    fn test_roundtrip_bool_true() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Bool;
        let value = MoveValue::Bool(true);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("bool true should roundtrip");
    }

    #[test]
    fn test_roundtrip_bool_false() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Bool;
        let value = MoveValue::Bool(false);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("bool false should roundtrip");
    }

    #[test]
    fn test_roundtrip_u8_zero() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U8;
        let bytes = vec![0u8];

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u8 zero should roundtrip");
    }

    #[test]
    fn test_roundtrip_u8_max() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U8;
        let bytes = vec![255u8];

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u8 max should roundtrip");
    }

    #[test]
    fn test_roundtrip_u64_zero() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;
        let bytes = 0u64.to_le_bytes().to_vec();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u64 zero should roundtrip");
    }

    #[test]
    fn test_roundtrip_u64_max() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;
        let bytes = u64::MAX.to_le_bytes().to_vec();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u64 max should roundtrip");
    }

    #[test]
    fn test_roundtrip_u128_max() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U128;
        let bytes = u128::MAX.to_le_bytes().to_vec();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u128 max should roundtrip");
    }

    #[test]
    fn test_roundtrip_u256_zero() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U256;
        let value = MoveValue::U256(U256::zero());
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u256 zero should roundtrip");
    }

    #[test]
    fn test_roundtrip_address_zero() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Address;
        let value = MoveValue::Address(AccountAddress::ZERO);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("address zero should roundtrip");
    }

    #[test]
    fn test_roundtrip_empty_vector() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8));
        let value = MoveValue::Vector(vec![]);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("empty vector should roundtrip");
    }

    #[test]
    fn test_roundtrip_vector_with_elements() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U64));
        let value = MoveValue::Vector(vec![
            MoveValue::U64(1),
            MoveValue::U64(2),
            MoveValue::U64(3),
        ]);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("vector with elements should roundtrip");
    }

    #[test]
    fn test_roundtrip_vector_addresses() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::Address));
        let value = MoveValue::Vector(vec![
            MoveValue::Address(AccountAddress::ZERO),
            MoveValue::Address(AccountAddress::ONE),
        ]);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("vector of addresses should roundtrip");
    }

    #[test]
    fn test_roundtrip_nested_vector() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::Vector(Box::new(
            MoveTypeLayout::U8,
        ))));
        let value = MoveValue::Vector(vec![
            MoveValue::Vector(vec![MoveValue::U8(1), MoveValue::U8(2)]),
            MoveValue::Vector(vec![MoveValue::U8(3)]),
        ]);
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("nested vector should roundtrip");
    }
}

// =============================================================================
// validate_bcs_roundtrip Tests - Invalid/Edge Cases
// =============================================================================

mod validate_bcs_roundtrip_invalid {
    use super::*;

    #[test]
    fn test_roundtrip_empty_bytes_for_u64() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;
        let bytes: Vec<u8> = vec![];

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect_err("empty bytes should fail for u64");
    }

    #[test]
    fn test_roundtrip_short_bytes_for_u64() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;
        let bytes = vec![1u8, 2u8, 3u8]; // Only 3 bytes, need 8

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect_err("short bytes should fail for u64");
    }

    #[test]
    fn test_roundtrip_extra_bytes_for_bool() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Bool;
        let bytes = vec![0u8, 1u8]; // Extra byte after bool

        // BCS deserialization may succeed but roundtrip may differ
        // depending on implementation - just ensure no panic
        let _ = validator.validate_bcs_roundtrip(&layout, &bytes);
    }

    #[test]
    fn test_roundtrip_invalid_bool_value() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Bool;
        let bytes = vec![2u8]; // Invalid bool (not 0 or 1)

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect_err("invalid bool value should fail");
    }

    #[test]
    fn test_roundtrip_short_address() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Address;
        let bytes = vec![0u8; 16]; // Only 16 bytes, need 32

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect_err("short address should fail");
    }

    #[test]
    fn test_roundtrip_malformed_vector_length() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U64));
        // Malformed ULEB128 length prefix indicating more data than present
        let bytes = vec![0xFF, 0xFF, 0xFF, 0x0F]; // Very large length

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect_err("malformed vector should fail");
    }

    #[test]
    fn test_roundtrip_truncated_vector() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U64));
        // Length says 2 elements, but only partial data provided
        let bytes = vec![2u8, 0, 0, 0, 0, 0, 0, 0, 0]; // Length 2, but only 1 element

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect_err("truncated vector should fail");
    }
}

// =============================================================================
// validate_bcs_roundtrip Tests - Boundary Conditions
// =============================================================================

mod validate_bcs_roundtrip_boundaries {
    use super::*;

    #[test]
    fn test_roundtrip_u16_min() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U16;
        let bytes = 0u16.to_le_bytes().to_vec();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u16 min should roundtrip");
    }

    #[test]
    fn test_roundtrip_u16_max() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U16;
        let bytes = u16::MAX.to_le_bytes().to_vec();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u16 max should roundtrip");
    }

    #[test]
    fn test_roundtrip_u32_max() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U32;
        let bytes = u32::MAX.to_le_bytes().to_vec();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("u32 max should roundtrip");
    }

    #[test]
    fn test_roundtrip_large_vector() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::Vector(Box::new(MoveTypeLayout::U8));
        // Create a vector with 1000 elements
        let value = MoveValue::Vector((0..1000).map(|i| MoveValue::U8(i as u8)).collect());
        let bytes = value.simple_serialize().unwrap();

        validator
            .validate_bcs_roundtrip(&layout, &bytes)
            .expect("large vector should roundtrip");
    }
}

// =============================================================================
// Error Message Quality Tests
// =============================================================================

mod error_message_tests {
    use super::*;

    #[test]
    fn test_module_not_found_error_is_actionable() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let result = validator.validate_target(
            AccountAddress::from_hex_literal("0x1234").unwrap(),
            "nonexistent_module",
            "any_func",
        );

        let err = result.unwrap_err().to_string();
        // Error should contain the module name for debugging
        assert!(
            err.contains("nonexistent_module") || err.contains("module not found"),
            "error should be actionable: {err}"
        );
    }

    #[test]
    fn test_function_not_found_error_is_actionable() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        let module = resolver.iter_modules().next().expect("should have module");
        let module_name = sui_sandbox::bytecode::compiled_module_name(module);

        let result = validator.validate_target(
            *module.self_id().address(),
            &module_name,
            "definitely_not_a_function",
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("definitely_not_a_function") || err.contains("function not found"),
            "error should mention the function: {err}"
        );
    }

    #[test]
    fn test_bcs_error_is_actionable() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        let layout = MoveTypeLayout::U64;
        let bytes = vec![1u8, 2u8]; // Wrong length

        let result = validator.validate_bcs_roundtrip(&layout, &bytes);
        let err = result.unwrap_err().to_string();

        // Error should mention BCS or deserialization
        assert!(
            err.contains("BCS") || err.contains("deserialize") || err.contains("failed"),
            "error should be actionable: {err}"
        );
    }
}

// =============================================================================
// Struct Layout Tests with Real Fixture
// =============================================================================

mod struct_layout_with_fixture {
    use super::*;

    #[test]
    fn test_resolve_fixture_struct_layout() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        // Find the fixture module address
        let module = find_test_module(&resolver).expect("test_module should exist");

        let struct_tag = StructTag {
            address: *module.self_id().address(),
            module: Identifier::new("test_module").unwrap(),
            name: Identifier::new("SimpleStruct").unwrap(),
            type_params: vec![],
        };

        let layout = validator
            .resolve_type_layout(&TypeTag::Struct(Box::new(struct_tag)))
            .expect("should resolve SimpleStruct layout");

        // Verify it's a struct layout
        assert!(
            matches!(layout, MoveTypeLayout::Struct(_)),
            "should be a struct layout"
        );
    }
}
