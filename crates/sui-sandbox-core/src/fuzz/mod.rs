//! Move function fuzzing framework.
//!
//! Provides type-aware random input generation and execution loop for
//! testing Move functions against the local VM with boundary-heavy
//! value distributions.
//!
//! # Architecture
//!
//! - [`classifier`]: Classifies function parameters as pure (fuzzable),
//!   system-injected, object-based, or unfuzzable
//! - [`value_gen`]: Boundary-heavy random BCS value generation
//! - [`runner`]: Fuzzing execution loop with gas profiling
//! - [`report`]: Result types for fuzz outcomes
//!
//! # Phase 2 Seam
//!
//! The [`runner::FuzzRunner`] is designed to support future coverage-guided
//! fuzzing via an optional `CoverageTracker` parameter (not yet implemented).

pub mod classifier;
pub mod report;
pub mod runner;
pub mod value_gen;

pub use classifier::{classify_params, ClassifiedFunction, ParamClass, PureType, SystemType};
pub use report::{
    AbortInfo, ErrorInfo, FuzzOutcomeSummary, FuzzReport, GasProfile, InterestingCase, Outcome,
};
pub use runner::{FuzzConfig, FuzzRunner};
pub use value_gen::ValueGenerator;
