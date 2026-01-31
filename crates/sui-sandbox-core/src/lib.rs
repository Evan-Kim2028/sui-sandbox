//! Sui Sandbox Core
//!
//! Move VM simulation engine for Sui transactions.
//!
//! This crate provides the core simulation capabilities for executing
//! Programmable Transaction Blocks (PTBs) in a local Move VM sandbox.
//!
//! # Features
//!
//! - **Transaction replay**: Replay historical transactions locally
//! - **PTB execution**: Execute arbitrary PTBs against cached state
//! - **VM harness**: Full Move VM with Sui native functions
//! - **Object runtime**: In-memory object storage with dynamic field support
//!
//! # Core Modules
//!
//! - [`vm`]: VMHarness for executing Move functions
//! - [`resolver`]: LocalModuleResolver for loading bytecode
//! - [`natives`]: Native function implementations
//! - [`sandbox_runtime`]: Object storage and dynamic field support (default runtime)
//! - [`sui_object_runtime`]: Sui native runtime integration (opt-in, 100% accuracy)
//! - [`well_known`]: Well-known Sui types and addresses
//!
//! # Example
//!
//! ```ignore
//! use sui_sandbox_core::vm::{VMHarness, SimulationConfig};
//! use sui_sandbox_core::resolver::LocalModuleResolver;
//!
//! // Create module resolver and load framework
//! let mut resolver = LocalModuleResolver::new();
//! resolver.load_sui_framework()?;
//!
//! // Create VM harness
//! let config = SimulationConfig::default();
//! let mut harness = VMHarness::new(&resolver, config)?;
//!
//! // Execute functions...
//! ```

#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

// Core simulation modules
pub mod constructor_map;
pub mod error_context;
pub mod errors;
pub mod fetcher;
pub mod gas;
pub mod mm2;
pub mod natives;
pub mod phases;
pub mod sandbox_runtime;

// Backward compatibility alias for renamed module
#[deprecated(since = "0.11.0", note = "Use sandbox_runtime instead")]
pub use sandbox_runtime as object_runtime;
pub mod predictive_prefetch;
pub mod ptb;
pub mod resolver;
pub mod sandbox_types;
pub mod session;
pub mod simulation;
pub mod state_source;
pub mod sui_object_runtime;
pub mod tx_replay;
pub mod types;
pub mod validator;
pub mod vm;
pub mod well_known;

// Package building and analysis (for creating mock contracts)
// Note: bytecode_analyzer functionality is in mm2/bytecode_analyzer.rs
pub mod output;
pub mod package_builder;
pub mod state_layer;
pub mod storage_log;

// Utilities for working around infrastructure limitations
pub mod utilities;

// Shared utilities for CLI and MCP integration
pub mod shared;

// Re-export main types at crate root for convenience
pub use fetcher::{FetchedObjectData, Fetcher, GrpcFetcher, MockFetcher, NoopFetcher};
pub use predictive_prefetch::{
    PredictedAccessInfo, PredictionStats, PredictivePrefetchConfig, PredictivePrefetchResult,
    PredictivePrefetcher,
};
pub use resolver::LocalModuleResolver;
pub use sandbox_runtime::{
    ChildFetcherFn, ComputedChildInfo, KeyBasedChildFetcherFn, ObjectRuntime, SharedObjectRuntime,
    VersionedChildFetcherFn,
};
pub use vm::{SimulationConfig, VMHarness};
