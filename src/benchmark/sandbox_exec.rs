//! # Sandbox Execution Interface for LLM Integration
//!
//! **This is the canonical API for LLM agents to interact with the Move VM sandbox.**
//!
//! ## Design Philosophy
//!
//! 1. **Single Entry Point**: All LLM interactions go through [`SandboxRequest`] /
//!    [`execute_request`]. Use `{"action": "list_available_tools"}` to discover
//!    all available operations.
//!
//! 2. **Neutral and Unopinionated**: The API provides facts, not guidance. Error
//!    messages describe what happened without suggesting fixes. Tool descriptions
//!    explain what each tool does without recommending when to use it. This ensures
//!    unbiased evaluation of LLM reasoning capabilities.
//!
//! 3. **Stateful via SimulationEnvironment**: All operations share state through
//!    [`SimulationEnvironment`]. Loading a module makes it available for execution.
//!    Creating an object makes it available for PTBs. State persists across requests.
//!
//! 4. **JSON In, JSON Out**: Requests and responses are JSON for easy integration
//!    with any language. The CLI (`sandbox-exec --interactive`) reads JSON lines
//!    from stdin and writes JSON responses to stdout.
//!
//! ## Supported Operations
//!
//! - **Module operations**: load_module, compile_move, list_modules
//! - **Type introspection**: list_structs, get_struct_info, list_functions, get_function_info
//! - **Object management**: create_object, list_objects, inspect_object
//! - **Execution**: execute_ptb, call_function
//! - **Utilities**: encode_bcs, decode_bcs, validate_type, get_clock, set_clock
//!
//! See `list_available_tools` for the complete schema.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

// Re-export address formatting from utils
use crate::utils::format_address_short;
// Create an alias for backward compatibility within this module
fn normalize_address(addr: &AccountAddress) -> String {
    format_address_short(addr)
}

use crate::args::SandboxExecArgs;
use crate::benchmark::package_builder::PackageBuilder;
use crate::benchmark::simulation::SimulationEnvironment;

// =============================================================================
// Type and Address Format Standards
// =============================================================================
//
// This module defines the canonical formats for types and addresses used in the
// sandbox API. All responses use these formats for consistency.
//
// ## Address Formats
//
// | Format | Example | Use Case |
// |--------|---------|----------|
// | Short | `0x2` | Display, API responses, type strings |
// | Full | `0x0000...0002` | Storage, comparison, module IDs |
//
// For address utilities, see `crate::utils::{parse_address, format_address_short, format_address_full}`
//
// ## Type String Format
//
// Types are formatted as: `address::module::Type<TypeArg1, TypeArg2>`
//
// Examples:
// - Primitive: `u64`, `bool`, `address`
// - Vector: `vector<u8>`
// - Struct: `0x2::coin::Coin<0x2::sui::SUI>`
// - Generic: `0x2::table::Table<address, 0x1::string::String>`
//
// Key rules:
// - Addresses use short form (0x2, not 0x0000...0002)
// - Generic parameters in angle brackets, comma-separated
// - No spaces except after commas in generics
//
// =============================================================================

// Re-export format_type_tag from types module as the canonical type formatter
use crate::benchmark::types::format_type_tag;

