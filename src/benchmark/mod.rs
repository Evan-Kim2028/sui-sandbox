//! # Local Bytecode Sandbox
//!
//! This module implements the **Local Bytecode Sandbox**—a deterministic, offline Move VM
//! environment for testing type inhabitation without deploying to any Sui network.
//!
//! ## Purpose
//!
//! The sandbox enables evaluation of LLM understanding of Move types by:
//! - Loading external package bytecode directly from `.mv` files
//! - Executing code in an embedded Move VM with synthetic state
//! - Validating that types can be successfully inhabited
//!
//! ## LLM Integration
//!
//! For LLM agent integration, use [`sandbox_exec::SandboxRequest`] as the canonical API:
//!
//! - **Entry point**: `execute_request(SandboxRequest)` handles all operations
//! - **Discovery**: `{"action": "list_available_tools"}` returns complete tool documentation
//! - **State**: All operations share [`simulation::SimulationEnvironment`] state
//!
//! ## Key Components
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`sandbox_exec`] | **Canonical LLM API** - JSON-based tool interface |
//! | [`simulation`] | Core execution environment with state management |
//! | [`ptb`] | Programmable Transaction Block construction & execution |
//! | [`vm`] | VMHarness orchestrating Move VM execution |
//! | [`natives`] | Native function implementations (real, mocked, unsupported) |
//! | [`object_runtime`] | VM extension for dynamic field operations |
//! | [`resolver`] | Module loading from bytecode files |
//! | [`errors`] | Error taxonomy with E101-E502 codes |
//! | [`mm2`] | Move Model 2 integration for static validation |
//!
//! ## Two-Tier Evaluation
//!
//! - **Tier A (Preflight)**: Types resolve, BCS serializes correctly, layouts are valid
//! - **Tier B (Execution)**: Code runs in the Move VM without aborting
//!
//! A Tier B hit indicates successful type inhabitation—the code understood the types
//! well enough to construct valid values.
//!
//! See `docs/ARCHITECTURE.md` for detailed architecture documentation.

// Local modules that remain in main crate (depend on DataFetcher or other main-crate types)
pub mod fetcher;
pub mod ptb_eval;
pub mod runner;
pub mod sandbox;
pub mod sandbox_exec;
pub mod session;
pub mod tx_replay;

// Re-export modules from sui-sandbox-core (no DataFetcher dependency)
pub use sui_sandbox_core::bytecode_analyzer;
pub use sui_sandbox_core::constructor_map;
pub use sui_sandbox_core::error_context;
pub use sui_sandbox_core::errors;
pub use sui_sandbox_core::mm2;
pub use sui_sandbox_core::natives;
pub use sui_sandbox_core::object_patcher;
pub use sui_sandbox_core::object_runtime;
pub use sui_sandbox_core::output;
pub use sui_sandbox_core::package_builder;
pub use sui_sandbox_core::phases;
pub use sui_sandbox_core::ptb;
pub use sui_sandbox_core::resolver;
pub use sui_sandbox_core::sandbox_types;
pub use sui_sandbox_core::simulation;
pub use sui_sandbox_core::state_layer;
pub use sui_sandbox_core::state_source;
pub use sui_sandbox_core::storage_log;
pub use sui_sandbox_core::sui_object_runtime;
pub use sui_sandbox_core::types;
pub use sui_sandbox_core::validator;
pub use sui_sandbox_core::vm;
pub use sui_sandbox_core::well_known;

// Re-export fetcher types from sui-sandbox-core for convenience
pub use sui_sandbox_core::fetcher::{FetchedObjectData, Fetcher, MockFetcher, NoopFetcher};
