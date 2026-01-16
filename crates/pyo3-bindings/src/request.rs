//! Request conversion from Python dicts to Rust SandboxRequest enum.
//!
//! This module handles the conversion of Python request dictionaries
//! to strongly-typed Rust request enums. Each variant is handled
//! explicitly to provide clear error messages on invalid input.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sui_move_interface_extractor::benchmark::sandbox_exec::{
    PtbArg, PtbCommand, PtbInput, SandboxRequest,
};

use crate::types::{extract_optional_string, extract_string, extract_string_list, py_to_json};

/// Convert a Python dict to a SandboxRequest enum variant.
pub fn dict_to_request(dict: &Bound<'_, PyDict>) -> PyResult<SandboxRequest> {
    let action: String = extract_string(dict, "action")?;

    match action.as_str() {
        // =================================================================
        // Module Operations
        // =================================================================
        "load_module" => {
            let bytecode_path = extract_string(dict, "bytecode_path")?;
            let module_name = extract_optional_string(dict, "module_name")?;
            Ok(SandboxRequest::LoadModule {
                bytecode_path,
                module_name,
            })
        }

        "list_modules" => Ok(SandboxRequest::ListModules),

        "compile_move" => {
            let package_name = extract_string(dict, "package_name")?;
            let module_name = extract_string(dict, "module_name")?;
            let source = extract_string(dict, "source")?;
            Ok(SandboxRequest::CompileMove {
                package_name,
                module_name,
                source,
            })
        }

        "module_summary" => {
            let module_path = extract_string(dict, "module_path")?;
            Ok(SandboxRequest::ModuleSummary { module_path })
        }

        "get_module_dependencies" => {
            let module_path = extract_string(dict, "module_path")?;
            Ok(SandboxRequest::GetModuleDependencies { module_path })
        }

        "disassemble_module" => {
            let module_path = extract_string(dict, "module_path")?;
            Ok(SandboxRequest::DisassembleModule { module_path })
        }

        // =================================================================
        // Function Introspection
        // =================================================================
        "list_functions" => {
            let module_path = extract_string(dict, "module_path")?;
            Ok(SandboxRequest::ListFunctions { module_path })
        }

        "get_function_info" => {
            let module_path = extract_string(dict, "module_path")?;
            let function_name = extract_string(dict, "function_name")?;
            Ok(SandboxRequest::GetFunctionInfo {
                module_path,
                function_name,
            })
        }

        "search_functions" => {
            let pattern = extract_string(dict, "pattern")?;
            let entry_only = dict
                .get_item("entry_only")?
                .map(|v| v.extract::<bool>())
                .transpose()?
                .unwrap_or(false);
            Ok(SandboxRequest::SearchFunctions {
                pattern,
                entry_only,
            })
        }

        "find_constructors" => {
            let type_path = extract_string(dict, "type_path")?;
            Ok(SandboxRequest::FindConstructors { type_path })
        }

        "disassemble_function" => {
            let module_path = extract_string(dict, "module_path")?;
            let function_name = extract_string(dict, "function_name")?;
            Ok(SandboxRequest::DisassembleFunction {
                module_path,
                function_name,
            })
        }

        // =================================================================
        // Struct Introspection
        // =================================================================
        "list_structs" => {
            let module_path = extract_string(dict, "module_path")?;
            Ok(SandboxRequest::ListStructs { module_path })
        }

        "get_struct_info" => {
            let type_path = extract_string(dict, "type_path")?;
            Ok(SandboxRequest::GetStructInfo { type_path })
        }

        "search_types" => {
            let pattern = extract_string(dict, "pattern")?;
            let ability_filter = extract_optional_string(dict, "ability_filter")?;
            Ok(SandboxRequest::SearchTypes {
                pattern,
                ability_filter,
            })
        }

        "validate_type" => {
            let type_str = extract_string(dict, "type_str")?;
            Ok(SandboxRequest::ValidateType { type_str })
        }

        // =================================================================
        // Object Operations
        // =================================================================
        "create_object" => {
            let object_type = extract_string(dict, "object_type")?;
            let fields = extract_field_map(dict, "fields")?;
            let object_id = extract_optional_string(dict, "object_id")?;
            Ok(SandboxRequest::CreateObject {
                object_type,
                fields,
                object_id,
            })
        }

        "create_test_object" => {
            let type_tag = extract_string(dict, "type_tag")?;
            let value = match dict.get_item("value")? {
                Some(v) => py_to_json(&v)?,
                None => serde_json::Value::Object(serde_json::Map::new()),
            };
            Ok(SandboxRequest::CreateTestObject { type_tag, value })
        }

        "list_objects" => Ok(SandboxRequest::ListObjects),

        "list_shared_objects" => Ok(SandboxRequest::ListSharedObjects),

        "inspect_object" => {
            let object_id = extract_string(dict, "object_id")?;
            Ok(SandboxRequest::InspectObject { object_id })
        }

        "list_cached_objects" => Ok(SandboxRequest::ListCachedObjects),

        "load_cached_object" => {
            let object_id = extract_string(dict, "object_id")?;
            let bcs_bytes = extract_string(dict, "bcs_bytes")?;
            let object_type = extract_optional_string(dict, "object_type")?;
            let is_shared = dict
                .get_item("is_shared")?
                .map(|v| v.extract::<bool>())
                .transpose()?
                .unwrap_or(false);
            Ok(SandboxRequest::LoadCachedObject {
                object_id,
                bcs_bytes,
                object_type,
                is_shared,
            })
        }

        "load_cached_objects" => {
            let objects = extract_string_map(dict, "objects")?;
            let object_types = extract_string_map(dict, "object_types").unwrap_or_default();
            let shared_object_ids = extract_string_list(dict, "shared_object_ids")?;
            Ok(SandboxRequest::LoadCachedObjects {
                objects,
                object_types,
                shared_object_ids,
            })
        }

        // =================================================================
        // Execution
        // =================================================================
        "execute_ptb" => {
            let inputs = extract_ptb_inputs(dict)?;
            let commands = extract_ptb_commands(dict)?;
            Ok(SandboxRequest::ExecutePtb { inputs, commands })
        }

        "validate_ptb" => {
            let inputs = extract_ptb_inputs(dict)?;
            let commands = extract_ptb_commands(dict)?;
            Ok(SandboxRequest::ValidatePtb { inputs, commands })
        }

        "call_function" => {
            let package = extract_string(dict, "package")?;
            let module = extract_string(dict, "module")?;
            let function = extract_string(dict, "function")?;
            let type_args = extract_string_list(dict, "type_args")?;
            let args = extract_json_list(dict, "args")?;
            Ok(SandboxRequest::CallFunction {
                package,
                module,
                function,
                type_args,
                args,
            })
        }

        // =================================================================
        // State Management
        // =================================================================
        "get_state" => Ok(SandboxRequest::GetState),

        "reset" => Ok(SandboxRequest::Reset),

        "get_clock" => Ok(SandboxRequest::GetClock),

        "set_clock" => {
            let timestamp_ms = dict
                .get_item("timestamp_ms")?
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing 'timestamp_ms'")
                })?
                .extract::<u64>()?;
            Ok(SandboxRequest::SetClock { timestamp_ms })
        }

        // =================================================================
        // Encoding/Decoding
        // =================================================================
        "encode_bcs" => {
            let type_str = extract_string(dict, "type_str")?;
            let value = match dict.get_item("value")? {
                Some(v) => py_to_json(&v)?,
                None => {
                    return Err(PyErr::new::<pyo3::exceptions::PyKeyError, _>(
                        "Missing 'value'",
                    ))
                }
            };
            Ok(SandboxRequest::EncodeBcs { type_str, value })
        }

        "decode_bcs" => {
            let type_str = extract_string(dict, "type_str")?;
            let bytes_hex = extract_string(dict, "bytes_hex")?;
            Ok(SandboxRequest::DecodeBcs {
                type_str,
                bytes_hex,
            })
        }

        "encode_vector" => {
            let element_type = extract_string(dict, "element_type")?;
            let values = extract_json_list(dict, "values")?;
            Ok(SandboxRequest::EncodeVector {
                element_type,
                values,
            })
        }

        // =================================================================
        // Utilities
        // =================================================================
        "generate_id" => Ok(SandboxRequest::GenerateId),

        "parse_address" => {
            let address = extract_string(dict, "address")?;
            Ok(SandboxRequest::ParseAddress { address })
        }

        "format_address" => {
            let address = extract_string(dict, "address")?;
            let format = extract_optional_string(dict, "format")?;
            Ok(SandboxRequest::FormatAddress { address, format })
        }

        "compute_hash" => {
            let bytes_hex = extract_string(dict, "bytes_hex")?;
            let algorithm = extract_optional_string(dict, "algorithm")?;
            Ok(SandboxRequest::ComputeHash {
                bytes_hex,
                algorithm,
            })
        }

        "convert_number" => {
            let value = extract_string(dict, "value")?;
            let from_type = extract_string(dict, "from_type")?;
            let to_type = extract_string(dict, "to_type")?;
            Ok(SandboxRequest::ConvertNumber {
                value,
                from_type,
                to_type,
            })
        }

        "parse_error" => {
            let error = extract_string(dict, "error")?;
            Ok(SandboxRequest::ParseError { error })
        }

        // =================================================================
        // Coin Operations
        // =================================================================
        "register_coin" => {
            let coin_type = extract_string(dict, "coin_type")?;
            let decimals = dict
                .get_item("decimals")?
                .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyKeyError, _>("Missing 'decimals'"))?
                .extract::<u8>()?;
            let symbol = extract_string(dict, "symbol")?;
            let name = extract_string(dict, "name")?;
            Ok(SandboxRequest::RegisterCoin {
                coin_type,
                decimals,
                symbol,
                name,
            })
        }

        "get_coin_metadata" => {
            let coin_type = extract_string(dict, "coin_type")?;
            Ok(SandboxRequest::GetCoinMetadata { coin_type })
        }

        "list_coins" => Ok(SandboxRequest::ListCoins),

        // =================================================================
        // Framework Cache
        // =================================================================
        "is_framework_cached" => Ok(SandboxRequest::IsFrameworkCached),

        "ensure_framework_cached" => Ok(SandboxRequest::EnsureFrameworkCached),

        // =================================================================
        // Events
        // =================================================================
        "list_events" => Ok(SandboxRequest::ListEvents),

        "get_events_by_type" => {
            let type_prefix = extract_string(dict, "type_prefix")?;
            Ok(SandboxRequest::GetEventsByType { type_prefix })
        }

        "get_last_tx_events" => Ok(SandboxRequest::GetLastTxEvents),

        "clear_events" => Ok(SandboxRequest::ClearEvents),

        // =================================================================
        // Shared Object Versioning
        // =================================================================
        "get_lamport_clock" => Ok(SandboxRequest::GetLamportClock),

        "get_shared_object_info" => {
            let object_id = extract_string(dict, "object_id")?;
            Ok(SandboxRequest::GetSharedObjectInfo { object_id })
        }

        "list_shared_locks" => Ok(SandboxRequest::ListSharedLocks),

        "advance_lamport_clock" => Ok(SandboxRequest::AdvanceLamportClock),

        // =================================================================
        // Struct Inspection
        // =================================================================
        "inspect_struct" => {
            let package = extract_string(dict, "package")?;
            let module = extract_optional_string(dict, "module")?;
            let struct_name = extract_optional_string(dict, "struct_name")?;
            Ok(SandboxRequest::InspectStruct {
                package,
                module,
                struct_name,
            })
        }

        "get_system_object_info" => {
            let object_name = extract_string(dict, "object_name")?;
            Ok(SandboxRequest::GetSystemObjectInfo { object_name })
        }

        "list_available_tools" => Ok(SandboxRequest::ListAvailableTools),

        _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "Unknown action: '{}'. Use 'list_available_tools' to see valid actions.",
            action
        ))),
    }
}