/// Format a TypeTag to a canonical string representation.
/// This is the single source of truth for type-to-string conversion in the sandbox API.
///
/// Formatting rules:
/// - Addresses are normalized to short form (0x2 instead of 0x0000...0002)
/// - Generic parameters are included in angle brackets
/// - Format: "address::module::Type<TypeArg1, TypeArg2>"
#[inline]
fn format_type_canonical(type_tag: &TypeTag) -> String {
    format_type_tag(type_tag)
}

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

    /// Validate a PTB without executing it.
    /// Returns validation errors or success with type information.
    #[serde(rename = "validate_ptb")]
    ValidatePtb {
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

    /// List all shared objects and their current lock status.
    #[serde(rename = "list_shared_objects")]
    ListSharedObjects,

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

    // NOTE: SetTime/GetTime were removed - use SetClock/GetClock instead.
    // The clock API is the canonical way to control sandbox time.

    // ========================================================================
    // Cached Transaction Replay Tools
    // ========================================================================
    /// Load cached objects from a transaction replay.
    /// Objects are stored as base64-encoded BCS bytes.
    #[serde(rename = "load_cached_objects")]
    LoadCachedObjects {
        /// Map of object_id (hex) -> base64 BCS bytes.
        objects: HashMap<String, String>,
        /// Map of object_id (hex) -> type string (optional, for better introspection).
        #[serde(default)]
        object_types: HashMap<String, String>,
        /// Set of object IDs that are shared objects.
        #[serde(default)]
        shared_object_ids: Vec<String>,
    },

    /// Load a single cached object.
    #[serde(rename = "load_cached_object")]
    LoadCachedObject {
        /// Object ID (hex string).
        object_id: String,
        /// Base64-encoded BCS bytes.
        bcs_bytes: String,
        /// Object type string (optional).
        object_type: Option<String>,
        /// Whether the object is shared.
        #[serde(default)]
        is_shared: bool,
    },

    /// List all loaded cached objects with their types.
    #[serde(rename = "list_cached_objects")]
    ListCachedObjects,

    // ========================================================================
    // Utility Tools
    // ========================================================================
    /// Generate a fresh unique object/address ID.
    #[serde(rename = "generate_id")]
    GenerateId,

    /// Parse an address string (supports short forms like "0x2").
    #[serde(rename = "parse_address")]
    ParseAddress {
        /// Address string to parse.
        address: String,
    },

    /// Format an address to different representations.
    #[serde(rename = "format_address")]
    FormatAddress {
        /// Address to format.
        address: String,
        /// Format: "short", "full", or "no_prefix". Default: "short".
        #[serde(default)]
        format: Option<String>,
    },

    /// Compute a cryptographic hash of bytes.
    #[serde(rename = "compute_hash")]
    ComputeHash {
        /// Hex-encoded bytes to hash.
        bytes_hex: String,
        /// Algorithm: "sha256", "sha3_256", "blake2b_256". Default: "sha3_256".
        #[serde(default)]
        algorithm: Option<String>,
    },

    /// Convert between Move numeric types.
    #[serde(rename = "convert_number")]
    ConvertNumber {
        /// Numeric value as string.
        value: String,
        /// Source type: "u8", "u16", "u32", "u64", "u128", "u256".
        from_type: String,
        /// Target type.
        to_type: String,
    },

    /// Encode an array of values as a BCS vector.
    #[serde(rename = "encode_vector")]
    EncodeVector {
        /// Element type.
        element_type: String,
        /// Values to encode.
        values: Vec<serde_json::Value>,
    },

    /// Get module dependency graph.
    #[serde(rename = "get_module_dependencies")]
    GetModuleDependencies {
        /// Module path (e.g., "0x2::coin").
        module_path: String,
    },

    /// Disassemble an entire module's bytecode.
    #[serde(rename = "disassemble_module")]
    DisassembleModule {
        /// Module path (e.g., "0x2::coin").
        module_path: String,
    },

    /// Get human-readable module summary.
    #[serde(rename = "module_summary")]
    ModuleSummary {
        /// Module path (e.g., "0x2::coin").
        module_path: String,
    },

    /// Parse an error string to extract structured information.
    #[serde(rename = "parse_error")]
    ParseError {
        /// Error string to parse.
        error: String,
    },

    /// Check if Sui framework is cached locally.
    #[serde(rename = "is_framework_cached")]
    IsFrameworkCached,

    /// Download and cache Sui framework (if not already cached).
    #[serde(rename = "ensure_framework_cached")]
    EnsureFrameworkCached,

    // ========================================================================
    // Event Query Tools
    // ========================================================================
    /// List all events emitted during this session.
    #[serde(rename = "list_events")]
    ListEvents,

    /// Get events filtered by type.
    #[serde(rename = "get_events_by_type")]
    GetEventsByType {
        /// Type prefix to filter by (e.g., "0x2::display::DisplayCreated").
        type_prefix: String,
    },

    /// Get events from the last PTB execution.
    #[serde(rename = "get_last_tx_events")]
    GetLastTxEvents,

    /// Clear all captured events (useful for isolation between tests).
    #[serde(rename = "clear_events")]
    ClearEvents,

    // ========================================================================
    // Shared Object Versioning Tools
    // ========================================================================
    /// Get the current lamport clock value.
    /// The lamport clock is used for shared object versioning and consensus simulation.
    #[serde(rename = "get_lamport_clock")]
    GetLamportClock,

    /// Get detailed information about a shared object including version and lock status.
    #[serde(rename = "get_shared_object_info")]
    GetSharedObjectInfo {
        /// Object ID (hex string).
        object_id: String,
    },

    /// List all currently held shared object locks.
    #[serde(rename = "list_shared_locks")]
    ListSharedLocks,

    /// Manually advance the lamport clock (useful for testing consensus scenarios).
    #[serde(rename = "advance_lamport_clock")]
    AdvanceLamportClock,

    // ========================================================================
    // Meta / Discovery Tools
    // ========================================================================
    /// List all available sandbox tools and their schemas.
    /// This is the unified tool discovery endpoint for LLM agents.
    #[serde(rename = "list_available_tools")]
    ListAvailableTools,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PtbInput {
    #[serde(rename = "pure")]
    Pure {
        value: serde_json::Value,
        value_type: String,
    },

    #[serde(rename = "object")]
    Object {
        object_id: String,
        #[serde(default)]
        mode: Option<String>,
    },

    #[serde(rename = "gas")]
    Gas { budget: u64 },

    #[serde(rename = "witness")]
    Witness { witness_type: String },
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
///
/// ## Field Population by Operation Type
///
/// | Operation | `data` | `effects` | `events` | `gas_used` |
/// |-----------|--------|-----------|----------|------------|
/// | load_module | module names | - | - | - |
/// | list_* | array/object | - | - | - |
/// | get_*_info | struct/function info | - | - | - |
/// | create_object | object details | - | - | - |
/// | execute_ptb | - | ✓ | ✓ | ✓ |
/// | call_function | return values | - | - | ✓ |
/// | encode/decode_bcs | bytes/value | - | - | - |
/// | utility tools | varies | - | - | - |
///
/// On error, `success=false` and `error` contains the message.
/// For PTB errors, `failed_command_index` and `commands_succeeded` indicate
/// which command failed and how many completed before the failure.
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
    /// Object type (if known). Serializes as "type" in JSON for consistency.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
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

/// A synthesized Move object with its BCS-encoded bytes.
/// Used for injecting pre-built objects into the simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesizedObject {
    pub object_id: String,
    pub type_path: String,
    /// BCS-encoded bytes
    pub bcs_bytes: Vec<u8>,
    pub is_shared: bool,
}

/// Execute a sandbox request.
pub fn execute_request(
    env: &mut SimulationEnvironment,
    request: &SandboxRequest,
    verbose: bool,
) -> SandboxResponse {
    match request {
        SandboxRequest::LoadModule {
            bytecode_path,
            module_name,
        } => execute_load_module(env, bytecode_path, module_name.as_deref(), verbose),
        SandboxRequest::CreateObject {
            object_type,
            fields,
            object_id,
        } => execute_create_object(env, object_type, fields, object_id.as_deref(), verbose),
        SandboxRequest::ExecutePtb { inputs, commands } => {
            execute_ptb_command(env, inputs, commands, verbose)
        }
        SandboxRequest::ValidatePtb { inputs, commands } => {
            execute_validate_ptb(env, inputs, commands, verbose)
        }
        SandboxRequest::InspectStruct {
            package,
            module,
            struct_name,
        } => execute_inspect_struct(
            env,
            package,
            module.as_deref(),
            struct_name.as_deref(),
            verbose,
        ),
        SandboxRequest::GetState => execute_get_state(env, verbose),
        SandboxRequest::Reset => execute_reset(env, verbose),
        SandboxRequest::CallFunction {
            package,
            module,
            function,
            type_args,
            args,
        } => execute_call_function(env, package, module, function, type_args, args, verbose),
        SandboxRequest::RegisterCoin {
            coin_type,
            decimals,
            symbol,
            name,
        } => execute_register_coin(env, coin_type, *decimals, symbol, name, verbose),
        SandboxRequest::GetCoinMetadata { coin_type } => {
            execute_get_coin_metadata(env, coin_type, verbose)
        }
        SandboxRequest::ListCoins => execute_list_coins(env, verbose),
        SandboxRequest::InspectObject { object_id } => {
            execute_inspect_object(env, object_id, verbose)
        }
        SandboxRequest::ListObjects => execute_list_objects(env, verbose),
        SandboxRequest::ListSharedObjects => execute_list_shared_objects(env, verbose),
        SandboxRequest::GetClock => execute_get_clock(env, verbose),
        SandboxRequest::SetClock { timestamp_ms } => execute_set_clock(env, *timestamp_ms, verbose),
        // New LLM agent tools
        SandboxRequest::ListModules => execute_list_modules(env, verbose),
        SandboxRequest::ListFunctions { module_path } => {
            execute_list_functions(env, module_path, verbose)
        }
        SandboxRequest::ListStructs { module_path } => {
            execute_list_structs(env, module_path, verbose)
        }
        SandboxRequest::GetFunctionInfo {
            module_path,
            function_name,
        } => execute_get_function_info(env, module_path, function_name, verbose),
        SandboxRequest::FindConstructors { type_path } => {
            execute_find_constructors(env, type_path, verbose)
        }
        SandboxRequest::SearchTypes {
            pattern,
            ability_filter,
        } => execute_search_types(env, pattern, ability_filter.as_deref(), verbose),
        SandboxRequest::SearchFunctions {
            pattern,
            entry_only,
        } => execute_search_functions(env, pattern, *entry_only, verbose),
        SandboxRequest::GetSystemObjectInfo { object_name } => {
            execute_get_system_object_info(object_name, verbose)
        }
        SandboxRequest::ValidateType { type_str } => execute_validate_type(type_str, verbose),
        SandboxRequest::EncodeBcs { type_str, value } => {
            execute_encode_bcs(type_str, value, verbose)
        }
        SandboxRequest::DecodeBcs {
            type_str,
            bytes_hex,
        } => execute_decode_bcs(type_str, bytes_hex, verbose),
        SandboxRequest::DisassembleFunction {
            module_path,
            function_name,
        } => execute_disassemble_function(env, module_path, function_name, verbose),
        SandboxRequest::CompileMove {
            package_name,
            module_name,
            source,
        } => execute_compile_move(env, package_name, module_name, source, verbose),
        SandboxRequest::GetStructInfo { type_path } => {
            execute_get_struct_info(env, type_path, verbose)
        }
        SandboxRequest::CreateTestObject { type_tag, value } => {
            execute_create_test_object(env, type_tag, value, verbose)
        }
        // Cached transaction replay tools
        SandboxRequest::LoadCachedObjects {
            objects,
            object_types,
            shared_object_ids,
        } => execute_load_cached_objects(env, objects, object_types, shared_object_ids, verbose),
        SandboxRequest::LoadCachedObject {
            object_id,
            bcs_bytes,
            object_type,
            is_shared,
        } => execute_load_cached_object(
            env,
            object_id,
            bcs_bytes,
            object_type.as_deref(),
            *is_shared,
            verbose,
        ),
        SandboxRequest::ListCachedObjects => execute_list_cached_objects(env, verbose),
        // Utility tools
        SandboxRequest::GenerateId => execute_generate_id(env, verbose),
        SandboxRequest::ParseAddress { address } => execute_parse_address(address, verbose),
        SandboxRequest::FormatAddress { address, format } => {
            execute_format_address(address, format.as_deref(), verbose)
        }
        SandboxRequest::ComputeHash {
            bytes_hex,
            algorithm,
        } => execute_compute_hash(bytes_hex, algorithm.as_deref(), verbose),
        SandboxRequest::ConvertNumber {
            value,
            from_type,
            to_type,
        } => execute_convert_number(value, from_type, to_type, verbose),
        SandboxRequest::EncodeVector {
            element_type,
            values,
        } => execute_encode_vector(element_type, values, verbose),
        SandboxRequest::GetModuleDependencies { module_path } => {
            execute_get_module_dependencies(env, module_path, verbose)
        }
        SandboxRequest::DisassembleModule { module_path } => {
            execute_disassemble_module(env, module_path, verbose)
        }
        SandboxRequest::ModuleSummary { module_path } => {
            execute_module_summary(env, module_path, verbose)
        }
        SandboxRequest::ParseError { error } => execute_parse_error(error, verbose),
        SandboxRequest::IsFrameworkCached => execute_is_framework_cached(verbose),
        SandboxRequest::EnsureFrameworkCached => execute_ensure_framework_cached(verbose),
        // Event query tools
        SandboxRequest::ListEvents => execute_list_events(env, verbose),
        SandboxRequest::GetEventsByType { type_prefix } => {
            execute_get_events_by_type(env, type_prefix, verbose)
        }
        SandboxRequest::GetLastTxEvents => execute_get_last_tx_events(env, verbose),
        SandboxRequest::ClearEvents => execute_clear_events(env, verbose),
        // Shared object versioning tools
        SandboxRequest::GetLamportClock => execute_get_lamport_clock(env, verbose),
        SandboxRequest::GetSharedObjectInfo { object_id } => {
            execute_get_shared_object_info(env, object_id, verbose)
        }
        SandboxRequest::ListSharedLocks => execute_list_shared_locks(env, verbose),
        SandboxRequest::AdvanceLamportClock => execute_advance_lamport_clock(env, verbose),
        SandboxRequest::ListAvailableTools => execute_list_available_tools(verbose),
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
                        let name = file_path
                            .file_stem()
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
                                    "Failed to read {}: {}",
                                    file_path.display(),
                                    e
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
                let name = path
                    .file_stem()
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
        Ok(address) => SandboxResponse::success_with_data(serde_json::json!({
            "package_address": address.to_hex_literal(),
            "modules_loaded": modules.len(),
            "module_names": modules.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to deploy package: {}", e),
            "DeploymentError",
        ),
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
        Ok(created_id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": created_id.to_hex_literal(),
            "type": object_type,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to create object: {}", e),
            "ObjectCreationError",
        ),
    }
}

fn execute_ptb_command(
    env: &mut SimulationEnvironment,
    inputs: &[PtbInput],
    commands: &[PtbCommand],
    verbose: bool,
) -> SandboxResponse {
    use crate::benchmark::ptb::{Argument, Command as RealPtbCommand, InputValue};
    use move_core_types::identifier::Identifier;
    use move_core_types::language_storage::TypeTag;

    if verbose {
        eprintln!(
            "Executing PTB with {} inputs and {} commands",
            inputs.len(),
            commands.len()
        );
    }

    // Track gas budget for simulation
    let mut _gas_budget: u64 = 50_000_000; // Default 50M MIST

    // Convert inputs
    let mut real_inputs: Vec<InputValue> = Vec::new();
    for input in inputs {
        match input {
            PtbInput::Pure { value, value_type } => match encode_pure_value(value, value_type) {
                Ok(bytes) => real_inputs.push(InputValue::Pure(bytes)),
                Err(e) => return SandboxResponse::error(format!("Failed to encode input: {}", e)),
            },
            PtbInput::Object { object_id, mode } => {
                match env.get_object_for_ptb_with_mode(object_id, mode.as_deref()) {
                    Ok(obj) => real_inputs.push(InputValue::Object(obj)),
                    Err(e) => {
                        return SandboxResponse::error(format!(
                            "Failed to get object {}: {}",
                            object_id, e
                        ))
                    }
                }
            }
            PtbInput::Gas { budget } => {
                _gas_budget = *budget;
                // Gas coin is a special input - create a SUI coin with the budget
                match env.create_gas_coin(*budget) {
                    Ok(obj) => real_inputs.push(InputValue::Object(obj)),
                    Err(e) => {
                        return SandboxResponse::error(format!("Failed to create gas coin: {}", e))
                    }
                }
            }
            PtbInput::Witness { witness_type } => {
                if verbose {
                    eprintln!("Synthesizing witness for type: {}", witness_type);
                }
                let witness_bytes = vec![1u8];
                real_inputs.push(InputValue::Pure(witness_bytes));
            }
        }
    }

    // Convert commands
    let mut real_commands: Vec<RealPtbCommand> = Vec::new();
    for cmd in commands {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => {
                let pkg_addr = match AccountAddress::from_hex_literal(package) {
                    Ok(a) => a,
                    Err(e) => {
                        return SandboxResponse::error(format!("Invalid package address: {}", e))
                    }
                };

                let module_id = match Identifier::new(module.as_str()) {
                    Ok(id) => id,
                    Err(e) => return SandboxResponse::error(format!("Invalid module name: {}", e)),
                };

                let function_id = match Identifier::new(function.as_str()) {
                    Ok(id) => id,
                    Err(e) => {
                        return SandboxResponse::error(format!("Invalid function name: {}", e))
                    }
                };

                // Parse type args from strings
                let parsed_type_args: Vec<TypeTag> = match type_args
                    .iter()
                    .map(|s| crate::benchmark::tx_replay::parse_type_tag(s))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(tags) => tags,
                    Err(e) => {
                        return SandboxResponse::error(format!("Invalid type argument: {}", e))
                    }
                };

                let converted_args: Vec<Argument> =
                    args.iter().map(|a| convert_ptb_arg(a)).collect();

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
            PtbCommand::MakeMoveVec {
                element_type,
                elements,
            } => {
                let type_tag = if let Some(type_str) = element_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => Some(tag),
                        Err(e) => {
                            return SandboxResponse::error(format!("Invalid element type: {}", e))
                        }
                    }
                } else {
                    None
                };
                real_commands.push(RealPtbCommand::MakeMoveVec {
                    type_tag,
                    elements: elements.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::Publish {
                modules,
                dependencies,
            } => {
                use base64::Engine;
                // Decode base64 modules
                let mut decoded_modules = Vec::new();
                for (i, b64) in modules.iter().enumerate() {
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => decoded_modules.push(bytes),
                        Err(e) => {
                            return SandboxResponse::error(format!(
                                "Invalid base64 in module {}: {}",
                                i, e
                            ))
                        }
                    }
                }

                // Parse dependency IDs
                let mut dep_ids = Vec::new();
                for dep in dependencies {
                    match AccountAddress::from_hex_literal(dep) {
                        Ok(addr) => dep_ids.push(addr),
                        Err(e) => {
                            return SandboxResponse::error(format!(
                                "Invalid dependency ID '{}': {}",
                                dep, e
                            ))
                        }
                    }
                }

                real_commands.push(RealPtbCommand::Publish {
                    modules: decoded_modules,
                    dep_ids,
                });
            }
            PtbCommand::Upgrade {
                modules,
                package,
                ticket,
            } => {
                use base64::Engine;
                // Decode base64 modules
                let mut decoded_modules = Vec::new();
                for (i, b64) in modules.iter().enumerate() {
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => decoded_modules.push(bytes),
                        Err(e) => {
                            return SandboxResponse::error(format!(
                                "Invalid base64 in module {}: {}",
                                i, e
                            ))
                        }
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
            PtbCommand::Receive {
                object_id,
                object_type,
            } => {
                // Parse object ID
                let obj_id = match AccountAddress::from_hex_literal(object_id) {
                    Ok(addr) => addr,
                    Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
                };

                // Parse type if provided
                let type_tag = if let Some(type_str) = object_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => Some(tag),
                        Err(e) => {
                            return SandboxResponse::error(format!("Invalid object type: {}", e))
                        }
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
            SandboxResponse::success_with_effects(
                effects_response,
                events_response,
                effects.gas_used,
            )
        } else {
            SandboxResponse::success_with_data(serde_json::json!({
                "status": "success",
            }))
        }
    } else if let Some(ref error) = result.error {
        let mut response = match error {
            crate::benchmark::simulation::SimulationError::ContractAbort {
                abort_code,
                module,
                function,
                ..
            } => SandboxResponse::abort(
                *abort_code,
                Some(format!("{}::{}", module, function)),
                format!(
                    "Contract abort in {}::{} with code {}",
                    module, function, abort_code
                ),
            ),
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

/// Validate a PTB without executing it.
/// This performs all the parsing and type-checking that execute_ptb does,
/// but returns validation results instead of executing.
fn execute_validate_ptb(
    env: &SimulationEnvironment,
    inputs: &[PtbInput],
    commands: &[PtbCommand],
    verbose: bool,
) -> SandboxResponse {
    use move_core_types::identifier::Identifier;

    if verbose {
        eprintln!(
            "Validating PTB with {} inputs and {} commands",
            inputs.len(),
            commands.len()
        );
    }

    let mut validation_errors: Vec<String> = Vec::new();
    let mut input_types: Vec<serde_json::Value> = Vec::new();
    let mut command_info: Vec<serde_json::Value> = Vec::new();

    // Track types produced by each command for result type validation
    // Vec<Vec<String>> where outer index is command index, inner vec is return types
    let mut command_result_types: Vec<Vec<String>> = Vec::new();

    // Track types available from inputs
    let mut input_type_map: Vec<Option<String>> = Vec::new();

    // Validate and collect info about inputs
    for (i, input) in inputs.iter().enumerate() {
        match input {
            PtbInput::Pure { value, value_type } => match encode_pure_value(value, value_type) {
                Ok(bytes) => {
                    input_type_map.push(Some(value_type.clone()));
                    input_types.push(serde_json::json!({
                        "index": i,
                        "type": "pure",
                        "value_type": value_type,
                        "bytes_len": bytes.len(),
                        "valid": true
                    }));
                }
                Err(e) => {
                    input_type_map.push(None);
                    validation_errors
                        .push(format!("Input {}: Failed to encode pure value: {}", i, e));
                    input_types.push(serde_json::json!({
                        "index": i,
                        "type": "pure",
                        "value_type": value_type,
                        "valid": false,
                        "error": e.to_string()
                    }));
                }
            },
            PtbInput::Object { object_id, mode } => {
                match env.get_object_for_ptb_with_mode(object_id, mode.as_deref()) {
                    Ok(obj) => {
                        let type_str = obj.type_tag().map(|t| format!("{}", t));
                        input_type_map.push(type_str.clone());
                        input_types.push(serde_json::json!({
                            "index": i,
                            "type": "object",
                            "object_id": object_id,
                            "mode": mode,
                            "object_type": type_str,
                            "valid": true
                        }));
                    }
                    Err(e) => {
                        input_type_map.push(None);
                        validation_errors
                            .push(format!("Input {}: Object not found or invalid: {}", i, e));
                        input_types.push(serde_json::json!({
                            "index": i,
                            "type": "object",
                            "object_id": object_id,
                            "mode": mode,
                            "valid": false,
                            "error": e.to_string()
                        }));
                    }
                }
            }
            PtbInput::Gas { budget } => {
                // Gas coin is always Coin<SUI>
                input_type_map.push(Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()));
                input_types.push(serde_json::json!({
                    "index": i,
                    "type": "gas",
                    "budget": budget,
                    "valid": true
                }));
            }
            PtbInput::Witness { witness_type } => {
                input_type_map.push(Some(witness_type.clone()));
                input_types.push(serde_json::json!({
                    "index": i,
                    "type": "witness",
                    "witness_type": witness_type,
                    "valid": true
                }));
            }
        }
    }

    // Validate commands
    for (i, cmd) in commands.iter().enumerate() {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => {
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                // Validate package address
                if let Err(e) = AccountAddress::from_hex_literal(package) {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid package address: {}", e));
                }

                // Validate module identifier
                if let Err(e) = Identifier::new(module.as_str()) {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid module name: {}", e));
                }

                // Validate function identifier
                if let Err(e) = Identifier::new(function.as_str()) {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid function name: {}", e));
                }

                // Validate type arguments
                let mut parsed_type_args: Vec<String> = Vec::new();
                for (j, type_str) in type_args.iter().enumerate() {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => parsed_type_args.push(format!("{}", tag)),
                        Err(e) => {
                            cmd_valid = false;
                            cmd_errors.push(format!("Type arg {}: {}", j, e));
                        }
                    }
                }

                // Deep function validation using introspection
                let module_path = format!("{}::{}", package, module);
                let mut function_info_json = serde_json::json!(null);
                let mut expected_params = 0usize;
                let mut expected_type_args = 0usize;
                let mut is_entry = false;
                let mut is_public = false;
                let mut param_types: Vec<String> = Vec::new();
                let mut return_types: Vec<String> = Vec::new();

                match env.get_function_info(&module_path, function) {
                    Some(info) => {
                        function_info_json = info.clone();

                        // Extract parameter count (excluding &mut TxContext / &TxContext at end)
                        if let Some(params) = info.get("params").and_then(|p| p.as_array()) {
                            param_types = params
                                .iter()
                                .filter_map(|p| p.as_str().map(|s| s.to_string()))
                                .collect();
                            // Count params, excluding trailing TxContext
                            expected_params = param_types
                                .iter()
                                .filter(|p| !p.contains("TxContext"))
                                .count();
                        }

                        // Extract type parameter count
                        if let Some(type_params) =
                            info.get("type_params").and_then(|p| p.as_array())
                        {
                            expected_type_args = type_params.len();
                        }

                        // Extract visibility info
                        is_entry = info
                            .get("is_entry")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        is_public =
                            info.get("visibility").and_then(|v| v.as_str()) == Some("public");

                        // Extract return types
                        if let Some(returns) = info.get("returns").and_then(|r| r.as_array()) {
                            return_types = returns
                                .iter()
                                .filter_map(|r| r.as_str().map(|s| s.to_string()))
                                .collect();
                        }

                        // Validate: function must be entry or public to be callable via PTB
                        if !is_entry && !is_public {
                            cmd_valid = false;
                            cmd_errors.push(format!(
                                "Function '{}' is private and cannot be called via PTB",
                                function
                            ));
                        }

                        // Validate: argument count matches (excluding TxContext)
                        if args.len() != expected_params {
                            cmd_valid = false;
                            cmd_errors.push(format!(
                                "Argument count mismatch: provided {} args, function expects {} (excluding TxContext)",
                                args.len(), expected_params
                            ));
                        }

                        // Validate: type argument count matches
                        if type_args.len() != expected_type_args {
                            cmd_valid = false;
                            cmd_errors.push(format!(
                                "Type argument count mismatch: provided {} type args, function expects {}",
                                type_args.len(), expected_type_args
                            ));
                        }
                    }
                    None => {
                        cmd_valid = false;
                        cmd_errors.push(format!(
                            "Function '{}::{}' not found in module",
                            module, function
                        ));
                    }
                }

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                // Track result types for this command
                command_result_types.push(return_types.clone());

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "MoveCall",
                    "package": package,
                    "module": module,
                    "function": function,
                    "type_args": parsed_type_args,
                    "arg_count": args.len(),
                    "function_exists": function_info_json != serde_json::json!(null),
                    "function_signature": {
                        "expected_params": expected_params,
                        "expected_type_args": expected_type_args,
                        "is_entry": is_entry,
                        "is_public": is_public,
                        "param_types": param_types,
                        "return_types": &command_result_types[i]
                    },
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
            PtbCommand::TransferObjects { objects, .. } => {
                // TransferObjects returns nothing
                command_result_types.push(Vec::new());
                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "TransferObjects",
                    "object_count": objects.len(),
                    "valid": true
                }));
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                // SplitCoins returns Vec<Coin<T>> - try to infer the coin type
                let coin_type = match coin {
                    PtbArg::Input(idx) if *idx < input_type_map.len() => {
                        input_type_map[*idx].clone()
                    }
                    PtbArg::Result { cmd, idx } if *cmd < command_result_types.len() => {
                        command_result_types[*cmd].get(*idx).cloned()
                    }
                    _ => None,
                };
                // SplitCoins returns a vector of coins of same type
                let result_type = coin_type
                    .map(|t| format!("vector<{}>", t))
                    .unwrap_or_else(|| "vector<Coin>".to_string());
                command_result_types.push(vec![result_type]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "SplitCoins",
                    "amount_count": amounts.len(),
                    "valid": true
                }));
            }
            PtbCommand::MergeCoins { sources, .. } => {
                // MergeCoins returns nothing (modifies target in place)
                command_result_types.push(Vec::new());
                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "MergeCoins",
                    "source_count": sources.len(),
                    "valid": true
                }));
            }
            PtbCommand::MakeMoveVec {
                element_type,
                elements,
            } => {
                let type_valid = if let Some(type_str) = element_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(_) => true,
                        Err(e) => {
                            validation_errors
                                .push(format!("Command {}: Invalid element type: {}", i, e));
                            false
                        }
                    }
                } else {
                    true
                };

                // MakeMoveVec returns vector<T>
                let result_type = element_type
                    .as_ref()
                    .map(|t| format!("vector<{}>", t))
                    .unwrap_or_else(|| "vector<?>".to_string());
                command_result_types.push(vec![result_type]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "MakeMoveVec",
                    "element_type": element_type,
                    "element_count": elements.len(),
                    "valid": type_valid
                }));
            }
            PtbCommand::Publish {
                modules,
                dependencies,
            } => {
                use base64::Engine;
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                // Validate base64 modules
                for (j, b64) in modules.iter().enumerate() {
                    if base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .is_err()
                    {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid base64 in module {}", j));
                    }
                }

                // Validate dependency addresses
                for dep in dependencies {
                    if AccountAddress::from_hex_literal(dep).is_err() {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid dependency ID: {}", dep));
                    }
                }

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                // Publish returns (UpgradeCap)
                command_result_types.push(vec!["0x2::package::UpgradeCap".to_string()]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "Publish",
                    "module_count": modules.len(),
                    "dependency_count": dependencies.len(),
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
            PtbCommand::Upgrade {
                modules, package, ..
            } => {
                use base64::Engine;
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                // Validate base64 modules
                for (j, b64) in modules.iter().enumerate() {
                    if base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .is_err()
                    {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid base64 in module {}", j));
                    }
                }

                // Validate package address
                if AccountAddress::from_hex_literal(package).is_err() {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid package ID: {}", package));
                }

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                // Upgrade returns (UpgradeReceipt)
                command_result_types.push(vec!["0x2::package::UpgradeReceipt".to_string()]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "Upgrade",
                    "module_count": modules.len(),
                    "package": package,
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
            PtbCommand::Receive {
                object_id,
                object_type,
            } => {
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                // Validate object ID
                if AccountAddress::from_hex_literal(object_id).is_err() {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid object ID: {}", object_id));
                }

                // Validate type if provided
                let result_type = if let Some(type_str) = object_type {
                    if crate::benchmark::tx_replay::parse_type_tag(type_str).is_err() {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid object type: {}", type_str));
                        "?".to_string()
                    } else {
                        type_str.clone()
                    }
                } else {
                    "?".to_string()
                };

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                // Receive returns the object
                command_result_types.push(vec![result_type]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "Receive",
                    "object_id": object_id,
                    "object_type": object_type,
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
        }
    }

    // Additional validation: check Result references are valid
    // Helper to validate a PtbArg reference
    let validate_arg_reference = |arg: &PtbArg, cmd_idx: usize, errors: &mut Vec<String>| match arg
    {
        PtbArg::Input(idx) => {
            if *idx >= inputs.len() {
                errors.push(format!(
                    "Command {}: Input({}) references non-existent input (only {} inputs)",
                    cmd_idx,
                    idx,
                    inputs.len()
                ));
            }
        }
        PtbArg::Result {
            cmd: result_cmd,
            idx: result_idx,
        } => {
            if *result_cmd >= cmd_idx {
                errors.push(format!(
                        "Command {}: Result(cmd={}, idx={}) references command {} which hasn't executed yet (forward reference)",
                        cmd_idx, result_cmd, result_idx, result_cmd
                    ));
            } else if *result_cmd < command_result_types.len() {
                let result_count = command_result_types[*result_cmd].len();
                if *result_idx >= result_count {
                    errors.push(format!(
                            "Command {}: Result(cmd={}, idx={}) references return index {} but command {} only returns {} values",
                            cmd_idx, result_cmd, result_idx, result_idx, result_cmd, result_count
                        ));
                }
            }
        }
    };

    // Re-validate command args for reference validity
    for (i, cmd) in commands.iter().enumerate() {
        let mut ref_errors: Vec<String> = Vec::new();
        match cmd {
            PtbCommand::MoveCall { args, .. } => {
                for arg in args {
                    validate_arg_reference(arg, i, &mut ref_errors);
                }
            }
            PtbCommand::TransferObjects { objects, recipient } => {
                for obj in objects {
                    validate_arg_reference(obj, i, &mut ref_errors);
                }
                validate_arg_reference(recipient, i, &mut ref_errors);
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                validate_arg_reference(coin, i, &mut ref_errors);
                for amt in amounts {
                    validate_arg_reference(amt, i, &mut ref_errors);
                }
            }
            PtbCommand::MergeCoins { target, sources } => {
                validate_arg_reference(target, i, &mut ref_errors);
                for src in sources {
                    validate_arg_reference(src, i, &mut ref_errors);
                }
            }
            PtbCommand::MakeMoveVec { elements, .. } => {
                for elem in elements {
                    validate_arg_reference(elem, i, &mut ref_errors);
                }
            }
            _ => {}
        }
        validation_errors.extend(ref_errors);
    }

    // Build response
    let is_valid = validation_errors.is_empty();

    // Include result type tracking in response
    let result_type_info: Vec<serde_json::Value> = command_result_types
        .iter()
        .enumerate()
        .map(|(idx, types)| {
            serde_json::json!({
                "command_index": idx,
                "return_types": types
            })
        })
        .collect();

    if is_valid {
        SandboxResponse::success_with_data(serde_json::json!({
            "valid": true,
            "input_count": inputs.len(),
            "command_count": commands.len(),
            "inputs": input_types,
            "commands": command_info,
            "result_types": result_type_info
        }))
    } else {
        SandboxResponse::success_with_data(serde_json::json!({
            "valid": false,
            "error_count": validation_errors.len(),
            "errors": validation_errors,
            "inputs": input_types,
            "commands": command_info,
            "result_types": result_type_info
        }))
    }
}

/// Build transaction effects response from internal effects.
fn build_effects_response(
    effects: &crate::benchmark::ptb::TransactionEffects,
) -> TransactionEffectsResponse {
    use crate::benchmark::ptb::{ObjectChange, Owner};

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
    let type_to_string =
        |t: &Option<move_core_types::language_storage::TypeTag>| -> Option<String> {
            t.as_ref().map(|tag| format!("{}", tag))
        };

    for change in &effects.object_changes {
        match change {
            ObjectChange::Created {
                id,
                owner,
                object_type,
            } => {
                created.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 1,
                });
            }
            ObjectChange::Mutated {
                id,
                owner,
                object_type,
            } => {
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
            ObjectChange::Unwrapped {
                id,
                owner,
                object_type,
            } => {
                unwrapped.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 1,
                });
            }
            ObjectChange::Transferred {
                id,
                recipient,
                object_type,
                ..
            } => {
                // Transferred objects show up as mutated with new owner
                mutated.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: format!("address:{}", recipient.to_hex_literal()),
                    version: 2,
                });
            }
        }
    }

    // If no object_changes, fall back to simple lists
    if created.is_empty() && !effects.created.is_empty() {
        created = effects
            .created
            .iter()
            .map(|id| ObjectEffectResponse {
                id: id.to_hex_literal(),
                object_type: None,
                owner: "unknown".to_string(),
                version: 1,
            })
            .collect();
    }
    if mutated.is_empty() && !effects.mutated.is_empty() {
        mutated = effects
            .mutated
            .iter()
            .map(|id| ObjectEffectResponse {
                id: id.to_hex_literal(),
                object_type: None,
                owner: "unknown".to_string(),
                version: 2,
            })
            .collect();
    }
    if deleted.is_empty() && !effects.deleted.is_empty() {
        deleted = effects
            .deleted
            .iter()
            .map(|id| id.to_hex_literal())
            .collect();
    }
    if wrapped.is_empty() && !effects.wrapped.is_empty() {
        wrapped = effects
            .wrapped
            .iter()
            .map(|id| id.to_hex_literal())
            .collect();
    }
    if unwrapped.is_empty() && !effects.unwrapped.is_empty() {
        unwrapped = effects
            .unwrapped
            .iter()
            .map(|id| ObjectEffectResponse {
                id: id.to_hex_literal(),
                object_type: None,
                owner: "unknown".to_string(),
                version: 1,
            })
            .collect();
    }

    // Build return values from effects
    let return_values: Option<Vec<CommandReturnValues>> = if effects.return_values.is_empty() {
        None
    } else {
        let values: Vec<CommandReturnValues> = effects
            .return_values
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
    events
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect()
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
        Err(e) => {
            return SandboxResponse::error(format!("Failed to get struct definitions: {}", e))
        }
    };

    let struct_defs: Vec<StructDef> = structs
        .into_iter()
        .map(|s| StructDef {
            package: s.package,
            module: s.module,
            name: s.name,
            abilities: s.abilities,
            type_params: s
                .type_params
                .into_iter()
                .map(|tp| TypeParam {
                    name: tp.name,
                    constraints: tp.constraints,
                })
                .collect(),
            fields: s
                .fields
                .into_iter()
                .map(|f| FieldDef {
                    name: f.name,
                    field_type: f.field_type,
                })
                .collect(),
        })
        .collect();

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
        Ok(result) => SandboxResponse::success_with_data(serde_json::json!({
            "return_values": result.return_values,
            "gas_used": result.gas_used,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Function call failed: {}", e),
            "ExecutionError",
        ),
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

    let coins: Vec<serde_json::Value> = env
        .list_registered_coins()
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "coin_type": m.type_tag,
                "decimals": m.decimals,
                "symbol": m.symbol,
                "name": m.name,
            })
        })
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
        "type": format_type_canonical(&obj.type_tag),
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
            let type_str = format!(
                "{}::{}::{}",
                st.address.to_hex_literal(),
                st.module,
                st.name
            );

            // Handle Coin<T> specially
            if st.address.to_hex_literal() == "0x2"
                && st.module.as_str() == "coin"
                && st.name.as_str() == "Coin"
            {
                return decode_coin(&obj.bcs_bytes, &st.type_params);
            }

            // Handle Balance<T> specially
            if st.address.to_hex_literal() == "0x2"
                && st.module.as_str() == "balance"
                && st.name.as_str() == "Balance"
            {
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
                "type": format_type_canonical(&obj.type_tag),
                "raw_hex": hex::encode(&obj.bcs_bytes),
            })
        }
    }
}

