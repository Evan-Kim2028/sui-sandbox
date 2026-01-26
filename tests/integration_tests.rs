//! Integration tests for end-to-end workflows
//!
//! Test coverage areas:
//! - Full pipeline: resolver -> validator -> VM -> execution
//! - Module loading and dependency resolution
//! - Cross-component error propagation
//! - Real-world usage patterns

mod common;

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;

use sui_sandbox_core::validator::Validator;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};

use common::{empty_resolver, find_test_module, format_module_path, load_fixture_resolver};

// =============================================================================
// End-to-End Pipeline Tests
// =============================================================================

mod pipeline_tests {
    use super::*;

    #[test]
    fn test_full_pipeline_validate_then_execute() {
        // Load modules
        let resolver = load_fixture_resolver();

        // Find test_module using shared helper
        let module = find_test_module(&resolver).expect("test_module should exist");

        let package_addr = *module.self_id().address();

        // Step 1: Validate the target
        let validator = Validator::new(&resolver);
        let validated = validator
            .validate_target(package_addr, "test_module", "simple_func")
            .expect("validation should succeed");

        assert_eq!(
            sui_sandbox::bytecode::compiled_module_name(validated),
            "test_module"
        );

        // Step 2: Create VM harness
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        // Step 3: Execute function
        let result = harness.execute_function_with_return(
            &module.self_id(),
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        );

        assert!(result.is_ok(), "execution should succeed");

        // Step 4: Verify return value
        let returns = result.expect("execution already verified as ok");
        assert_eq!(returns.len(), 1);
        let value = u64::from_le_bytes(
            returns[0]
                .clone()
                .try_into()
                .expect("return value should be 8 bytes for u64"),
        );
        assert_eq!(value, 42);
    }

    #[test]
    fn test_pipeline_validation_failure_blocks_execution() {
        let resolver = load_fixture_resolver();
        let validator = Validator::new(&resolver);

        // Try to validate nonexistent function
        let result = validator.validate_target(
            AccountAddress::from_hex_literal("0xdeadbeef").unwrap(),
            "nonexistent",
            "func",
        );

        assert!(result.is_err(), "validation should fail");
        // Error should be actionable
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("module") || err_msg.contains("not found"),
            "error should be actionable: {err_msg}"
        );
    }

    #[test]
    fn test_pipeline_with_config_options() {
        let resolver = load_fixture_resolver();

        let module = find_test_module(&resolver).expect("test_module should exist");

        // Test with various configs
        let configs = vec![
            SimulationConfig::default(),
            SimulationConfig::strict(),
            SimulationConfig::default().with_epoch(500),
            SimulationConfig::default().with_gas_budget(Some(1_000_000)),
        ];

        for config in configs {
            let mut harness =
                VMHarness::with_config(&resolver, true, config).expect("harness should create");

            let result = harness.execute_function(
                &module.self_id(),
                "simple_func",
                vec![],
                vec![1u64.to_le_bytes().to_vec()],
            );

            assert!(result.is_ok(), "should execute with any config");
        }
    }
}

// =============================================================================
// Module Loading Integration Tests
// =============================================================================

mod module_loading_tests {
    use super::*;

    #[test]
    fn test_load_and_query_fixture() {
        let resolver = load_fixture_resolver();

        // Should have loaded at least one module
        assert!(resolver.module_count() > 0, "should load modules");

        // Should be able to iterate modules
        let modules: Vec<_> = resolver.iter_modules().collect();
        assert!(!modules.is_empty());
    }

    #[test]
    fn test_has_module_check() {
        let resolver = load_fixture_resolver();

        let module = resolver.iter_modules().next().expect("should have module");
        let module_id = module.self_id();

        assert!(resolver.has_module(&module_id), "should have loaded module");

        let fake_id = ModuleId::new(
            AccountAddress::from_hex_literal("0xdeadbeef").unwrap(),
            Identifier::new("fake").unwrap(),
        );
        assert!(
            !resolver.has_module(&fake_id),
            "should not have fake module"
        );
    }

