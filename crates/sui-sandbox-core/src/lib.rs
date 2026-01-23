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
//! - [`object_runtime`]: Object storage and dynamic field support
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

pub mod error_context;
pub mod errors;
pub mod fetcher;
pub mod natives;
pub mod object_patcher;
pub mod object_runtime;
pub mod ptb;
pub mod resolver;
pub mod sandbox_types;
pub mod simulation;
pub mod state_layer;
pub mod state_source;
pub mod sui_object_runtime;
pub mod types;
pub mod validator;
pub mod vm;
pub mod well_known;

// Re-export main types at crate root for convenience
pub use fetcher::{FetchedObjectData, Fetcher, MockFetcher, NoopFetcher};
pub use object_runtime::{ObjectRuntime, SharedObjectRuntime};
pub use resolver::LocalModuleResolver;
pub use vm::{SimulationConfig, VMHarness};
