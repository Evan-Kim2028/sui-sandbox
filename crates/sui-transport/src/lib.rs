//! Sui Transport Layer
//!
//! Network transport for Sui data fetching via gRPC and GraphQL.
//!
//! This crate provides:
//! - [`grpc`]: gRPC client for real-time streaming and batch fetching
//! - [`graphql`]: GraphQL client for querying packages, objects, and transactions
//!
//! # Example
//!
//! ```ignore
//! use sui_transport::graphql::GraphQLClient;
//! use sui_transport::grpc::GrpcClient;
//!
//! // GraphQL queries
//! let client = GraphQLClient::mainnet();
//! let pkg = client.fetch_package("0x2")?;
//!
//! // gRPC streaming (async)
//! let grpc = GrpcClient::mainnet().await?;
//! ```

pub mod graphql;
pub mod grpc;

// Re-export main types for convenience
pub use graphql::GraphQLClient;
pub use grpc::GrpcClient;

/// Create a Tokio runtime and connect to a gRPC endpoint.
///
/// Configuration via environment variables:
///
/// - `SUI_GRPC_ENDPOINT` - gRPC endpoint (default: `https://fullnode.mainnet.sui.io:443`)
/// - `SUI_GRPC_API_KEY` - API key (optional, depends on provider)
///
/// Returns both the runtime (for blocking operations) and the connected client.
pub fn create_grpc_client() -> anyhow::Result<(tokio::runtime::Runtime, GrpcClient)> {
    let rt = tokio::runtime::Runtime::new()?;

    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;

    Ok((rt, grpc))
}

/// Create a gRPC client with explicit endpoint and optional API key.
///
/// Use this when you need direct control over the endpoint and API key,
/// bypassing environment variable configuration.
pub fn create_grpc_client_with_config(
    endpoint: &str,
    api_key: Option<String>,
) -> anyhow::Result<(tokio::runtime::Runtime, GrpcClient)> {
    let rt = tokio::runtime::Runtime::new()?;
    let grpc = rt.block_on(async { GrpcClient::with_api_key(endpoint, api_key).await })?;
    Ok((rt, grpc))
}
