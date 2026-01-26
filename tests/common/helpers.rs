//! Common test helper functions.
//!
//! Provides utilities for common test operations like finding modules,
//! formatting paths, and setting up test scenarios.

use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use sui_sandbox_core::resolver::LocalModuleResolver;

/// Find the test_module in a resolver.
///
/// This is a common pattern used throughout tests to locate the fixture
/// test module for execution.
///
/// # Arguments
///
/// * `resolver` - The resolver to search in
///
/// # Returns
///
/// Returns `Some(&CompiledModule)` if found, `None` otherwise.
///
/// # Example
///
/// ```ignore
/// use common::{fixtures::load_fixture_resolver, helpers::find_test_module};
///
/// let resolver = load_fixture_resolver();
/// let module = find_test_module(&resolver).expect("test_module should exist");
/// ```
pub fn find_test_module(resolver: &LocalModuleResolver) -> Option<&CompiledModule> {
    resolver
        .iter_modules()
        .find(|m| sui_sandbox::bytecode::compiled_module_name(m) == "test_module")
}

/// Find a module by name in a resolver.
///
/// # Arguments
///
/// * `resolver` - The resolver to search in
/// * `name` - The module name to find
///
/// # Returns
///
/// Returns `Some(&CompiledModule)` if found, `None` otherwise.
pub fn find_module_by_name<'a>(
    resolver: &'a LocalModuleResolver,
    name: &str,
) -> Option<&'a CompiledModule> {
    resolver
        .iter_modules()
        .find(|m| sui_sandbox::bytecode::compiled_module_name(m) == name)
}

/// Format a module path in the standard "0x...::name" format.
///
/// # Arguments
///
/// * `module` - The compiled module to format
///
/// # Returns
///
/// A string in the format "0xADDRESS::module_name"
///
/// # Example
///
/// ```ignore
/// use common::helpers::format_module_path;
///
/// let path = format_module_path(&module);
/// assert!(path.contains("::"));
/// ```
pub fn format_module_path(module: &CompiledModule) -> String {
    format!(
        "{}::{}",
        module.self_id().address().to_hex_literal(),
        module.self_id().name()
    )
}

/// Create a module ID from address string and name.
///
/// # Arguments
///
/// * `addr` - Hex address string (e.g., "0x2")
/// * `name` - Module name
///
/// # Returns
///
/// The constructed `ModuleId`.
///
/// # Panics
///
/// Panics if the address is invalid or name is not a valid identifier.
pub fn make_module_id(addr: &str, name: &str) -> ModuleId {
    ModuleId::new(
        AccountAddress::from_hex_literal(addr).expect("valid address"),
        Identifier::new(name).expect("valid identifier"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::fixtures::load_fixture_resolver;

    #[test]
    fn test_find_test_module() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver);
        assert!(module.is_some(), "should find test_module in fixture");
    }

    #[test]
    fn test_find_module_by_name() {
        let resolver = load_fixture_resolver();
        let module = find_module_by_name(&resolver, "test_module");
        assert!(module.is_some());
    }

    #[test]
    fn test_format_module_path() {
        let resolver = load_fixture_resolver();
        let module = find_test_module(&resolver).unwrap();
        let path = format_module_path(module);

        assert!(path.contains("::"), "path should contain ::");
        assert!(
            path.contains("test_module"),
            "path should contain module name"
        );
        assert!(path.starts_with("0x"), "path should start with 0x");
    }

    #[test]
    fn test_make_module_id() {
        let id = make_module_id("0x2", "coin");
        assert_eq!(id.name().as_str(), "coin");
    }
}
