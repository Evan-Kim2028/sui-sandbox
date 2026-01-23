//! Shared test utilities for integration tests.

/// Get gRPC endpoint from environment variable.
///
/// Returns `Some(endpoint)` if `SUI_GRPC_ENDPOINT` is set, `None` otherwise.
pub fn get_grpc_endpoint() -> Option<String> {
    std::env::var("SUI_GRPC_ENDPOINT").ok()
}
