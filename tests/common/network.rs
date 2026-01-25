//! Network-dependent test utilities.
//!
//! Provides helpers for tests that require network access to external services
//! like Sui mainnet, testnet, or gRPC endpoints.

use std::env;

/// Environment variable for gRPC endpoint configuration.
pub const GRPC_ENDPOINT_VAR: &str = "SUI_GRPC_ENDPOINT";

/// Environment variable to enable network tests.
pub const RUN_NETWORK_TESTS_VAR: &str = "RUN_NETWORK_TESTS";

/// Get gRPC endpoint from environment variable.
///
/// Returns `Some(endpoint)` if `SUI_GRPC_ENDPOINT` is set, `None` otherwise.
pub fn get_grpc_endpoint() -> Option<String> {
    env::var(GRPC_ENDPOINT_VAR).ok()
}

/// Check if network tests should be run.
///
/// Network tests are enabled when `RUN_NETWORK_TESTS` environment variable
/// is set to any non-empty value.
pub fn should_run_network_tests() -> bool {
    env::var(RUN_NETWORK_TESTS_VAR)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Macro to skip a test if network tests are not enabled.
///
/// Usage:
/// ```ignore
/// #[test]
/// fn test_network_feature() {
///     skip_if_no_network!();
///     // ... network-dependent test code ...
/// }
/// ```
#[macro_export]
macro_rules! skip_if_no_network {
    () => {
        if !$crate::common::network::should_run_network_tests() {
            eprintln!(
                "Skipping {}: {} not set",
                module_path!(),
                $crate::common::network::RUN_NETWORK_TESTS_VAR
            );
            return;
        }
    };
}

/// Macro to skip a test if gRPC endpoint is not configured.
/// Returns the endpoint if available.
///
/// Usage:
/// ```ignore
/// #[test]
/// fn test_grpc_feature() {
///     let endpoint = require_grpc!();
///     // ... use endpoint ...
/// }
/// ```
#[macro_export]
macro_rules! require_grpc {
    () => {
        match $crate::common::network::get_grpc_endpoint() {
            Some(endpoint) => endpoint,
            None => {
                eprintln!(
                    "Skipping {}: {} not set",
                    module_path!(),
                    $crate::common::network::GRPC_ENDPOINT_VAR
                );
                return;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_grpc_endpoint_when_not_set() {
        // This test verifies behavior when env var is not set
        // We can't reliably test the "set" case without affecting other tests
        let result = get_grpc_endpoint();
        // Just verify it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_should_run_network_tests_default() {
        // By default, network tests should be disabled in CI
        // This test documents the expected default behavior
        // The actual value depends on the test environment
        let _ = should_run_network_tests();
    }
}
