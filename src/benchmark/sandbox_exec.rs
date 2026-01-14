//! # Sandbox Execution Interface for LLM Integration
//!
//! This module provides a JSON-based interface for LLM agents to interact with
//! the Move VM sandbox. It supports:
//!
//! - Loading compiled Move modules from bytecode
//! - Creating objects with specific field values
//! - Executing PTBs (Programmable Transaction Blocks)
//! - Inspecting struct definitions
//!
//! The interface is designed to be called from Python via subprocess, with
//! JSON input/output for easy integration.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::args::SandboxExecArgs;
use crate::benchmark::package_builder::PackageBuilder;
use crate::benchmark::simulation::SimulationEnvironment;

/// Request format for sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum SandboxRequest {
    /// Load a compiled Move module from bytecode file(s).
    #[serde(rename = "load_module")]
    LoadModule {
        /// Path to the bytecode directory (containing .mv files).
        bytecode_path: String,
        /// Optional module name filter.
        module_name: Option<String>,
    },

    /// Create an object with specific field values.
    #[serde(rename = "create_object")]
    CreateObject {
        /// Full type string (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
        object_type: String,
        /// Field values as JSON (will be BCS encoded).
        fields: HashMap<String, serde_json::Value>,
        /// Optional specific object ID (hex string).
        object_id: Option<String>,
    },

    /// Execute a PTB with the given commands.
    #[serde(rename = "execute_ptb")]
    ExecutePtb {
        /// PTB inputs (pure values and object references).
        inputs: Vec<PtbInput>,
        /// PTB commands to execute.
        commands: Vec<PtbCommand>,
    },

    /// Get struct definition(s) from loaded modules.
    #[serde(rename = "inspect_struct")]
    InspectStruct {
        /// Package address (hex string, e.g., "0x2").
        package: String,
        /// Optional module name filter.
        module: Option<String>,
        /// Optional struct name filter.
        struct_name: Option<String>,
    },

    /// Get current sandbox state (loaded modules, objects).
    #[serde(rename = "get_state")]
    GetState,

    /// Reset sandbox to initial state.
    #[serde(rename = "reset")]
    Reset,

    /// Call a specific Move function directly.
    #[serde(rename = "call_function")]
    CallFunction {
        /// Package address.
        package: String,
        /// Module name.
        module: String,
        /// Function name.
        function: String,
        /// Type arguments (as strings).
        type_args: Vec<String>,
        /// Arguments (as JSON, will be BCS encoded).
        args: Vec<serde_json::Value>,
    },

    /// Register a custom coin with its metadata.
    #[serde(rename = "register_coin")]
    RegisterCoin {
        /// Full coin type (e.g., "0xabc::my_coin::MY_COIN").
        coin_type: String,
        /// Number of decimal places.
        decimals: u8,
        /// Coin symbol (e.g., "MYCOIN").
        symbol: String,
        /// Coin name (e.g., "My Coin").
        name: String,
    },

    /// Get coin metadata for a registered coin.
    #[serde(rename = "get_coin_metadata")]
    GetCoinMetadata {
        /// Full coin type (e.g., "0x2::sui::SUI").
        coin_type: String,
    },

    /// List all registered coins.
    #[serde(rename = "list_coins")]
    ListCoins,

    /// Inspect an object's current state (decode BCS to readable fields).
    #[serde(rename = "inspect_object")]
    InspectObject {
        /// Object ID (hex string).
        object_id: String,
    },

    /// List all objects in the sandbox with their types.
    #[serde(rename = "list_objects")]
    ListObjects,

    /// Get the current Clock timestamp.
    #[serde(rename = "get_clock")]
    GetClock,

    /// Advance the Clock to a new timestamp.
    #[serde(rename = "set_clock")]
    SetClock {
        /// New timestamp in milliseconds since Unix epoch.
        timestamp_ms: u64,
    },

    // ========================================================================
    // LLM Agent Tools - Additional introspection and utility actions
    // ========================================================================

    /// List all functions in a module.
    #[serde(rename = "list_functions")]
    ListFunctions {
        /// Module path (e.g., "0x2::coin").
        module_path: String,
    },

    /// List all structs in a module.
    #[serde(rename = "list_structs")]
    ListStructs {
        /// Module path (e.g., "0x2::coin").
        module_path: String,
    },

    /// Get detailed function information.
    #[serde(rename = "get_function_info")]
    GetFunctionInfo {
        /// Module path.
        module_path: String,
        /// Function name.
        function_name: String,
    },

    /// Find constructors for a type.
    #[serde(rename = "find_constructors")]
    FindConstructors {
        /// Full type path (e.g., "0x2::coin::Coin").
        type_path: String,
    },

    /// Search for types matching a pattern.
    #[serde(rename = "search_types")]
    SearchTypes {
        /// Pattern with * wildcard.
        pattern: String,
        /// Optional ability filter.
        ability_filter: Option<String>,
    },

    /// Search for functions matching a pattern.
    #[serde(rename = "search_functions")]
    SearchFunctions {
        /// Pattern with * wildcard.
        pattern: String,
        /// Only return entry functions.
        #[serde(default)]
        entry_only: bool,
    },

    /// Get system object information.
    #[serde(rename = "get_system_object_info")]
    GetSystemObjectInfo {
        /// Object name: "clock", "random", "deny_list", "system_state".
        object_name: String,
    },

    /// Validate a type string.
    #[serde(rename = "validate_type")]
    ValidateType {
        /// Type string to validate.
        type_str: String,
    },

    /// Encode a value to BCS.
    #[serde(rename = "encode_bcs")]
    EncodeBcs {
        /// Type string.
        type_str: String,
        /// Value to encode.
        value: serde_json::Value,
    },

    /// Decode BCS bytes.
    #[serde(rename = "decode_bcs")]
    DecodeBcs {
        /// Type string.
        type_str: String,
        /// Hex-encoded bytes.
        bytes_hex: String,
    },

    /// Disassemble a function to bytecode.
    #[serde(rename = "disassemble_function")]
    DisassembleFunction {
        /// Module path.
        module_path: String,
        /// Function name.
        function_name: String,
    },

    /// List all loaded modules.
    #[serde(rename = "list_modules")]
    ListModules,

    /// Compile Move source code to bytecode.
    #[serde(rename = "compile_move")]
    CompileMove {
        /// Package name (used for addresses).
        package_name: String,
        /// Module name (without .move extension).
        module_name: String,
        /// Move source code.
        source: String,
    },

    /// Get struct type definition details.
    #[serde(rename = "get_struct_info")]
    GetStructInfo {
        /// Full type path like "0x2::coin::Coin".
        type_path: String,
    },

    /// Create a test object in the sandbox.
    #[serde(rename = "create_test_object")]
    CreateTestObject {
        /// Type of object to create.
        type_tag: String,
        /// Initial value (JSON).
        value: serde_json::Value,
    },

    /// Set the sandbox clock time.
    #[serde(rename = "set_time")]
    SetTime {
        /// Timestamp in milliseconds.
        timestamp_ms: u64,
    },

    /// Get current sandbox clock time.
    #[serde(rename = "get_time")]
    GetTime,
}

/// PTB input specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PtbInput {
    /// Pure value (will be BCS encoded).
    #[serde(rename = "pure")]
    Pure {
        /// The value as JSON.
        value: serde_json::Value,
        /// Type hint for encoding (e.g., "u64", "address", "vector<u8>").
        value_type: String,
    },

    /// Object reference (immutable).
    #[serde(rename = "object")]
    Object {
        /// Object ID (hex string).
        object_id: String,
        /// Object access mode: "immutable", "mutable", "owned", "shared" (default: inferred).
        #[serde(default)]
        mode: Option<String>,
    },

    /// Gas coin input (for gas payment simulation).
    #[serde(rename = "gas")]
    Gas {
        /// Gas budget in MIST (1 SUI = 10^9 MIST).
        budget: u64,
    },

    /// One-Time Witness (OTW) input for create_currency and similar functions.
    /// The witness is synthesized as a placeholder value that the VM accepts in test mode.
    #[serde(rename = "witness")]
    Witness {
        /// Full type path of the OTW type (e.g., "0xabc::my_coin::MY_COIN").
        witness_type: String,
    },
}