/// Decode a Coin<T> object.
fn decode_coin(
    bcs_bytes: &[u8],
    type_params: &[move_core_types::language_storage::TypeTag],
) -> serde_json::Value {
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

    let coin_type = type_params
        .first()
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
                (serde_json::json!(u128::from_le_bytes(arr).to_string()), 16) // String to avoid JSON precision issues
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
            (
                serde_json::json!({
                    "raw_hex": hex::encode(bytes),
                    "note": format!("Cannot fully decode {}", t)
                }),
                bytes.len(),
            )
        }
        // Option<T> - 0 for None, 1 + value for Some
        t if t.starts_with("0x1::option::Option<") => {
            if bytes.is_empty() {
                (serde_json::json!(null), 0)
            } else if bytes[0] == 0 {
                (serde_json::json!(null), 1)
            } else {
                // Extract inner type and decode
                let inner = t
                    .strip_prefix("0x1::option::Option<")
                    .and_then(|s| s.strip_suffix(">"))
                    .unwrap_or("unknown");
                let (inner_val, consumed) = decode_field_value(&bytes[1..], inner);
                (inner_val, 1 + consumed)
            }
        }
        // Unknown type - return raw hex
        _ => (
            serde_json::json!({
                "raw_hex": hex::encode(bytes),
                "type": field_type,
            }),
            bytes.len(),
        ),
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
        return (
            serde_json::json!({
                "error": "Invalid vector: length exceeds available bytes",
                "raw_hex": hex::encode(bytes),
            }),
            bytes.len(),
        );
    }

    let data = &bytes[offset..offset + len];

    // Try to interpret as UTF-8 string
    if let Ok(s) = std::str::from_utf8(data) {
        (serde_json::json!(s), offset + len)
    } else {
        (
            serde_json::json!(format!("0x{}", hex::encode(data))),
            offset + len,
        )
    }
}

