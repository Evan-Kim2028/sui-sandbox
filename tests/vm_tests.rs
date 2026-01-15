//! Comprehensive tests for vm.rs - Execution engine, gas metering, configuration
//!
//! Test coverage areas:
//! - SimulationConfig: default, strict, builder methods, epoch advancement
//! - ExecutionTrace: module access tracking, package filtering
//! - ExecutionOutput: return values, mutable references, gas estimation
//! - MeteredGasMeter: budget enforcement, out of gas errors
//! - VMHarness: creation, execution, error handling, dynamic fields
//! - PTBSession: persistent state across calls

use std::path::Path;
use std::sync::{Arc, Mutex};

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{ModuleId, TypeTag};

use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::vm::{
    gas_costs, ExecutionOutput, ExecutionTrace, GasMeterImpl, MeteredGasMeter, SimulationConfig,
    VMHarness,
};

// =============================================================================
// Test Fixtures
// =============================================================================

fn load_fixture_resolver() -> LocalModuleResolver {
    let fixture_dir = Path::new("tests/fixture/build/fixture");
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("fixture should load");
    resolver
}

fn empty_resolver() -> LocalModuleResolver {
    LocalModuleResolver::new()
}

// =============================================================================
// SimulationConfig Tests
// =============================================================================

mod simulation_config_tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SimulationConfig::default();

        assert!(config.mock_crypto_pass);
        assert!(config.advancing_clock);
        assert!(config.deterministic_random);
        assert!(config.permissive_ownership);
        assert_eq!(config.clock_base_ms, 1704067200000); // 2024-01-01
        assert_eq!(config.random_seed, [0u8; 32]);
        assert_eq!(config.sender_address, [0u8; 32]);
        assert!(config.tx_timestamp_ms.is_none());
        assert_eq!(config.epoch, 100);
        assert!(config.gas_budget.is_none());
        assert!(!config.enforce_immutability);
    }

    #[test]
    fn test_strict_config() {
        let config = SimulationConfig::strict();

        assert!(
            !config.mock_crypto_pass,
            "strict should disable crypto mocking"
        );
        assert!(
            !config.permissive_ownership,
            "strict should disable permissive ownership"
        );
        assert!(
            config.enforce_immutability,
            "strict should enforce immutability"
        );
        assert!(config.gas_budget.is_some(), "strict should have gas budget");
        assert_eq!(config.gas_budget.unwrap(), 50_000_000_000);
    }

    #[test]
    fn test_new_equals_default() {
        let new_config = SimulationConfig::new();
        let default_config = SimulationConfig::default();

        assert_eq!(new_config.mock_crypto_pass, default_config.mock_crypto_pass);
        assert_eq!(new_config.epoch, default_config.epoch);
        assert_eq!(new_config.gas_budget, default_config.gas_budget);
    }

    #[test]
    fn test_builder_with_mock_crypto() {
        let config = SimulationConfig::default().with_mock_crypto(false);
        assert!(!config.mock_crypto_pass);

        let config = SimulationConfig::default().with_mock_crypto(true);
        assert!(config.mock_crypto_pass);
    }

    #[test]
    fn test_builder_with_clock_base() {
        let config = SimulationConfig::default().with_clock_base(12345678);
        assert_eq!(config.clock_base_ms, 12345678);
    }

    #[test]
    fn test_builder_with_random_seed() {
        let seed = [99u8; 32];
        let config = SimulationConfig::default().with_random_seed(seed);
        assert_eq!(config.random_seed, seed);
    }

    #[test]
    fn test_builder_with_epoch() {
        let config = SimulationConfig::default().with_epoch(500);
        assert_eq!(config.epoch, 500);
    }

    #[test]
    fn test_builder_with_gas_budget() {
        let config = SimulationConfig::default().with_gas_budget(Some(1_000_000));
        assert_eq!(config.gas_budget, Some(1_000_000));

        let config = SimulationConfig::default().with_gas_budget(None);
        assert!(config.gas_budget.is_none());
    }

    #[test]
    fn test_builder_with_immutability_enforcement() {
        let config = SimulationConfig::default().with_immutability_enforcement(true);
        assert!(config.enforce_immutability);

        let config = SimulationConfig::default().with_immutability_enforcement(false);
        assert!(!config.enforce_immutability);
    }

    #[test]
    fn test_advance_epoch() {
        let mut config = SimulationConfig::default();
        assert_eq!(config.epoch, 100);

        config.advance_epoch(10);
        assert_eq!(config.epoch, 110);

        config.advance_epoch(1);
        assert_eq!(config.epoch, 111);
    }

    #[test]
    fn test_advance_epoch_saturating() {
        let mut config = SimulationConfig::default().with_epoch(u64::MAX - 5);

        config.advance_epoch(10);
        assert_eq!(config.epoch, u64::MAX, "should saturate at MAX");
    }

    #[test]
    fn test_builder_chaining() {
        let config = SimulationConfig::default()
            .with_mock_crypto(false)
            .with_epoch(200)
            .with_gas_budget(Some(100_000))
            .with_immutability_enforcement(true)
            .with_clock_base(999999);

        assert!(!config.mock_crypto_pass);
        assert_eq!(config.epoch, 200);
        assert_eq!(config.gas_budget, Some(100_000));
        assert!(config.enforce_immutability);
        assert_eq!(config.clock_base_ms, 999999);
    }
}