// =============================================================================
// Helper functions for complex type extraction
// =============================================================================

/// Extract a HashMap<String, serde_json::Value> from a dict field (for object fields).
fn extract_field_map(
    dict: &Bound<'_, PyDict>,
    key: &str,
) -> PyResult<std::collections::HashMap<String, serde_json::Value>> {
    match dict.get_item(key)? {
        Some(v) => {
            let inner_dict = v.downcast::<PyDict>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                    "Field '{}' must be a dict",
                    key
                ))
            })?;
            let mut map = std::collections::HashMap::new();
            for (k, v) in inner_dict.iter() {
                let key: String = k.extract()?;
                let val = py_to_json(&v)?;
                map.insert(key, val);
            }
            Ok(map)
        }
        None => Ok(std::collections::HashMap::new()),
    }
}

/// Extract a HashMap<String, String> from a dict field.
fn extract_string_map(
    dict: &Bound<'_, PyDict>,
    key: &str,
) -> PyResult<std::collections::HashMap<String, String>> {
    match dict.get_item(key)? {
        Some(v) => {
            let inner_dict = v.downcast::<PyDict>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                    "Field '{}' must be a dict",
                    key
                ))
            })?;
            let mut map = std::collections::HashMap::new();
            for (k, v) in inner_dict.iter() {
                let key: String = k.extract()?;
                let val: String = v.extract()?;
                map.insert(key, val);
            }
            Ok(map)
        }
        None => Ok(std::collections::HashMap::new()),
    }
}

