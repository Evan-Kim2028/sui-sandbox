//! Test setup helpers for common initialization patterns.
//!
//! Provides high-level helpers that combine fixture loading with
//! harness/validator creation to reduce boilerplate in tests.

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::ModuleId;
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::vm::VMHarness;

use super::fixtures::load_fixture_resolver;
use super::helpers::find_test_module;

/// Load fixture resolver and find the test module, returning both.
///
/// This is the most common setup pattern in tests. Panics if
/// the test module is not found.
///
/// # Returns
///
/// A tuple of (resolver, module reference).
pub fn fixture_with_test_module() -> (LocalModuleResolver, ModuleId) {
    let resolver = load_fixture_resolver();
    let module = find_test_module(&resolver).expect("test_module should exist in fixture");
    let module_id = module.self_id().clone();
    (resolver, module_id)
}

/// Load fixture resolver and return it with the test module details.
///
/// Returns resolver, module ID, and package address for convenience.
pub fn fixture_with_module_details() -> (LocalModuleResolver, ModuleId, AccountAddress) {
    let resolver = load_fixture_resolver();
    let module = find_test_module(&resolver).expect("test_module should exist in fixture");
    let module_id = module.self_id().clone();
    let package_addr = *module.self_id().address();
    (resolver, module_id, package_addr)
}

/// Create a VM harness with the fixture resolver in restricted mode.
///
/// This is the standard setup for most VM execution tests.
///
/// # Returns
///
/// A tuple of (harness, module_id) ready for execution.
pub fn harness_with_fixture() -> (VMHarness<'static>, ModuleId) {
    // Use Box::leak to give the resolver a 'static lifetime
    // This is fine for tests since they run to completion
    let resolver = Box::leak(Box::new(load_fixture_resolver()));
    let module = find_test_module(resolver).expect("test_module should exist in fixture");
    let module_id = module.self_id().clone();
    let harness = VMHarness::new(resolver, true).expect("harness should create");
    (harness, module_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixture_with_test_module() {
        let (resolver, module_id) = fixture_with_test_module();
        assert!(resolver.module_count() > 0);
        assert_eq!(module_id.name().as_str(), "test_module");
    }

    #[test]
    fn test_fixture_with_module_details() {
        let (resolver, module_id, package_addr) = fixture_with_module_details();
        assert!(resolver.module_count() > 0);
        assert_eq!(module_id.name().as_str(), "test_module");
        assert_eq!(module_id.address(), &package_addr);
    }

    #[test]
    fn test_harness_with_fixture() {
        let (_harness, module_id) = harness_with_fixture();
        assert_eq!(module_id.name().as_str(), "test_module");
    }
}