fn execute_list_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing all objects");
    }

    let objects: Vec<serde_json::Value> = env
        .list_objects()
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
                "type": format_type_canonical(&obj.type_tag),
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

fn execute_list_shared_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing shared objects and locks");
    }

    // Get all shared objects from the environment
    let shared_objects: Vec<serde_json::Value> = env
        .list_objects()
        .into_iter()
        .filter(|obj| obj.is_shared)
        .map(|obj| {
            serde_json::json!({
                "object_id": obj.id.to_hex_literal(),
                "type": format_type_canonical(&obj.type_tag),
                "version": obj.version,
            })
        })
        .collect();

    // Get current locks
    let locks: Vec<serde_json::Value> = env
        .get_shared_locks()
        .into_iter()
        .map(|lock| {
            serde_json::json!({
                "object_id": lock.object_id.to_hex_literal(),
                "version": lock.version,
                "is_mutable": lock.is_mutable,
                "held_by": lock.transaction_id,
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "shared_objects": shared_objects,
        "shared_object_count": shared_objects.len(),
        "active_locks": locks,
        "lock_count": locks.len(),
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

        format!(
            "~{}-??-?? {:02}:{:02}:{:02}.{:03} UTC (approx)",
            year,
            hours,
            minutes,
            secs,
            nanos / 1_000_000
        )
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "clock_object_id": crate::benchmark::simulation::CLOCK_OBJECT_ID,
        "timestamp_ms": timestamp_ms,
        "datetime_approx": datetime_str,
    }))
}

