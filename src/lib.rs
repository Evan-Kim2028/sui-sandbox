//! Sui Move Interface Extractor
//!
//! Tools for analyzing Sui Move packages:
//!
//! - **Interface extraction**: Extract module interfaces from bytecode or RPC
//! - **Bytecode analysis**: Parse and analyze compiled Move bytecode
//! - **State fetching**: GraphQL/gRPC clients and historical replay helpers
//!
//! For transaction simulation, see the `sui-sandbox-core` crate.

#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub mod cache;
pub mod ptb_classifier;

// Re-export modules from sui-package-extractor crate
pub use sui_package_extractor::bytecode;
pub use sui_package_extractor::normalization;
pub use sui_package_extractor::types;
pub use sui_package_extractor::utils;

// Re-export modules from transport/prefetch crates
pub use sui_prefetch;
pub use sui_state_fetcher;
pub use sui_transport::graphql;
pub use sui_transport::grpc;