/// Extract a list of JSON values from a dict field.
fn extract_json_list(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Vec<serde_json::Value>> {
    match dict.get_item(key)? {
        Some(v) => {
            let list = v.downcast::<PyList>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                    "Field '{}' must be a list",
                    key
                ))
            })?;
            list.iter().map(|item| py_to_json(&item)).collect()
        }
        None => Ok(Vec::new()),
    }
}

/// Extract PTB inputs from the 'inputs' field.
fn extract_ptb_inputs(dict: &Bound<'_, PyDict>) -> PyResult<Vec<PtbInput>> {
    match dict.get_item("inputs")? {
        Some(v) => {
            let list = v.downcast::<PyList>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("'inputs' must be a list")
            })?;
            list.iter()
                .enumerate()
                .map(|(i, item)| {
                    let input_dict = item.downcast::<PyDict>().map_err(|_| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                            "inputs[{}] must be a dict",
                            i
                        ))
                    })?;
                    parse_ptb_input(&input_dict, i)
                })
                .collect()
        }
        None => Ok(Vec::new()),
    }
}

/// Parse a single PTB input from a dict.
fn parse_ptb_input(dict: &Bound<'_, PyDict>, index: usize) -> PyResult<PtbInput> {
    let input_type: String = dict
        .get_item("type")?
        .ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "inputs[{}]: missing 'type' field",
                index
            ))
        })?
        .extract()?;

    match input_type.as_str() {
        "pure" => {
            let value = match dict.get_item("value")? {
                Some(v) => py_to_json(&v)?,
                None => {
                    return Err(PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                        "inputs[{}]: missing 'value' for pure input",
                        index
                    )))
                }
            };
            let value_type = extract_string(dict, "value_type").map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                    "inputs[{}]: missing 'value_type' for pure input",
                    index
                ))
            })?;
            Ok(PtbInput::Pure { value, value_type })
        }

        "object" => {
            let object_id = extract_string(dict, "object_id").map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                    "inputs[{}]: missing 'object_id' for object input",
                    index
                ))
            })?;
            let mode = extract_optional_string(dict, "mode")?;
            Ok(PtbInput::Object { object_id, mode })
        }

        "gas" => {
            let budget = dict
                .get_item("budget")?
                .map(|v| v.extract::<u64>())
                .transpose()?
                .unwrap_or(10_000_000); // Default 10 SUI
            Ok(PtbInput::Gas { budget })
        }

        "witness" => {
            let witness_type = extract_string(dict, "witness_type").map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                    "inputs[{}]: missing 'witness_type' for witness input",
                    index
                ))
            })?;
            Ok(PtbInput::Witness { witness_type })
        }

        _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "inputs[{}]: unknown input type '{}'",
            index, input_type
        ))),
    }
}