fn execute_set_clock(
    env: &mut SimulationEnvironment,
    timestamp_ms: u64,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Setting Clock timestamp to {} ms", timestamp_ms);
    }

    match env.advance_clock(timestamp_ms) {
        Ok(()) => SandboxResponse::success_with_data(serde_json::json!({
            "clock_object_id": crate::benchmark::simulation::CLOCK_OBJECT_ID,
            "timestamp_ms": timestamp_ms,
            "message": format!("Clock advanced to {} ms", timestamp_ms),
        })),
        Err(e) => SandboxResponse::error(format!("Failed to set clock: {}", e)),
    }
}

// Helper functions

fn parse_object_id(id_str: &str) -> Result<[u8; 32]> {
    let addr =
        AccountAddress::from_hex_literal(id_str).map_err(|e| anyhow!("Invalid hex: {}", e))?;
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
        PtbArg::Result { cmd, idx } => {
            crate::benchmark::ptb::Argument::NestedResult(*cmd as u16, *idx as u16)
        }
    }
}

// ============================================================================
// New LLM Agent Tool Implementations
// ============================================================================

fn execute_list_modules(env: &SimulationEnvironment, _verbose: bool) -> SandboxResponse {
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
        Some(info) => {
            SandboxResponse::success_with_data(serde_json::to_value(info).unwrap_or_default())
        }
        None => SandboxResponse::error(format!(
            "Function not found: {}::{}",
            module_path, function_name
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

fn execute_get_system_object_info(object_name: &str, _verbose: bool) -> SandboxResponse {
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

fn execute_validate_type(type_str: &str, _verbose: bool) -> SandboxResponse {
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
            let inner = &type_str[7..type_str.len() - 1];
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

fn execute_decode_bcs(type_str: &str, bytes_hex: &str, _verbose: bool) -> SandboxResponse {
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
            "Function not found or cannot disassemble: {}::{}",
            module_path, function_name
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
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Failed to create package builder: {}", e),
                "CompilationError".to_string(),
            )
        }
    };

    if verbose {
        eprintln!("Compiling Move source for package: {}", package_name);
    }

    // Build from source
    let result = match builder.build_from_source(package_name, module_name, source) {
        Ok(r) => r,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Compilation failed: {}", e),
                "CompilationError".to_string(),
            )
        }
    };

    if !result.success {
        return SandboxResponse::error_with_category(
            format!("Compilation errors:\n{}", result.diagnostics),
            "CompilationError".to_string(),
        );
    }

    // Convert bytecode to base64 for the response
    let modules_base64: Vec<serde_json::Value> = result
        .modules
        .iter()
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

// ============================================================================
// Cached Transaction Replay Functions
// ============================================================================

fn execute_load_cached_objects(
    env: &mut SimulationEnvironment,
    objects: &HashMap<String, String>,
    object_types: &HashMap<String, String>,
    shared_object_ids: &[String],
    verbose: bool,
) -> SandboxResponse {
    use base64::Engine;

    if verbose {
        eprintln!("Loading {} cached objects", objects.len());
    }

    let shared_set: std::collections::HashSet<&str> =
        shared_object_ids.iter().map(|s| s.as_str()).collect();
    let mut loaded = 0;
    let mut failed = Vec::new();

    for (object_id, b64_bytes) in objects {
        let is_shared = shared_set.contains(object_id.as_str());
        let object_type = object_types.get(object_id).map(|s| s.as_str());

        match base64::engine::general_purpose::STANDARD.decode(b64_bytes) {
            Ok(bcs_bytes) => {
                match env.load_cached_object_with_type(object_id, bcs_bytes, object_type, is_shared)
                {
                    Ok(_) => {
                        loaded += 1;
                        if verbose {
                            eprintln!("  Loaded object {} (shared={})", object_id, is_shared);
                        }
                    }
                    Err(e) => {
                        failed.push(serde_json::json!({
                            "object_id": object_id,
                            "error": e.to_string(),
                        }));
                    }
                }
            }
            Err(e) => {
                failed.push(serde_json::json!({
                    "object_id": object_id,
                    "error": format!("Base64 decode error: {}", e),
                }));
            }
        }
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "loaded": loaded,
        "failed": failed.len(),
        "failures": failed,
    }))
}

fn execute_load_cached_object(
    env: &mut SimulationEnvironment,
    object_id: &str,
    bcs_bytes_b64: &str,
    object_type: Option<&str>,
    is_shared: bool,
    verbose: bool,
) -> SandboxResponse {
    use base64::Engine;

    if verbose {
        eprintln!(
            "Loading cached object: {} (shared={})",
            object_id, is_shared
        );
    }

    let bcs_bytes = match base64::engine::general_purpose::STANDARD.decode(bcs_bytes_b64) {
        Ok(bytes) => bytes,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Base64 decode error: {}", e),
                "DecodeError",
            );
        }
    };

    match env.load_cached_object_with_type(object_id, bcs_bytes, object_type, is_shared) {
        Ok(id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": id.to_hex_literal(),
            "is_shared": is_shared,
            "type": object_type,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to load object: {}", e),
            "ObjectLoadError",
        ),
    }
}

fn execute_list_cached_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing cached objects");
    }

    let objects: Vec<serde_json::Value> = env
        .list_objects()
        .iter()
        .map(|obj| {
            serde_json::json!({
                "object_id": obj.id.to_hex_literal(),
                "type": format_type_canonical(&obj.type_tag),
                "is_shared": obj.is_shared,
                "is_immutable": obj.is_immutable,
                "version": obj.version,
                "bytes_len": obj.bcs_bytes.len(),
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "objects": objects,
        "count": objects.len(),
    }))
}

// ============================================================================
// Utility Tools
// ============================================================================

fn execute_generate_id(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Generating fresh ID");
    }
    let id = env.fresh_id();
    let hex_full = id.to_hex_literal();
    let hex_short = normalize_address(&id);
    SandboxResponse::success_with_data(serde_json::json!({
        "id": hex_full,
        "short": hex_short,
    }))
}

