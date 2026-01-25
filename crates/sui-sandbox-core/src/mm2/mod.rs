//! # MM2 Bytecode Analysis - Internal Infrastructure
//!
//! **Important**: This module is internal infrastructure for the data prefetching pipeline.
//! It is NOT a user-facing API. See `predictive_prefetch.rs` for the public interface.
//!
//! ## Purpose
//!
//! This module analyzes Move bytecode to predict which dynamic fields a transaction
//! will access, enabling proactive data fetching before execution. This reduces
//! replay failures caused by missing dynamic field data.
//!
//! ## How It Fits in the System
//!
//! ```text
//! Transaction Replay Pipeline
//!          │
//!          ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Layer 1: Ground Truth Prefetch                               │
//! │   └─ Uses unchanged_loaded_runtime_objects from tx effects   │
//! └─────────────────────────────────────────────────────────────┘
//!          │
//!          ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Layer 2: Predictive Prefetch (THIS MODULE)                   │
//! │   └─ BytecodeAnalyzer: Find dynamic_field::* calls           │
//! │   └─ CallGraph: Trace through wrapper functions              │
//! │   └─ FieldAccessPredictor: Resolve types to concrete keys    │
//! │   └─ KeySynthesizer: Derive child object IDs                 │
//! └─────────────────────────────────────────────────────────────┘
//!          │
//!          ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │ Layer 3: On-Demand Fetch (fallback during execution)         │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Module Components
//!
//! - `bytecode_analyzer`: Walks bytecode to find `dynamic_field::*` operations
//! - `call_graph`: Builds call graph and propagates sink status backwards
//! - `field_access_predictor`: Combines analysis with type resolution
//! - `key_synthesizer`: Derives child object IDs from key type patterns
//! - `type_synthesizer`: Synthesizes values for type inhabitation
//! - `type_validator`: Static type checking (secondary use case)
//!
//! ## Why Call Graph Analysis?
//!
//! Many protocols wrap dynamic field operations in helper functions:
//!
//! ```text
//! table::borrow()           ← calls dynamic_field::borrow_child_object
//! pool::get_balance()       ← calls table::borrow
//! swap::execute()           ← calls pool::get_balance
//! ```
//!
//! Direct bytecode analysis only catches the first level. Call graph analysis
//! propagates "sink" status backwards to find ALL functions that transitively
//! access dynamic fields.

pub mod bytecode_analyzer;
pub mod call_graph;
pub mod constructor_graph;
pub mod field_access_predictor;
pub mod key_synthesizer;
pub mod model;
pub mod type_synthesizer;
pub mod type_validator;

pub use bytecode_analyzer::{
    BytecodeAnalyzer, DynamicFieldAccessKind, DynamicFieldAccessPattern, FunctionAccessAnalysis,
};
pub use call_graph::{
    AccessConfidence, CallGraph, CallGraphStats, FunctionKey, ResolvedAccess, SinkPath,
    TypeParamMapping, TypeParamResolution,
};
pub use constructor_graph::{
    ConstructorGraph, ExecutionChain, ExecutionStep, ParamRequirement, Producer, ProducerChain,
    ProducerStep, MAX_CHAIN_DEPTH,
};
pub use field_access_predictor::{
    Confidence, FieldAccessPredictor, PredictedAccess, PredictorStats,
};
pub use key_synthesizer::{KeyValueSynthesizer, SynthesizerStats};
pub use model::{ReturnTypeArg, TypeModel};
pub use type_synthesizer::{SynthesisResult, TypeSynthesizer};
pub use type_validator::TypeValidator;