/// PTB command specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PtbCommand {
    /// Move call.
    #[serde(rename = "move_call")]
    MoveCall {
        /// Package address.
        package: String,
        /// Module name.
        module: String,
        /// Function name.
        function: String,
        /// Type arguments.
        type_args: Vec<String>,
        /// Argument indices (into inputs array) or nested results.
        args: Vec<PtbArg>,
    },

    /// Transfer objects.
    #[serde(rename = "transfer_objects")]
    TransferObjects {
        /// Object argument indices.
        objects: Vec<PtbArg>,
        /// Recipient address (input index or pure).
        recipient: PtbArg,
    },

    /// Split coins.
    #[serde(rename = "split_coins")]
    SplitCoins {
        /// Coin to split (input index).
        coin: PtbArg,
        /// Amounts to split (input indices).
        amounts: Vec<PtbArg>,
    },

    /// Merge coins.
    #[serde(rename = "merge_coins")]
    MergeCoins {
        /// Target coin.
        target: PtbArg,
        /// Coins to merge into target.
        sources: Vec<PtbArg>,
    },

    /// Create a vector from elements.
    #[serde(rename = "make_move_vec")]
    MakeMoveVec {
        /// Element type (e.g., "u64", "0x2::coin::Coin<0x2::sui::SUI>").
        element_type: Option<String>,
        /// Elements (input indices or results).
        elements: Vec<PtbArg>,
    },

    /// Publish new modules.
    #[serde(rename = "publish")]
    Publish {
        /// Module bytecode as base64-encoded strings.
        modules: Vec<String>,
        /// Dependency package IDs.
        dependencies: Vec<String>,
    },

    /// Upgrade existing package.
    #[serde(rename = "upgrade")]
    Upgrade {
        /// Module bytecode as base64-encoded strings.
        modules: Vec<String>,
        /// Package ID being upgraded.
        package: String,
        /// Upgrade ticket (input index).
        ticket: PtbArg,
    },

    /// Receive an object from a previous transaction (for transaction chaining).
    #[serde(rename = "receive")]
    Receive {
        /// Object ID to receive (hex string).
        object_id: String,
        /// Expected object type (optional, for validation).
        object_type: Option<String>,
    },
}

/// PTB argument reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PtbArg {
    /// Reference to an input by index.
    Input(usize),
    /// Reference to a result: (command_index, result_index).
    Result { cmd: usize, idx: usize },
}

/// Response format from sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResponse {
    /// Whether the operation succeeded.
    pub success: bool,

    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Error category if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,

    /// Abort code if contract aborted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abort_code: Option<u64>,

    /// Module where abort occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abort_module: Option<String>,

    /// Result data (depends on operation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,

    /// Transaction effects (for PTB execution).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<TransactionEffectsResponse>,

    /// Emitted events (for PTB execution).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<EventResponse>>,

    /// Gas used in MIST (1 SUI = 10^9 MIST).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<u64>,

    /// Index of the command that failed (0-based), if execution failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command_index: Option<usize>,

    /// Description of the failed command (e.g., "MoveCall 0x2::coin::split").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command_description: Option<String>,

    /// Number of commands that succeeded before the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands_succeeded: Option<usize>,
}

/// Transaction effects in response format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEffectsResponse {
    /// Objects created by this transaction.
    pub created: Vec<ObjectEffectResponse>,
    /// Objects mutated by this transaction.
    pub mutated: Vec<ObjectEffectResponse>,
    /// Objects deleted by this transaction.
    pub deleted: Vec<String>,
    /// Objects wrapped by this transaction.
    pub wrapped: Vec<String>,
    /// Objects unwrapped by this transaction.
    pub unwrapped: Vec<ObjectEffectResponse>,
    /// Return values from each command (hex-encoded BCS bytes).
    /// Each entry corresponds to a command in execution order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_values: Option<Vec<CommandReturnValues>>,
}

/// Return values from a single PTB command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandReturnValues {
    /// Command index (0-based).
    pub command_index: usize,
    /// Return values as hex-encoded BCS bytes.
    pub values: Vec<String>,
    /// Number of return values.
    pub count: usize,
}

/// Individual object effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectEffectResponse {
    /// Object ID.
    pub id: String,
    /// Object type (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    /// Owner after the transaction.
    pub owner: String,
    /// Object version after the transaction.
    pub version: u64,
}

/// Emitted event in response format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventResponse {
    /// Event type tag.
    pub event_type: String,
    /// Event data as hex.
    pub data_hex: String,
    /// Sequence number.
    pub sequence: u64,
}

impl SandboxResponse {
    pub fn success() -> Self {
        Self {
            success: true,
            error: None,
            error_category: None,
            abort_code: None,
            abort_module: None,
            data: None,
            effects: None,
            events: None,
            gas_used: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: None,
        }
    }

    pub fn success_with_data(data: serde_json::Value) -> Self {
        Self {
            success: true,
            error: None,
            error_category: None,
            abort_code: None,
            abort_module: None,
            data: Some(data),
            effects: None,
            events: None,
            gas_used: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: None,
        }
    }

    pub fn success_with_effects(
        effects: TransactionEffectsResponse,
        events: Vec<EventResponse>,
        gas_used: u64,
    ) -> Self {
        Self {
            success: true,
            error: None,
            error_category: None,
            abort_code: None,
            abort_module: None,
            data: None,
            effects: Some(effects),
            events: Some(events),
            gas_used: Some(gas_used),
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(message.into()),
            error_category: None,
            abort_code: None,
            abort_module: None,
            data: None,
            effects: None,
            events: None,
            gas_used: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: None,
        }
    }

    pub fn error_with_category(message: impl Into<String>, category: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(message.into()),
            error_category: Some(category.into()),
            abort_code: None,
            abort_module: None,
            data: None,
            effects: None,
            events: None,
            gas_used: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: None,
        }
    }

    pub fn abort(code: u64, module: Option<String>, message: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(message.into()),
            error_category: Some("ContractAbort".to_string()),
            abort_code: Some(code),
            abort_module: module,
            data: None,
            effects: None,
            events: None,
            gas_used: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: None,
        }
    }
}

/// Struct definition for inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructDef {
    pub package: String,
    pub module: String,
    pub name: String,
    pub abilities: Vec<String>,
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeParam {
    pub name: String,
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub field_type: String,
}

/// Sandbox state for LLM context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxState {
    pub loaded_packages: Vec<String>,
    pub loaded_modules: Vec<ModuleInfo>,
    pub created_objects: Vec<ObjectInfo>,
    pub sender: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub package: String,
    pub name: String,
    pub struct_count: usize,
    pub function_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub id: String,
    pub object_type: String,
}

