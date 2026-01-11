//! Integration tests for MM2 (Move Model 2) type validation.
//!
//! These tests verify that the MM2-based static type checking works correctly
//! with real Move bytecode.

use sui_move_interface_extractor::benchmark::mm2::{ConstructorGraph, TypeModel, TypeValidator};
use sui_move_interface_extractor::benchmark::phases::{resolution, typecheck};
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;

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
    assert!(!info.type_parameters.is_empty(), "Coin should have type params");

    // Coin should have store ability
    assert!(
        info.abilities.0.iter().any(|a| *a == move_model_2::summary::Ability::Store),
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
    assert!(result.is_ok(), "Should resolve coin::value: {:?}", result.err());

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
    assert!(tc_result.is_ok(), "Type check should succeed: {:?}", tc_result.err());

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
