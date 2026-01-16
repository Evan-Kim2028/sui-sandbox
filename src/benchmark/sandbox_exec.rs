//! # Sandbox Execution Interface for LLM Integration
//!
//! **NOTE: This module has been refactored into `sandbox/` submodule.**
//!
//! This file provides backwards-compatible re-exports from the new modular structure.
//! For new code, prefer importing directly from `crate::benchmark::sandbox`.
//!
//! The new structure is:
//! - `sandbox/types.rs` - All request/response type definitions
//! - `sandbox/handlers/` - Handler implementations by category
//! - `sandbox/cli.rs` - Command-line interface
//! - `sandbox/mod.rs` - Main dispatcher and re-exports

// Re-export everything from the new sandbox module for backwards compatibility
pub use crate::benchmark::sandbox::*;