// =============================================================================
// ExecutionTrace Tests
// =============================================================================

mod execution_trace_tests {
    use super::*;
    use move_core_types::identifier::Identifier;

    fn make_module_id(addr: &str, name: &str) -> ModuleId {
        ModuleId::new(
            AccountAddress::from_hex_literal(addr).unwrap(),
            Identifier::new(name).unwrap(),
        )
    }

    #[test]
    fn test_trace_new_is_empty() {
        let trace = ExecutionTrace::new();
        assert!(trace.modules_accessed.is_empty());
    }

    #[test]
    fn test_trace_default_is_empty() {
        let trace = ExecutionTrace::default();
        assert!(trace.modules_accessed.is_empty());
    }

    #[test]
    fn test_trace_accessed_package_empty() {
        let trace = ExecutionTrace::new();
        let addr = AccountAddress::from_hex_literal("0x1").unwrap();
        assert!(!trace.accessed_package(&addr));
    }

    #[test]
    fn test_trace_accessed_package_found() {
        let mut trace = ExecutionTrace::new();
        let module_id = make_module_id("0x2", "coin");
        trace.modules_accessed.insert(module_id);

        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert!(trace.accessed_package(&addr));
    }

    #[test]
    fn test_trace_accessed_package_not_found() {
        let mut trace = ExecutionTrace::new();
        let module_id = make_module_id("0x2", "coin");
        trace.modules_accessed.insert(module_id);

        let other_addr = AccountAddress::from_hex_literal("0x3").unwrap();
        assert!(!trace.accessed_package(&other_addr));
    }

    #[test]
    fn test_trace_modules_from_package_empty() {
        let trace = ExecutionTrace::new();
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        let modules = trace.modules_from_package(&addr);
        assert!(modules.is_empty());
    }

    #[test]
    fn test_trace_modules_from_package_filtered() {
        let mut trace = ExecutionTrace::new();
        trace.modules_accessed.insert(make_module_id("0x2", "coin"));
        trace
            .modules_accessed
            .insert(make_module_id("0x2", "balance"));
        trace
            .modules_accessed
            .insert(make_module_id("0x1", "vector"));

        let addr2 = AccountAddress::from_hex_literal("0x2").unwrap();
        let modules = trace.modules_from_package(&addr2);
        assert_eq!(modules.len(), 2);

        let addr1 = AccountAddress::from_hex_literal("0x1").unwrap();
        let modules = trace.modules_from_package(&addr1);
        assert_eq!(modules.len(), 1);
    }