/// Execute a sandbox request.
pub fn execute_request(
    env: &mut SimulationEnvironment,
    request: &SandboxRequest,
    verbose: bool,
) -> SandboxResponse {
    match request {
        SandboxRequest::LoadModule { bytecode_path, module_name } => {
            execute_load_module(env, bytecode_path, module_name.as_deref(), verbose)
        }
        SandboxRequest::CreateObject { object_type, fields, object_id } => {
            execute_create_object(env, object_type, fields, object_id.as_deref(), verbose)
        }
        SandboxRequest::ExecutePtb { inputs, commands } => {
            execute_ptb_command(env, inputs, commands, verbose)
        }
        SandboxRequest::InspectStruct { package, module, struct_name } => {
            execute_inspect_struct(env, package, module.as_deref(), struct_name.as_deref(), verbose)
        }
        SandboxRequest::GetState => {
            execute_get_state(env, verbose)
        }
        SandboxRequest::Reset => {
            execute_reset(env, verbose)
        }
        SandboxRequest::CallFunction { package, module, function, type_args, args } => {
            execute_call_function(env, package, module, function, type_args, args, verbose)
        }
        SandboxRequest::RegisterCoin { coin_type, decimals, symbol, name } => {
            execute_register_coin(env, coin_type, *decimals, symbol, name, verbose)
        }
        SandboxRequest::GetCoinMetadata { coin_type } => {
            execute_get_coin_metadata(env, coin_type, verbose)
        }
        SandboxRequest::ListCoins => {
            execute_list_coins(env, verbose)
        }
        SandboxRequest::InspectObject { object_id } => {
            execute_inspect_object(env, object_id, verbose)
        }
        SandboxRequest::ListObjects => {
            execute_list_objects(env, verbose)
        }
        SandboxRequest::GetClock => {
            execute_get_clock(env, verbose)
        }
        SandboxRequest::SetClock { timestamp_ms } => {
            execute_set_clock(env, *timestamp_ms, verbose)
        }
        // New LLM agent tools
        SandboxRequest::ListModules => {
            execute_list_modules(env, verbose)
        }
        SandboxRequest::ListFunctions { module_path } => {
            execute_list_functions(env, module_path, verbose)
        }
        SandboxRequest::ListStructs { module_path } => {
            execute_list_structs(env, module_path, verbose)
        }
        SandboxRequest::GetFunctionInfo { module_path, function_name } => {
            execute_get_function_info(env, module_path, function_name, verbose)
        }
        SandboxRequest::FindConstructors { type_path } => {
            execute_find_constructors(env, type_path, verbose)
        }
        SandboxRequest::SearchTypes { pattern, ability_filter } => {
            execute_search_types(env, pattern, ability_filter.as_deref(), verbose)
        }
        SandboxRequest::SearchFunctions { pattern, entry_only } => {
            execute_search_functions(env, pattern, *entry_only, verbose)
        }
        SandboxRequest::GetSystemObjectInfo { object_name } => {
            execute_get_system_object_info(object_name, verbose)
        }
        SandboxRequest::ValidateType { type_str } => {
            execute_validate_type(type_str, verbose)
        }
        SandboxRequest::EncodeBcs { type_str, value } => {
            execute_encode_bcs(type_str, value, verbose)
        }
        SandboxRequest::DecodeBcs { type_str, bytes_hex } => {
            execute_decode_bcs(type_str, bytes_hex, verbose)
        }
        SandboxRequest::DisassembleFunction { module_path, function_name } => {
            execute_disassemble_function(env, module_path, function_name, verbose)
        }
        SandboxRequest::CompileMove { package_name, module_name, source } => {
            execute_compile_move(env, package_name, module_name, source, verbose)
        }
        SandboxRequest::GetStructInfo { type_path } => {
            execute_get_struct_info(env, type_path, verbose)
        }
        SandboxRequest::CreateTestObject { type_tag, value } => {
            execute_create_test_object(env, type_tag, value, verbose)
        }
        SandboxRequest::SetTime { timestamp_ms } => {
            execute_set_clock(env, *timestamp_ms, verbose)
        }
        SandboxRequest::GetTime => {
            execute_get_clock(env, verbose)
        }
    }
}

fn execute_load_module(
    env: &mut SimulationEnvironment,
    bytecode_path: &str,
    module_name: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    let path = Path::new(bytecode_path);

    if !path.exists() {
        return SandboxResponse::error(format!("Bytecode path does not exist: {}", bytecode_path));
    }

    // Load .mv files from directory
    let mut modules: Vec<(String, Vec<u8>)> = Vec::new();

    if path.is_dir() {
        // Read all .mv files in directory
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.extension().map_or(false, |e| e == "mv") {
                        let name = file_path.file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();

                        // Apply module name filter if specified
                        if let Some(filter) = module_name {
                            if !name.contains(filter) {
                                continue;
                            }
                        }

                        match std::fs::read(&file_path) {
                            Ok(bytes) => {
                                if verbose {
                                    eprintln!("Loading module: {} ({} bytes)", name, bytes.len());
                                }
                                modules.push((name, bytes));
                            }
                            Err(e) => {
                                return SandboxResponse::error(format!(
                                    "Failed to read {}: {}", file_path.display(), e
                                ));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                return SandboxResponse::error(format!("Failed to read directory: {}", e));
            }
        }
    } else {
        // Single file
        match std::fs::read(path) {
            Ok(bytes) => {
                let name = path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                modules.push((name, bytes));
            }
            Err(e) => {
                return SandboxResponse::error(format!("Failed to read file: {}", e));
            }
        }
    }

    if modules.is_empty() {
        return SandboxResponse::error("No .mv files found in bytecode path");
    }

    // Deploy to environment
    match env.deploy_package(modules.clone()) {
        Ok(address) => {
            SandboxResponse::success_with_data(serde_json::json!({
                "package_address": address.to_hex_literal(),
                "modules_loaded": modules.len(),
                "module_names": modules.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            }))
        }
        Err(e) => {
            SandboxResponse::error_with_category(
                format!("Failed to deploy package: {}", e),
                "DeploymentError",
            )
        }
    }
}

fn execute_create_object(
    env: &mut SimulationEnvironment,
    object_type: &str,
    fields: &HashMap<String, serde_json::Value>,
    object_id: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Creating object of type: {}", object_type);
        eprintln!("Fields: {:?}", fields);
    }

    // Parse object ID if provided
    let id = if let Some(id_str) = object_id {
        match parse_object_id(id_str) {
            Ok(id) => Some(id),
            Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
        }
    } else {
        None
    };

    // Create the object in the environment
    match env.create_object_from_json(object_type, fields, id) {
        Ok(created_id) => {
            SandboxResponse::success_with_data(serde_json::json!({
                "object_id": created_id.to_hex_literal(),
                "object_type": object_type,
            }))
        }
        Err(e) => {
            SandboxResponse::error_with_category(
                format!("Failed to create object: {}", e),
                "ObjectCreationError",
            )
        }
    }
}

