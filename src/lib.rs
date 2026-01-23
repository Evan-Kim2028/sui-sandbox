//! Sui Move Interface Extractor
//!
//! Tools for analyzing, testing, and simulating Sui Move packages:
//!
//! - **Interface extraction**: Extract module interfaces from bytecode or RPC
//! - **Bytecode analysis**: Parse and analyze compiled Move bytecode
//! - **Transaction simulation**: Execute PTBs in a local sandbox environment
//! - **Data fetching**: Unified API for gRPC streaming, GraphQL queries, and JSON-RPC
//!
//! See [`data_fetcher`] for fetching on-chain data and [`benchmark`] for simulation.

#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub mod args;
pub mod benchmark;
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
