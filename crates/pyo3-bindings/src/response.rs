//! Python-exposed response types.
//!
//! These #[pyclass] types provide compile-time schema guarantees.
//! Python code receives typed objects instead of untyped dicts,
//! preventing schema mismatch bugs.

use pyo3::prelude::*;
use sui_move_interface_extractor::benchmark::sandbox_exec::SandboxResponse as RustResponse;

use crate::types::json_to_py;

/// Main response type returned by all sandbox operations.
///
/// This replaces the untyped dict responses from JSON-based IPC.
/// All fields are accessible as Python attributes with proper types.
#[pyclass(frozen)]
#[derive(Debug)]
pub struct SandboxResponse {
    /// Whether the operation succeeded.
    #[pyo3(get)]
    pub success: bool,

    /// Error message if success is False.
    #[pyo3(get)]
    pub error: Option<String>,

    /// Error category (e.g., "ValidationError", "ExecutionError").
    #[pyo3(get)]
    pub error_category: Option<String>,

    /// Abort code if this was a contract abort.
    #[pyo3(get)]
    pub abort_code: Option<u64>,

    /// Module that aborted (for contract aborts).
    #[pyo3(get)]
    pub abort_module: Option<String>,

    /// Operation-specific data (type depends on the request).
    /// Stored as Py<PyAny> to avoid Clone issues.
    data_inner: Option<Py<PyAny>>,

    /// Transaction effects (for execute_ptb).
    effects_inner: Option<Py<TransactionEffects>>,

    /// Events emitted (for execute_ptb).
    events_inner: Option<Py<PyAny>>,

    /// Gas used (for execute_ptb and call_function).
    #[pyo3(get)]
    pub gas_used: Option<u64>,

    /// Index of failed command (for PTB failures).
    #[pyo3(get)]
    pub failed_command_index: Option<usize>,

    /// Description of failed command.
    #[pyo3(get)]
    pub failed_command_description: Option<String>,

    /// Number of commands that succeeded before failure.
    #[pyo3(get)]
    pub commands_succeeded: Option<usize>,
}

#[pymethods]
impl SandboxResponse {
    /// Operation-specific data (type depends on the request).
    #[getter]
    fn data(&self, py: Python<'_>) -> Option<PyObject> {
        self.data_inner.as_ref().map(|d| d.clone_ref(py).into())
    }

    /// Transaction effects (for execute_ptb).
    #[getter]
    fn effects(&self, py: Python<'_>) -> Option<Py<TransactionEffects>> {
        self.effects_inner.as_ref().map(|e| e.clone_ref(py))
    }

    /// Events emitted (for execute_ptb).
    #[getter]
    fn events(&self, py: Python<'_>) -> Option<PyObject> {
        self.events_inner.as_ref().map(|e| e.clone_ref(py).into())
    }

    fn __repr__(&self) -> String {
        if self.success {
            if let Some(gas) = self.gas_used {
                format!("SandboxResponse(success=True, gas_used={})", gas)
            } else {
                "SandboxResponse(success=True)".to_string()
            }
        } else {
            format!(
                "SandboxResponse(success=False, error={:?})",
                self.error.as_deref().unwrap_or("unknown")
            )
        }
    }
}

/// Transaction effects from PTB execution.
#[pyclass(frozen)]
#[derive(Debug)]
pub struct TransactionEffects {
    /// Objects created by the transaction.
    #[pyo3(get)]
    pub created: Vec<ObjectEffect>,

    /// Objects mutated by the transaction.
    #[pyo3(get)]
    pub mutated: Vec<ObjectEffect>,

    /// Object IDs deleted by the transaction.
    #[pyo3(get)]
    pub deleted: Vec<String>,

    /// Objects wrapped (transferred to another object).
    #[pyo3(get)]
    pub wrapped: Vec<String>,

    /// Objects unwrapped (extracted from another object).
    #[pyo3(get)]
    pub unwrapped: Vec<ObjectEffect>,

    /// Return values from each command.
    return_values_inner: Option<Py<PyAny>>,
}

#[pymethods]
impl TransactionEffects {
    /// Return values from each command.
    #[getter]
    fn return_values(&self, py: Python<'_>) -> Option<PyObject> {
        self.return_values_inner
            .as_ref()
            .map(|r| r.clone_ref(py).into())
    }

    fn __repr__(&self) -> String {
        format!(
            "TransactionEffects(created={}, mutated={}, deleted={})",
            self.created.len(),
            self.mutated.len(),
            self.deleted.len()
        )
    }
}

/// Information about an object affected by a transaction.
#[pyclass(frozen, get_all)]
#[derive(Clone, Debug)]
pub struct ObjectEffect {
    /// Object ID.
    pub id: String,

    /// Object type (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    pub object_type: Option<String>,

    /// Owner after the transaction.
    pub owner: String,

    /// Object version after the transaction.
    pub version: u64,
}

#[pymethods]
impl ObjectEffect {
    fn __repr__(&self) -> String {
        format!(
            "ObjectEffect(id={}, type={:?})",
            &self.id[..16.min(self.id.len())],
            self.object_type
        )
    }
}

/// Event emitted during transaction execution.
#[pyclass(frozen, get_all)]
#[derive(Clone, Debug)]
pub struct EventData {
    /// Event type (e.g., "0x2::coin::CoinCreated").
    pub event_type: String,

    /// Hex-encoded event data.
    pub data_hex: String,

    /// Event sequence number.
    pub sequence: u64,
}

#[pymethods]
impl EventData {
    fn __repr__(&self) -> String {
        format!("EventData(type={})", self.event_type)
    }
}