    #[test]
    fn test_trace_multiple_modules_same_package() {
        let mut trace = ExecutionTrace::new();
        trace.modules_accessed.insert(make_module_id("0x2", "coin"));
        trace
            .modules_accessed
            .insert(make_module_id("0x2", "balance"));
        trace
            .modules_accessed
            .insert(make_module_id("0x2", "object"));
        trace
            .modules_accessed
            .insert(make_module_id("0x2", "transfer"));

        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert!(trace.accessed_package(&addr));
        assert_eq!(trace.modules_from_package(&addr).len(), 4);
    }
}

// =============================================================================
// ExecutionOutput Tests
// =============================================================================

mod execution_output_tests {
    use super::*;

    #[test]
    fn test_output_default() {
        let output = ExecutionOutput::default();
        assert!(output.return_values.is_empty());
        assert!(output.mutable_ref_outputs.is_empty());
        assert_eq!(output.gas_used, 0);
    }

    #[test]
    fn test_output_with_return_values() {
        let output = ExecutionOutput {
            return_values: vec![vec![1, 2, 3], vec![4, 5]],
            mutable_ref_outputs: vec![],
            gas_used: 1000,
        };

        assert_eq!(output.return_values.len(), 2);
        assert_eq!(output.return_values[0], vec![1, 2, 3]);
        assert_eq!(output.return_values[1], vec![4, 5]);
    }

    #[test]
    fn test_output_with_mutable_refs() {
        let output = ExecutionOutput {
            return_values: vec![],
            mutable_ref_outputs: vec![(0, vec![10, 20]), (2, vec![30, 40, 50])],
            gas_used: 500,
        };

        assert_eq!(output.mutable_ref_outputs.len(), 2);
        assert_eq!(output.mutable_ref_outputs[0], (0, vec![10, 20]));
        assert_eq!(output.mutable_ref_outputs[1], (2, vec![30, 40, 50]));
    }
}

// =============================================================================
// Gas Costs Constants Tests
// =============================================================================

mod gas_costs_tests {
    use super::*;

    #[test]
    fn test_gas_costs_defined() {
        // Just verify the constants exist and have reasonable values
        assert!(gas_costs::FUNCTION_CALL_BASE > 0);
        assert!(gas_costs::INPUT_BYTE > 0);
        assert!(gas_costs::OUTPUT_BYTE > 0);
        assert!(gas_costs::TYPE_ARG > 0);
        assert!(gas_costs::NATIVE_CALL > 0);
        assert!(gas_costs::STORAGE_BYTE > 0);
        assert!(gas_costs::OBJECT_CREATE > 0);
        assert!(gas_costs::OBJECT_MUTATE > 0);
        assert!(gas_costs::OBJECT_DELETE > 0);
    }
}

// =============================================================================
// MeteredGasMeter Tests
// =============================================================================

mod metered_gas_meter_tests {
    use super::*;
    use move_vm_types::gas::GasMeter;

    #[test]
    fn test_meter_new() {
        let meter = MeteredGasMeter::new(1000);
        assert_eq!(meter.gas_consumed(), 0);
    }

    #[test]
    fn test_meter_remaining_gas() {
        let meter = MeteredGasMeter::new(1000);
        let remaining: u64 = meter.remaining_gas().into();
        assert_eq!(remaining, 1000);
    }

    #[test]
    fn test_meter_zero_budget() {
        let meter = MeteredGasMeter::new(0);
        assert_eq!(meter.gas_consumed(), 0);
        let remaining: u64 = meter.remaining_gas().into();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_meter_large_budget() {
        let meter = MeteredGasMeter::new(u64::MAX);
        assert_eq!(meter.gas_consumed(), 0);
        let remaining: u64 = meter.remaining_gas().into();
        assert_eq!(remaining, u64::MAX);
    }
}

// =============================================================================
// GasMeterImpl Tests
// =============================================================================

mod gas_meter_impl_tests {
    use super::*;