fn execute_ptb_command(
    env: &mut SimulationEnvironment,
    inputs: &[PtbInput],
    commands: &[PtbCommand],
    verbose: bool,
) -> SandboxResponse {
    use crate::benchmark::ptb::{InputValue, Command as RealPtbCommand, Argument};
    use move_core_types::identifier::Identifier;
    use move_core_types::language_storage::TypeTag;

    if verbose {
        eprintln!("Executing PTB with {} inputs and {} commands", inputs.len(), commands.len());
    }

    // Track gas budget for simulation
    let mut _gas_budget: u64 = 50_000_000; // Default 50M MIST

    // Convert inputs
    let mut real_inputs: Vec<InputValue> = Vec::new();
    for input in inputs {
        match input {
            PtbInput::Pure { value, value_type } => {
                match encode_pure_value(value, value_type) {
                    Ok(bytes) => real_inputs.push(InputValue::Pure(bytes)),
                    Err(e) => return SandboxResponse::error(format!("Failed to encode input: {}", e)),
                }
            }
            PtbInput::Object { object_id, mode } => {
                match env.get_object_for_ptb_with_mode(object_id, mode.as_deref()) {
                    Ok(obj) => real_inputs.push(InputValue::Object(obj)),
                    Err(e) => return SandboxResponse::error(format!("Failed to get object {}: {}", object_id, e)),
                }
            }
            PtbInput::Gas { budget } => {
                _gas_budget = *budget;
                // Gas coin is a special input - create a SUI coin with the budget
                match env.create_gas_coin(*budget) {
                    Ok(obj) => real_inputs.push(InputValue::Object(obj)),
                    Err(e) => return SandboxResponse::error(format!("Failed to create gas coin: {}", e)),
                }
            }
            PtbInput::Witness { witness_type } => {
                // Synthesize OTW witness as a placeholder byte array.
                // The VM in test mode accepts this for OTW types.
                // OTW structs typically have no fields, so we just need a minimal BCS encoding.
                if verbose {
                    eprintln!("Synthesizing OTW witness for type: {}", witness_type);
                }
                // OTW is a unit struct with no fields, so BCS encoding is empty or minimal.
                // For Sui's VM in test mode, we use vec![1u8] as a placeholder marker.
                let witness_bytes = vec![1u8];
                real_inputs.push(InputValue::Pure(witness_bytes));
            }
        }
    }

    // Convert commands
    let mut real_commands: Vec<RealPtbCommand> = Vec::new();
    for cmd in commands {
        match cmd {
            PtbCommand::MoveCall { package, module, function, type_args, args } => {
                let pkg_addr = match AccountAddress::from_hex_literal(package) {
                    Ok(a) => a,
                    Err(e) => return SandboxResponse::error(format!("Invalid package address: {}", e)),
                };

                let module_id = match Identifier::new(module.as_str()) {
                    Ok(id) => id,
                    Err(e) => return SandboxResponse::error(format!("Invalid module name: {}", e)),
                };

                let function_id = match Identifier::new(function.as_str()) {
                    Ok(id) => id,
                    Err(e) => return SandboxResponse::error(format!("Invalid function name: {}", e)),
                };

                // Parse type args from strings
                let parsed_type_args: Vec<TypeTag> = match type_args.iter()
                    .map(|s| crate::benchmark::tx_replay::parse_type_tag(s))
                    .collect::<Result<Vec<_>, _>>() {
                        Ok(tags) => tags,
                        Err(e) => return SandboxResponse::error(format!("Invalid type argument: {}", e)),
                    };

                let converted_args: Vec<Argument> = args.iter().map(|a| convert_ptb_arg(a)).collect();

                real_commands.push(RealPtbCommand::MoveCall {
                    package: pkg_addr,
                    module: module_id,
                    function: function_id,
                    type_args: parsed_type_args,
                    args: converted_args,
                });
            }
            PtbCommand::TransferObjects { objects, recipient } => {
                real_commands.push(RealPtbCommand::TransferObjects {
                    objects: objects.iter().map(convert_ptb_arg).collect(),
                    address: convert_ptb_arg(recipient),
                });
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                real_commands.push(RealPtbCommand::SplitCoins {
                    coin: convert_ptb_arg(coin),
                    amounts: amounts.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::MergeCoins { target, sources } => {
                real_commands.push(RealPtbCommand::MergeCoins {
                    destination: convert_ptb_arg(target),
                    sources: sources.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::MakeMoveVec { element_type, elements } => {
                let type_tag = if let Some(type_str) = element_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => Some(tag),
                        Err(e) => return SandboxResponse::error(format!("Invalid element type: {}", e)),
                    }
                } else {
                    None
                };
                real_commands.push(RealPtbCommand::MakeMoveVec {
                    type_tag,
                    elements: elements.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::Publish { modules, dependencies } => {
                use base64::Engine;
                // Decode base64 modules
                let mut decoded_modules = Vec::new();
                for (i, b64) in modules.iter().enumerate() {
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => decoded_modules.push(bytes),
                        Err(e) => return SandboxResponse::error(format!("Invalid base64 in module {}: {}", i, e)),
                    }
                }

                // Parse dependency IDs
                let mut dep_ids = Vec::new();
                for dep in dependencies {
                    match AccountAddress::from_hex_literal(dep) {
                        Ok(addr) => dep_ids.push(addr),
                        Err(e) => return SandboxResponse::error(format!("Invalid dependency ID '{}': {}", dep, e)),
                    }
                }

                real_commands.push(RealPtbCommand::Publish {
                    modules: decoded_modules,
                    dep_ids,
                });
            }
            PtbCommand::Upgrade { modules, package, ticket } => {
                use base64::Engine;
                // Decode base64 modules
                let mut decoded_modules = Vec::new();
                for (i, b64) in modules.iter().enumerate() {
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => decoded_modules.push(bytes),
                        Err(e) => return SandboxResponse::error(format!("Invalid base64 in module {}: {}", i, e)),
                    }
                }

                // Parse package ID
                let pkg_id = match AccountAddress::from_hex_literal(package) {
                    Ok(addr) => addr,
                    Err(e) => return SandboxResponse::error(format!("Invalid package ID: {}", e)),
                };

                real_commands.push(RealPtbCommand::Upgrade {
                    modules: decoded_modules,
                    package: pkg_id,
                    ticket: convert_ptb_arg(ticket),
                });
            }
            PtbCommand::Receive { object_id, object_type } => {
                // Parse object ID
                let obj_id = match AccountAddress::from_hex_literal(object_id) {
                    Ok(addr) => addr,
                    Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
                };

                // Parse type if provided
                let type_tag = if let Some(type_str) = object_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => Some(tag),
                        Err(e) => return SandboxResponse::error(format!("Invalid object type: {}", e)),
                    }
                } else {
                    None
                };

                real_commands.push(RealPtbCommand::Receive {
                    object_id: obj_id,
                    object_type: type_tag,
                });
            }
        }
    }

    // Execute
    let result = env.execute_ptb(real_inputs, real_commands);

    if result.success {
        // Build enhanced response with full effects
        if let Some(ref effects) = result.effects {
            let effects_response = build_effects_response(effects);
            let events_response = build_events_response(&effects.events);
            SandboxResponse::success_with_effects(effects_response, events_response, effects.gas_used)
        } else {
            SandboxResponse::success_with_data(serde_json::json!({
                "status": "success",
            }))
        }
    } else if let Some(ref error) = result.error {
        let mut response = match error {
            crate::benchmark::simulation::SimulationError::ContractAbort { abort_code, module, function, .. } => {
                SandboxResponse::abort(
                    *abort_code,
                    Some(format!("{}::{}", module, function)),
                    format!("Contract abort in {}::{} with code {}", module, function, abort_code),
                )
            }
            _ => SandboxResponse::error_with_category(
                error.to_string(),
                categorize_simulation_error(error),
            ),
        };
        // Add command failure context
        response.failed_command_index = result.failed_command_index;
        response.failed_command_description = result.failed_command_description.clone();
        response.commands_succeeded = if result.commands_succeeded > 0 {
            Some(result.commands_succeeded)
        } else {
            None
        };
        response
    } else {
        let mut response = SandboxResponse::error("Unknown execution error");
        response.failed_command_index = result.failed_command_index;
        response.failed_command_description = result.failed_command_description.clone();
        response.commands_succeeded = if result.commands_succeeded > 0 {
            Some(result.commands_succeeded)
        } else {
            None
        };
        response
    }
}

/// Build transaction effects response from internal effects.
fn build_effects_response(effects: &crate::benchmark::ptb::TransactionEffects) -> TransactionEffectsResponse {
    use crate::benchmark::ptb::{Owner, ObjectChange};

    let owner_to_string = |owner: &Owner| -> String {
        match owner {
            Owner::Address(addr) => format!("address:{}", addr.to_hex_literal()),
            Owner::Shared => "shared".to_string(),
            Owner::Immutable => "immutable".to_string(),
        }
    };

    // Extract detailed info from object_changes
    let mut created = Vec::new();
    let mut mutated = Vec::new();
    let mut deleted = Vec::new();
    let mut wrapped = Vec::new();
    let mut unwrapped = Vec::new();

    // Helper to format TypeTag as string
    let type_to_string = |t: &Option<move_core_types::language_storage::TypeTag>| -> Option<String> {
        t.as_ref().map(|tag| format!("{}", tag))
    };

    for change in &effects.object_changes {
        match change {
            ObjectChange::Created { id, owner, object_type } => {
                created.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 1,
                });
            }
            ObjectChange::Mutated { id, owner, object_type } => {
                mutated.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 2,
                });
            }
            ObjectChange::Deleted { id, .. } => {
                deleted.push(id.to_hex_literal());
            }
            ObjectChange::Wrapped { id, .. } => {
                wrapped.push(id.to_hex_literal());
            }
            ObjectChange::Unwrapped { id, owner, object_type } => {
                unwrapped.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 1,
                });
            }
        }
    }

    // If no object_changes, fall back to simple lists
    if created.is_empty() && !effects.created.is_empty() {
        created = effects.created.iter().map(|id| ObjectEffectResponse {
            id: id.to_hex_literal(),
            object_type: None,
            owner: "unknown".to_string(),
            version: 1,
        }).collect();
    }
    if mutated.is_empty() && !effects.mutated.is_empty() {
        mutated = effects.mutated.iter().map(|id| ObjectEffectResponse {
            id: id.to_hex_literal(),
            object_type: None,
            owner: "unknown".to_string(),
            version: 2,
        }).collect();
    }
    if deleted.is_empty() && !effects.deleted.is_empty() {
        deleted = effects.deleted.iter().map(|id| id.to_hex_literal()).collect();
    }
    if wrapped.is_empty() && !effects.wrapped.is_empty() {
        wrapped = effects.wrapped.iter().map(|id| id.to_hex_literal()).collect();
    }
    if unwrapped.is_empty() && !effects.unwrapped.is_empty() {
        unwrapped = effects.unwrapped.iter().map(|id| ObjectEffectResponse {
            id: id.to_hex_literal(),
            object_type: None,
            owner: "unknown".to_string(),
            version: 1,
        }).collect();
    }

    // Build return values from effects
    let return_values: Option<Vec<CommandReturnValues>> = if effects.return_values.is_empty() {
        None
    } else {
        let values: Vec<CommandReturnValues> = effects.return_values
            .iter()
            .enumerate()
            .map(|(i, vals)| CommandReturnValues {
                command_index: i,
                values: vals.iter().map(|v| hex::encode(v)).collect(),
                count: vals.len(),
            })
            .collect();
        Some(values)
    };

    TransactionEffectsResponse {
        created,
        mutated,
        deleted,
        wrapped,
        unwrapped,
        return_values,
    }
}

