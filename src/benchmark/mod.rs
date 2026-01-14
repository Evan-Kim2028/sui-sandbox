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
//! ## Key Components
//!
//! - [`vm`]: VMHarness orchestrating Move VM execution
//! - [`natives`]: Native function implementations (real, mocked, and unsupported)
//! - [`object_runtime`]: VM extension for dynamic field operations
//! - [`resolver`]: Module loading from bytecode files
//! - [`runner`]: Benchmark orchestration and constructor chaining
//! - [`constructor_map`]: Constructor discovery for type synthesis
//! - [`validator`]: Type layout resolution and BCS validation
//!
//! ## Two-Tier Evaluation
//!
//! - **Tier A (Preflight)**: Types resolve, BCS serializes correctly, layouts are valid
//! - **Tier B (Execution)**: Code runs in the Move VM without aborting
//!
//! A Tier B hit indicates successful type inhabitation—the code understood the types
//! well enough to construct valid values.
//!
//! See `docs/LOCAL_BYTECODE_SANDBOX.md` for detailed architecture documentation.

pub mod bytecode_analyzer;
pub mod constructor_map;
pub mod errors;
pub mod mm2;
pub mod natives;
pub mod object_runtime;
pub mod object_store;
pub mod phases;
pub mod ptb;
pub mod ptb_eval;
pub mod resolver;
pub mod runner;
pub mod sandbox_exec;
pub mod simulation;
pub mod tx_replay;
pub mod validator;
pub mod vm;
pub mod llm_tools;
pub mod package_builder;
pub mod storage_log;