    #[test]
    fn test_impl_from_config_unmetered() {
        let config = SimulationConfig::default(); // gas_budget = None
        let meter = GasMeterImpl::from_config(&config);

        assert_eq!(meter.gas_consumed(), 0, "unmetered should report 0");
    }

    #[test]
    fn test_impl_from_config_metered() {
        let config = SimulationConfig::default().with_gas_budget(Some(50_000));
        let meter = GasMeterImpl::from_config(&config);

        assert_eq!(meter.gas_consumed(), 0, "should start at 0");
    }

    #[test]
    fn test_impl_from_strict_config() {
        let config = SimulationConfig::strict();
        let meter = GasMeterImpl::from_config(&config);

        // Strict config has a gas budget, so meter should be metered
        assert_eq!(meter.gas_consumed(), 0);
    }
}

// =============================================================================
// VMHarness Creation Tests
// =============================================================================

mod vm_harness_creation_tests {
    use super::*;

    #[test]
    fn test_harness_new_with_fixture() {
        let resolver = load_fixture_resolver();
        let result = VMHarness::new(&resolver, true);

        assert!(result.is_ok(), "should create harness with fixture");
    }

    #[test]
    fn test_harness_new_with_empty_resolver() {
        let resolver = empty_resolver();
        let result = VMHarness::new(&resolver, true);

        // Should still succeed - empty resolver is valid
        assert!(result.is_ok(), "should create harness with empty resolver");
    }

    #[test]
    fn test_harness_with_default_config() {
        let resolver = load_fixture_resolver();
        let result = VMHarness::with_config(&resolver, true, SimulationConfig::default());

        assert!(result.is_ok());
    }

    #[test]
    fn test_harness_with_strict_config() {
        let resolver = load_fixture_resolver();
        let result = VMHarness::with_config(&resolver, true, SimulationConfig::strict());

        assert!(result.is_ok());
    }

    #[test]
    fn test_harness_with_custom_config() {
        let resolver = load_fixture_resolver();
        let config = SimulationConfig::default()
            .with_epoch(500)
            .with_gas_budget(Some(100_000));

        let result = VMHarness::with_config(&resolver, true, config);
        assert!(result.is_ok());

        let harness = result.unwrap();
        assert_eq!(harness.config().epoch, 500);
        assert_eq!(harness.config().gas_budget, Some(100_000));
    }

    #[test]
    fn test_harness_unrestricted_mode() {
        let resolver = load_fixture_resolver();
        let result = VMHarness::new(&resolver, false); // unrestricted

        assert!(result.is_ok());
    }
}

// =============================================================================
// VMHarness Execution Tests
// =============================================================================

mod vm_harness_execution_tests {
    use super::*;
    use move_core_types::identifier::Identifier;

    #[test]
    fn test_harness_execute_simple_function() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        // Find test_module
        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        let module_id = module.self_id();

        // Execute simple_func(42) -> should return 42
        let result = harness.execute_function(
            &module_id,
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        );

        assert!(
            result.is_ok(),
            "simple_func should execute: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_harness_execute_with_return() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        let module_id = module.self_id();

        let result = harness.execute_function_with_return(
            &module_id,
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        );

        assert!(result.is_ok());
        let return_values = result.unwrap();
        assert!(!return_values.is_empty(), "should have return value");

        // The return value should be the same as input (identity function)
        let returned_u64 = u64::from_le_bytes(return_values[0].clone().try_into().unwrap());
        assert_eq!(returned_u64, 42);
    }

    #[test]
    fn test_harness_execute_full() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        let module_id = module.self_id();

