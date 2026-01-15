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

pub mod bytecode_analyzer;
pub mod constructor_map;
pub mod errors;
pub mod mm2;
pub mod natives;
pub mod object_runtime;
pub mod output;
pub mod package_builder;
pub mod phases;
pub mod ptb;
pub mod ptb_eval;
pub mod resolver;
pub mod runner;
pub mod sandbox_exec;
pub mod simulation;
pub mod state_layer;
pub mod storage_log;
pub mod tx_replay;
pub mod types;
pub mod validator;
pub mod vm;