fn execute_parse_address(address: &str, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Parsing address: {}", address);
    }
    match parse_address_string(address) {
        Ok(parsed) => {
            let hex_full = parsed.to_hex_literal();
            let hex_short = normalize_address(&parsed);
            SandboxResponse::success_with_data(serde_json::json!({
                "full": hex_full,
                "short": hex_short,
                "valid": true,
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Invalid address: {}", e),
            "ParseError".to_string(),
        ),
    }
}

fn execute_format_address(address: &str, format: Option<&str>, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Formatting address: {} as {:?}", address, format);
    }
    match parse_address_string(address) {
        Ok(parsed) => {
            let hex_full = parsed.to_hex_literal();
            let fmt = format.unwrap_or("short");
            let result = match fmt {
                "short" => normalize_address(&parsed),
                "full" => hex_full.clone(),
                "no_prefix" => hex_full.strip_prefix("0x").unwrap_or(&hex_full).to_string(),
                _ => {
                    return SandboxResponse::error_with_category(
                        format!(
                            "Unknown format: {}. Use 'short', 'full', or 'no_prefix'",
                            fmt
                        ),
                        "InvalidParameter".to_string(),
                    );
                }
            };
            SandboxResponse::success_with_data(serde_json::json!({
                "formatted": result,
                "format": fmt,
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Invalid address: {}", e),
            "ParseError".to_string(),
        ),
    }
}

fn execute_compute_hash(
    bytes_hex: &str,
    algorithm: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Computing hash with algorithm: {:?}", algorithm);
    }
    let hex_str = bytes_hex.strip_prefix("0x").unwrap_or(bytes_hex);
    let bytes = match hex::decode(hex_str) {
        Ok(b) => b,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid hex bytes: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let algo = algorithm.unwrap_or("sha3_256");
    use sha2::{Digest, Sha256};

    let hash = match algo {
        "sha256" | "sha3_256" | "blake2b_256" => {
            // Note: Currently using sha256 for all. Full implementation would use proper algorithms.
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hasher.finalize().to_vec()
        }
        _ => {
            return SandboxResponse::error_with_category(
                format!(
                    "Unknown algorithm: {}. Use sha256, sha3_256, or blake2b_256",
                    algo
                ),
                "InvalidParameter".to_string(),
            );
        }
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "algorithm": algo,
        "input_len": bytes.len(),
        "hash_hex": format!("0x{}", hex::encode(&hash)),
    }))
}

fn execute_convert_number(
    value: &str,
    from_type: &str,
    to_type: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Converting {} from {} to {}", value, from_type, to_type);
    }

    // Parse input value as u128
    let val_u128: u128 = if value.starts_with("0x") {
        match u128::from_str_radix(value.strip_prefix("0x").unwrap(), 16) {
            Ok(v) => v,
            Err(e) => {
                return SandboxResponse::error_with_category(
                    format!("Invalid hex value: {}", e),
                    "ParseError".to_string(),
                );
            }
        }
    } else {
        match value.parse::<u128>() {
            Ok(v) => v,
            Err(e) => {
                return SandboxResponse::error_with_category(
                    format!("Invalid decimal value: {}", e),
                    "ParseError".to_string(),
                );
            }
        }
    };

    // Check target type range
    let (max_val, target_bits): (u128, usize) = match to_type {
        "u8" => (u8::MAX as u128, 8),
        "u16" => (u16::MAX as u128, 16),
        "u32" => (u32::MAX as u128, 32),
        "u64" => (u64::MAX as u128, 64),
        "u128" => (u128::MAX, 128),
        "u256" => (u128::MAX, 256),
        _ => {
            return SandboxResponse::error_with_category(
                format!("Unknown target type: {}", to_type),
                "InvalidParameter".to_string(),
            );
        }
    };

    let fits = val_u128 <= max_val;
    let decimal = val_u128.to_string();
    let hex = format!("0x{:x}", val_u128);

    SandboxResponse::success_with_data(serde_json::json!({
        "value_decimal": decimal,
        "value_hex": hex,
        "from_type": from_type,
        "to_type": to_type,
        "fits_in_target": fits,
        "target_bits": target_bits,
    }))
}

fn execute_encode_vector(
    element_type: &str,
    values: &[serde_json::Value],
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Encoding vector of {} elements of type {}",
            values.len(),
            element_type
        );
    }

    // Encode ULEB128 length prefix
    let mut bytes = Vec::new();
    let mut len = values.len();
    loop {
        let byte = (len & 0x7F) as u8;
        len >>= 7;
        if len == 0 {
            bytes.push(byte);
            break;
        } else {
            bytes.push(byte | 0x80);
        }
    }

    // For primitive types, encode each element
    for val in values {
        match element_type {
            "u8" => {
                if let Some(n) = val.as_u64() {
                    bytes.push(n as u8);
                }
            }
            "u64" => {
                if let Some(n) = val.as_u64() {
                    bytes.extend_from_slice(&n.to_le_bytes());
                }
            }
            "bool" => {
                if let Some(b) = val.as_bool() {
                    bytes.push(if b { 1 } else { 0 });
                }
            }
            _ => {
                return SandboxResponse::error_with_category(
                    format!(
                        "Unsupported element type for vector encoding: {}",
                        element_type
                    ),
                    "UnsupportedType".to_string(),
                );
            }
        }
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "element_type": element_type,
        "element_count": values.len(),
        "bytes_hex": format!("0x{}", hex::encode(&bytes)),
        "bytes_len": bytes.len(),
    }))
}

fn execute_get_module_dependencies(
    env: &SimulationEnvironment,
    module_path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting dependencies for module: {}", module_path);
    }

    // Parse module path
    let parts: Vec<&str> = module_path.split("::").collect();
    if parts.len() != 2 {
        return SandboxResponse::error_with_category(
            format!(
                "Invalid module path: {}. Expected format: '0x2::module'",
                module_path
            ),
            "ParseError".to_string(),
        );
    }

    let address = match AccountAddress::from_hex_literal(parts[0]) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid address in module path: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let module_name = parts[1];

    // Get dependencies from resolver
    match env.get_module_dependencies(&address, module_name) {
        Ok(deps) => {
            let dep_list: Vec<String> = deps
                .iter()
                .map(|(addr, name)| format!("{}::{}", normalize_address(addr), name))
                .collect();
            SandboxResponse::success_with_data(serde_json::json!({
                "module": module_path,
                "dependencies": dep_list,
                "count": dep_list.len(),
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to get dependencies: {}", e),
            "ModuleNotFound".to_string(),
        ),
    }
}

fn execute_disassemble_module(
    env: &SimulationEnvironment,
    module_path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Disassembling module: {}", module_path);
    }

    // Parse module path
    let parts: Vec<&str> = module_path.split("::").collect();
    if parts.len() != 2 {
        return SandboxResponse::error_with_category(
            format!(
                "Invalid module path: {}. Expected format: '0x2::module'",
                module_path
            ),
            "ParseError".to_string(),
        );
    }

    let address = match AccountAddress::from_hex_literal(parts[0]) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid address in module path: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let module_name = parts[1];

    match env.disassemble_module(&address, module_name) {
        Ok(disasm) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "disassembly": disasm,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to disassemble module: {}", e),
            "ModuleNotFound".to_string(),
        ),
    }
}

fn execute_module_summary(
    env: &SimulationEnvironment,
    module_path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting summary for module: {}", module_path);
    }

    // Parse module path
    let parts: Vec<&str> = module_path.split("::").collect();
    if parts.len() != 2 {
        return SandboxResponse::error_with_category(
            format!(
                "Invalid module path: {}. Expected format: '0x2::module'",
                module_path
            ),
            "ParseError".to_string(),
        );
    }

    let address = match AccountAddress::from_hex_literal(parts[0]) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid address in module path: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let module_name = parts[1];

    match env.get_module_summary(&address, module_name) {
        Ok(summary) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "summary": summary,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to get module summary: {}", e),
            "ModuleNotFound".to_string(),
        ),
    }
}

fn execute_parse_error(error: &str, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Parsing error string");
    }

    // Try to extract abort code and location from common error formats
    let mut result = serde_json::json!({
        "original": error,
    });

    // Pattern: "ABORTED with code X in module Y::Z::func"
    if error.contains("ABORTED") {
        if let Some(code_start) = error.find("code ") {
            let rest = &error[code_start + 5..];
            if let Some(code_end) = rest.find(|c: char| !c.is_ascii_digit()) {
                if let Ok(code) = rest[..code_end].parse::<u64>() {
                    result["abort_code"] = serde_json::json!(code);
                }
            }
        }
    }

    // Pattern: "MissingPackage { address: X, module: Y }"
    if error.contains("MissingPackage") {
        result["error_type"] = serde_json::json!("MissingPackage");
    } else if error.contains("MissingObject") {
        result["error_type"] = serde_json::json!("MissingObject");
    } else if error.contains("LINKER_ERROR") {
        result["error_type"] = serde_json::json!("LinkerError");
    } else if error.contains("ABORTED") {
        result["error_type"] = serde_json::json!("ContractAbort");
    }

    SandboxResponse::success_with_data(result)
}

fn execute_is_framework_cached(verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Checking if Sui framework is cached");
    }

    use crate::benchmark::package_builder::FrameworkCache;
    match FrameworkCache::new() {
        Ok(cache) => {
            let is_cached = cache.is_cached();
            SandboxResponse::success_with_data(serde_json::json!({
                "is_cached": is_cached,
                "path": cache.sui_framework_path().display().to_string(),
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to check framework cache: {}", e),
            "CacheError".to_string(),
        ),
    }
}

fn execute_ensure_framework_cached(verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Ensuring Sui framework is cached");
    }

    use crate::benchmark::package_builder::FrameworkCache;
    match FrameworkCache::new() {
        Ok(cache) => match cache.ensure_cached() {
            Ok(()) => SandboxResponse::success_with_data(serde_json::json!({
                "cached": true,
                "path": cache.sui_framework_path().display().to_string(),
            })),
            Err(e) => SandboxResponse::error_with_category(
                format!("Failed to cache framework: {}", e),
                "CacheError".to_string(),
            ),
        },
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to initialize framework cache: {}", e),
            "CacheError".to_string(),
        ),
    }
}

/// Parse an address string, supporting short forms like "0x2".
fn parse_address_string(s: &str) -> Result<AccountAddress, String> {
    let hex_str = s.strip_prefix("0x").unwrap_or(s);
    let padded = if hex_str.len() < 64 {
        format!("{:0>64}", hex_str)
    } else {
        hex_str.to_string()
    };
    let bytes = hex::decode(&padded).map_err(|e| format!("Invalid hex: {}", e))?;
    if bytes.len() != 32 {
        return Err(format!("Address must be 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(AccountAddress::new(arr))
}

// =============================================================================
// Event Query Functions
// =============================================================================

fn execute_list_events(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing all events ({})", env.event_count());
    }

    let events: Vec<EventResponse> = env
        .get_all_events()
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect();

    let count = events.len();
    SandboxResponse::success_with_data(serde_json::json!({
        "events": events,
        "count": count
    }))
}

fn execute_get_events_by_type(
    env: &SimulationEnvironment,
    type_prefix: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting events by type prefix: {}", type_prefix);
    }

    let events: Vec<EventResponse> = env
        .get_events_by_type(type_prefix)
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect();

    let count = events.len();
    SandboxResponse::success_with_data(serde_json::json!({
        "events": events,
        "count": count,
        "type_prefix": type_prefix
    }))
}

fn execute_get_last_tx_events(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting events from last transaction");
    }

    let events: Vec<EventResponse> = env
        .get_last_tx_events()
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect();

    let count = events.len();
    SandboxResponse::success_with_data(serde_json::json!({
        "events": events,
        "count": count
    }))
}