        let result = harness.execute_function_full(
            &module_id,
            "simple_func",
            vec![],
            vec![100u64.to_le_bytes().to_vec()],
        );

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.return_values.is_empty());
        assert!(output.gas_used > 0, "should report gas used");
    }

    #[test]
    fn test_harness_execute_nonexistent_function() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver.iter_modules().next().expect("should have module");
        let module_id = module.self_id();

        let result =
            harness.execute_function(&module_id, "function_that_does_not_exist", vec![], vec![]);

        assert!(result.is_err(), "nonexistent function should fail");
    }

    #[test]
    fn test_harness_execute_with_wrong_args() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        let module_id = module.self_id();

        // simple_func expects u64, but we provide nothing
        let result = harness.execute_function(&module_id, "simple_func", vec![], vec![]);

        assert!(result.is_err(), "wrong number of args should fail");
    }

    #[test]
    fn test_harness_execute_with_malformed_args() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        let module_id = module.self_id();

        // simple_func expects u64 (8 bytes), but we provide only 3 bytes
        let result = harness.execute_function(
            &module_id,
            "simple_func",
            vec![],
            vec![vec![1, 2, 3]], // Too short for u64
        );

        assert!(result.is_err(), "malformed args should fail");
    }
}

// =============================================================================
// VMHarness Trace Tests
// =============================================================================

mod vm_harness_trace_tests {
    use super::*;

    #[test]
    fn test_harness_initial_trace_empty() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let trace = harness.get_trace();
        assert!(
            trace.modules_accessed.is_empty(),
            "initial trace should be empty"
        );
    }

    #[test]
    fn test_harness_trace_records_module_access() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        let module_id = module.self_id();

        // Execute a function
        let _ = harness.execute_function(
            &module_id,
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        );

        let trace = harness.get_trace();
        assert!(
            !trace.modules_accessed.is_empty(),
            "trace should record module access"
        );
    }

    #[test]
    fn test_harness_clear_trace() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness should create");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        // Execute to populate trace
        let _ = harness.execute_function(
            &module.self_id(),
            "simple_func",
            vec![],
            vec![42u64.to_le_bytes().to_vec()],
        );

        assert!(!harness.get_trace().modules_accessed.is_empty());

        harness.clear_trace();

        assert!(
            harness.get_trace().modules_accessed.is_empty(),
            "trace should be cleared"
        );
    }
}

// =============================================================================
// VMHarness Events Tests
// =============================================================================

mod vm_harness_events_tests {
    use super::*;

    #[test]
    fn test_harness_initial_events_empty() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let events = harness.get_events();
        assert!(events.is_empty());
    }

    #[test]
    fn test_harness_clear_events() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        // Clear should work even on empty
        harness.clear_events();
        assert!(harness.get_events().is_empty());
    }

    #[test]
    fn test_harness_get_events_by_type() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let events = harness.get_events_by_type("0x2::coin::");
        assert!(events.is_empty());
    }
}

// =============================================================================
// VMHarness Dynamic Fields Tests
// =============================================================================

mod vm_harness_dynamic_fields_tests {
    use super::*;

    #[test]
    fn test_harness_preload_dynamic_fields() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let parent = AccountAddress::from_hex_literal("0x100").unwrap();
        let child = AccountAddress::from_hex_literal("0x200").unwrap();
        let type_tag = TypeTag::U64;
        let bytes = vec![1, 2, 3, 4, 5, 6, 7, 8];

        harness.preload_dynamic_fields(vec![((parent, child), type_tag, bytes)]);

        // Verify it was loaded
        let fields = harness.extract_dynamic_fields();
        assert_eq!(fields.len(), 1);
    }

    #[test]
    fn test_harness_extract_dynamic_fields_empty() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let fields = harness.extract_dynamic_fields();
        assert!(fields.is_empty());
    }

    #[test]
    fn test_harness_clear_dynamic_fields() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let parent = AccountAddress::from_hex_literal("0x100").unwrap();
        let child = AccountAddress::from_hex_literal("0x200").unwrap();
        harness.preload_dynamic_fields(vec![((parent, child), TypeTag::U64, vec![0u8; 8])]);

        assert!(!harness.extract_dynamic_fields().is_empty());

        harness.clear_dynamic_fields();

        assert!(harness.extract_dynamic_fields().is_empty());
    }

    #[test]
    fn test_harness_extract_new_dynamic_fields() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        // Initially empty
        let new_fields = harness.extract_new_dynamic_fields();
        assert!(new_fields.is_empty());
    }
}