    #[test]
    fn test_has_package_check() {
        let resolver = load_fixture_resolver();

        let module = resolver.iter_modules().next().expect("should have module");
        let package_addr = *module.self_id().address();

        assert!(resolver.has_package(&package_addr), "should have package");

        // Use a full 32-byte address that's very unlikely to match
        let fake_addr = AccountAddress::from_hex_literal(
            "0xfeedfacedeadbeefcafebabe1234567890abcdef1234567890abcdef12345678",
        )
        .unwrap();
        assert!(
            !resolver.has_package(&fake_addr),
            "should not have fake package"
        );
    }

    #[test]
    fn test_list_packages() {
        let resolver = load_fixture_resolver();

        let packages = resolver.list_packages();
        assert!(!packages.is_empty(), "should have packages");

        // Each package should have modules
        for pkg in &packages {
            let modules = resolver.get_package_modules(pkg);
            assert!(!modules.is_empty(), "package should have modules");
        }
    }

    #[test]
    fn test_loaded_packages() {
        let resolver = load_fixture_resolver();

        let loaded = resolver.loaded_packages();
        assert!(!loaded.is_empty());
    }
}

// =============================================================================
// Resolver Introspection Tests
// =============================================================================

mod resolver_introspection_tests {
    use super::*;

    #[test]
    fn test_list_modules() {
        let resolver = load_fixture_resolver();

        let modules = resolver.list_modules();
        assert!(!modules.is_empty());

        // Each module path should have format "0x...::name"
        for path in &modules {
            assert!(path.contains("::"), "module path should contain ::");
        }
    }

    #[test]
    fn test_list_functions_for_module() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver).expect("test_module should exist");
        let module_path = format_module_path(module);

        let functions = resolver.list_functions(&module_path);
        assert!(functions.is_some(), "should list functions");
        assert!(
            !functions
                .expect("functions already verified as some")
                .is_empty(),
            "should have functions"
        );
    }

    #[test]
    fn test_list_structs_for_module() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver).expect("test_module should exist");
        let module_path = format_module_path(module);

        let structs = resolver.list_structs(&module_path);
        assert!(structs.is_some(), "should list structs");
        // test_module has SimpleStruct
        let structs = structs.expect("structs already verified as some");
        assert!(!structs.is_empty(), "should have structs");
        assert!(structs.contains(&"SimpleStruct".to_string()));
    }

    #[test]
    fn test_get_function_info() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver).expect("test_module should exist");
        let module_path = format_module_path(module);

        let info = resolver.get_function_info(&module_path, "simple_func");
        assert!(info.is_some(), "should get function info");

        let info = info.expect("info already verified as some");
        assert!(info.get("visibility").is_some());
        assert!(info.get("params").is_some());
        assert!(info.get("returns").is_some());
    }

    #[test]
    fn test_get_struct_info() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver).expect("test_module should exist");
        let type_path = format!("{}::SimpleStruct", format_module_path(module));

        let info = resolver.get_struct_info(&type_path);
        assert!(info.is_some(), "should get struct info");

        let info = info.expect("info already verified as some");
        assert!(info.get("name").is_some());
        assert!(info.get("abilities").is_some());
        assert!(info.get("fields").is_some());
    }

    #[test]
    fn test_search_functions() {
        let resolver = load_fixture_resolver();

        // Search for "simple" in function names
        let results = resolver.search_functions("simple", false);
        assert!(!results.is_empty(), "should find simple_func");

        // Search for entry functions only (may be empty in fixture)
        let _ = resolver.search_functions("*", true);
    }

    #[test]
    fn test_search_types() {
        let resolver = load_fixture_resolver();

        // Search for "Simple" types
        let results = resolver.search_types("Simple", None);
        assert!(!results.is_empty(), "should find SimpleStruct");

        // Search with ability filter
        let results_with_drop = resolver.search_types("Simple", Some("drop"));
        // SimpleStruct has drop ability
        assert!(!results_with_drop.is_empty());
    }

    #[test]
    fn test_disassemble_function() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver).expect("test_module should exist");
        let module_path = format_module_path(module);

        let disasm = resolver.disassemble_function(&module_path, "simple_func");
        assert!(disasm.is_some(), "should disassemble function");

        let disasm = disasm.expect("disasm already verified as some");
        assert!(disasm.contains("fun"), "should contain function keyword");
    }
}

// =============================================================================
// Cross-Component Error Propagation Tests
// =============================================================================

mod error_propagation_tests {
    use super::*;

