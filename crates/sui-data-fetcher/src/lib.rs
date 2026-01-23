//! Sui Data Fetcher
//!
//! Data fetching layer for Sui network using GraphQL, gRPC, and local cache.
//!
//! This crate provides:
//! - [`graphql`]: GraphQL client for querying packages, objects, and transactions
//! - [`grpc`]: gRPC client for real-time streaming and batch fetching
//! - [`conversion`]: Conversion utilities between gRPC and internal types
//! - [`utilities`]: Data helpers for aggregating gRPC responses
//!
//! # Example
//!
//! ```ignore
//! use sui_data_fetcher::graphql::GraphQLClient;
//! use sui_data_fetcher::grpc::GrpcClient;
//!
//! // GraphQL queries
//! let client = GraphQLClient::mainnet();
//! let pkg = client.fetch_package("0x2")?;
//!
//! // gRPC streaming (async)
//! let grpc = GrpcClient::mainnet().await?;
//! ```

pub mod conversion;
pub mod graphql;
pub mod grpc;
pub mod utilities;

// Re-export main types
pub use conversion::grpc_to_fetched_transaction;
pub use graphql::GraphQLClient;
pub use grpc::GrpcClient;
pub use utilities::{collect_historical_versions, create_grpc_client};