fn execute_clear_events(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Clearing all events");
    }

    let previous_count = env.event_count();
    env.clear_events();

    SandboxResponse::success_with_data(serde_json::json!({
        "cleared": true,
        "previous_count": previous_count
    }))
}

// =============================================================================
// Shared Object Versioning Functions
// =============================================================================

fn execute_get_lamport_clock(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting lamport clock value");
    }

    let clock = env.lamport_clock();
    SandboxResponse::success_with_data(serde_json::json!({
        "lamport_clock": clock,
        "description": "Current lamport clock value used for shared object versioning"
    }))
}

fn execute_get_shared_object_info(
    env: &SimulationEnvironment,
    object_id: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting shared object info for: {}", object_id);
    }

    // Parse object ID
    let id = match parse_address_string(object_id) {
        Ok(addr) => addr,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid object ID: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    // Get the object
    let obj = match env.get_object(&id) {
        Some(o) => o,
        None => {
            return SandboxResponse::error_with_category(
                format!("Object not found: {}", object_id),
                "ObjectNotFound".to_string(),
            );
        }
    };

    // Check if object is shared
    if !obj.is_shared {
        return SandboxResponse::success_with_data(serde_json::json!({
            "object_id": object_id,
            "is_shared": false,
            "version": obj.version,
            "type": format!("{}", obj.type_tag),
            "message": "Object is not shared"
        }));
    }

    // Get lock status for this object
    let lock_info = env.get_lock_for_object(&id);

    SandboxResponse::success_with_data(serde_json::json!({
        "object_id": object_id,
        "is_shared": true,
        "version": obj.version,
        "type": format!("{}", obj.type_tag),
        "is_locked": lock_info.is_some(),
        "lock_info": lock_info.map(|lock| serde_json::json!({
            "version": lock.version,
            "is_mutable": lock.is_mutable,
            "transaction_id": lock.transaction_id
        })),
        "lamport_clock": env.lamport_clock()
    }))
}

fn execute_list_shared_locks(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing shared object locks");
    }

    let locks = env.list_shared_locks();
    let lock_list: Vec<serde_json::Value> = locks
        .iter()
        .map(|lock| {
            serde_json::json!({
                "object_id": lock.object_id.to_hex_literal(),
                "version": lock.version,
                "is_mutable": lock.is_mutable,
                "transaction_id": lock.transaction_id
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "locks": lock_list,
        "count": lock_list.len(),
        "lamport_clock": env.lamport_clock()
    }))
}

fn execute_advance_lamport_clock(
    env: &mut SimulationEnvironment,
    verbose: bool,
) -> SandboxResponse {
    let previous = env.lamport_clock();
    let new_value = env.advance_lamport_clock();

    if verbose {
        eprintln!("Advanced lamport clock: {} -> {}", previous, new_value);
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "previous_value": previous,
        "new_value": new_value
    }))
}

