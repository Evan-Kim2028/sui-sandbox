//! Fixture loading utilities for tests.
//!
//! Provides standardized ways to load test fixtures, resolvers, and modules.

use std::path::Path;
use sui_sandbox_core::resolver::LocalModuleResolver;

/// Default fixture directory path relative to the project root.
pub const FIXTURE_DIR: &str = "tests/fixture/build/fixture";

/// Load a resolver with the standard test fixture modules.
///
/// This loads the pre-compiled bytecode from `tests/fixture/build/fixture`.
///
/// # Panics
///
/// Panics if the fixture directory doesn't exist or contains invalid bytecode.
///
/// # Example
///
/// ```ignore
/// use common::fixtures::load_fixture_resolver;
///
/// let resolver = load_fixture_resolver();
/// assert!(resolver.module_count() > 0);
/// ```
pub fn load_fixture_resolver() -> LocalModuleResolver {
    let fixture_dir = Path::new(FIXTURE_DIR);
    let mut resolver = LocalModuleResolver::new();
    resolver
        .load_from_dir(fixture_dir)
        .expect("fixture should load - ensure tests/fixture is built");
    resolver
}

/// Create an empty resolver with no modules loaded.
///
/// Useful for testing error cases or starting from a clean state.
///
/// # Example
///
/// ```ignore
/// use common::fixtures::empty_resolver;
///
/// let resolver = empty_resolver();
/// assert_eq!(resolver.module_count(), 0);
/// ```
pub fn empty_resolver() -> LocalModuleResolver {
    LocalModuleResolver::new()
}

/// Load a resolver with Sui framework modules only.
///
/// This is useful when you need framework types but no custom modules.
///
/// # Panics
///
/// Panics if the framework fails to load.
pub fn framework_resolver() -> LocalModuleResolver {
    LocalModuleResolver::with_sui_framework().expect("framework should load")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_fixture_resolver_succeeds() {
        let resolver = load_fixture_resolver();
        assert!(resolver.module_count() > 0, "fixture should have modules");
    }

    #[test]
    fn test_empty_resolver_is_empty() {
        let resolver = empty_resolver();
        assert_eq!(resolver.module_count(), 0);
        assert!(resolver.list_packages().is_empty());
    }

    #[test]
    fn test_framework_resolver_has_modules() {
        let resolver = framework_resolver();
        assert!(resolver.module_count() > 0, "framework should have modules");
        // Framework should have common modules like coin
        assert!(resolver.has_package(
            &move_core_types::account_address::AccountAddress::from_hex_literal("0x2").unwrap()
        ));
    }
}
