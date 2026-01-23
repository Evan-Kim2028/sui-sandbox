//! Sui Move Interface Extractor
//!
//! Tools for analyzing Sui Move packages:
//!
//! - **Interface extraction**: Extract module interfaces from bytecode or RPC
//! - **Bytecode analysis**: Parse and analyze compiled Move bytecode
//! - **Data fetching**: Unified API for gRPC streaming, GraphQL queries, and JSON-RPC
//!
//! For transaction simulation, see the `sui-sandbox-core` crate.

#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub mod args;
pub mod cache;
pub mod comparator;
pub mod corpus;
pub mod data_fetcher;
pub mod move_stubs;
pub mod rpc;
pub mod runner;

// Re-export modules from sui-package-extractor crate
pub use sui_package_extractor::bytecode;
pub use sui_package_extractor::normalization;
pub use sui_package_extractor::types;
pub use sui_package_extractor::utils;

// Re-export modules from sui-data-fetcher crate
pub use sui_data_fetcher::graphql;
pub use sui_data_fetcher::grpc;