/// Build events response from emitted events.
fn build_events_response(events: &[crate::benchmark::natives::EmittedEvent]) -> Vec<EventResponse> {
    events.iter().map(|e| EventResponse {
        event_type: e.type_tag.clone(),
        data_hex: hex::encode(&e.data),
        sequence: e.sequence,
    }).collect()
}

fn execute_inspect_struct(
    env: &mut SimulationEnvironment,
    package: &str,
    module: Option<&str>,
    struct_name: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Inspecting structs in package {}", package);
    }

    let structs = match env.get_struct_definitions(package, module, struct_name) {
        Ok(s) => s,
        Err(e) => return SandboxResponse::error(format!("Failed to get struct definitions: {}", e)),
    };

    let struct_defs: Vec<StructDef> = structs.into_iter().map(|s| StructDef {
        package: s.package,
        module: s.module,
        name: s.name,
        abilities: s.abilities,
        type_params: s.type_params.into_iter().map(|tp| TypeParam {
            name: tp.name,
            constraints: tp.constraints,
        }).collect(),
        fields: s.fields.into_iter().map(|f| FieldDef {
            name: f.name,
            field_type: f.field_type,
        }).collect(),
    }).collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "structs": struct_defs,
        "count": struct_defs.len(),
    }))
}

fn execute_get_state(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting sandbox state");
    }

    let state = env.get_state_summary();

    SandboxResponse::success_with_data(serde_json::json!({
        "loaded_packages": state.loaded_packages,
        "loaded_modules": state.loaded_modules,
        "object_count": state.object_count,
        "sender": state.sender,
        "timestamp_ms": state.timestamp_ms,
    }))
}

fn execute_reset(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Resetting sandbox");
    }

    match env.reset() {
        Ok(_) => SandboxResponse::success(),
        Err(e) => SandboxResponse::error(format!("Failed to reset: {}", e)),
    }
}

fn execute_call_function(
    env: &mut SimulationEnvironment,
    package: &str,
    module: &str,
    function: &str,
    type_args: &[String],
    args: &[serde_json::Value],
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Calling {}::{}::{}", package, module, function);
    }

    match env.call_function(package, module, function, type_args, args) {
        Ok(result) => {
            SandboxResponse::success_with_data(serde_json::json!({
                "return_values": result.return_values,
                "gas_used": result.gas_used,
            }))
        }
        Err(e) => {
            SandboxResponse::error_with_category(
                format!("Function call failed: {}", e),
                "ExecutionError",
            )
        }
    }
}

fn execute_register_coin(
    env: &mut SimulationEnvironment,
    coin_type: &str,
    decimals: u8,
    symbol: &str,
    name: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Registering coin: {} ({} decimals)", coin_type, decimals);
    }

    env.register_coin(coin_type, decimals, symbol, name);

    SandboxResponse::success_with_data(serde_json::json!({
        "coin_type": coin_type,
        "decimals": decimals,
        "symbol": symbol,
        "name": name,
    }))
}

fn execute_get_coin_metadata(
    env: &SimulationEnvironment,
    coin_type: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting coin metadata for: {}", coin_type);
    }

    match env.get_coin_metadata(coin_type) {
        Some(metadata) => SandboxResponse::success_with_data(serde_json::json!({
            "coin_type": coin_type,
            "decimals": metadata.decimals,
            "symbol": metadata.symbol,
            "name": metadata.name,
        })),
        None => SandboxResponse::error(format!("Coin {} not found in registry", coin_type)),
    }
}

fn execute_list_coins(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing registered coins");
    }

    let coins: Vec<serde_json::Value> = env.list_registered_coins()
        .into_iter()
        .map(|m| serde_json::json!({
            "coin_type": m.type_tag,
            "decimals": m.decimals,
            "symbol": m.symbol,
            "name": m.name,
        }))
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "coins": coins,
        "count": coins.len(),
    }))
}

fn execute_inspect_object(
    env: &SimulationEnvironment,
    object_id: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Inspecting object: {}", object_id);
    }

    let addr = match AccountAddress::from_hex_literal(object_id) {
        Ok(a) => a,
        Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
    };

    let obj = match env.get_object(&addr) {
        Some(o) => o,
        None => return SandboxResponse::error(format!("Object {} not found", object_id)),
    };

    // Try to decode the BCS bytes based on the type
    let decoded_fields = decode_object_state(env, obj);

    let ownership = if obj.is_shared {
        "shared"
    } else if obj.is_immutable {
        "immutable"
    } else {
        "owned"
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "object_id": object_id,
        "type": format!("{}", obj.type_tag),
        "ownership": ownership,
        "version": obj.version,
        "bcs_bytes_hex": hex::encode(&obj.bcs_bytes),
        "bcs_bytes_len": obj.bcs_bytes.len(),
        "decoded": decoded_fields,
    }))
}

/// Decode object BCS state into readable JSON.
fn decode_object_state(
    env: &SimulationEnvironment,
    obj: &crate::benchmark::simulation::SimulatedObject,
) -> serde_json::Value {
    use move_core_types::language_storage::TypeTag;

    // Handle common types specially
    match &obj.type_tag {
        TypeTag::Struct(st) => {
            let type_str = format!("{}::{}::{}",
                st.address.to_hex_literal(),
                st.module,
                st.name
            );

            // Handle Coin<T> specially
            if st.address.to_hex_literal() == "0x2" && st.module.as_str() == "coin" && st.name.as_str() == "Coin" {
                return decode_coin(&obj.bcs_bytes, &st.type_params);
            }

            // Handle Balance<T> specially
            if st.address.to_hex_literal() == "0x2" && st.module.as_str() == "balance" && st.name.as_str() == "Balance" {
                return decode_balance(&obj.bcs_bytes);
            }

            // Try to decode using struct definition from loaded modules
            if let Ok(defs) = env.get_struct_definitions(
                &st.address.to_hex_literal(),
                Some(st.module.as_str()),
                Some(st.name.as_str()),
            ) {
                if let Some(def) = defs.first() {
                    return decode_struct_with_definition(&obj.bcs_bytes, def);
                }
            }

            // Fallback: return raw info
            serde_json::json!({
                "type": type_str,
                "raw_hex": hex::encode(&obj.bcs_bytes),
                "note": "Could not decode struct fields - type definition not loaded"
            })
        }
        _ => {
            // For non-struct types, return raw
            serde_json::json!({
                "type": format!("{}", obj.type_tag),
                "raw_hex": hex::encode(&obj.bcs_bytes),
            })
        }
    }
}

/// Decode a Coin<T> object.
fn decode_coin(bcs_bytes: &[u8], type_params: &[move_core_types::language_storage::TypeTag]) -> serde_json::Value {
    // Coin<T> = { id: UID (32 bytes), balance: Balance<T> (8 bytes) }
    if bcs_bytes.len() < 40 {
        return serde_json::json!({
            "error": "Invalid Coin: too few bytes",
            "raw_hex": hex::encode(bcs_bytes),
        });
    }

    let id_bytes = &bcs_bytes[0..32];
    let balance_bytes = &bcs_bytes[32..40];
    let balance = u64::from_le_bytes(balance_bytes.try_into().unwrap_or([0; 8]));

    let coin_type = type_params.first()
        .map(|t| format!("{}", t))
        .unwrap_or_else(|| "unknown".to_string());

    serde_json::json!({
        "type": format!("0x2::coin::Coin<{}>", coin_type),
        "id": format!("0x{}", hex::encode(id_bytes)),
        "balance": balance,
        "coin_type": coin_type,
    })
}