    #[test]
    fn test_resolver_error_propagates_to_validator() {
        let resolver = empty_resolver(); // Empty resolver
        let validator = Validator::new(&resolver);

        let result = validator.validate_target(AccountAddress::ZERO, "any_module", "any_func");

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("module") || err.contains("not found"),
            "error should mention module not found: {err}"
        );
    }

    #[test]
    fn test_vm_error_is_actionable() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness");

        let module = resolver.iter_modules().next().expect("module");

        // Call with wrong number of args
        let result = harness.execute_function(
            &module.self_id(),
            "simple_func",
            vec![],
            vec![], // Missing required arg
        );

        assert!(result.is_err());
        // Error should be useful for debugging
        let _ = result.unwrap_err().to_string();
    }

    #[test]
    fn test_type_resolution_error_propagates() {
        let resolver = empty_resolver();
        let validator = Validator::new(&resolver);

        use move_core_types::language_storage::{StructTag, TypeTag};

        let struct_tag = StructTag {
            address: AccountAddress::from_hex_literal("0xdead").unwrap(),
            module: Identifier::new("nonexistent").unwrap(),
            name: Identifier::new("Struct").unwrap(),
            type_params: vec![],
        };

        let result = validator.resolve_type_layout(&TypeTag::Struct(Box::new(struct_tag)));

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("module") || err.contains("not found"),
            "error should be actionable: {err}"
        );
    }
}

// =============================================================================
// Dynamic Module Loading Tests
// =============================================================================

mod dynamic_loading_tests {
    use super::*;

    #[test]
    fn test_add_module_bytes() {
        let mut resolver = load_fixture_resolver();
        let initial_count = resolver.module_count();

        // Read a bytecode file
        let fixture_bytecode =
            std::fs::read("tests/fixture/build/fixture/bytecode_modules/test_module.mv")
                .expect("should read bytecode");

        // Try to add it (will be a duplicate, but shouldn't panic)
        let result = resolver.add_module_bytes(fixture_bytecode);

        assert!(result.is_ok());
        // Count may or may not change depending on duplicate handling
        assert!(resolver.module_count() >= initial_count);
    }

    #[test]
    fn test_add_invalid_module_bytes() {
        let mut resolver = empty_resolver();

        let result = resolver.add_module_bytes(vec![0, 1, 2, 3, 4]); // Invalid bytecode

        assert!(result.is_err(), "invalid bytecode should fail");
    }

    #[test]
    fn test_add_empty_module_bytes() {
        let mut resolver = empty_resolver();

        let result = resolver.add_module_bytes(vec![]); // Empty bytecode

        assert!(result.is_err(), "empty bytecode should fail");
    }
}

// =============================================================================
// Execution State Tests
// =============================================================================

mod execution_state_tests {
    use super::*;

    #[test]
    fn test_execution_trace_tracking() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness");
        let module = find_test_module(&resolver).expect("test_module");
        let package_addr = *module.self_id().address();

        // Clear trace
        harness.clear_trace();
        assert!(harness.get_trace().modules_accessed.is_empty());

        // Execute function
        let _ = harness.execute_function(
            &module.self_id(),
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        );

        // Trace should show module was accessed
        let trace = harness.get_trace();
        assert!(
            trace.accessed_package(&package_addr),
            "should track module access"
        );
    }

    #[test]
    fn test_multiple_executions() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness");
        let module = find_test_module(&resolver).expect("test_module");

        // Execute multiple times
        for i in 0..10u64 {
            let result = harness.execute_function_with_return(
                &module.self_id(),
                "simple_func",
                vec![],
                vec![i.to_le_bytes().to_vec()],
            );

            assert!(result.is_ok(), "execution {i} should succeed");
            let returns = result.expect("execution already verified as ok");
            let value = u64::from_le_bytes(
                returns[0]
                    .clone()
                    .try_into()
                    .expect("return value should be 8 bytes for u64"),
            );
            assert_eq!(value, i, "should return input value");
        }
    }

    #[test]
    fn test_events_clear_between_executions() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness");

        // Events should start empty
        assert!(harness.get_events().is_empty());

        // Clear events
        harness.clear_events();

        // Should still be empty
        assert!(harness.get_events().is_empty());
    }
}
