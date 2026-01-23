//! # Simulation Environment for LLM-Driven PTB Construction
//!
//! **This is the primary API for sandbox/PTB simulation.** Use this module for:
//! - Interactive PTB building and execution
//! - LLM-driven transaction construction
//! - Debugging and testing Move transactions
//!
//! This module provides an interactive simulation environment that allows an LLM to:
//! 1. Deploy Move packages (bytecode)
//! 2. Create objects with specific state
//! 3. Execute PTBs and receive structured error feedback
//! 4. Iteratively fix issues until PTBs succeed
//!
//! ## Key Features
//!
//! - **Full PTB Support**: MoveCall, SplitCoins, MergeCoins, TransferObjects, Publish, Upgrade
//! - **Dynamic Publishing**: Publish modules and call them within the SAME PTB
//! - **Session Persistence**: Published modules persist across PTBs within the same session
//! - **Object Tracking**: Created, mutated, deleted objects tracked across commands
//! - **Shared Object Locking**: Automatic lock acquisition/release for shared objects
//! - **Dynamic Fields**: Tables, Bags, and dynamic field operations fully supported
//! - **Return Value Capture**: Function return values available in transaction effects
//! - **Gas Accounting**: Estimated gas usage with budget enforcement
//! - **Epoch & Time**: Configurable epoch numbers and timestamps for TxContext
//! - **Randomness**: Deterministic random number generation with configurable seeds
//! - **Lamport Clock**: Version tracking for shared object consensus simulation
//! - **Structured Errors**: Errors designed for programmatic consumption
//!
//! ## Module Organization
//!
//! - [`types`]: Core types (SimulatedObject, ExecutionResult, CoinMetadata, etc.)
//! - [`errors`]: Structured error types (SimulationError)
//! - [`state`]: Persistent state types for save/load (PersistentState, FetcherConfig)
//! - [`consensus`]: Shared object locking and consensus simulation
//! - [`environment`]: The main SimulationEnvironment struct and implementation
//!
//! ## Example Usage
//!
//! ```no_run
//! use sui_sandbox_core::simulation::SimulationEnvironment;
//!
//! let mut env = SimulationEnvironment::new().unwrap();
//!
//! // Create objects needed for the PTB
//! let coin_id = env.create_coin("0x2::sui::SUI", 1_000_000_000).unwrap();
//!
//! // Execute a PTB (see examples/ for complete usage)
//! ```

// Sub-modules
pub mod consensus;
pub mod environment;
pub mod errors;
pub mod state;
pub mod types;

// Re-export all public items for convenience
pub use consensus::{ConsensusOrderEntry, ConsensusValidation, LockResult, SharedObjectLock};
pub use environment::SimulationEnvironment;
pub use errors::SimulationError;
pub use state::{
    FetcherConfig, PersistentState, SerializedDynamicField, SerializedModule, SerializedObject,
    SerializedPendingReceive, StateMetadata,
};
pub use types::{
    CoinMetadata, CompileError, CompileErrorDetail, CompileResult, ExecutionResult,
    FieldDefinition, FunctionCallResult, SimulatedObject, StateCheckpoint, StateSummary,
    StructDefinition, TypeParamDef, CLOCK_OBJECT_ID, DEFAULT_CLOCK_BASE_MS, DEFAULT_GAS_PRICE,
    RANDOM_OBJECT_ID, SUI_COIN_TYPE, SUI_DECIMALS, SUI_SYMBOL,
};

// Re-export EmittedEvent for convenience
pub use crate::natives::EmittedEvent;

// Re-export the leb128_encode helper
pub use types::leb128_encode;