/// Decode a Balance<T> object.
fn decode_balance(bcs_bytes: &[u8]) -> serde_json::Value {
    // Balance<T> = { value: u64 (8 bytes) }
    if bcs_bytes.len() < 8 {
        return serde_json::json!({
            "error": "Invalid Balance: too few bytes",
            "raw_hex": hex::encode(bcs_bytes),
        });
    }

    let value = u64::from_le_bytes(bcs_bytes[0..8].try_into().unwrap_or([0; 8]));

    serde_json::json!({
        "type": "0x2::balance::Balance",
        "value": value,
    })
}

/// Decode a struct using its definition.
fn decode_struct_with_definition(
    bcs_bytes: &[u8],
    def: &crate::benchmark::simulation::StructDefinition,
) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    let mut offset = 0;

    for field in &def.fields {
        let (value, consumed) = decode_field_value(&bcs_bytes[offset..], &field.field_type);
        fields.insert(field.name.clone(), value);
        offset += consumed;
        if offset >= bcs_bytes.len() {
            break;
        }
    }

    serde_json::json!({
        "type": format!("{}::{}::{}", def.package, def.module, def.name),
        "fields": fields,
    })
}

/// Decode a single field value from BCS bytes.
/// Returns (decoded_value, bytes_consumed).
fn decode_field_value(bytes: &[u8], field_type: &str) -> (serde_json::Value, usize) {
    if bytes.is_empty() {
        return (serde_json::json!(null), 0);
    }

    match field_type {
        "u8" => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else {
                (serde_json::json!(bytes[0]), 1)
            }
        }
        "u16" => {
            if let Some(arr) = bytes.get(0..2).and_then(|s| <[u8; 2]>::try_from(s).ok()) {
                (serde_json::json!(u16::from_le_bytes(arr)), 2)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u32" => {
            if let Some(arr) = bytes.get(0..4).and_then(|s| <[u8; 4]>::try_from(s).ok()) {
                (serde_json::json!(u32::from_le_bytes(arr)), 4)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u64" => {
            if let Some(arr) = bytes.get(0..8).and_then(|s| <[u8; 8]>::try_from(s).ok()) {
                (serde_json::json!(u64::from_le_bytes(arr)), 8)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u128" => {
            if let Some(arr) = bytes.get(0..16).and_then(|s| <[u8; 16]>::try_from(s).ok()) {
                (serde_json::json!(u128::from_le_bytes(arr).to_string()), 16)  // String to avoid JSON precision issues
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "u256" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        "bool" => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else {
                (serde_json::json!(bytes[0] != 0), 1)
            }
        }
        "address" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        // UID is { id: { bytes: address } }
        "0x2::object::UID" | "UID" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        // ID is { bytes: address }
        "0x2::object::ID" | "ID" => {
            if let Some(slice) = bytes.get(0..32) {
                (serde_json::json!(format!("0x{}", hex::encode(slice))), 32)
            } else {
                (serde_json::json!(null), bytes.len())
            }
        }
        // String/vector<u8> - ULEB128 length prefix
        t if t.starts_with("vector<u8>") || t == "0x1::string::String" || t == "String" => {
            decode_vector_u8(bytes)
        }
        // Generic vector - ULEB128 length prefix
        t if t.starts_with("vector<") => {
            // For now, return raw hex for complex vectors
            (serde_json::json!({
                "raw_hex": hex::encode(bytes),
                "note": format!("Cannot fully decode {}", t)
            }), bytes.len())
        }
        // Option<T> - 0 for None, 1 + value for Some
        t if t.starts_with("0x1::option::Option<") => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else if bytes[0] == 0 {
                (serde_json::json!(null), 1)
            } else {
                // Extract inner type and decode
                let inner = t.strip_prefix("0x1::option::Option<")
                    .and_then(|s| s.strip_suffix(">"))
                    .unwrap_or("unknown");
                let (inner_val, consumed) = decode_field_value(&bytes[1..], inner);
                (inner_val, 1 + consumed)
            }
        }
        // Unknown type - return raw hex
        _ => {
            (serde_json::json!({
                "raw_hex": hex::encode(bytes),
                "type": field_type,
            }), bytes.len())
        }
    }
}

/// Decode a vector<u8> or String from BCS bytes.
fn decode_vector_u8(bytes: &[u8]) -> (serde_json::Value, usize) {
    // ULEB128 length prefix
    let mut offset = 0;
    let mut len: usize = 0;
    let mut shift = 0;

    while offset < bytes.len() {
        let byte = bytes[offset];
        len |= ((byte & 0x7f) as usize) << shift;
        offset += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }

    if offset + len > bytes.len() {
        return (serde_json::json!({
            "error": "Invalid vector: length exceeds available bytes",
            "raw_hex": hex::encode(bytes),
        }), bytes.len());
    }

    let data = &bytes[offset..offset + len];

    // Try to interpret as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        (serde_json::json!(s), offset + len)
    } else {
        (serde_json::json!(format!("0x{}", hex::encode(data))), offset + len)
    }
}

fn execute_list_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing all objects");
    }

    let objects: Vec<serde_json::Value> = env.list_objects()
        .into_iter()
        .map(|obj| {
            let ownership = if obj.is_shared {
                "shared"
            } else if obj.is_immutable {
                "immutable"
            } else {
                "owned"
            };

            serde_json::json!({
                "object_id": obj.id.to_hex_literal(),
                "type": format!("{}", obj.type_tag),
                "ownership": ownership,
                "version": obj.version,
                "bcs_bytes_len": obj.bcs_bytes.len(),
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "objects": objects,
        "count": objects.len(),
    }))
}

fn execute_get_clock(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting Clock timestamp");
    }

    let timestamp_ms = env.get_clock_timestamp_ms();

    // Convert to human-readable datetime string
    let datetime_str = {
        let seconds = (timestamp_ms / 1000) as i64;
        let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
        // Simple ISO 8601 format approximation
        let days_since_epoch = seconds / 86400;
        let remaining_seconds = seconds % 86400;
        let hours = remaining_seconds / 3600;
        let minutes = (remaining_seconds % 3600) / 60;
        let secs = remaining_seconds % 60;

        // Approximate year calculation (not accounting for leap years perfectly)
        let year = 1970 + (days_since_epoch / 365);

        format!("~{}-??-?? {:02}:{:02}:{:02}.{:03} UTC (approx)", year, hours, minutes, secs, nanos / 1_000_000)
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "clock_object_id": crate::benchmark::simulation::CLOCK_OBJECT_ID,
        "timestamp_ms": timestamp_ms,
        "datetime_approx": datetime_str,
    }))
}

fn execute_set_clock(env: &mut SimulationEnvironment, timestamp_ms: u64, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Setting Clock timestamp to {} ms", timestamp_ms);
    }

    match env.advance_clock(timestamp_ms) {
        Ok(()) => {
            SandboxResponse::success_with_data(serde_json::json!({
                "clock_object_id": crate::benchmark::simulation::CLOCK_OBJECT_ID,
                "timestamp_ms": timestamp_ms,
                "message": format!("Clock advanced to {} ms", timestamp_ms),
            }))
        }
        Err(e) => {
            SandboxResponse::error(format!("Failed to set clock: {}", e))
        }
    }
}

// Helper functions

fn parse_object_id(id_str: &str) -> Result<[u8; 32]> {
    let addr = AccountAddress::from_hex_literal(id_str)
        .map_err(|e| anyhow!("Invalid hex: {}", e))?;
    Ok(addr.into_bytes())
}