/// Extract PTB commands from the 'commands' field.
fn extract_ptb_commands(dict: &Bound<'_, PyDict>) -> PyResult<Vec<PtbCommand>> {
    match dict.get_item("commands")? {
        Some(v) => {
            let list = v.downcast::<PyList>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>("'commands' must be a list")
            })?;
            list.iter()
                .enumerate()
                .map(|(i, item)| {
                    let cmd_dict = item.downcast::<PyDict>().map_err(|_| {
                        PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                            "commands[{}] must be a dict",
                            i
                        ))
                    })?;
                    parse_ptb_command(&cmd_dict, i)
                })
                .collect()
        }
        None => Ok(Vec::new()),
    }
}

/// Parse a single PTB command from a dict.
fn parse_ptb_command(dict: &Bound<'_, PyDict>, index: usize) -> PyResult<PtbCommand> {
    let cmd_type: String = dict
        .get_item("type")?
        .ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "commands[{}]: missing 'type' field",
                index
            ))
        })?
        .extract()?;

    match cmd_type.as_str() {
        "move_call" => {
            let package = extract_string(dict, "package")?;
            let module = extract_string(dict, "module")?;
            let function = extract_string(dict, "function")?;
            let type_args = extract_string_list(dict, "type_args")?;
            let args = extract_ptb_args(dict, "args")?;
            Ok(PtbCommand::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            })
        }

        "transfer_objects" => {
            let objects = extract_ptb_args(dict, "objects")?;
            let recipient = extract_single_ptb_arg(dict, "recipient", index)?;
            Ok(PtbCommand::TransferObjects { objects, recipient })
        }

        "split_coins" => {
            let coin = extract_single_ptb_arg(dict, "coin", index)?;
            let amounts = extract_ptb_args(dict, "amounts")?;
            Ok(PtbCommand::SplitCoins { coin, amounts })
        }

        "merge_coins" => {
            let target = extract_single_ptb_arg(dict, "target", index)?;
            let sources = extract_ptb_args(dict, "sources")?;
            Ok(PtbCommand::MergeCoins { target, sources })
        }

        "make_move_vec" => {
            let element_type = extract_optional_string(dict, "element_type")?;
            let elements = extract_ptb_args(dict, "elements")?;
            Ok(PtbCommand::MakeMoveVec {
                element_type,
                elements,
            })
        }

        "publish" => {
            let modules = extract_string_list(dict, "modules")?;
            let dependencies = extract_string_list(dict, "dependencies")?;
            Ok(PtbCommand::Publish {
                modules,
                dependencies,
            })
        }

        "upgrade" => {
            let modules = extract_string_list(dict, "modules")?;
            let package = extract_string(dict, "package")?;
            let ticket = extract_single_ptb_arg(dict, "ticket", index)?;
            Ok(PtbCommand::Upgrade {
                modules,
                package,
                ticket,
            })
        }

        "receive" => {
            let object_id = extract_string(dict, "object_id")?;
            let object_type = extract_optional_string(dict, "object_type")?;
            Ok(PtbCommand::Receive {
                object_id,
                object_type,
            })
        }

        _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "commands[{}]: unknown command type '{}'",
            index, cmd_type
        ))),
    }
}

