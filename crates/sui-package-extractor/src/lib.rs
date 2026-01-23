//! Sui Package Extractor
//!
//! Package interface extraction for Sui Move packages.
//!
//! This crate provides tools for extracting and analyzing Move module interfaces
//! from compiled bytecode or on-chain packages.
//!
//! # Features
//!
//! - **Bytecode analysis**: Parse and analyze compiled Move bytecode
//! - **Interface extraction**: Extract struct and function signatures
//! - **Type normalization**: Convert Move types to JSON representations
//!
//! # Example
//!
//! ```ignore
//! use sui_package_extractor::bytecode;
//!
//! // Load compiled modules
//! let modules = bytecode::read_local_compiled_modules("./build/package")?;
//!
//! // Build interface
//! let (names, interface) = bytecode::build_bytecode_interface_value_from_compiled_modules(
//!     "0x1234",
//!     &modules,
//! )?;
//! ```

pub mod bytecode;
pub mod normalization;
pub mod types;
pub mod utils;

// Re-export main types
pub use bytecode::{
    build_bytecode_interface_value_from_compiled_modules, read_local_compiled_modules,
};
pub use types::{BytecodeModuleJson, BytecodePackageInterfaceJson};
