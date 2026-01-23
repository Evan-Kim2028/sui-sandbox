//! # Phase-Based Pipeline Architecture
//!
//! This module organizes the type inhabitation pipeline into distinct phases,
//! enabling better error reporting and optional static-only validation.
//!
//! ## Phase Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                    Type Inhabitation Pipeline                        │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │ Phase 1: RESOLUTION                                                  │
//! │ - Load target and helper package bytecode                            │
//! │ - Verify module and function existence                               │
//! │ Errors: E101_ModuleNotFound, E102_FunctionNotFound, E103_NotCallable │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │ Phase 2: TYPE CHECK (static, using MM2)                              │
//! │ - Validate function signatures                                       │
//! │ - Check generic instantiation validity                               │
//! │ - Verify ability constraints                                         │
//! │ Errors: E201_TypeMismatch, E202_AbilityViolation, E203_GenericBounds │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │ Phase 3: SYNTHESIS                                                   │
//! │ - Find constructor chains for required types                         │
//! │ - Generate default values for primitives                             │
//! │ - Build BCS-serialized arguments                                     │
//! │ Errors: E301_NoConstructor, E302_ChainTooDeep, E303_UnsupportedParam │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │ Phase 4: EXECUTION (optional)                                        │
//! │ - Execute constructor chain in VM                                    │
//! │ - Execute target function                                            │
//! │ - Capture execution trace                                            │
//! │ Errors: E401_VMSetupFailed, E402_ConstructorAborted, E403_TargetAbort│
//! ├─────────────────────────────────────────────────────────────────────┤
//! │ Phase 5: VALIDATION                                                  │
//! │ - Verify target modules were accessed                                │
//! │ - Check return types match expectations                              │
//! │ Errors: E501_NoTargetAccess, E502_ReturnTypeMismatch                 │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! The phases can be run incrementally via the CLI or programmatically.
//! See `examples/` for complete usage patterns.

pub mod resolution;
pub mod typecheck;

// Re-export key types
pub use resolution::ResolutionContext;
pub use typecheck::TypeCheckResult;
