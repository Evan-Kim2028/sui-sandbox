//! Sui Move Sandbox - Native Python Bindings
//!
//! This crate provides PyO3 bindings for the Sui Move simulation sandbox,
//! enabling direct Python access without JSON serialization overhead.
//!
//! # Example
//!
//! ```python
//! from sui_sandbox import SandboxEnvironment
//!
//! env = SandboxEnvironment()
//! result = env.execute({"action": "list_modules"})
//! print(f"Loaded {len(result.data['modules'])} modules")
//! ```

use pyo3::prelude::*;

mod environment;
mod request;
mod response;
mod types;

pub use environment::SandboxEnvironment;
pub use response::*;

/// Sui Move Sandbox - Native Python bindings
///
/// This module provides direct access to the Sui Move simulation sandbox
/// without JSON serialization overhead. Response types are fully typed,
/// preventing schema mismatch bugs at compile time.
#[pymodule]
fn sui_sandbox(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Main environment class
    m.add_class::<SandboxEnvironment>()?;

    // Response types - exposed for isinstance() checks and type hints
    m.add_class::<response::SandboxResponse>()?;
    m.add_class::<response::TransactionEffects>()?;
    m.add_class::<response::ObjectEffect>()?;
    m.add_class::<response::EventData>()?;
    m.add_class::<response::ModuleSummary>()?;
    m.add_class::<response::FunctionInfo>()?;
    m.add_class::<response::StructInfo>()?;
    m.add_class::<response::FieldInfo>()?;

    // Module metadata
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add(
        "__doc__",
        "Native Python bindings for Sui Move simulation sandbox",
    )?;

    Ok(())
}
