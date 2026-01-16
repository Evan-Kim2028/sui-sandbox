//! Type converters for Move/Sui types to Python.
//!
//! This module provides bidirectional conversion between Rust types
//! (AccountAddress, TypeTag, etc.) and Python objects (strings, dicts).

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

// AccountAddress is available for potential future use
#[allow(unused_imports)]
use move_core_types::account_address::AccountAddress;

/// Convert serde_json::Value to PyObject.
/// This handles arbitrary nested data structures from sandbox responses.
pub fn json_to_py(py: Python<'_>, value: serde_json::Value) -> PyResult<PyObject> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => Ok(b.into_py(py)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_py(py))
            } else if let Some(u) = n.as_u64() {
                Ok(u.into_py(py))
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_py(py))
            } else {
                // Fallback for very large numbers
                Ok(n.to_string().into_py(py))
            }
        }
        serde_json::Value::String(s) => Ok(s.into_py(py)),
        serde_json::Value::Array(arr) => {
            let list = PyList::empty_bound(py);
            for item in arr {
                list.append(json_to_py(py, item)?)?;
            }
            Ok(list.into())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new_bound(py);
            for (k, v) in map {
                dict.set_item(k, json_to_py(py, v)?)?;
            }
            Ok(dict.into())
        }
    }
}

/// Convert PyObject to serde_json::Value.
/// Used for request arguments that need to be passed to Rust.
pub fn py_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        Ok(serde_json::Value::Null)
    } else if let Ok(b) = obj.extract::<bool>() {
        Ok(serde_json::Value::Bool(b))
    } else if let Ok(i) = obj.extract::<i64>() {
        Ok(serde_json::json!(i))
    } else if let Ok(f) = obj.extract::<f64>() {
        Ok(serde_json::json!(f))
    } else if let Ok(s) = obj.extract::<String>() {
        Ok(serde_json::Value::String(s))
    } else if let Ok(list) = obj.downcast::<PyList>() {
        let arr: PyResult<Vec<serde_json::Value>> =
            list.iter().map(|item| py_to_json(&item)).collect();
        Ok(serde_json::Value::Array(arr?))
    } else if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            map.insert(key, py_to_json(&v)?);
        }
        Ok(serde_json::Value::Object(map))
    } else {
        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
            "Cannot convert {} to JSON",
            obj.get_type().name()?
        )))
    }
}

/// Helper to extract a required string field from a dict.
pub fn extract_string(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<String> {
    dict.get_item(key)?
        .ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "Missing required field: {}",
                key
            ))
        })?
        .extract()
}

/// Helper to extract an optional string field from a dict.
pub fn extract_optional_string(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<String>> {
    match dict.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract()?)),
        _ => Ok(None),
    }
}

/// Helper to extract a list of strings from a dict.
pub fn extract_string_list(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Vec<String>> {
    match dict.get_item(key)? {
        Some(v) if !v.is_none() => {
            let list = v.downcast::<PyList>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                    "Field '{}' must be a list",
                    key
                ))
            })?;
            list.iter().map(|item| item.extract()).collect()
        }
        _ => Ok(Vec::new()),
    }
}
