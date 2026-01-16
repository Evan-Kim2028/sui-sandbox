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
pub mod bytecode;
pub mod cache;
pub mod comparator;
pub mod corpus;
pub mod data_fetcher;
pub mod graphql;
pub mod grpc;
pub mod move_stubs;
pub mod normalization;
pub mod rpc;
pub mod runner;
pub mod tx_cache; // Legacy - use `cache` module instead
pub mod types;
pub mod utils;