fn encode_pure_value(value: &serde_json::Value, value_type: &str) -> Result<Vec<u8>> {
    use bcs;

    match value_type {
        "u8" => {
            let v: u8 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u64" => {
            let v: u64 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "u128" => {
            let v: u128 = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "bool" => {
            let v: bool = serde_json::from_value(value.clone())?;
            Ok(bcs::to_bytes(&v)?)
        }
        "address" => {
            let s: String = serde_json::from_value(value.clone())?;
            let addr = AccountAddress::from_hex_literal(&s)?;
            Ok(bcs::to_bytes(&addr)?)
        }
        "vector<u8>" => {
            // Can be hex string or array of u8
            if let Some(s) = value.as_str() {
                let bytes = hex::decode(s.trim_start_matches("0x"))?;
                Ok(bcs::to_bytes(&bytes)?)
            } else {
                let v: Vec<u8> = serde_json::from_value(value.clone())?;
                Ok(bcs::to_bytes(&v)?)
            }
        }
        _ => Err(anyhow!("Unsupported value type: {}", value_type)),
    }
}

fn convert_ptb_arg(arg: &PtbArg) -> crate::benchmark::ptb::Argument {
    match arg {
        PtbArg::Input(idx) => crate::benchmark::ptb::Argument::Input(*idx as u16),
        PtbArg::Result { cmd, idx } => crate::benchmark::ptb::Argument::NestedResult(*cmd as u16, *idx as u16),
    }
}

// ============================================================================
// New LLM Agent Tool Implementations
// ============================================================================

fn execute_list_modules(
    env: &SimulationEnvironment,
    _verbose: bool,
) -> SandboxResponse {
    let modules = env.list_modules();
    SandboxResponse::success_with_data(serde_json::json!({
        "modules": modules,
        "count": modules.len(),
    }))
}

fn execute_list_functions(
    env: &SimulationEnvironment,
    module_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.list_functions(module_path) {
        Some(functions) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "functions": functions,
            "count": functions.len(),
        })),
        None => SandboxResponse::error(format!("Module not found: {}", module_path)),
    }
}

fn execute_list_structs(
    env: &SimulationEnvironment,
    module_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.list_structs(module_path) {
        Some(structs) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "structs": structs,
            "count": structs.len(),
        })),
        None => SandboxResponse::error(format!("Module not found: {}", module_path)),
    }
}

fn execute_get_function_info(
    env: &SimulationEnvironment,
    module_path: &str,
    function_name: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.get_function_info(module_path, function_name) {
        Some(info) => SandboxResponse::success_with_data(serde_json::to_value(info).unwrap_or_default()),
        None => SandboxResponse::error(format!(
            "Function not found: {}::{}", module_path, function_name
        )),
    }
}

fn execute_find_constructors(
    env: &SimulationEnvironment,
    type_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    let constructors = env.find_constructors(type_path);
    SandboxResponse::success_with_data(serde_json::json!({
        "type": type_path,
        "constructors": constructors,
        "count": constructors.len(),
    }))
}

fn execute_search_types(
    env: &SimulationEnvironment,
    pattern: &str,
    ability_filter: Option<&str>,
    _verbose: bool,
) -> SandboxResponse {
    let results = env.search_types(pattern, ability_filter);
    SandboxResponse::success_with_data(serde_json::json!({
        "pattern": pattern,
        "ability_filter": ability_filter,
        "matches": results,
        "count": results.len(),
    }))
}

fn execute_search_functions(
    env: &SimulationEnvironment,
    pattern: &str,
    entry_only: bool,
    _verbose: bool,
) -> SandboxResponse {
    let results = env.search_functions(pattern, entry_only);
    SandboxResponse::success_with_data(serde_json::json!({
        "pattern": pattern,
        "entry_only": entry_only,
        "matches": results,
        "count": results.len(),
    }))
}

fn execute_get_system_object_info(
    object_name: &str,
    _verbose: bool,
) -> SandboxResponse {
    let info = match object_name.to_lowercase().as_str() {
        "clock" => serde_json::json!({
            "name": "Clock",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000006",
            "short_id": "0x6",
            "type": "0x2::clock::Clock",
            "is_shared": true,
            "description": "Global clock for timestamp access. Use Clock::timestamp_ms() to get current time.",
            "fields": [
                {"name": "id", "type": "UID"},
                {"name": "timestamp_ms", "type": "u64"}
            ],
            "common_usage": "&Clock as function parameter for time-dependent logic"
        }),
        "random" => serde_json::json!({
            "name": "Random",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000008",
            "short_id": "0x8",
            "type": "0x2::random::Random",
            "is_shared": true,
            "description": "On-chain randomness source.",
            "common_usage": "&Random as function parameter for randomness-dependent logic"
        }),
        "deny_list" => serde_json::json!({
            "name": "DenyList",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000403",
            "short_id": "0x403",
            "type": "0x2::deny_list::DenyList",
            "is_shared": true,
            "description": "Global deny list for regulated coins."
        }),
        "system_state" => serde_json::json!({
            "name": "SuiSystemState",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000005",
            "short_id": "0x5",
            "type": "0x3::sui_system::SuiSystemState",
            "is_shared": true,
            "description": "Sui system state containing validator info, epoch data, etc."
        }),
        _ => {
            return SandboxResponse::error(format!(
                "Unknown system object: {}. Valid: clock, random, deny_list, system_state",
                object_name
            ));
        }
    };
    SandboxResponse::success_with_data(info)
}

fn execute_validate_type(
    type_str: &str,
    _verbose: bool,
) -> SandboxResponse {
    // Validate type string
    let info = match type_str {
        "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address" | "signer" => {
            serde_json::json!({
                "valid": true,
                "kind": "primitive",
                "type": type_str,
            })
        }
        _ if type_str.starts_with("vector<") && type_str.ends_with(">") => {
            let inner = &type_str[7..type_str.len()-1];
            serde_json::json!({
                "valid": true,
                "kind": "vector",
                "element_type": inner,
            })
        }
        _ if type_str.contains("::") => {
            serde_json::json!({
                "valid": true,
                "kind": "struct",
                "full_path": type_str,
            })
        }
        _ => {
            serde_json::json!({
                "valid": false,
                "error": format!("Unable to parse type: {}", type_str),
            })
        }
    };
    SandboxResponse::success_with_data(info)
}

fn execute_encode_bcs(
    type_str: &str,
    value: &serde_json::Value,
    _verbose: bool,
) -> SandboxResponse {
    match encode_pure_value(value, type_str) {
        Ok(bytes) => {
            let hex_str = hex::encode::<&[u8]>(&bytes);
            SandboxResponse::success_with_data(serde_json::json!({
                "type": type_str,
                "bytes_hex": hex_str,
                "bytes_len": bytes.len(),
            }))
        }
        Err(e) => SandboxResponse::error(format!("BCS encode failed: {}", e)),
    }
}

fn execute_decode_bcs(
    type_str: &str,
    bytes_hex: &str,
    _verbose: bool,
) -> SandboxResponse {
    let bytes = match hex::decode(bytes_hex.trim_start_matches("0x")) {
        Ok(b) => b,
        Err(e) => return SandboxResponse::error(format!("Invalid hex: {}", e)),
    };

    // Decode based on type
    let decoded: serde_json::Value = match type_str {
        "bool" => {
            if bytes.is_empty() {
                return SandboxResponse::error("Empty bytes");
            }
            serde_json::json!(bytes[0] != 0)
        }
        "u8" => {
            if bytes.is_empty() {
                return SandboxResponse::error("Empty bytes");
            }
            serde_json::json!(bytes[0])
        }
        "u64" => {
            if bytes.len() < 8 {
                return SandboxResponse::error("Not enough bytes for u64");
            }
            let val = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            serde_json::json!(val)
        }
        "address" => {
            if bytes.len() < 32 {
                return SandboxResponse::error("Not enough bytes for address");
            }
            serde_json::json!(format!("0x{}", hex::encode(&bytes[..32])))
        }
        _ => {
            return SandboxResponse::error(format!("Cannot decode type: {}", type_str));
        }
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "type": type_str,
        "value": decoded,
    }))
}