/// Summary of a Move module's contents.
#[pyclass(frozen, get_all)]
#[derive(Clone, Debug)]
pub struct ModuleSummary {
    /// Module path (e.g., "0x2::coin").
    pub module: String,

    /// Human-readable summary text.
    pub summary: String,
}

#[pymethods]
impl ModuleSummary {
    fn __repr__(&self) -> String {
        format!("ModuleSummary(module={})", self.module)
    }
}

/// Information about a Move function.
#[pyclass(frozen, get_all)]
#[derive(Clone, Debug)]
pub struct FunctionInfo {
    /// Full path (e.g., "0x2::coin::split").
    pub path: String,

    /// Function name.
    pub name: String,

    /// Visibility (public, friend, private).
    pub visibility: String,

    /// Whether this is an entry function.
    pub is_entry: bool,

    /// Type parameter names.
    pub type_parameters: Vec<String>,

    /// Parameter type strings.
    pub parameters: Vec<String>,

    /// Return type strings.
    pub return_types: Vec<String>,
}

#[pymethods]
impl FunctionInfo {
    fn __repr__(&self) -> String {
        format!(
            "FunctionInfo(path={}, is_entry={})",
            self.path, self.is_entry
        )
    }
}

/// Information about a Move struct.
#[pyclass(frozen, get_all)]
#[derive(Clone, Debug)]
pub struct StructInfo {
    /// Full path (e.g., "0x2::coin::Coin").
    pub path: String,

    /// Struct name.
    pub name: String,

    /// Abilities (copy, drop, store, key).
    pub abilities: Vec<String>,

    /// Type parameter names.
    pub type_parameters: Vec<String>,

    /// Field information.
    pub fields: Vec<FieldInfo>,
}

#[pymethods]
impl StructInfo {
    fn __repr__(&self) -> String {
        format!(
            "StructInfo(path={}, fields={})",
            self.path,
            self.fields.len()
        )
    }
}

/// Information about a struct field.
#[pyclass(frozen, get_all)]
#[derive(Clone, Debug)]
pub struct FieldInfo {
    /// Field name.
    pub name: String,

    /// Field type string.
    pub field_type: String,
}

#[pymethods]
impl FieldInfo {
    fn __repr__(&self) -> String {
        format!("FieldInfo(name={}, type={})", self.name, self.field_type)
    }
}

// =============================================================================
// Conversion from Rust types to Python types
// =============================================================================

impl SandboxResponse {
    /// Convert a Rust SandboxResponse to the Python-exposed type.
    pub fn from_rust(py: Python<'_>, rust: RustResponse) -> PyResult<Self> {
        // Convert data (serde_json::Value -> PyObject)
        let data_inner = match rust.data {
            Some(v) => Some(json_to_py(py, v)?.into_py(py).into_bound(py).unbind()),
            None => None,
        };

        // Convert effects
        let effects_inner = match rust.effects {
            Some(e) => Some(Py::new(py, TransactionEffects::from_rust(py, e)?)?),
            None => None,
        };

        // Convert events
        let events_inner = match rust.events {
            Some(evs) => {
                let event_list: Vec<EventData> =
                    evs.into_iter().map(EventData::from_rust).collect();
                Some(event_list.into_py(py).into_bound(py).unbind())
            }
            None => None,
        };

        Ok(SandboxResponse {
            success: rust.success,
            error: rust.error,
            error_category: rust.error_category,
            abort_code: rust.abort_code,
            abort_module: rust.abort_module,
            data_inner,
            effects_inner,
            events_inner,
            gas_used: rust.gas_used,
            failed_command_index: rust.failed_command_index,
            failed_command_description: rust.failed_command_description,
            commands_succeeded: rust.commands_succeeded,
        })
    }
}

impl TransactionEffects {
    /// Convert from Rust TransactionEffectsResponse.
    pub fn from_rust(
        py: Python<'_>,
        rust: sui_move_interface_extractor::benchmark::sandbox_exec::TransactionEffectsResponse,
    ) -> PyResult<Self> {
        let created = rust
            .created
            .into_iter()
            .map(ObjectEffect::from_rust)
            .collect();
        let mutated = rust
            .mutated
            .into_iter()
            .map(ObjectEffect::from_rust)
            .collect();
        let deleted = rust.deleted;
        let wrapped = rust.wrapped;
        let unwrapped = rust
            .unwrapped
            .into_iter()
            .map(ObjectEffect::from_rust)
            .collect();

        // Convert return_values (Option<Vec<CommandReturnValues>> -> PyObject)
        let return_values_inner = match rust.return_values {
            Some(rv) if !rv.is_empty() => {
                let json_val = serde_json::to_value(&rv).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?;
                Some(
                    json_to_py(py, json_val)?
                        .into_py(py)
                        .into_bound(py)
                        .unbind(),
                )
            }
            _ => None,
        };

        Ok(TransactionEffects {
            created,
            mutated,
            deleted,
            wrapped,
            unwrapped,
            return_values_inner,
        })
    }
}

impl ObjectEffect {
    /// Convert from Rust ObjectEffectResponse.
    pub fn from_rust(
        rust: sui_move_interface_extractor::benchmark::sandbox_exec::ObjectEffectResponse,
    ) -> Self {
        ObjectEffect {
            id: rust.id,
            object_type: rust.object_type,
            owner: rust.owner,
            version: rust.version,
        }
    }
}

impl EventData {
    /// Convert from Rust EventResponse.
    pub fn from_rust(
        rust: sui_move_interface_extractor::benchmark::sandbox_exec::EventResponse,
    ) -> Self {
        EventData {
            event_type: rust.event_type,
            data_hex: rust.data_hex,
            sequence: rust.sequence,
        }
    }
}