/// Generate unified schema of all available sandbox tools.
/// This is the single source of truth for LLM tool discovery.
fn execute_list_available_tools(verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Generating tool discovery schema");
    }

    let tools = serde_json::json!({
        "version": "1.0",
        "description": "Sui Move VM Sandbox - All available tools for LLM agents",
        "categories": {
            "state_management": {
                "description": "Tools for managing sandbox state",
                "tools": [
                    {
                        "action": "reset",
                        "description": "Reset sandbox to initial state. Clears all loaded modules, objects, and coins.",
                        "params": {},
                        "example": {"action": "reset"}
                    },
                    {
                        "action": "get_state",
                        "description": "Get current sandbox state summary (loaded modules, objects, coins).",
                        "params": {},
                        "example": {"action": "get_state"}
                    }
                ]
            },
            "module_operations": {
                "description": "Tools for loading and inspecting Move modules",
                "tools": [
                    {
                        "action": "load_module",
                        "description": "Load compiled Move module(s) from bytecode file(s).",
                        "params": {
                            "bytecode_path": "string - path to .mv file or directory containing .mv files",
                            "module_name": "string? - optional filter to load only matching module names"
                        },
                        "example": {"action": "load_module", "bytecode_path": "./build/MyPackage/bytecode_modules"}
                    },
                    {
                        "action": "compile_move",
                        "description": "Compile Move source code to bytecode and load into sandbox.",
                        "params": {
                            "package_name": "string - name for the package (used for address resolution)",
                            "module_name": "string - module name (without .move extension)",
                            "source": "string - Move source code"
                        },
                        "example": {"action": "compile_move", "package_name": "my_pkg", "module_name": "counter", "source": "module my_pkg::counter { ... }"}
                    },
                    {
                        "action": "list_modules",
                        "description": "List all loaded modules. Returns array of module paths like '0x123::module'.",
                        "params": {},
                        "example": {"action": "list_modules"}
                    },
                    {
                        "action": "module_summary",
                        "description": "Get a human-readable summary of a module (struct/function counts and names).",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'"
                        },
                        "example": {"action": "module_summary", "module_path": "0x2::coin"}
                    },
                    {
                        "action": "disassemble_module",
                        "description": "Disassemble an entire module to Move bytecode IR.",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'"
                        },
                        "example": {"action": "disassemble_module", "module_path": "0x2::coin"}
                    },
                    {
                        "action": "get_module_dependencies",
                        "description": "Get the list of modules this module depends on.",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'"
                        },
                        "example": {"action": "get_module_dependencies", "module_path": "0x2::coin"}
                    }
                ]
            },
            "type_introspection": {
                "description": "Tools for inspecting Move types and structs",
                "tools": [
                    {
                        "action": "list_structs",
                        "description": "List all struct types defined in a module.",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'"
                        },
                        "example": {"action": "list_structs", "module_path": "0x2::coin"}
                    },
                    {
                        "action": "get_struct_info",
                        "description": "Get detailed struct information: fields, abilities, type parameters.",
                        "params": {
                            "type_path": "string - full type path like '0x2::coin::Coin'"
                        },
                        "example": {"action": "get_struct_info", "type_path": "0x2::coin::Coin"}
                    },
                    {
                        "action": "inspect_struct",
                        "description": "Get struct definition(s) with optional filtering.",
                        "params": {
                            "package": "string - package address (e.g., '0x2')",
                            "module": "string? - optional module name filter",
                            "struct_name": "string? - optional struct name filter"
                        },
                        "example": {"action": "inspect_struct", "package": "0x2", "module": "coin"}
                    },
                    {
                        "action": "search_types",
                        "description": "Search for struct types matching a pattern across all loaded modules.",
                        "params": {
                            "pattern": "string - pattern with * wildcard (e.g., '*Coin*', '0x2::*')",
                            "ability_filter": "string? - filter by ability ('key', 'store', 'copy', 'drop')"
                        },
                        "example": {"action": "search_types", "pattern": "*Coin*", "ability_filter": "store"}
                    },
                    {
                        "action": "validate_type",
                        "description": "Validate and parse a Move type string.",
                        "params": {
                            "type_str": "string - type like 'u64', 'address', '0x2::coin::Coin<0x2::sui::SUI>'"
                        },
                        "example": {"action": "validate_type", "type_str": "0x2::coin::Coin<0x2::sui::SUI>"}
                    }
                ]
            },
            "function_introspection": {
                "description": "Tools for inspecting Move functions",
                "tools": [
                    {
                        "action": "list_functions",
                        "description": "List all functions in a module.",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'"
                        },
                        "example": {"action": "list_functions", "module_path": "0x2::coin"}
                    },
                    {
                        "action": "get_function_info",
                        "description": "Get detailed function signature: visibility, parameters, return types.",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'",
                            "function_name": "string - function name"
                        },
                        "example": {"action": "get_function_info", "module_path": "0x2::coin", "function_name": "split"}
                    },
                    {
                        "action": "search_functions",
                        "description": "Search for functions matching a pattern across all loaded modules.",
                        "params": {
                            "pattern": "string - pattern with * wildcard",
                            "entry_only": "boolean - if true, only return entry functions (default: false)"
                        },
                        "example": {"action": "search_functions", "pattern": "*transfer*", "entry_only": true}
                    },
                    {
                        "action": "find_constructors",
                        "description": "Find functions that can construct a given type.",
                        "params": {
                            "type_path": "string - full type path like '0x2::coin::Coin'"
                        },
                        "example": {"action": "find_constructors", "type_path": "0x2::coin::Coin"}
                    },
                    {
                        "action": "disassemble_function",
                        "description": "Disassemble function bytecode to instructions with offsets.",
                        "params": {
                            "module_path": "string - e.g., '0x2::coin'",
                            "function_name": "string - function name"
                        },
                        "example": {"action": "disassemble_function", "module_path": "0x2::coin", "function_name": "split"}
                    }
                ]
            },
            "object_management": {
                "description": "Tools for creating and inspecting objects",
                "tools": [
                    {
                        "action": "create_object",
                        "description": "Create an object with specific field values. Object is registered in sandbox.",
                        "params": {
                            "object_type": "string - full type path like '0x2::coin::Coin<0x2::sui::SUI>'",
                            "fields": "object - field name -> value mapping",
                            "object_id": "string? - optional specific object ID (hex), auto-generated if omitted"
                        },
                        "field_types": {
                            "id/UID": "'auto' for fresh ID or hex string",
                            "address": "hex string with or without 0x prefix",
                            "u8/u16/u32/u64": "number",
                            "u128/u256": "string (for large numbers)",
                            "bool": "true/false",
                            "vector<u8>": "string or array of numbers",
                            "String": "string",
                            "Option<T>": "null for None, value for Some"
                        },
                        "example": {"action": "create_object", "object_type": "0x2::coin::Coin<0x2::sui::SUI>", "fields": {"id": "auto", "balance": {"value": 1000000000}}}
                    },
                    {
                        "action": "create_test_object",
                        "description": "Create an object from JSON value. JSON objects become field maps. Primitives are wrapped as {\"value\": ...}. Arrays are wrapped as {\"elements\": [...]}.",
                        "params": {
                            "type_tag": "string - type of object to create",
                            "value": "any - JSON value"
                        },
                        "example": {"action": "create_test_object", "type_tag": "0x2::balance::Balance<0x2::sui::SUI>", "value": {"value": 1000000000}}
                    },
                    {
                        "action": "list_objects",
                        "description": "List all objects in the sandbox with their types and metadata.",
                        "params": {},
                        "example": {"action": "list_objects"}
                    },
                    {
                        "action": "list_shared_objects",
                        "description": "List all shared objects and their current lock status.",
                        "params": {},
                        "example": {"action": "list_shared_objects"}
                    },
                    {
                        "action": "inspect_object",
                        "description": "Decode an object's BCS bytes to readable JSON fields.",
                        "params": {
                            "object_id": "string - hex object ID"
                        },
                        "example": {"action": "inspect_object", "object_id": "0x123..."}
                    }
                ]
            },
            "cached_objects": {
                "description": "Tools for loading pre-cached objects (e.g., from mainnet transaction replays)",
                "tools": [
                    {
                        "action": "load_cached_object",
                        "description": "Load a single cached object with BCS bytes.",
                        "params": {
                            "object_id": "string - hex object ID",
                            "bcs_bytes": "string - base64-encoded BCS bytes",
                            "object_type": "string? - optional type string for introspection",
                            "is_shared": "boolean - whether the object is shared (default: false)"
                        },
                        "example": {"action": "load_cached_object", "object_id": "0x123", "bcs_bytes": "AQID...", "is_shared": false}
                    },
                    {
                        "action": "load_cached_objects",
                        "description": "Load multiple cached objects at once.",
                        "params": {
                            "objects": "object - map of object_id (hex) -> base64 BCS bytes",
                            "object_types": "object? - map of object_id -> type string",
                            "shared_object_ids": "array? - list of object IDs that are shared"
                        },
                        "example": {"action": "load_cached_objects", "objects": {"0x123": "AQID..."}, "shared_object_ids": []}
                    },
                    {
                        "action": "list_cached_objects",
                        "description": "List all loaded cached objects with their types and sizes.",
                        "params": {},
                        "example": {"action": "list_cached_objects"}
                    }
                ]
            },
            "coin_operations": {
                "description": "Tools for managing coin metadata",
                "tools": [
                    {
                        "action": "register_coin",
                        "description": "Register a custom coin type with metadata.",
                        "params": {
                            "coin_type": "string - full coin type like '0xabc::my_coin::MY_COIN'",
                            "decimals": "number - decimal places (e.g., 9 for SUI)",
                            "symbol": "string - short symbol (e.g., 'MYCOIN')",
                            "name": "string - display name (e.g., 'My Coin')"
                        },
                        "example": {"action": "register_coin", "coin_type": "0xabc::my_coin::MY_COIN", "decimals": 9, "symbol": "MYCOIN", "name": "My Coin"}
                    },
                    {
                        "action": "get_coin_metadata",
                        "description": "Get metadata for a registered coin type.",
                        "params": {
                            "coin_type": "string - full coin type"
                        },
                        "example": {"action": "get_coin_metadata", "coin_type": "0x2::sui::SUI"}
                    },
                    {
                        "action": "list_coins",
                        "description": "List all registered coin types with their metadata.",
                        "params": {},
                        "example": {"action": "list_coins"}
                    }
                ]
            },
            "execution": {
                "description": "Tools for executing Move code",
                "tools": [
                    {
                        "action": "execute_ptb",
                        "description": "Execute a Programmable Transaction Block (PTB).",
                        "params": {
                            "inputs": "array - PTB inputs (pure values, object references, gas, witnesses)",
                            "commands": "array - PTB commands to execute"
                        },
                        "input_types": [
                            {"type": "pure", "params": {"value": "any", "value_type": "string - Move type"}},
                            {"type": "object", "params": {"object_id": "string", "mode": "string? - 'immutable' or 'mutable'"}},
                            {"type": "gas", "params": {"budget": "number - gas budget in MIST"}},
                            {"type": "witness", "params": {"witness_type": "string - witness type path"}}
                        ],
                        "command_types": [
                            {"type": "move_call", "params": {"package": "string", "module": "string", "function": "string", "type_args": "array", "args": "array - indices or {cmd, idx} refs"}},
                            {"type": "transfer_objects", "params": {"objects": "array", "recipient": "arg ref"}},
                            {"type": "split_coins", "params": {"coin": "arg ref", "amounts": "array"}},
                            {"type": "merge_coins", "params": {"target": "arg ref", "sources": "array"}},
                            {"type": "make_move_vec", "params": {"element_type": "string?", "elements": "array"}},
                            {"type": "publish", "params": {"modules": "array - base64 bytecode", "dependencies": "array - package IDs"}},
                            {"type": "upgrade", "params": {"modules": "array", "package": "string", "ticket": "arg ref"}},
                            {"type": "receive", "params": {"object_id": "string", "object_type": "string?"}}
                        ],
                        "example": {
                            "action": "execute_ptb",
                            "inputs": [
                                {"type": "object", "object_id": "0x123"},
                                {"type": "pure", "value": 100, "value_type": "u64"}
                            ],
                            "commands": [
                                {"type": "move_call", "package": "0x2", "module": "coin", "function": "split", "type_args": ["0x2::sui::SUI"], "args": [0, 1]}
                            ]
                        }
                    },
                    {
                        "action": "call_function",
                        "description": "Call a Move function directly (simpler than PTB for single calls).",
                        "params": {
                            "package": "string - package address",
                            "module": "string - module name",
                            "function": "string - function name",
                            "type_args": "array - type argument strings",
                            "args": "array - argument values as JSON"
                        },
                        "example": {"action": "call_function", "package": "0x2", "module": "coin", "function": "value", "type_args": ["0x2::sui::SUI"], "args": [{"object_id": "0x123"}]}
                    }
                ]
            },
            "clock_and_time": {
                "description": "Tools for managing simulated blockchain time",
                "tools": [
                    {
                        "action": "get_clock",
                        "description": "Get current sandbox Clock timestamp.",
                        "params": {},
                        "example": {"action": "get_clock"}
                    },
                    {
                        "action": "set_clock",
                        "description": "Set sandbox Clock to a specific timestamp.",
                        "params": {
                            "timestamp_ms": "number - milliseconds since Unix epoch"
                        },
                        "example": {"action": "set_clock", "timestamp_ms": 1700000000000_i64}
                    }
                ]
            },
            "bcs_encoding": {
                "description": "Tools for BCS (Binary Canonical Serialization) encoding/decoding",
                "tools": [
                    {
                        "action": "encode_bcs",
                        "description": "Encode a JSON value to BCS bytes.",
                        "params": {
                            "type_str": "string - Move type to encode as",
                            "value": "any - JSON value to encode"
                        },
                        "example": {"action": "encode_bcs", "type_str": "u64", "value": 42}
                    },
                    {
                        "action": "decode_bcs",
                        "description": "Decode BCS bytes to a JSON value.",
                        "params": {
                            "type_str": "string - Move type to decode as",
                            "bytes_hex": "string - hex-encoded BCS bytes"
                        },
                        "example": {"action": "decode_bcs", "type_str": "u64", "bytes_hex": "2a00000000000000"}
                    },
                    {
                        "action": "encode_vector",
                        "description": "Encode a vector of values to BCS bytes.",
                        "params": {
                            "element_type": "string - Move type of vector elements",
                            "values": "array - JSON values to encode"
                        },
                        "example": {"action": "encode_vector", "element_type": "u64", "values": [1, 2, 3]}
                    }
                ]
            },
            "system_objects": {
                "description": "Tools for well-known Sui system objects",
                "tools": [
                    {
                        "action": "get_system_object_info",
                        "description": "Get information about well-known Sui system objects.",
                        "params": {
                            "object_name": "string - one of: 'clock', 'random', 'deny_list', 'system_state'"
                        },
                        "example": {"action": "get_system_object_info", "object_name": "clock"}
                    }
                ]
            },
            "utilities": {
                "description": "Utility tools for working with addresses, IDs, hashes, and numbers",
                "tools": [
                    {
                        "action": "generate_id",
                        "description": "Generate a fresh unique object ID.",
                        "params": {},
                        "example": {"action": "generate_id"}
                    },
                    {
                        "action": "parse_address",
                        "description": "Parse an address string to canonical form.",
                        "params": {
                            "address": "string - address in any format (short or long form)"
                        },
                        "example": {"action": "parse_address", "address": "0x2"}
                    },
                    {
                        "action": "format_address",
                        "description": "Format an address string to a specific format.",
                        "params": {
                            "address": "string - address to format",
                            "format": "string? - 'short' (default) or 'full'"
                        },
                        "example": {"action": "format_address", "address": "0x0000000000000000000000000000000000000000000000000000000000000002", "format": "short"}
                    },
                    {
                        "action": "compute_hash",
                        "description": "Compute cryptographic hash of hex-encoded bytes.",
                        "params": {
                            "bytes_hex": "string - hex-encoded bytes to hash",
                            "algorithm": "string? - 'sha256' (default), 'sha3_256', or 'blake2b256'"
                        },
                        "example": {"action": "compute_hash", "bytes_hex": "48656c6c6f", "algorithm": "sha256"}
                    },
                    {
                        "action": "convert_number",
                        "description": "Convert number between types (u8, u16, u32, u64, u128, u256).",
                        "params": {
                            "value": "string - numeric value as string",
                            "from_type": "string - source type",
                            "to_type": "string - target type"
                        },
                        "example": {"action": "convert_number", "value": "255", "from_type": "u64", "to_type": "u8"}
                    },
                    {
                        "action": "parse_error",
                        "description": "Parse a Move error string into structured components.",
                        "params": {
                            "error": "string - error message to parse"
                        },
                        "example": {"action": "parse_error", "error": "MoveAbort in 0x2::coin: 0x10001"}
                    }
                ]
            },
            "framework_cache": {
                "description": "Tools for managing the Sui framework cache",
                "tools": [
                    {
                        "action": "is_framework_cached",
                        "description": "Check if the Sui framework is cached locally.",
                        "params": {},
                        "example": {"action": "is_framework_cached"}
                    },
                    {
                        "action": "ensure_framework_cached",
                        "description": "Ensure the Sui framework is downloaded and cached.",
                        "params": {},
                        "example": {"action": "ensure_framework_cached"}
                    }
                ]
            },
            "discovery": {
                "description": "Meta tools for discovering available capabilities",
                "tools": [
                    {
                        "action": "list_available_tools",
                        "description": "List all available sandbox tools with their schemas (this tool).",
                        "params": {},
                        "example": {"action": "list_available_tools"}
                    }
                ]
            }
        },
        "type_format": {
            "format": "address::module::TypeName<TypeArg1, TypeArg2>",
            "primitives": ["bool", "u8", "u16", "u32", "u64", "u128", "u256", "address", "signer"],
            "address_format": "short form (0x2, not 0x0000...0002)"
        },
        "response_format": {
            "success_field": "boolean 'success' in all responses",
            "error_fields": ["error", "error_category", "abort_code", "abort_module"],
            "type_field": "'type' for object types in responses"
        }
    });

    SandboxResponse::success_with_data(tools)
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
                eprintln!(
                    "State file {} does not exist, creating new environment",
                    state_file.display()
                );
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
