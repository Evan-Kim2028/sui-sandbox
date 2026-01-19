//! # Sandbox Request/Response Types
//!
//! This module contains all data structures for sandbox request/response serialization.
//! These types are used for JSON-based communication between LLM agents and the sandbox.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Request Types
// =============================================================================

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
    // Mainnet State Import Tools
    // ========================================================================
    // These tools allow importing real mainnet state into the sandbox for
    // accurate simulation. Use sparingly - prefer cached data when available.
    /// Import a package from Sui mainnet into the sandbox.
    /// This fetches the package bytecode and deploys it locally.
    ///
    /// Use this when you need to interact with a mainnet package that isn't
    /// already loaded. The package will be available at the same address
    /// as on mainnet.
    ///
    /// Example: Import the SuiNS package to test name resolution.
    #[serde(rename = "import_package_from_mainnet")]
    ImportPackageFromMainnet {
        /// Package address on mainnet (hex string, e.g., "0xd22b24490e0bae52676651b4f56660a5ff8022a2576e0089f79b3c88d44e08f0").
        package_id: String,
        /// Network to fetch from. Default: "mainnet".
        #[serde(default)]
        network: Option<String>,
    },

    /// Import an object from Sui mainnet into the sandbox.
    /// This fetches the object's current state (type, fields, ownership).
    ///
    /// Use this when you need a real object for simulation (e.g., a specific
    /// NFT, a pool, or a shared object). The object will be available at
    /// the same address as on mainnet.
    ///
    /// Note: If the object's type requires a package that isn't loaded,
    /// you should import the package first.
    #[serde(rename = "import_object_from_mainnet")]
    ImportObjectFromMainnet {
        /// Object ID on mainnet (hex string).
        object_id: String,
        /// Network to fetch from. Default: "mainnet".
        #[serde(default)]
        network: Option<String>,
    },

    /// Import multiple objects from mainnet in a batch.
    /// More efficient than individual imports for multiple objects.
    #[serde(rename = "import_objects_from_mainnet")]
    ImportObjectsFromMainnet {
        /// List of object IDs to import.
        object_ids: Vec<String>,
        /// Network to fetch from. Default: "mainnet".
        #[serde(default)]
        network: Option<String>,
    },

    /// Import an object at a specific historical version from mainnet.
    /// Useful for replaying transactions or testing against past state.
    #[serde(rename = "import_object_at_version")]
    ImportObjectAtVersion {
        /// Object ID on mainnet.
        object_id: String,
        /// Specific version to fetch.
        version: u64,
        /// Network to fetch from. Default: "mainnet".
        #[serde(default)]
        network: Option<String>,
    },

    // ========================================================================
    // Sender Management
    // ========================================================================
    /// Set the transaction sender address.
    /// This address is used as the sender in TxContext for all subsequent operations.
    #[serde(rename = "set_sender")]
    SetSender {
        /// Sender address (hex string, e.g., "0x123...").
        address: String,
    },

    /// Get the current transaction sender address.
    #[serde(rename = "get_sender")]
    GetSender,

    // ========================================================================
    // State Persistence
    // ========================================================================
    /// Save the current sandbox state to a file.
    /// This creates a JSON snapshot that can be loaded later.
    #[serde(rename = "save_state")]
    SaveState {
        /// Path to save the state file.
        path: String,
        /// Optional description for the saved state.
        #[serde(default)]
        description: Option<String>,
        /// Optional tags for categorizing the saved state.
        #[serde(default)]
        tags: Option<Vec<String>>,
    },

    /// Load a previously saved sandbox state from a file.
    /// This replaces the current state with the loaded state.
    #[serde(rename = "load_state")]
    LoadState {
        /// Path to the state file to load.
        path: String,
    },

    // ========================================================================
    // Meta / Discovery Tools
    // ========================================================================
    /// List all available sandbox tools and their schemas.
    /// This is the unified tool discovery endpoint for LLM agents.
    #[serde(rename = "list_available_tools")]
    ListAvailableTools,
}

// =============================================================================
// PTB Types
// =============================================================================

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

// =============================================================================
// Response Types
// =============================================================================

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

// =============================================================================
// Introspection Types
// =============================================================================

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