fn execute_disassemble_function(
    env: &SimulationEnvironment,
    module_path: &str,
    function_name: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.disassemble_function(module_path, function_name) {
        Some(disasm) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "function": function_name,
            "disassembly": disasm,
        })),
        None => SandboxResponse::error(format!(
            "Function not found or cannot disassemble: {}::{}", module_path, function_name
        )),
    }
}

fn execute_compile_move(
    env: &mut SimulationEnvironment,
    package_name: &str,
    module_name: &str,
    source: &str,
    verbose: bool,
) -> SandboxResponse {
    // Create package builder with temp directory
    let builder = match PackageBuilder::new_temp() {
        Ok(b) => b,
        Err(e) => return SandboxResponse::error_with_category(
            format!("Failed to create package builder: {}", e),
            "CompilationError".to_string(),
        ),
    };

    if verbose {
        eprintln!("Compiling Move source for package: {}", package_name);
    }

    // Build from source
    let result = match builder.build_from_source(package_name, module_name, source) {
        Ok(r) => r,
        Err(e) => return SandboxResponse::error_with_category(
            format!("Compilation failed: {}", e),
            "CompilationError".to_string(),
        ),
    };

    if !result.success {
        return SandboxResponse::error_with_category(
            format!("Compilation errors:\n{}", result.diagnostics),
            "CompilationError".to_string(),
        );
    }

    // Convert bytecode to base64 for the response
    let modules_base64: Vec<serde_json::Value> = result.modules.iter()
        .map(|(name, bytes)| {
            use base64::Engine;
            serde_json::json!({
                "name": name,
                "bytecode_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                "size_bytes": bytes.len(),
            })
        })
        .collect();

    // Try to deploy the compiled modules to the sandbox
    let deploy_result = env.deploy_package(result.modules.clone());

    let deployed = deploy_result.is_ok();
    let package_id = deploy_result.ok().map(|id| id.to_hex_literal());

    SandboxResponse::success_with_data(serde_json::json!({
        "success": true,
        "compiled": true,
        "deployed": deployed,
        "package_id": package_id,
        "modules": modules_base64,
        "diagnostics": result.diagnostics,
    }))
}

fn execute_get_struct_info(
    env: &SimulationEnvironment,
    type_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    // Use the resolver to get struct info
    match env.get_struct_info(type_path) {
        Some(info) => SandboxResponse::success_with_data(info),
        None => SandboxResponse::error(format!("Struct not found: {}", type_path)),
    }
}

fn execute_create_test_object(
    env: &mut SimulationEnvironment,
    type_tag: &str,
    value: &serde_json::Value,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Creating test object of type: {}", type_tag);
    }

    match env.create_test_object(type_tag, value.clone()) {
        Ok(object_id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": object_id.to_hex_literal(),
            "type": type_tag,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to create test object: {}", e),
            "ObjectCreationError".to_string(),
        ),
    }
}

fn categorize_simulation_error(error: &crate::benchmark::simulation::SimulationError) -> String {
    use crate::benchmark::simulation::SimulationError;
    match error {
        SimulationError::MissingPackage { .. } => "MissingPackage".to_string(),
        SimulationError::MissingObject { .. } => "MissingObject".to_string(),
        SimulationError::TypeMismatch { .. } => "TypeMismatch".to_string(),
        SimulationError::ContractAbort { .. } => "ContractAbort".to_string(),
        SimulationError::DeserializationFailed { .. } => "DeserializationFailed".to_string(),
        SimulationError::ExecutionError { .. } => "ExecutionError".to_string(),
        SimulationError::SharedObjectLockConflict { .. } => "SharedObjectLockConflict".to_string(),
    }
}

/// Run the sandbox execution command.
pub fn run_sandbox_exec(args: &SandboxExecArgs) -> Result<()> {
    // Interactive mode - read JSON lines from stdin, write responses to stdout
    if args.interactive {
        return run_interactive_sandbox(args);
    }

    // Single-shot mode
    // Read input
    let input_json: String = if args.input.as_os_str() == "-" {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        std::fs::read_to_string(&args.input)?
    };

    // Parse request
    let request: SandboxRequest = serde_json::from_str(&input_json)
        .map_err(|e| anyhow!("Failed to parse request JSON: {}", e))?;

    if args.verbose {
        eprintln!("Received request: {:?}", request);
    }

    // Create environment - load from state file if provided
    let mut env = if let Some(ref state_file) = args.state_file {
        if state_file.exists() {
            if args.verbose {
                eprintln!("Loading state from {}", state_file.display());
            }
            SimulationEnvironment::from_state_file(state_file)?
        } else {
            if args.verbose {
                eprintln!("State file {} does not exist, creating new environment", state_file.display());
            }
            SimulationEnvironment::new()?
        }
    } else {
        SimulationEnvironment::new()?
    };

    if args.enable_fetching {
        env = env.with_mainnet_fetching();
    }

    // Load bytecode from persistent directory if specified
    if let Some(ref bytecode_dir) = args.bytecode_dir {
        if bytecode_dir.exists() {
            if args.verbose {
                eprintln!("Loading bytecode from {}", bytecode_dir.display());
            }
            let load_req = SandboxRequest::LoadModule {
                bytecode_path: bytecode_dir.to_string_lossy().to_string(),
                module_name: None,
            };
            let _ = execute_request(&mut env, &load_req, args.verbose);
        }
    }

    // Execute request
    let response = execute_request(&mut env, &request, args.verbose);

    // Save state after execution if state file is specified and saving is enabled
    if let Some(ref state_file) = args.state_file {
        if !args.no_save_state {
            if args.verbose {
                eprintln!("Saving state to {}", state_file.display());
            }
            if let Err(e) = env.save_state(state_file) {
                eprintln!("Warning: Failed to save state: {}", e);
            }
        }
    }

    // Write output
    let output_json = serde_json::to_string_pretty(&response)?;

    if args.output.as_os_str() == "-" {
        println!("{}", output_json);
    } else {
        std::fs::write(&args.output, output_json)?;
    }

    Ok(())
}

/// Run sandbox in interactive mode - JSON line protocol.
fn run_interactive_sandbox(args: &SandboxExecArgs) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};

    // Create environment
    let mut env = if let Some(ref state_file) = args.state_file {
        if state_file.exists() {
            if args.verbose {
                eprintln!("Loading state from {}", state_file.display());
            }
            SimulationEnvironment::from_state_file(state_file)?
        } else {
            SimulationEnvironment::new()?
        }
    } else {
        SimulationEnvironment::new()?
    };

    if args.enable_fetching {
        env = env.with_mainnet_fetching();
    }

    // Load bytecode from persistent directory if specified
    if let Some(ref bytecode_dir) = args.bytecode_dir {
        if bytecode_dir.exists() {
            if args.verbose {
                eprintln!("Loading bytecode from {}", bytecode_dir.display());
            }
            let load_req = SandboxRequest::LoadModule {
                bytecode_path: bytecode_dir.to_string_lossy().to_string(),
                module_name: None,
            };
            let _ = execute_request(&mut env, &load_req, args.verbose);
        }
    }

    if args.verbose {
        eprintln!("Interactive sandbox ready. Reading JSON lines from stdin...");
    }

    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                if args.verbose {
                    eprintln!("Error reading line: {}", e);
                }
                break;
            }
        };

        // Skip empty lines
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse request
        let request: SandboxRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let error_response = SandboxResponse::error_with_category(
                    format!("JSON parse error: {}", e),
                    "ParseError",
                );
                let json = serde_json::to_string(&error_response).unwrap_or_default();
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
                continue;
            }
        };

        if args.verbose {
            eprintln!("Request: {:?}", request);
        }

        // Execute request
        let response = execute_request(&mut env, &request, args.verbose);

        // Write response as single JSON line
        let json = serde_json::to_string(&response)?;
        writeln!(stdout, "{}", json)?;
        stdout.flush()?;

        // Optionally save state after each request
        if let Some(ref state_file) = args.state_file {
            if !args.no_save_state {
                if let Err(e) = env.save_state(state_file) {
                    if args.verbose {
                        eprintln!("Warning: Failed to save state: {}", e);
                    }
                }
            }
        }
    }

    if args.verbose {
        eprintln!("Interactive sandbox exiting.");
    }

    Ok(())
}
