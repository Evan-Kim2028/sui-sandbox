//! # Move Model 2 (MM2) Integration
//!
//! This module provides integration with the `move-model-2` crate for static type
//! validation in the type inhabitation pipeline.
//!
//! ## Purpose
//!
//! MM2 enables us to:
//! - Build a semantic model from compiled bytecode (no source required)
//! - Perform static type checking before VM execution
//! - Validate generic instantiations and ability constraints
//! - Analyze function signatures and struct layouts
//!
//! ## Architecture
//!
//! ```text
//! CompiledModules ──► Model::from_compiled() ──► TypeModel
//!                                                  │
//!                      ┌───────────────────────────┼───────────────────────────┐
//!                      │                           │                           │
//!                      ▼                           ▼                           ▼
//!               get_function()              get_struct()              validate_call()
//!               (signatures)               (field types)              (type checking)
//! ```
//!
//! ## Phase 2: TypeCheck
//!
//! This module implements Phase 2 of the v0.4.0 pipeline, catching type errors
//! statically before attempting VM execution.

pub mod constructor_graph;
pub mod model;
pub mod type_synthesizer;
pub mod type_validator;

pub use constructor_graph::{
    ConstructorGraph, ExecutionChain, ExecutionStep, ParamRequirement, Producer, ProducerChain,
    ProducerStep, MAX_CHAIN_DEPTH,
};
pub use model::TypeModel;
pub use type_synthesizer::{SynthesisResult, TypeSynthesizer};
pub use type_validator::TypeValidator;