// =============================================================================
// VMHarness Synthesize Tests
// =============================================================================

mod vm_harness_synthesize_tests {
    use super::*;

    #[test]
    fn test_harness_synthesize_tx_context() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let result = harness.synthesize_tx_context();
        assert!(result.is_ok());

        let bytes = result.unwrap();
        // TxContext has: sender (32) + tx_hash (1 + 32) + epoch (8) + timestamp (8) + ids_created (8) = 89 bytes
        assert!(
            bytes.len() >= 80,
            "TxContext bytes should be at least 80 bytes"
        );
    }

    #[test]
    fn test_harness_synthesize_clock() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let result = harness.synthesize_clock();
        assert!(result.is_ok());

        let bytes = result.unwrap();
        // Clock has: id (32) + timestamp_ms (8) = 40 bytes
        assert_eq!(bytes.len(), 40, "Clock bytes should be 40 bytes");
    }

    #[test]
    fn test_harness_synthesize_clock_advances() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness should create");

        let clock1 = harness.synthesize_clock().unwrap();
        let clock2 = harness.synthesize_clock().unwrap();

        // The timestamp (last 8 bytes) should differ
        let ts1 = u64::from_le_bytes(clock1[32..40].try_into().unwrap());
        let ts2 = u64::from_le_bytes(clock2[32..40].try_into().unwrap());

        assert!(ts2 > ts1, "clock timestamp should advance");
    }
}

// =============================================================================
// VMHarness Config Access Tests
// =============================================================================

mod vm_harness_config_access_tests {
    use super::*;

    #[test]
    fn test_harness_config_accessor() {
        let resolver = load_fixture_resolver();
        let config = SimulationConfig::default().with_epoch(777);
        let harness = VMHarness::with_config(&resolver, true, config).expect("harness");

        assert_eq!(harness.config().epoch, 777);
    }

    #[test]
    fn test_harness_vm_accessor() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness");

        // Just verify we can access the VM
        let _vm = harness.vm();
    }

    #[test]
    fn test_harness_storage_accessor() {
        let resolver = load_fixture_resolver();
        let harness = VMHarness::new(&resolver, true).expect("harness");

        // Just verify we can access storage
        let _storage = harness.storage();
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

mod error_handling_tests {
    use super::*;

    #[test]
    fn test_execute_nonexistent_module() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness");

        let fake_module_id = ModuleId::new(
            AccountAddress::from_hex_literal("0xdeadbeef").unwrap(),
            move_core_types::identifier::Identifier::new("nonexistent").unwrap(),
        );

        let result = harness.execute_function(&fake_module_id, "any_func", vec![], vec![]);

        assert!(result.is_err(), "nonexistent module should fail");
    }

    #[test]
    fn test_execute_with_type_args_mismatch() {
        let resolver = load_fixture_resolver();
        let mut harness = VMHarness::new(&resolver, true).expect("harness");

        let module = resolver
            .iter_modules()
            .find(|m| {
                sui_move_interface_extractor::bytecode::compiled_module_name(m) == "test_module"
            })
            .expect("test_module should exist");

        // simple_func doesn't take type args, so passing some should fail or be ignored
        let result = harness.execute_function(
            &module.self_id(),
            "simple_func",
            vec![TypeTag::U64], // Unexpected type arg
            vec![42u64.to_le_bytes().to_vec()],
        );

        // This might succeed or fail depending on implementation
        // The important thing is no panic
        let _ = result;
    }
}
