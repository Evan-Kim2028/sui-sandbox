//! Python-accessible sandbox environment wrapper.
//!
//! This module provides the main entry point for Python code to interact
//! with the Sui Move simulation sandbox.

use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::cell::RefCell;

use sui_move_interface_extractor::benchmark::sandbox_exec::execute_request;
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;

use crate::request::dict_to_request;
use crate::response::SandboxResponse;

/// Sui Move simulation sandbox environment.
///
/// This class wraps the Rust SimulationEnvironment and provides
/// a safe interface for Python to execute sandbox operations.
///
/// # Thread Safety
///
/// This class is NOT thread-safe. The underlying Move VM contains
/// non-Send types. Use only from a single thread.
///
/// # Example
///
/// ```python
/// from sui_sandbox import SandboxEnvironment
///
/// env = SandboxEnvironment()
///
/// # List loaded modules
/// result = env.execute({"action": "list_modules"})
/// print(f"Loaded {len(result.data['modules'])} modules")
///
/// # Execute a PTB
/// result = env.execute({
///     "action": "execute_ptb",
///     "inputs": [...],
///     "commands": [...],
/// })
/// if result.success:
///     print(f"Created {len(result.effects.created)} objects")
/// ```
#[pyclass(unsendable)] // Critical: prevents Python from moving between threads
pub struct SandboxEnvironment {
    inner: RefCell<SimulationEnvironment>,
    verbose: bool,
}

#[pymethods]
impl SandboxEnvironment {
    /// Create a new sandbox environment.
    ///
    /// The sandbox is initialized with the Sui framework (0x1, 0x2, 0x3)
    /// pre-loaded.
    ///
    /// # Arguments
    ///
    /// * `verbose` - Enable verbose output to stderr (default: False)
    ///
    /// # Raises
    ///
    /// * `RuntimeError` - If sandbox initialization fails
    #[new]
    #[pyo3(signature = (verbose = false))]
    pub fn new(verbose: bool) -> PyResult<Self> {
        let env = SimulationEnvironment::new().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Failed to create sandbox: {}",
                e
            ))
        })?;

        Ok(Self {
            inner: RefCell::new(env),
            verbose,
        })
    }

    /// Execute a sandbox request.
    ///
    /// This is the main entry point for all sandbox operations.
    /// Takes a request dictionary and returns a typed SandboxResponse.
    ///
    /// # Arguments
    ///
    /// * `request` - A dict containing at minimum an "action" field.
    ///               See `list_available_tools` action for all options.
    ///
    /// # Returns
    ///
    /// A `SandboxResponse` object with typed fields:
    /// - `success`: bool
    /// - `error`: Optional[str]
    /// - `data`: Optional[Any] (operation-specific)
    /// - `effects`: Optional[TransactionEffects] (for PTB execution)
    /// - `events`: Optional[List[EventData]] (for PTB execution)
    /// - `gas_used`: Optional[int]
    ///
    /// # Example
    ///
    /// ```python
    /// # Get module summary
    /// result = env.execute({
    ///     "action": "module_summary",
    ///     "module_path": "0x2::coin",
    /// })
    /// print(result.data["summary"])
    /// ```
    pub fn execute(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyDict>,
    ) -> PyResult<SandboxResponse> {
        // Convert Python dict to Rust request enum
        let rust_request = dict_to_request(request)?;

        // Borrow mutably and execute
        let mut env = self.inner.try_borrow_mut().map_err(|_| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Sandbox is already borrowed. Concurrent access is not supported.",
            )
        })?;

        let rust_response = execute_request(&mut env, &rust_request, self.verbose);

        // Convert to Python response type
        SandboxResponse::from_rust(py, rust_response)
    }

    /// Reset the sandbox state.
    ///
    /// This clears all objects and resets the sandbox to its initial state,
    /// but keeps loaded modules.
    ///
    /// # Raises
    ///
    /// * `RuntimeError` - If reset fails
    pub fn reset(&self) -> PyResult<()> {
        let mut env = self.inner.try_borrow_mut().map_err(|_| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Sandbox is already borrowed. Concurrent access is not supported.",
            )
        })?;

        env.reset_state().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!("Failed to reset: {}", e))
        })
    }

    /// Get the current timestamp in milliseconds.
    #[getter]
    pub fn timestamp_ms(&self) -> PyResult<u64> {
        let env = self.inner.try_borrow().map_err(|_| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Sandbox is already borrowed. Concurrent access is not supported.",
            )
        })?;
        Ok(env.get_clock_timestamp_ms())
    }

    /// Check if verbose mode is enabled.
    #[getter]
    pub fn verbose(&self) -> bool {
        self.verbose
    }

    fn __repr__(&self) -> String {
        match self.inner.try_borrow() {
            Ok(env) => {
                let object_count = env.object_count();
                format!(
                    "SandboxEnvironment(objects={}, verbose={})",
                    object_count, self.verbose
                )
            }
            Err(_) => "SandboxEnvironment(borrowed)".to_string(),
        }
    }
}