/// Extract a list of PtbArg from a dict field.
fn extract_ptb_args(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Vec<PtbArg>> {
    match dict.get_item(key)? {
        Some(v) => {
            let list = v.downcast::<PyList>().map_err(|_| {
                PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                    "Field '{}' must be a list",
                    key
                ))
            })?;
            list.iter()
                .enumerate()
                .map(|(i, item)| parse_ptb_arg(&item, key, i))
                .collect()
        }
        None => Ok(Vec::new()),
    }
}

/// Extract a single PtbArg from a dict field.
fn extract_single_ptb_arg(
    dict: &Bound<'_, PyDict>,
    key: &str,
    cmd_index: usize,
) -> PyResult<PtbArg> {
    let item = dict.get_item(key)?.ok_or_else(|| {
        PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
            "commands[{}]: missing '{}' field",
            cmd_index, key
        ))
    })?;
    parse_ptb_arg(&item, key, 0)
}

/// Parse a PtbArg from a Python object.
/// Supports:
/// - Integer: Input(n)
/// - Dict with {cmd, idx}: Result{cmd, idx}
fn parse_ptb_arg(obj: &Bound<'_, PyAny>, field_name: &str, index: usize) -> PyResult<PtbArg> {
    // Try as integer first (Input reference)
    if let Ok(n) = obj.extract::<usize>() {
        return Ok(PtbArg::Input(n));
    }

    // Try as dict (Result reference)
    if let Ok(dict) = obj.downcast::<PyDict>() {
        if let (Some(cmd), Some(idx)) = (dict.get_item("cmd")?, dict.get_item("idx")?) {
            let cmd: usize = cmd.extract()?;
            let idx: usize = idx.extract()?;
            return Ok(PtbArg::Result { cmd, idx });
        }
    }

    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
        "{}[{}]: expected integer (input index) or {{cmd, idx}} (result reference)",
        field_name, index
    )))
}
