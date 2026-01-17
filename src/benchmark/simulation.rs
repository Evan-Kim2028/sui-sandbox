//! # Simulation Environment for LLM-Driven PTB Construction
//!
//! **This is the primary API for sandbox/PTB simulation.** Use this module for:
//! - Interactive PTB building and execution
//! - LLM-driven transaction construction
//! - Debugging and testing Move transactions
//!
//! This module provides an interactive simulation environment that allows an LLM to:
//! 1. Deploy Move packages (bytecode)
//! 2. Create objects with specific state
//! 3. Execute PTBs and receive structured error feedback
//! 4. Iteratively fix issues until PTBs succeed
//!
//! ## Key Features
//!
//! - **Full PTB Support**: MoveCall, SplitCoins, MergeCoins, TransferObjects, Publish, Upgrade
//! - **Dynamic Publishing**: Publish modules and call them within the SAME PTB - published
//!   modules are pre-processed before execution and immediately available for subsequent commands
//! - **Session Persistence**: Published modules persist across PTBs within the same session,
//!   enabling iterative development workflows (publish once, call many times)
//! - **Object Tracking**: Created, mutated, deleted objects tracked across commands
//! - **Shared Object Locking**: Automatic lock acquisition/release for shared objects
//! - **Dynamic Fields**: Tables, Bags, and dynamic field operations fully supported
//! - **Return Value Capture**: Function return values available in transaction effects
//! - **Gas Accounting**: Estimated gas usage with budget enforcement
//! - **Epoch & Time**: Configurable epoch numbers and timestamps for TxContext
//! - **Randomness**: Deterministic random number generation with configurable seeds (0x8 Random object)
//! - **Lamport Clock**: Version tracking for shared object consensus simulation
//! - **Structured Errors**: Errors designed for programmatic consumption
//!
//! ## Design Philosophy
//!
//! The environment provides neutral, factual error feedback:
//! - Errors report what happened without prescribing fixes
//! - The goal is 1:1 parity with mainnet execution semantics
//!
//! ## Extensibility
//!
//! The `StateSource` trait allows plugging in different backends for module/object
//! resolution. See [`state_source`](super::state_source) for details.
//!
//! ## Example Usage
//!
//! ```ignore
//! let mut env = SimulationEnvironment::new()?;
//!
//! // Deploy a package
//! env.deploy_package_from_mainnet("0x1234...")?;
//!
//! // Create objects needed for the PTB
//! let coin_id = env.create_coin("0x2::sui::SUI", 1_000_000_000)?;
//!
//! // Execute a PTB
//! let result = env.execute_ptb(commands)?;
//! match result {
//!     Ok(effects) => println!("Success: {:?}", effects),
//!     Err(SimulationError::MissingPackage { address, module }) => {
//!         // Package not found - address contains the missing package ID
//!     }
//!     Err(SimulationError::MissingObject { id, expected_type }) => {
//!         // Object not found - id contains the missing object ID
//!     }
//!     Err(e) => println!("Error: {}", e),
//! }
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use move_core_types::resolver::ModuleResolver;
use std::collections::BTreeMap;

use crate::benchmark::errors::{Phase, PhaseOptionExt, PhaseResultExt};
use crate::benchmark::resolver::LocalModuleResolver;

// ============================================================================
// Coin Constants
// ============================================================================

/// SUI coin decimals (1 SUI = 10^9 MIST)
pub const SUI_DECIMALS: u8 = 9;

/// SUI coin symbol
pub const SUI_SYMBOL: &str = "SUI";

/// SUI coin type string
pub const SUI_COIN_TYPE: &str = "0x2::sui::SUI";

/// Default gas price in MIST
pub const DEFAULT_GAS_PRICE: u64 = 1000;

/// Clock object ID (0x6) - well-known system object
pub const CLOCK_OBJECT_ID: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000006";

/// Random object ID (0x8) - well-known system object for on-chain randomness
pub const RANDOM_OBJECT_ID: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000008";

/// Default Clock timestamp base (2024-01-01 00:00:00 UTC)
pub const DEFAULT_CLOCK_BASE_MS: u64 = 1704067200000;

/// Coin metadata for registered coins
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoinMetadata {
    /// Number of decimal places
    pub decimals: u8,
    /// Coin symbol (e.g., "SUI", "USDC")
    pub symbol: String,
    /// Coin name (e.g., "Sui", "USD Coin")
    pub name: String,
    /// Full type tag (e.g., "0x2::sui::SUI")
    pub type_tag: String,
}
use crate::benchmark::fetcher::{FetchedObjectData, Fetcher, GrpcFetcher};
use crate::benchmark::object_runtime::ChildFetcherFn;
use crate::benchmark::ptb::{Command, InputValue, ObjectInput, TransactionEffects};
use crate::benchmark::vm::VMHarness;

// Re-export EmittedEvent for convenience
pub use crate::benchmark::natives::EmittedEvent;

/// Structured error types matching Sui mainnet error semantics.
#[derive(Debug, Clone)]
pub enum SimulationError {
    /// LINKER_ERROR: A required package/module is not available.
    MissingPackage {
        address: String,
        module: Option<String>,
    },

    /// A required object is not available.
    MissingObject {
        id: String,
        expected_type: Option<String>,
    },

    /// Type mismatch between expected and provided.
    TypeMismatch {
        expected: String,
        got: String,
        location: String,
    },

    /// ABORTED: Contract assertion failed.
    ContractAbort {
        module: String,
        function: String,
        abort_code: u64,
        message: Option<String>,
    },

    /// FAILED_TO_DESERIALIZE_ARGUMENT: Deserialization failed for an argument.
    DeserializationFailed {
        argument_index: usize,
        expected_type: String,
    },

    /// Other execution error.
    ExecutionError { message: String },

    /// Shared object lock conflict (concurrent access detection).
    SharedObjectLockConflict {
        object_id: AccountAddress,
        held_by: Option<String>,
        reason: String,
    },
}

impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulationError::MissingPackage { address, module } => {
                if let Some(m) = module {
                    write!(f, "LINKER_ERROR: Cannot find ModuleId {{ address: {}, name: Identifier(\"{}\") }}",
                        address.trim_start_matches("0x"), m)
                } else {
                    write!(f, "LINKER_ERROR: Cannot find package {}", address)
                }
            }
            SimulationError::MissingObject { id, expected_type } => {
                if let Some(t) = expected_type {
                    write!(f, "ObjectNotFound: {} (expected type: {})", id, t)
                } else {
                    write!(f, "ObjectNotFound: {}", id)
                }
            }
            SimulationError::TypeMismatch {
                expected,
                got,
                location,
            } => {
                write!(
                    f,
                    "TYPE_MISMATCH at {}: expected {}, got {}",
                    location, expected, got
                )
            }
            SimulationError::ContractAbort {
                module,
                function,
                abort_code,
                message,
            } => {
                if let Some(msg) = message {
                    write!(f, "MoveAbort(MoveLocation {{ module: {}::{}, function: 0, instruction: 0 }}, {}) in {}",
                        module, function, abort_code, msg)
                } else {
                    write!(f, "MoveAbort(MoveLocation {{ module: {}::{}, function: 0, instruction: 0 }}, {})",
                        module, function, abort_code)
                }
            }
            SimulationError::DeserializationFailed {
                argument_index,
                expected_type,
            } => {
                write!(
                    f,
                    "FAILED_TO_DESERIALIZE_ARGUMENT: argument {} cannot be deserialized as {}",
                    argument_index, expected_type
                )
            }
            SimulationError::ExecutionError { message } => {
                write!(f, "{}", message)
            }
            SimulationError::SharedObjectLockConflict {
                object_id,
                held_by,
                reason,
            } => {
                if let Some(tx) = held_by {
                    write!(
                        f,
                        "SharedObjectLockConflict: object {} is locked by transaction {}: {}",
                        object_id.to_hex_literal(),
                        tx,
                        reason
                    )
                } else {
                    write!(
                        f,
                        "SharedObjectLockConflict: object {}: {}",
                        object_id.to_hex_literal(),
                        reason
                    )
                }
            }
        }
    }
}

impl std::error::Error for SimulationError {}

// ============================================================================
// Compile Types
// ============================================================================

/// Result of a successful Move source compilation.
#[derive(Debug)]
pub struct CompileResult {
    /// Path to the build directory containing compiled artifacts.
    pub build_dir: std::path::PathBuf,

    /// Paths to compiled bytecode files (.mv).
    pub modules: Vec<std::path::PathBuf>,

    /// Compilation warnings (if any).
    pub warnings: Vec<String>,
}

/// Detailed information about a compile error.
#[derive(Debug, Clone)]
pub struct CompileErrorDetail {
    /// Source file where the error occurred.
    pub file: Option<String>,

    /// Line number in the source file.
    pub line: Option<u32>,

    /// Column number in the source file.
    pub column: Option<u32>,

    /// Error message from the compiler.
    pub message: String,
}

impl CompileErrorDetail {
    /// Format this error for display.
    pub fn format(&self) -> String {
        let location = match (&self.file, self.line, self.column) {
            (Some(f), Some(l), Some(c)) => format!("{}:{}:{}: ", f, l, c),
            (Some(f), Some(l), None) => format!("{}:{}: ", f, l),
            (Some(f), None, None) => format!("{}: ", f),
            _ => String::new(),
        };

        format!("{}{}", location, self.message)
    }
}

/// Error from Move source compilation.
#[derive(Debug)]
pub struct CompileError {
    /// Structured compile errors.
    pub errors: Vec<CompileErrorDetail>,

    /// Raw compiler output for debugging.
    pub raw_output: String,
}

impl CompileError {
    /// Format all errors for display.
    pub fn format_errors(&self) -> String {
        if self.errors.is_empty() {
            self.raw_output.clone()
        } else {
            self.errors
                .iter()
                .map(|e| e.format())
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format_errors())
    }
}

impl std::error::Error for CompileError {}

// ============================================================================
// Execution Types
// ============================================================================

/// Result of a PTB execution.
#[derive(Debug)]
pub struct ExecutionResult {
    /// Whether execution succeeded.
    pub success: bool,

    /// Effects if successful.
    pub effects: Option<TransactionEffects>,

    /// Structured error if failed.
    pub error: Option<SimulationError>,

    /// Raw error message (for debugging).
    pub raw_error: Option<String>,

    /// Index of the command that failed (0-based), if execution failed.
    pub failed_command_index: Option<usize>,

    /// Description of the failed command (e.g., "MoveCall 0x2::coin::split").
    pub failed_command_description: Option<String>,

    /// Number of commands that succeeded before the failure.
    pub commands_succeeded: usize,
}

/// An object in the simulation environment.
#[derive(Debug, Clone)]
pub struct SimulatedObject {
    /// Object ID.
    pub id: AccountAddress,

    /// Move type of the object.
    pub type_tag: TypeTag,

    /// BCS-serialized object contents.
    pub bcs_bytes: Vec<u8>,

    /// Whether this object is shared.
    pub is_shared: bool,

    /// Whether this object is immutable.
    pub is_immutable: bool,

    /// Version number (for tracking mutations).
    pub version: u64,
}

// ============================================================================
// Persistent State Types (for serialization)
// ============================================================================

/// Serializable version of SimulatedObject for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedObject {
    /// Object ID as hex string.
    pub id: String,
    /// Move type as string (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    pub type_tag: String,
    /// BCS-serialized object contents (base64 encoded).
    pub bcs_bytes_b64: String,
    /// Whether this object is shared.
    pub is_shared: bool,
    /// Whether this object is immutable.
    pub is_immutable: bool,
    /// Version number.
    pub version: u64,
}

/// Serializable module for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedModule {
    /// Module ID (package::module).
    pub id: String,
    /// Module bytecode (base64 encoded).
    pub bytecode_b64: String,
}

/// Serializable dynamic field for persistence.
/// Dynamic fields are used by Table, Bag, and other collection types.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedDynamicField {
    /// Parent object ID (hex string).
    pub parent_id: String,
    /// Child object ID (hex string).
    pub child_id: String,
    /// Type tag as string (e.g., "0x2::dynamic_field::Field<u64, 0x2::coin::Coin<0x2::sui::SUI>>").
    pub type_tag: String,
    /// BCS-serialized field value (base64 encoded).
    pub value_b64: String,
}

/// Serializable pending receive for persistence.
/// Pending receives are objects that have been transferred to another object
/// and are waiting to be received via `transfer::receive`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SerializedPendingReceive {
    /// Recipient object ID (hex string).
    pub recipient_id: String,
    /// Sent object ID (hex string).
    pub sent_id: String,
    /// Object type tag as string.
    pub type_tag: String,
    /// BCS-serialized object bytes (base64 encoded).
    pub object_bytes_b64: String,
}

/// Persistent sandbox state that can be saved to/loaded from a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistentState {
    /// Version of the state format (for forward compatibility).
    pub version: u32,
    /// Objects in the environment.
    pub objects: Vec<SerializedObject>,
    /// Non-framework modules (user-deployed packages).
    pub modules: Vec<SerializedModule>,
    /// Coin registry.
    pub coin_registry: std::collections::HashMap<String, CoinMetadata>,
    /// Sender address.
    pub sender: String,
    /// ID counter for generating fresh IDs.
    pub id_counter: u64,
    /// Timestamp in milliseconds.
    pub timestamp_ms: Option<u64>,
    /// Dynamic fields (Table/Bag entries) - added in v2.
    #[serde(default)]
    pub dynamic_fields: Vec<SerializedDynamicField>,
    /// Pending receives (send-to-object pattern) - added in v2.
    #[serde(default)]
    pub pending_receives: Vec<SerializedPendingReceive>,
    /// Simulation configuration (epoch, gas, clock, etc.) - added in v3.
    #[serde(default)]
    pub config: Option<crate::benchmark::vm::SimulationConfig>,
    /// State file metadata - added in v3.
    #[serde(default)]
    pub metadata: Option<StateMetadata>,
    /// Fetcher configuration for mainnet data access - added in v4.
    /// When present, the fetcher will be auto-reconnected on state load.
    #[serde(default)]
    pub fetcher_config: Option<FetcherConfig>,
}

/// Metadata for state files - helps organize multiple simulation scenarios.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct StateMetadata {
    /// Human-readable description of this simulation state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When this state was created (ISO 8601 timestamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// When this state was last modified (ISO 8601 timestamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// Tags for categorization (e.g., ["defi", "cetus", "testnet"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Configuration for data fetching - persisted separately from TransactionFetcher
/// because the fetcher itself contains non-serializable runtime state (gRPC clients, tokio runtime).
///
/// This allows save/load to remember that mainnet fetching was enabled and auto-reconnect
/// when the state is restored.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct FetcherConfig {
    /// Whether mainnet fetching is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Network to fetch from: "mainnet" or "testnet".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Custom endpoint URL (if not using default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Whether to use archive endpoint for historical data.
    #[serde(default)]
    pub use_archive: bool,
}

impl FetcherConfig {
    /// Create a config for mainnet fetching.
    pub fn mainnet() -> Self {
        Self {
            enabled: true,
            network: Some("mainnet".to_string()),
            endpoint: None,
            use_archive: false,
        }
    }

    /// Create a config for mainnet with archive support.
    pub fn mainnet_with_archive() -> Self {
        Self {
            enabled: true,
            network: Some("mainnet".to_string()),
            endpoint: None,
            use_archive: true,
        }
    }

    /// Create a config for testnet fetching.
    pub fn testnet() -> Self {
        Self {
            enabled: true,
            network: Some("testnet".to_string()),
            endpoint: None,
            use_archive: false,
        }
    }

    /// Create a config with a custom endpoint.
    pub fn custom(endpoint: impl Into<String>) -> Self {
        Self {
            enabled: true,
            network: None,
            endpoint: Some(endpoint.into()),
            use_archive: false,
        }
    }

    /// Check if this config represents no fetching (disabled).
    pub fn is_disabled(&self) -> bool {
        !self.enabled
    }
}

impl PersistentState {
    /// Current state format version.
    /// v1: Initial version (objects, modules, coins, sender, id_counter, timestamp)
    /// v2: Added dynamic_fields and pending_receives for Table/Bag persistence
    /// v3: Added config (SimulationConfig) and metadata (description, timestamps, tags)
    /// v4: Added fetcher_config for persistent mainnet fetching configuration
    pub const CURRENT_VERSION: u32 = 4;
}

// ============================================================================
// Struct Definition Types (for inspection)
// ============================================================================

/// Struct definition extracted from bytecode.
#[derive(Debug, Clone)]
pub struct StructDefinition {
    pub package: String,
    pub module: String,
    pub name: String,
    pub abilities: Vec<String>,
    pub type_params: Vec<TypeParamDef>,
    pub fields: Vec<FieldDefinition>,
}

#[derive(Debug, Clone)]
pub struct TypeParamDef {
    pub name: String,
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FieldDefinition {
    pub name: String,
    pub field_type: String,
}

/// State summary for sandbox inspection.
#[derive(Debug, Clone)]
pub struct StateSummary {
    pub loaded_packages: Vec<String>,
    pub loaded_modules: Vec<(String, String)>,
    pub object_count: usize,
    pub sender: String,
    pub timestamp_ms: u64,
}

/// Shared object lock entry tracking access mode and version.
#[derive(Debug, Clone)]
pub struct SharedObjectLock {
    /// Object ID of the shared object.
    pub object_id: AccountAddress,
    /// Version at which the lock was acquired.
    pub version: u64,
    /// Whether the lock is for mutable access.
    pub is_mutable: bool,
    /// Transaction ID that holds the lock (for diagnostics).
    pub transaction_id: Option<String>,
}

/// Consensus ordering entry for tracking transaction serialization.
///
/// In Sui's consensus model, shared object transactions are serialized.
/// Each transaction has:
/// - A sequence number (global ordering)
/// - Read versions for each object it reads
/// - Write versions for each object it writes
///
/// Two transactions conflict (serialization violation) if:
/// - Tx B reads an object that Tx A writes, and B.seq > A.seq but B.read_version < A.write_version
/// - Tx B writes an object that Tx A reads, and B.seq > A.seq but B.write_version <= A.read_version
#[derive(Debug, Clone)]
pub struct ConsensusOrderEntry {
    /// Transaction sequence number (global ordering from consensus).
    pub sequence: u64,
    /// Transaction ID for diagnostics.
    pub transaction_id: String,
    /// Object ID -> version read by this transaction.
    pub read_versions: BTreeMap<AccountAddress, u64>,
    /// Object ID -> version written by this transaction.
    pub write_versions: BTreeMap<AccountAddress, u64>,
    /// Timestamp when transaction was ordered.
    pub timestamp_ms: u64,
}

/// Result of consensus validation.
#[derive(Debug, Clone)]
pub enum ConsensusValidation {
    /// Transaction can proceed - no serialization conflicts.
    Valid,
    /// Serialization conflict detected.
    SerializationConflict {
        /// The conflicting object ID.
        object_id: AccountAddress,
        /// Our transaction's intended version.
        our_version: u64,
        /// The conflicting transaction's version.
        their_version: u64,
        /// The conflicting transaction ID.
        conflicting_tx: String,
        /// Description of the conflict.
        reason: String,
    },
    /// Stale read detected (read version is behind current object version).
    StaleRead {
        /// The object with stale read.
        object_id: AccountAddress,
        /// Version we tried to read.
        read_version: u64,
        /// Current object version.
        current_version: u64,
    },
}

/// Result of attempting to acquire shared object locks.
#[derive(Debug, Clone)]
pub enum LockResult {
    /// All locks acquired successfully.
    Success {
        /// The locks that were acquired.
        locks: Vec<SharedObjectLock>,
    },
    /// Lock conflict detected.
    Conflict {
        /// The object ID that has a conflict.
        object_id: AccountAddress,
        /// The conflicting lock.
        existing_lock: SharedObjectLock,
        /// Description of the conflict.
        reason: String,
    },
}

/// Result of a direct function call.
#[derive(Debug, Clone)]
pub struct FunctionCallResult {
    pub return_values: Vec<Vec<u8>>,
    pub gas_used: u64,
}

// ============================================================================
// State Checkpoint Types
// ============================================================================

/// A checkpoint of simulation state for rollback support.
///
/// Captures the full simulation state at a point in time, allowing
/// restoration to that state later.
#[derive(Debug, Clone)]
pub struct StateCheckpoint {
    /// Snapshot of all objects.
    pub objects: BTreeMap<AccountAddress, SimulatedObject>,
    /// Snapshot of dynamic fields.
    pub dynamic_fields: BTreeMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)>,
    /// Snapshot of shared locks.
    pub shared_locks: BTreeMap<AccountAddress, SharedObjectLock>,
    /// Lamport clock at checkpoint time.
    pub lamport_clock: u64,
    /// Consensus sequence at checkpoint time.
    pub consensus_sequence: u64,
    /// Transaction counter at checkpoint time.
    pub tx_counter: u64,
    /// ID counter at checkpoint time.
    pub id_counter: u64,
}

/// Encode a u64 as ULEB128.
fn leb128_encode(mut val: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;
        if val != 0 {
            byte |= 0x80;
        }
        bytes.push(byte);
        if val == 0 {
            break;
        }
    }
    bytes
}

/// The main simulation environment.
pub struct SimulationEnvironment {
    /// Module resolver with loaded packages.
    resolver: LocalModuleResolver,

    /// Object store: id -> SimulatedObject.
    objects: BTreeMap<AccountAddress, SimulatedObject>,

    /// Counter for generating fresh object IDs.
    id_counter: u64,

    /// Data fetcher for on-demand mainnet/testnet data loading.
    /// Uses the Fetcher trait for abstraction over data sources.
    fetcher: Option<Box<dyn Fetcher>>,

    /// Fetcher configuration (serializable, persisted across save/load).
    /// This is kept separate from `fetcher` because the fetcher contains
    /// non-serializable runtime state (gRPC clients, tokio runtime).
    fetcher_config: FetcherConfig,

    /// Transaction sender address for TxContext.
    sender: AccountAddress,

    /// Transaction timestamp in milliseconds for TxContext.
    timestamp_ms: Option<u64>,

    /// Registry of coin metadata (type_tag -> CoinMetadata).
    /// Pre-populated with SUI coin.
    coin_registry: BTreeMap<String, CoinMetadata>,

    /// Dynamic field store: (parent_id, child_id) -> (type_tag, bytes).
    /// Used to persist Table/Bag entries across PTB executions.
    dynamic_fields: BTreeMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)>,

    /// Shared object locks: object_id -> lock info.
    /// Used to simulate concurrent access control for shared objects.
    shared_locks: BTreeMap<AccountAddress, SharedObjectLock>,

    /// Transaction counter for generating unique transaction IDs.
    tx_counter: u64,

    /// Pending receives: (recipient_object_id, sent_object_id) -> object bytes.
    /// Used for the send-to-object pattern where objects are transferred to another
    /// object and later received via `transfer::receive`.
    pending_receives: BTreeMap<(AccountAddress, AccountAddress), (Vec<u8>, TypeTag)>,

    /// Lamport clock for shared object versioning and consensus simulation.
    /// Incremented on every transaction that touches shared objects.
    lamport_clock: u64,

    /// Simulation configuration (epoch, gas, etc.).
    config: crate::benchmark::vm::SimulationConfig,

    /// Consensus ordering history for serialization validation.
    /// Stores recent transaction ordering entries for conflict detection.
    consensus_history: Vec<ConsensusOrderEntry>,

    /// Global sequence counter for consensus ordering.
    /// Incremented for every shared object transaction.
    consensus_sequence: u64,

    /// All events captured during this session, across all PTB executions.
    all_events: Vec<EmittedEvent>,

    /// Events from the last PTB execution only.
    last_tx_events: Vec<EmittedEvent>,

    /// Optional callback for on-demand child object fetching during execution.
    /// This is called when a dynamic field child is requested but not found in preloaded state.
    child_fetcher: Option<std::sync::Arc<ChildFetcherFn>>,
}

impl SimulationEnvironment {
    /// Create a new simulation environment with the Sui framework loaded.
    pub fn new() -> Result<Self> {
        let resolver = LocalModuleResolver::with_sui_framework()?;
        Self::with_resolver(resolver)
    }

    /// Create a new simulation environment with a pre-configured resolver.
    ///
    /// This allows reusing a resolver that already has modules loaded,
    /// which is useful for benchmark scenarios where you want to test
    /// multiple functions against the same package corpus.
    pub fn with_resolver(resolver: LocalModuleResolver) -> Result<Self> {
        // Initialize coin registry with SUI
        let mut coin_registry = BTreeMap::new();
        coin_registry.insert(
            SUI_COIN_TYPE.to_string(),
            CoinMetadata {
                decimals: SUI_DECIMALS,
                symbol: SUI_SYMBOL.to_string(),
                name: "Sui".to_string(),
                type_tag: SUI_COIN_TYPE.to_string(),
            },
        );

        let mut env = Self {
            resolver,
            objects: BTreeMap::new(),
            id_counter: 0,
            fetcher: None,
            fetcher_config: FetcherConfig::default(),
            sender: AccountAddress::ZERO,
            timestamp_ms: None,
            coin_registry,
            dynamic_fields: BTreeMap::new(),
            shared_locks: BTreeMap::new(),
            tx_counter: 0,
            pending_receives: BTreeMap::new(),
            lamport_clock: 0,
            config: crate::benchmark::vm::SimulationConfig::default(),
            consensus_history: Vec::new(),
            consensus_sequence: 0,
            all_events: Vec::new(),
            last_tx_events: Vec::new(),
            child_fetcher: None,
        };

        // Initialize the Clock object (0x6)
        env.initialize_clock()?;

        // Initialize the Random object (0x8)
        env.initialize_random()?;

        Ok(env)
    }

    /// Initialize the Clock object at address 0x6.
    /// The Clock is a shared, immutable system object used for time-dependent operations.
    fn initialize_clock(&mut self) -> Result<()> {
        let clock_id = AccountAddress::from_hex_literal(CLOCK_OBJECT_ID)
            .map_err(|e| anyhow!("Invalid clock ID: {}", e))?;

        // Clock struct: { id: UID, timestamp_ms: u64 }
        let timestamp_ms = self.timestamp_ms.unwrap_or(DEFAULT_CLOCK_BASE_MS);
        let mut clock_bytes = Vec::with_capacity(40);
        clock_bytes.extend_from_slice(clock_id.as_ref()); // UID (32 bytes)
        clock_bytes.extend_from_slice(&timestamp_ms.to_le_bytes()); // timestamp_ms (8 bytes)

        // Clock type: 0x2::clock::Clock
        let clock_type = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("clock").unwrap(),
            name: Identifier::new("Clock").unwrap(),
            type_params: vec![],
        }));

        let clock_obj = SimulatedObject {
            id: clock_id,
            type_tag: clock_type,
            bcs_bytes: clock_bytes,
            is_shared: true,
            is_immutable: false, // Clock is shared, not immutable
            version: 1,
        };
        self.objects.insert(clock_id, clock_obj);

        Ok(())
    }

    /// Initialize the Random object at address 0x8.
    /// The Random object is a shared system object for on-chain randomness.
    /// In simulation, it produces deterministic values based on the configured seed.
    fn initialize_random(&mut self) -> Result<()> {
        let random_id = AccountAddress::from_hex_literal(RANDOM_OBJECT_ID)
            .map_err(|e| anyhow!("Invalid random ID: {}", e))?;

        // Random struct: { id: UID, inner: Versioned { id: UID, version: u64 } }
        // Simplified: we just need a valid object with the UID
        // The actual randomness is handled by the native function mocks
        let mut random_bytes = Vec::with_capacity(48);
        random_bytes.extend_from_slice(random_id.as_ref()); // UID (32 bytes)
                                                            // Inner Versioned struct: { id: UID, version: u64 }
                                                            // For simplicity, use same ID and version 1
        random_bytes.extend_from_slice(random_id.as_ref()); // inner UID (32 bytes)
        random_bytes.extend_from_slice(&1u64.to_le_bytes()); // version (8 bytes)

        // Random type: 0x2::random::Random
        let random_type = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("random").unwrap(),
            name: Identifier::new("Random").unwrap(),
            type_params: vec![],
        }));

        let random_obj = SimulatedObject {
            id: random_id,
            type_tag: random_type,
            bcs_bytes: random_bytes,
            is_shared: true,
            is_immutable: false, // Random is shared, not immutable
            version: 1,
        };
        self.objects.insert(random_id, random_obj);

        Ok(())
    }

    /// Get the Random object for PTB execution.
    /// Returns it as a shared object input.
    pub fn get_random_object(&self) -> Result<crate::benchmark::ptb::ObjectInput> {
        let random_id = AccountAddress::from_hex_literal(RANDOM_OBJECT_ID)
            .map_err(|e| anyhow!("Invalid random ID: {}", e))?;

        let random_obj = self
            .objects
            .get(&random_id)
            .ok_or_else(|| anyhow!("Random object not found - this should not happen"))?;

        Ok(crate::benchmark::ptb::ObjectInput::Shared {
            id: random_id,
            bytes: random_obj.bcs_bytes.clone(),
            type_tag: Some(random_obj.type_tag.clone()),
        })
    }

    /// Update the Clock object's timestamp.
    /// Call this to advance time in the simulation.
    pub fn advance_clock(&mut self, new_timestamp_ms: u64) -> Result<()> {
        let clock_id = AccountAddress::from_hex_literal(CLOCK_OBJECT_ID)
            .map_err(|e| anyhow!("Invalid clock ID: {}", e))?;

        if let Some(clock_obj) = self.objects.get_mut(&clock_id) {
            // Update timestamp_ms in the BCS bytes (bytes 32-40)
            if clock_obj.bcs_bytes.len() >= 40 {
                clock_obj.bcs_bytes[32..40].copy_from_slice(&new_timestamp_ms.to_le_bytes());
                clock_obj.version += 1;
            }
        } else {
            // Re-initialize if somehow missing
            self.timestamp_ms = Some(new_timestamp_ms);
            self.initialize_clock()?;
        }

        Ok(())
    }

    /// Get the current Clock timestamp in milliseconds.
    pub fn get_clock_timestamp_ms(&self) -> u64 {
        let clock_id = AccountAddress::from_hex_literal(CLOCK_OBJECT_ID).ok();
        if let Some(id) = clock_id {
            if let Some(clock_obj) = self.objects.get(&id) {
                if clock_obj.bcs_bytes.len() >= 40 {
                    return u64::from_le_bytes(clock_obj.bcs_bytes[32..40].try_into().unwrap());
                }
            }
        }
        self.timestamp_ms.unwrap_or(DEFAULT_CLOCK_BASE_MS)
    }

    /// Get the Clock object for PTB execution.
    /// Returns it as a shared object input.
    pub fn get_clock_object(&self) -> Result<crate::benchmark::ptb::ObjectInput> {
        let clock_id = AccountAddress::from_hex_literal(CLOCK_OBJECT_ID)
            .map_err(|e| anyhow!("Invalid clock ID: {}", e))?;

        let clock_obj = self
            .objects
            .get(&clock_id)
            .ok_or_else(|| anyhow!("Clock object not found - this should not happen"))?;

        Ok(crate::benchmark::ptb::ObjectInput::Shared {
            id: clock_id,
            bytes: clock_obj.bcs_bytes.clone(),
            type_tag: Some(clock_obj.type_tag.clone()),
        })
    }

    /// Enable mainnet fetching for on-demand package/object loading.
    pub fn with_mainnet_fetching(mut self) -> Self {
        self.fetcher = Some(Box::new(GrpcFetcher::mainnet()));
        self.fetcher_config = FetcherConfig::mainnet();
        self
    }

    /// Enable mainnet fetching with archive support for historical data.
    pub fn with_mainnet_archive_fetching(mut self) -> Self {
        self.fetcher = Some(Box::new(GrpcFetcher::mainnet_with_archive()));
        self.fetcher_config = FetcherConfig::mainnet_with_archive();
        self
    }

    /// Enable fetching with a specific configuration.
    pub fn with_fetcher_config(mut self, config: FetcherConfig) -> Self {
        self.fetcher_config = config.clone();
        if let Some(fetcher) = Self::create_fetcher_from_config(&config) {
            self.fetcher = Some(fetcher);
        }
        self
    }

    /// Enable fetching with a custom fetcher implementation.
    ///
    /// This allows using custom data sources (cached files, mocks, etc.)
    /// instead of the default gRPC-based fetcher.
    pub fn with_fetcher(mut self, fetcher: Box<dyn Fetcher>, config: FetcherConfig) -> Self {
        self.fetcher = Some(fetcher);
        self.fetcher_config = config;
        self
    }

    /// Create a boxed Fetcher from a FetcherConfig.
    fn create_fetcher_from_config(config: &FetcherConfig) -> Option<Box<dyn Fetcher>> {
        GrpcFetcher::from_config(config).map(|f| Box::new(f) as Box<dyn Fetcher>)
    }

    /// Get the current fetcher configuration.
    pub fn fetcher_config(&self) -> &FetcherConfig {
        &self.fetcher_config
    }

    /// Check if mainnet fetching is enabled.
    pub fn is_fetching_enabled(&self) -> bool {
        self.fetcher.is_some() && self.fetcher_config.enabled
    }

    /// Get the network name of the current fetcher (for logging/debugging).
    pub fn fetcher_network(&self) -> &str {
        self.fetcher
            .as_ref()
            .map(|f| f.network_name())
            .unwrap_or("none")
    }

    /// Reset the environment state while preserving the loaded modules.
    ///
    /// This clears all objects, dynamic fields, and transaction state,
    /// but keeps the resolver with all loaded packages. Useful for
    /// running multiple benchmark iterations against the same package corpus.
    pub fn reset_state(&mut self) -> Result<()> {
        self.objects.clear();
        self.id_counter = 0;
        self.dynamic_fields.clear();
        self.shared_locks.clear();
        self.tx_counter = 0;
        self.pending_receives.clear();
        self.lamport_clock = 0;
        self.consensus_history.clear();
        self.consensus_sequence = 0;

        // Re-initialize system objects
        self.initialize_clock()?;
        self.initialize_random()?;

        Ok(())
    }

    /// Get the current transaction sender address.
    pub fn sender(&self) -> AccountAddress {
        self.sender
    }

    /// Set the transaction sender address for TxContext.
    pub fn set_sender(&mut self, sender: AccountAddress) {
        self.sender = sender;
    }

    /// Set a callback for on-demand child object fetching during execution.
    /// This callback is called when a dynamic field child is requested but not found
    /// in the preloaded set. It receives the child object ID and should return
    /// the object's type and BCS bytes if available.
    ///
    /// This is useful for replaying historical transactions where dynamic fields
    /// need to be fetched on-demand from an archive or RPC endpoint.
    pub fn set_child_fetcher(&mut self, fetcher: ChildFetcherFn) {
        self.child_fetcher = Some(std::sync::Arc::new(fetcher));
    }

    /// Clear the child fetcher callback.
    pub fn clear_child_fetcher(&mut self) {
        self.child_fetcher = None;
    }

    /// Set the transaction timestamp for TxContext.
    /// This also updates the Clock object's timestamp.
    pub fn set_timestamp_ms(&mut self, timestamp_ms: u64) {
        self.timestamp_ms = Some(timestamp_ms);
        // Also update the Clock object
        let _ = self.advance_clock(timestamp_ms);
    }

    /// Generate a fresh object ID.
    pub fn fresh_id(&mut self) -> AccountAddress {
        let id = self.id_counter;
        self.id_counter += 1;
        let mut bytes = [0u8; 32];
        // Use a prefix to make generated IDs recognizable
        bytes[0] = 0xAA; // Marker for generated IDs
        bytes[24..32].copy_from_slice(&id.to_le_bytes());
        AccountAddress::new(bytes)
    }

    /// Deploy a package from bytecode modules.
    /// Returns the package address extracted from the module bytecode.
    pub fn deploy_package(&mut self, modules: Vec<(String, Vec<u8>)>) -> Result<AccountAddress> {
        // Add modules to resolver
        let (count, package_addr) = self.resolver.add_package_modules(modules)?;
        if count == 0 {
            return Err(anyhow!("No modules loaded"));
        }

        // Return the package address from bytecode, or generate a fresh ID if not found
        Ok(package_addr.unwrap_or_else(|| self.fresh_id()))
    }

    /// Fetch and deploy a package from mainnet.
    pub fn deploy_package_from_mainnet(&mut self, package_id: &str) -> Result<AccountAddress> {
        let fetcher = self.fetcher.as_ref().ok_or_else(|| {
            anyhow!("Mainnet fetching not enabled. Call with_mainnet_fetching() first.")
        })?;

        let modules = fetcher.fetch_package_modules(package_id)?;
        let (count, _) = self.resolver.add_package_modules(modules)?;

        if count == 0 {
            return Err(anyhow!("No modules loaded from package {}", package_id));
        }

        AccountAddress::from_hex_literal(package_id)
            .map_err(|e| anyhow!("Invalid package address: {}", e))
    }

    /// Deploy a package with pre-fetched modules at a specific address.
    /// This is used by the mainnet import feature when modules are already fetched
    /// via DataFetcher.
    pub fn deploy_package_at_address(
        &mut self,
        package_id: &str,
        modules: Vec<(String, Vec<u8>)>,
    ) -> Result<AccountAddress> {
        let target_addr = AccountAddress::from_hex_literal(package_id)
            .map_err(|e| anyhow!("Invalid package address: {}", e))?;

        let (count, _) = self
            .resolver
            .add_package_modules_at(modules, Some(target_addr))?;

        if count == 0 {
            return Err(anyhow!("No modules loaded for package {}", package_id));
        }

        Ok(target_addr)
    }

    /// Get mutable access to the resolver for advanced operations.
    ///
    /// This is useful for session management where the session needs to load
    /// modules directly into the resolver.
    pub fn resolver_mut(&mut self) -> &mut LocalModuleResolver {
        &mut self.resolver
    }

    /// Load an object from fetched data into the environment.
    ///
    /// This is a convenience method for loading objects from external sources
    /// (e.g., from a Fetcher or cached data).
    pub fn load_object_from_data(
        &mut self,
        object_id: &str,
        bcs_bytes: Vec<u8>,
        type_string: Option<&str>,
        is_shared: bool,
        is_immutable: bool,
        version: u64,
    ) -> Result<AccountAddress> {
        let id = AccountAddress::from_hex_literal(object_id)?;

        let type_tag = if let Some(type_str) = type_string {
            Self::parse_type_string(type_str).ok_or_else(|| {
                anyhow!(
                    "Failed to parse type string '{}' for object {}",
                    type_str,
                    object_id
                )
            })?
        } else {
            TypeTag::Address
        };

        let obj = SimulatedObject {
            id,
            type_tag,
            bcs_bytes,
            is_shared,
            is_immutable,
            version,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Create a new object with the given type and BCS bytes.
    pub fn create_object(
        &mut self,
        type_tag: TypeTag,
        bcs_bytes: Vec<u8>,
        is_shared: bool,
    ) -> AccountAddress {
        let id = self.fresh_id();
        let obj = SimulatedObject {
            id,
            type_tag,
            bcs_bytes,
            is_shared,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(id, obj);
        id
    }

    /// Create a Coin<T> object with the given balance.
    pub fn create_coin(&mut self, coin_type: &str, balance: u64) -> Result<AccountAddress> {
        // Parse the coin type
        let inner_type = crate::benchmark::tx_replay::parse_type_tag(coin_type)?;

        // Coin<T> structure: { id: UID, balance: Balance<T> }
        // UID is 32 bytes, Balance<T> is just a u64
        let id = self.fresh_id();
        let mut bcs_bytes = Vec::new();
        // UID (object ID)
        bcs_bytes.extend_from_slice(id.as_ref());
        // Balance (u64)
        bcs_bytes.extend_from_slice(&balance.to_le_bytes());

        let coin_type_tag = TypeTag::Struct(Box::new(StructTag {
            address: AccountAddress::from_hex_literal("0x2").unwrap(),
            module: Identifier::new("coin").unwrap(),
            name: Identifier::new("Coin").unwrap(),
            type_params: vec![inner_type],
        }));

        let obj = SimulatedObject {
            id,
            type_tag: coin_type_tag,
            bcs_bytes,
            is_shared: false,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Inject a pre-synthesized object into the simulation environment.
    ///
    /// This allows objects created via `ObjectSynthesizer` to be used in transaction execution.
    /// The object_id in the SynthesizedObject is parsed and used as the object's ID.
    pub fn inject_object(
        &mut self,
        type_path: &str,
        object_id: &str,
        bcs_bytes: Vec<u8>,
        is_shared: bool,
    ) -> Result<AccountAddress> {
        // Parse the type path into a TypeTag
        let type_tag = crate::benchmark::tx_replay::parse_type_tag(type_path)?;

        // Parse the object ID
        let id = AccountAddress::from_hex_literal(object_id)
            .with_phase_context(Phase::Synthesis, || {
                format!("parsing object ID '{}'", object_id)
            })?;

        let obj = SimulatedObject {
            id,
            type_tag,
            bcs_bytes,
            is_shared,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Inject a SynthesizedObject (from LlmToolkit) into the simulation.
    /// This is a convenience wrapper around inject_object.
    pub fn inject_synthesized(
        &mut self,
        synthesized: &crate::benchmark::sandbox_exec::SynthesizedObject,
    ) -> Result<AccountAddress> {
        self.inject_object(
            &synthesized.type_path,
            &synthesized.object_id,
            synthesized.bcs_bytes.clone(),
            synthesized.is_shared,
        )
    }

    /// Register a new coin type with its metadata.
    /// This allows the sandbox to track decimal places for custom coins.
    pub fn register_coin(&mut self, coin_type: &str, decimals: u8, symbol: &str, name: &str) {
        self.coin_registry.insert(
            coin_type.to_string(),
            CoinMetadata {
                decimals,
                symbol: symbol.to_string(),
                name: name.to_string(),
                type_tag: coin_type.to_string(),
            },
        );
    }

    /// Get coin metadata for a given coin type.
    pub fn get_coin_metadata(&self, coin_type: &str) -> Option<&CoinMetadata> {
        self.coin_registry.get(coin_type)
    }

    /// Get coin decimals for a given coin type.
    /// Returns SUI_DECIMALS (9) as default if coin is not registered.
    pub fn get_coin_decimals(&self, coin_type: &str) -> u8 {
        self.coin_registry
            .get(coin_type)
            .map(|m| m.decimals)
            .unwrap_or(SUI_DECIMALS)
    }

    /// List all registered coins.
    pub fn list_registered_coins(&self) -> Vec<&CoinMetadata> {
        self.coin_registry.values().collect()
    }

    /// Create a SUI coin with the given balance in MIST.
    pub fn create_sui_coin(&mut self, balance_mist: u64) -> Result<AccountAddress> {
        self.create_coin(SUI_COIN_TYPE, balance_mist)
    }

    /// Get an object by ID.
    pub fn get_object(&self, id: &AccountAddress) -> Option<&SimulatedObject> {
        self.objects.get(id)
    }

    /// Set object bytes directly.
    pub fn set_object_bytes(&mut self, id: AccountAddress, bytes: Vec<u8>) -> Result<()> {
        let obj = self
            .objects
            .get_mut(&id)
            .ok_or_phase_with(Phase::Execution, || {
                format!("object {} not found", id.to_hex_literal())
            })?;
        obj.bcs_bytes = bytes;
        obj.version += 1;
        Ok(())
    }

    /// Load a cached object with its exact ID and BCS bytes.
    /// This is used to replay transactions with the exact same object state.
    pub fn load_cached_object(
        &mut self,
        object_id: &str,
        bcs_bytes: Vec<u8>,
        is_shared: bool,
    ) -> Result<AccountAddress> {
        let id = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow!("Invalid object ID '{}': {}", object_id, e))?;

        // The BCS bytes from the RPC include metadata prefix, but we want just the Move struct content
        // For now, store as-is and let execution handle it
        let obj = SimulatedObject {
            id,
            type_tag: TypeTag::Address, // Placeholder - we don't have type info in the cache
            bcs_bytes,
            is_shared,
            is_immutable: false, // Could be detected from object flags if needed
            version: 1,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Load a cached object with its exact ID, BCS bytes, and optional type information.
    /// This is used to replay transactions when type information is available.
    pub fn load_cached_object_with_type(
        &mut self,
        object_id: &str,
        bcs_bytes: Vec<u8>,
        type_str: Option<&str>,
        is_shared: bool,
    ) -> Result<AccountAddress> {
        let id = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow!("Invalid object ID '{}': {}", object_id, e))?;

        // Parse type string if provided, otherwise use placeholder
        let type_tag = if let Some(ts) = type_str {
            Self::parse_type_string(ts).ok_or_else(|| {
                anyhow!("Failed to parse type string '{}': invalid format (expected ADDRESS::MODULE::NAME or primitive type)", ts)
            })?
        } else {
            TypeTag::Address
        };

        let obj = SimulatedObject {
            id,
            type_tag,
            bcs_bytes,
            is_shared,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Load multiple cached objects from a map of object_id -> bcs_bytes_base64.
    pub fn load_cached_objects(
        &mut self,
        objects: &std::collections::HashMap<String, String>,
    ) -> Result<usize> {
        use base64::Engine;
        let mut loaded = 0;
        for (object_id, b64_bytes) in objects {
            if let Ok(bcs_bytes) = base64::engine::general_purpose::STANDARD.decode(b64_bytes) {
                // Assume shared if object ID appears in shared objects commonly
                // For now default to non-shared, self-healing can fix if needed
                if self.load_cached_object(object_id, bcs_bytes, false).is_ok() {
                    loaded += 1;
                }
            }
        }
        Ok(loaded)
    }

    /// Fetch an object from mainnet and add it to the environment.
    /// Uses the full fetch to get type, ownership, and version information.
    pub fn fetch_object_from_mainnet(&mut self, object_id: &str) -> Result<AccountAddress> {
        let fetcher = self
            .fetcher
            .as_ref()
            .ok_or_else(|| anyhow!("Mainnet fetching not enabled"))?;

        let fetched = fetcher.fetch_object(object_id)?;
        self.load_fetched_object(object_id, fetched)
    }

    /// Load a fetched object into the simulation environment.
    ///
    /// This is a helper method that converts FetchedObjectData to SimulatedObject
    /// and inserts it into the object store.
    fn load_fetched_object(
        &mut self,
        object_id: &str,
        fetched: FetchedObjectData,
    ) -> Result<AccountAddress> {
        let id = AccountAddress::from_hex_literal(object_id)?;

        // Parse type string into TypeTag if available
        let type_tag = if let Some(type_str) = &fetched.type_string {
            Self::parse_type_string(type_str).ok_or_else(|| {
                anyhow!("Failed to parse type string '{}' for object {}: invalid format (expected ADDRESS::MODULE::NAME or primitive type)", type_str, object_id)
            })?
        } else {
            TypeTag::Address // Fallback placeholder when type is unknown
        };

        let obj = SimulatedObject {
            id,
            type_tag,
            bcs_bytes: fetched.bcs_bytes,
            is_shared: fetched.is_shared,
            is_immutable: fetched.is_immutable,
            version: fetched.version,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Parse a type string like "0x2::coin::Coin<0x2::sui::SUI>" into a TypeTag.
    ///
    /// Delegates to the canonical implementation in the types module.
    /// Uses caching for improved performance on repeated calls.
    ///
    /// Supports:
    /// - Simple types: "0x2::sui::SUI"
    /// - Single generics: "0x2::coin::Coin<0x2::sui::SUI>"
    /// - Multiple generics: "0xabc::pool::Pool<0x2::usdc::USDC, 0x2::sui::SUI>"
    /// - Nested generics: "0x2::option::Option<0x2::coin::Coin<0x2::sui::SUI>>"
    /// - Primitive types: "u8", "u64", "bool", "address", "vector<u8>"
    pub fn parse_type_string(type_str: &str) -> Option<TypeTag> {
        crate::benchmark::types::parse_type_string(type_str)
    }

    /// Format a TypeTag back to a string (for debugging/display).
    /// Delegates to the canonical implementation in types module.
    pub fn format_type_tag(type_tag: &TypeTag) -> String {
        crate::benchmark::types::format_type_tag(type_tag)
    }

    /// List all available packages.
    pub fn list_packages(&self) -> Vec<AccountAddress> {
        self.resolver.list_packages()
    }

    /// List all objects in the environment.
    pub fn list_objects(&self) -> Vec<&SimulatedObject> {
        self.objects.values().collect()
    }

    /// Pre-publish modules and return (package_id, upgrade_cap_id).
    /// This adds the modules to the resolver before PTB execution.
    fn pre_publish_modules(
        &mut self,
        modules: &[Vec<u8>],
    ) -> Result<(AccountAddress, AccountAddress)> {
        if modules.is_empty() {
            return Err(anyhow!("Publish requires at least one module"));
        }

        // Parse all modules and collect package address
        let mut package_addr: Option<AccountAddress> = None;
        let mut module_names = Vec::new();

        for module_bytes in modules {
            let module =
                move_binary_format::CompiledModule::deserialize_with_defaults(module_bytes)
                    .map_err(|e| anyhow!("Failed to deserialize module: {:?}", e))?;

            let module_id = module.self_id();
            module_names.push(module_id.name().to_string());

            if let Some(addr) = package_addr {
                if module_id.address() != &addr {
                    return Err(anyhow!(
                        "All modules must have same address. Expected {}, got {}",
                        addr.to_hex_literal(),
                        module_id.address().to_hex_literal()
                    ));
                }
            } else {
                package_addr = Some(*module_id.address());
            }
        }

        let package_addr = package_addr
            .ok_or_else(|| anyhow!("No modules provided - cannot determine package address"))?;

        // Add modules to resolver
        let modules_with_names: Vec<(String, Vec<u8>)> = module_names
            .iter()
            .zip(modules.iter())
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect();

        self.resolver.add_package_modules(modules_with_names)?;

        // Create UpgradeCap
        let upgrade_cap_id = self.fresh_id();

        // Store UpgradeCap as an object
        let mut upgrade_cap_bytes = Vec::new();
        upgrade_cap_bytes.extend_from_slice(upgrade_cap_id.as_ref()); // UID
        upgrade_cap_bytes.extend_from_slice(package_addr.as_ref()); // package ID
        upgrade_cap_bytes.extend_from_slice(&1u64.to_le_bytes()); // version
        upgrade_cap_bytes.push(0u8); // policy

        let upgrade_cap = SimulatedObject {
            id: upgrade_cap_id,
            type_tag: TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
                address: AccountAddress::from_hex_literal("0x2").unwrap(),
                module: move_core_types::identifier::Identifier::new("package").unwrap(),
                name: move_core_types::identifier::Identifier::new("UpgradeCap").unwrap(),
                type_params: vec![],
            })),
            bcs_bytes: upgrade_cap_bytes,
            is_shared: false,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(upgrade_cap_id, upgrade_cap);

        Ok((package_addr, upgrade_cap_id))
    }

    /// Pre-upgrade modules for an existing package.
    /// This replaces modules in the resolver and returns the (new_package_id, upgrade_receipt_id).
    fn pre_upgrade_modules(
        &mut self,
        modules: &[Vec<u8>],
        original_package: AccountAddress,
    ) -> Result<(AccountAddress, AccountAddress)> {
        if modules.is_empty() {
            return Err(anyhow!("Upgrade requires at least one module"));
        }

        // Parse all modules and verify they upgrade the correct package
        let mut module_names = Vec::new();

        for module_bytes in modules {
            let module =
                move_binary_format::CompiledModule::deserialize_with_defaults(module_bytes)
                    .map_err(|e| anyhow!("Failed to deserialize module: {:?}", e))?;

            let module_id = module.self_id();
            module_names.push(module_id.name().to_string());

            // Note: In a real upgrade, modules would be compiled against the original package
            // but with a new address. For simulation, we allow the address to be different.
        }

        // Generate a new package address for the upgraded version
        // In real Sui, this is deterministic based on the original package + digest
        let new_package_addr = self.fresh_id();

        // Add modules to resolver with the new package address
        // We need to rewrite the module addresses
        let mut modules_with_names: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, module_bytes) in module_names.iter().zip(modules.iter()) {
            // For simplicity, we'll add modules as-is and track the mapping
            // A full implementation would rewrite addresses
            modules_with_names.push((name.clone(), module_bytes.clone()));
        }

        self.resolver.add_package_modules(modules_with_names)?;

        // Create UpgradeReceipt
        let receipt_id = self.fresh_id();

        // Store UpgradeReceipt as an object
        // UpgradeReceipt { cap: ID, package: ID }
        let mut receipt_bytes = Vec::new();
        receipt_bytes.extend_from_slice(receipt_id.as_ref()); // UID
        receipt_bytes.extend_from_slice(original_package.as_ref()); // cap ID (placeholder)
        receipt_bytes.extend_from_slice(new_package_addr.as_ref()); // new package ID

        let receipt = SimulatedObject {
            id: receipt_id,
            type_tag: TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
                address: AccountAddress::from_hex_literal("0x2").unwrap(),
                module: move_core_types::identifier::Identifier::new("package").unwrap(),
                name: move_core_types::identifier::Identifier::new("UpgradeReceipt").unwrap(),
                type_params: vec![],
            })),
            bcs_bytes: receipt_bytes,
            is_shared: false,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(receipt_id, receipt);

        Ok((new_package_addr, receipt_id))
    }

    /// Execute a PTB with the given inputs and commands.
    pub fn execute_ptb(
        &mut self,
        inputs: Vec<InputValue>,
        commands: Vec<Command>,
    ) -> ExecutionResult {
        use crate::benchmark::ptb::ObjectInput;

        // Extract shared objects from inputs and acquire locks
        let shared_objects: Vec<(AccountAddress, bool)> = inputs
            .iter()
            .filter_map(|input| {
                match input {
                    InputValue::Object(ObjectInput::Shared { id, .. }) => {
                        // Shared objects are accessed mutably by default in PTBs
                        Some((*id, true))
                    }
                    _ => None,
                }
            })
            .collect();

        // Acquire locks if there are shared objects
        let acquired_locks = if !shared_objects.is_empty() {
            self.tx_counter += 1;
            let tx_id = format!("ptb_{}", self.tx_counter);

            match self.acquire_shared_locks(shared_objects.clone(), Some(tx_id.clone())) {
                LockResult::Success { locks } => Some(locks),
                LockResult::Conflict {
                    object_id,
                    existing_lock,
                    reason,
                } => {
                    let raw_error = format!(
                        "Shared object lock conflict on {}: {}",
                        object_id.to_hex_literal(),
                        &reason
                    );
                    return ExecutionResult {
                        success: false,
                        effects: None,
                        error: Some(SimulationError::SharedObjectLockConflict {
                            object_id,
                            held_by: existing_lock.transaction_id,
                            reason,
                        }),
                        raw_error: Some(raw_error),
                        failed_command_index: None,
                        failed_command_description: Some("Shared object locking".to_string()),
                        commands_succeeded: 0,
                    };
                }
            }
        } else {
            None
        };

        // Execute the PTB (locks will be released after execution via RAII or explicit release)
        let result = self.execute_ptb_inner(inputs, commands, None);

        // Release locks if we acquired any
        if let Some(locks) = acquired_locks {
            self.release_shared_locks(&locks);
        }

        result
    }

    /// Execute a PTB with a gas budget limit.
    ///
    /// If gas usage exceeds the budget during execution, the PTB will fail
    /// with an out-of-gas error at the command that exceeded the limit.
    ///
    /// ## Parameters
    /// - `inputs`: Input values for the PTB
    /// - `commands`: Commands to execute
    /// - `gas_budget`: Maximum gas units allowed. Use `None` for unlimited gas.
    ///
    /// ## Example
    /// ```ignore
    /// let result = env.execute_ptb_with_gas_budget(
    ///     inputs,
    ///     commands,
    ///     Some(10_000_000), // 10M gas units
    /// );
    /// if !result.success {
    ///     if let Some(err) = &result.raw_error {
    ///         if err.contains("out of gas") {
    ///             println!("Transaction exceeded gas budget!");
    ///         }
    ///     }
    /// }
    /// ```
    pub fn execute_ptb_with_gas_budget(
        &mut self,
        inputs: Vec<InputValue>,
        commands: Vec<Command>,
        gas_budget: Option<u64>,
    ) -> ExecutionResult {
        use crate::benchmark::ptb::ObjectInput;

        // Extract shared objects from inputs and acquire locks
        let shared_objects: Vec<(AccountAddress, bool)> = inputs
            .iter()
            .filter_map(|input| match input {
                InputValue::Object(ObjectInput::Shared { id, .. }) => Some((*id, true)),
                _ => None,
            })
            .collect();

        // Acquire locks if there are shared objects
        let acquired_locks = if !shared_objects.is_empty() {
            self.tx_counter += 1;
            let tx_id = format!("ptb_{}", self.tx_counter);

            match self.acquire_shared_locks(shared_objects.clone(), Some(tx_id.clone())) {
                LockResult::Success { locks } => Some(locks),
                LockResult::Conflict {
                    object_id,
                    existing_lock,
                    reason,
                } => {
                    let raw_error = format!(
                        "Shared object lock conflict on {}: {}",
                        object_id.to_hex_literal(),
                        &reason
                    );
                    return ExecutionResult {
                        success: false,
                        effects: None,
                        error: Some(SimulationError::SharedObjectLockConflict {
                            object_id,
                            held_by: existing_lock.transaction_id,
                            reason,
                        }),
                        raw_error: Some(raw_error),
                        failed_command_index: None,
                        failed_command_description: Some("Shared object locking".to_string()),
                        commands_succeeded: 0,
                    };
                }
            }
        } else {
            None
        };

        // Execute the PTB with gas budget
        let result = self.execute_ptb_inner(inputs, commands, gas_budget);

        // Release locks if we acquired any
        if let Some(locks) = acquired_locks {
            self.release_shared_locks(&locks);
        }

        result
    }

    /// Inner PTB execution without lock handling.
    fn execute_ptb_inner(
        &mut self,
        inputs: Vec<InputValue>,
        commands: Vec<Command>,
        gas_budget: Option<u64>,
    ) -> ExecutionResult {
        // Pre-process: Handle Publish and Upgrade commands by adding modules to resolver first
        // This allows the published/upgraded modules to be available for subsequent MoveCall commands
        let mut published_packages: Vec<(AccountAddress, AccountAddress)> = Vec::new(); // (package_id, upgrade_cap_id)
        let mut upgraded_packages: Vec<(AccountAddress, AccountAddress)> = Vec::new(); // (new_package_id, receipt_id)

        for cmd in &commands {
            match cmd {
                Command::Publish { modules, .. } => match self.pre_publish_modules(modules) {
                    Ok((pkg_id, cap_id)) => {
                        published_packages.push((pkg_id, cap_id));
                    }
                    Err(e) => {
                        return ExecutionResult {
                            success: false,
                            effects: None,
                            error: Some(SimulationError::ExecutionError {
                                message: format!("Failed to publish modules: {}", e),
                            }),
                            raw_error: Some(e.to_string()),
                            failed_command_index: None,
                            failed_command_description: Some(
                                "Publish (pre-processing)".to_string(),
                            ),
                            commands_succeeded: 0,
                        };
                    }
                },
                Command::Upgrade {
                    modules, package, ..
                } => match self.pre_upgrade_modules(modules, *package) {
                    Ok((new_pkg_id, receipt_id)) => {
                        upgraded_packages.push((new_pkg_id, receipt_id));
                    }
                    Err(e) => {
                        return ExecutionResult {
                            success: false,
                            effects: None,
                            error: Some(SimulationError::ExecutionError {
                                message: format!("Failed to upgrade package: {}", e),
                            }),
                            raw_error: Some(e.to_string()),
                            failed_command_index: None,
                            failed_command_description: Some(
                                "Upgrade (pre-processing)".to_string(),
                            ),
                            commands_succeeded: 0,
                        };
                    }
                },
                _ => {}
            }
        }

        // Build VM config with correct sender and timestamp
        let mut config = crate::benchmark::vm::SimulationConfig {
            sender_address: self.sender.into(),
            tx_timestamp_ms: self.timestamp_ms,
            ..Default::default()
        };
        // Use the timestamp for clock as well
        if let Some(ts) = self.timestamp_ms {
            config.clock_base_ms = ts;
        }

        // Create VM harness with custom config
        let mut harness = match VMHarness::with_config(&self.resolver, false, config) {
            Ok(h) => h,
            Err(e) => {
                return ExecutionResult {
                    success: false,
                    effects: None,
                    error: Some(SimulationError::ExecutionError {
                        message: format!("Failed to create VM: {}", e),
                    }),
                    raw_error: Some(e.to_string()),
                    failed_command_index: None,
                    failed_command_description: Some("VM initialization".to_string()),
                    commands_succeeded: 0,
                };
            }
        };

        // Set up on-demand child fetcher if configured
        if let Some(ref fetcher_arc) = self.child_fetcher {
            let fetcher_clone = fetcher_arc.clone();
            harness.set_child_fetcher(Box::new(move |child_id| fetcher_clone(child_id)));
        }

        // Preload dynamic field state from the environment.
        // This makes existing Table/Bag entries available to MoveCall functions.
        let preload_fields: Vec<_> = self
            .dynamic_fields
            .iter()
            .map(|((p, c), (t, b))| ((*p, *c), t.clone(), b.clone()))
            .collect();
        harness.preload_dynamic_fields(preload_fields);

        // Preload pending receives for transfer::receive in Move code.
        // This makes objects sent to other objects available for receiving.
        let preload_receives: Vec<_> = self
            .pending_receives
            .iter()
            .map(|((r, s), (b, t))| ((*r, *s), t.clone(), b.clone()))
            .collect();
        harness.preload_pending_receives(preload_receives);

        // Create PTB executor with pre-published and pre-upgraded package info
        // and the current sender address for ownership validation
        let mut executor = crate::benchmark::ptb::PTBExecutor::new_with_packages_and_sender(
            &mut harness,
            published_packages,
            upgraded_packages,
            self.sender,
        );

        // Set gas budget if specified
        executor.set_gas_budget(gas_budget);

        // Add pending receives for the PTB Receive command with type info
        // (this is separate from Move-level transfer::receive)
        for ((_recipient_id, sent_id), (bytes, type_tag)) in &self.pending_receives {
            executor.add_pending_receive_with_type(*sent_id, bytes.clone(), type_tag.clone());
        }

        // Add inputs
        for input in inputs {
            executor.add_input(input);
        }

        // Execute commands
        match executor.execute_commands(&commands) {
            Ok(effects) => {
                // Apply object changes to our store
                self.apply_object_changes(&effects);

                // Capture events from this execution
                self.last_tx_events = effects.events.clone();
                self.all_events.extend(effects.events.clone());

                ExecutionResult {
                    success: effects.success,
                    effects: Some(effects.clone()),
                    error: if effects.success {
                        None
                    } else {
                        effects.error.as_ref().map(|e| self.parse_error(e))
                    },
                    raw_error: effects.error.clone(),
                    failed_command_index: effects.failed_command_index,
                    failed_command_description: effects.failed_command_description.clone(),
                    commands_succeeded: effects.commands_succeeded,
                }
            }
            Err(e) => {
                // Clear last tx events on error
                self.last_tx_events.clear();

                let error = self.parse_error(&e.to_string());
                ExecutionResult {
                    success: false,
                    effects: None,
                    error: Some(error),
                    raw_error: Some(e.to_string()),
                    failed_command_index: None,
                    failed_command_description: None,
                    commands_succeeded: 0,
                }
            }
        }
    }

    /// Apply object changes from transaction effects to the object store.
    /// This syncs both created and mutated object bytes back to the environment.
    fn apply_object_changes(&mut self, effects: &crate::benchmark::ptb::TransactionEffects) {
        use crate::benchmark::ptb::{ObjectChange, Owner};

        for change in &effects.object_changes {
            match change {
                ObjectChange::Created {
                    id,
                    owner,
                    object_type,
                } => {
                    // Get the bytes from the effects if available
                    let bcs_bytes = effects
                        .created_object_bytes
                        .get(id)
                        .cloned()
                        .unwrap_or_default();

                    let (is_shared, is_immutable) = match owner {
                        Owner::Shared => (true, false),
                        Owner::Immutable => (false, true),
                        Owner::Address(_) => (false, false),
                    };

                    // Create or update the object
                    if let Some(existing) = self.objects.get_mut(id) {
                        // Update existing object with new bytes and type
                        if !bcs_bytes.is_empty() {
                            existing.bcs_bytes = bcs_bytes;
                        }
                        if let Some(t) = object_type {
                            existing.type_tag = t.clone();
                        }
                        existing.is_shared = is_shared;
                        existing.is_immutable = is_immutable;
                    } else {
                        // Create new object
                        let obj = SimulatedObject {
                            id: *id,
                            type_tag: object_type.clone().unwrap_or(TypeTag::Address),
                            bcs_bytes,
                            is_shared,
                            is_immutable,
                            version: 1,
                        };
                        self.objects.insert(*id, obj);
                    }
                }
                ObjectChange::Mutated {
                    id,
                    owner,
                    object_type,
                } => {
                    // Get the mutated bytes from the effects
                    let mutated_bytes = effects.mutated_object_bytes.get(id);

                    // Update the object if it exists
                    if let Some(obj) = self.objects.get_mut(id) {
                        // Update ownership
                        match owner {
                            Owner::Shared => {
                                obj.is_shared = true;
                                obj.is_immutable = false;
                            }
                            Owner::Immutable => {
                                obj.is_shared = false;
                                obj.is_immutable = true;
                            }
                            Owner::Address(_) => {
                                // Keep current shared/immutable status for address ownership
                            }
                        }
                        obj.version += 1;

                        // Update type if we have it
                        if let Some(t) = object_type {
                            obj.type_tag = t.clone();
                        }

                        // CRITICAL: Sync the mutated bytes back to the object store
                        // This is what enables multi-step PTB flows to see correct state
                        if let Some(new_bytes) = mutated_bytes {
                            if !new_bytes.is_empty() {
                                obj.bcs_bytes = new_bytes.clone();
                            }
                        }
                    }
                }
                ObjectChange::Deleted { id, .. } => {
                    self.objects.remove(id);
                }
                ObjectChange::Wrapped { id, .. } => {
                    // Wrapped objects are removed from top-level store
                    // (they exist inside another object)
                    self.objects.remove(id);
                }
                ObjectChange::Unwrapped {
                    id,
                    owner,
                    object_type,
                } => {
                    // Get bytes from created_object_bytes (unwrapped objects are tracked there)
                    let bcs_bytes = effects
                        .created_object_bytes
                        .get(id)
                        .cloned()
                        .unwrap_or_default();

                    let (is_shared, is_immutable) = match owner {
                        Owner::Shared => (true, false),
                        Owner::Immutable => (false, true),
                        Owner::Address(_) => (false, false),
                    };

                    if let Some(existing) = self.objects.get_mut(id) {
                        // Update existing
                        if !bcs_bytes.is_empty() {
                            existing.bcs_bytes = bcs_bytes;
                        }
                        if let Some(t) = object_type {
                            existing.type_tag = t.clone();
                        }
                    } else {
                        // Create new
                        let obj = SimulatedObject {
                            id: *id,
                            type_tag: object_type.clone().unwrap_or(TypeTag::Address),
                            bcs_bytes,
                            is_shared,
                            is_immutable,
                            version: 1,
                        };
                        self.objects.insert(*id, obj);
                    }
                }
                ObjectChange::Transferred {
                    id,
                    recipient,
                    object_type,
                    object_bytes,
                } => {
                    // Remove the object from top-level objects (it's now owned by recipient)
                    self.objects.remove(id);

                    // Add to pending_receives so it can be received in the next PTB
                    // The recipient address is the "receiving object" (or address)
                    let type_tag = object_type.clone().unwrap_or(TypeTag::Address);
                    self.pending_receives
                        .insert((*recipient, *id), (object_bytes.clone(), type_tag));
                }
            }
        }

        // Sync dynamic field entries from the PTB execution
        for ((parent_id, child_id), (type_tag, bytes)) in &effects.dynamic_field_entries {
            self.dynamic_fields
                .insert((*parent_id, *child_id), (type_tag.clone(), bytes.clone()));
        }

        // Clear received objects from pending_receives
        // When a Receive command successfully receives an object, it should be
        // removed from pending_receives so it can't be received again.
        for received_id in &effects.received {
            // Find and remove the entry with this sent_id
            self.pending_receives
                .retain(|(_, sent_id), _| sent_id != received_id);
        }
    }

    /// Get a dynamic field entry by parent and child ID.
    /// Used for looking up Table/Bag entries.
    pub fn get_dynamic_field(
        &self,
        parent_id: AccountAddress,
        child_id: AccountAddress,
    ) -> Option<&(TypeTag, Vec<u8>)> {
        self.dynamic_fields.get(&(parent_id, child_id))
    }

    /// Get all dynamic fields for a parent object.
    /// Returns an iterator over (child_id, type_tag, bytes) tuples.
    pub fn get_dynamic_fields_for_parent(
        &self,
        parent_id: AccountAddress,
    ) -> impl Iterator<Item = (AccountAddress, &TypeTag, &Vec<u8>)> {
        self.dynamic_fields
            .iter()
            .filter(move |((p, _), _)| *p == parent_id)
            .map(|((_, c), (t, b))| (*c, t, b))
    }

    /// Set a dynamic field entry directly.
    /// Used for pre-populating state from mainnet or tests.
    pub fn set_dynamic_field(
        &mut self,
        parent_id: AccountAddress,
        child_id: AccountAddress,
        type_tag: TypeTag,
        bytes: Vec<u8>,
    ) {
        self.dynamic_fields
            .insert((parent_id, child_id), (type_tag, bytes));
    }

    /// Remove a dynamic field entry.
    pub fn remove_dynamic_field(
        &mut self,
        parent_id: AccountAddress,
        child_id: AccountAddress,
    ) -> Option<(TypeTag, Vec<u8>)> {
        self.dynamic_fields.remove(&(parent_id, child_id))
    }

    // ============================================================
    // Send-to-Object Pattern (transfer::receive)
    // ============================================================

    /// Send an object to another object (send-to-object pattern).
    ///
    /// This simulates `transfer::public_transfer(obj, object_id)` where the recipient
    /// is an object ID rather than an address. The object becomes "pending" and can
    /// be received in a subsequent transaction using the Receive command.
    ///
    /// ## Parameters
    /// - `recipient_object_id`: The object that will receive the transferred object
    /// - `sent_object_id`: The object being sent
    /// - `object_bytes`: BCS-serialized bytes of the sent object
    /// - `object_type`: Type tag of the sent object
    ///
    /// ## Example
    /// ```ignore
    /// // Transaction 1: Send a coin to an escrow object
    /// env.send_to_object(escrow_id, coin_id, coin_bytes, coin_type);
    ///
    /// // Transaction 2: Escrow receives the coin
    /// let result = env.execute_ptb(
    ///     vec![escrow_input],
    ///     vec![Command::Receive { object_id: coin_id, object_type: Some(coin_type) }],
    /// );
    /// ```
    pub fn send_to_object(
        &mut self,
        recipient_object_id: AccountAddress,
        sent_object_id: AccountAddress,
        object_bytes: Vec<u8>,
        object_type: TypeTag,
    ) {
        self.pending_receives.insert(
            (recipient_object_id, sent_object_id),
            (object_bytes, object_type),
        );
        // Remove from top-level objects since it's now pending
        self.objects.remove(&sent_object_id);
    }

    /// Get all pending receives for an object.
    /// Returns (sent_object_id, bytes, type_tag) for each pending receive.
    pub fn get_pending_receives(
        &self,
        recipient_object_id: AccountAddress,
    ) -> Vec<(AccountAddress, &Vec<u8>, &TypeTag)> {
        self.pending_receives
            .iter()
            .filter(|((recipient, _), _)| *recipient == recipient_object_id)
            .map(|((_, sent_id), (bytes, type_tag))| (*sent_id, bytes, type_tag))
            .collect()
    }

    /// Check if an object has pending receives.
    pub fn has_pending_receives(&self, recipient_object_id: AccountAddress) -> bool {
        self.pending_receives
            .keys()
            .any(|(recipient, _)| *recipient == recipient_object_id)
    }

    /// Clear a specific pending receive (called after successful Receive command).
    pub fn clear_pending_receive(
        &mut self,
        recipient_object_id: AccountAddress,
        sent_object_id: AccountAddress,
    ) -> Option<(Vec<u8>, TypeTag)> {
        self.pending_receives
            .remove(&(recipient_object_id, sent_object_id))
    }

    // ============================================================
    // Shared Object Locking
    // ============================================================

    /// Attempt to acquire locks on shared objects for a transaction.
    ///
    /// This simulates Sui's shared object consensus locking mechanism.
    /// In the real Sui network, shared objects require consensus to determine
    /// access order. This simulation provides:
    ///
    /// 1. **Version tracking**: Each lock records the object version
    /// 2. **Conflict detection**: Detects when two transactions try to
    ///    mutably access the same shared object
    /// 3. **Read-only access**: Multiple transactions can read-only access
    ///    a shared object concurrently
    ///
    /// ## Parameters
    ///
    /// - `shared_objects`: List of (object_id, is_mutable) pairs
    /// - `transaction_id`: Optional identifier for the transaction (for diagnostics)
    ///
    /// ## Returns
    ///
    /// - `LockResult::Success` with acquired locks if all objects can be locked
    /// - `LockResult::Conflict` if there's a locking conflict
    ///
    /// ## Example
    ///
    /// ```ignore
    /// // Try to lock a shared object for mutable access
    /// let result = env.acquire_shared_locks(
    ///     vec![(object_id, true)], // mutable access
    ///     Some("tx_1".to_string()),
    /// );
    ///
    /// match result {
    ///     LockResult::Success { locks } => {
    ///         // Execute the transaction
    ///         let effects = env.execute_ptb(inputs, commands);
    ///         // Release locks after execution
    ///         env.release_shared_locks(&locks);
    ///     }
    ///     LockResult::Conflict { object_id, reason, .. } => {
    ///         // Handle conflict (e.g., retry with backoff)
    ///         println!("Lock conflict on {}: {}", object_id, reason);
    ///     }
    /// }
    /// ```
    pub fn acquire_shared_locks(
        &mut self,
        shared_objects: Vec<(AccountAddress, bool)>,
        transaction_id: Option<String>,
    ) -> LockResult {
        // Generate a unique transaction ID if not provided
        let tx_id = transaction_id.unwrap_or_else(|| {
            self.tx_counter += 1;
            format!("tx_{}", self.tx_counter)
        });

        // Check for conflicts first (before acquiring any locks)
        for (object_id, is_mutable) in &shared_objects {
            if let Some(existing_lock) = self.shared_locks.get(object_id) {
                // Conflict if:
                // - Existing lock is mutable (exclusive), OR
                // - New request is mutable (exclusive)
                if existing_lock.is_mutable || *is_mutable {
                    let reason = if existing_lock.is_mutable && *is_mutable {
                        "Both transactions require mutable access".to_string()
                    } else if existing_lock.is_mutable {
                        "Existing transaction holds mutable lock".to_string()
                    } else {
                        "New transaction requires mutable access but object has read lock"
                            .to_string()
                    };

                    return LockResult::Conflict {
                        object_id: *object_id,
                        existing_lock: existing_lock.clone(),
                        reason,
                    };
                }
                // If both are read-only, no conflict - allow multiple readers
            }
        }

        // No conflicts - acquire all locks
        // Advance lamport clock for this transaction
        self.lamport_clock += 1;
        let tx_lamport = self.lamport_clock;

        let mut acquired_locks = Vec::new();
        for (object_id, is_mutable) in shared_objects {
            // Use lamport clock for version if object doesn't exist yet
            let version = self
                .objects
                .get(&object_id)
                .map(|o| o.version.max(tx_lamport))
                .unwrap_or(tx_lamport);

            let lock = SharedObjectLock {
                object_id,
                version,
                is_mutable,
                transaction_id: Some(tx_id.clone()),
            };

            self.shared_locks.insert(object_id, lock.clone());
            acquired_locks.push(lock);
        }

        LockResult::Success {
            locks: acquired_locks,
        }
    }

    /// Release shared object locks after transaction completion.
    ///
    /// Call this after executing a transaction to release the locks.
    /// This allows other transactions to access the shared objects.
    pub fn release_shared_locks(&mut self, locks: &[SharedObjectLock]) {
        for lock in locks {
            // Only release if we still hold the lock
            if let Some(current) = self.shared_locks.get(&lock.object_id) {
                if current.transaction_id == lock.transaction_id {
                    self.shared_locks.remove(&lock.object_id);
                }
            }
        }
    }

    /// Release all locks for a specific transaction.
    pub fn release_locks_for_transaction(&mut self, transaction_id: &str) {
        let to_remove: Vec<_> = self
            .shared_locks
            .iter()
            .filter(|(_, lock)| lock.transaction_id.as_deref() == Some(transaction_id))
            .map(|(id, _)| *id)
            .collect();

        for id in to_remove {
            self.shared_locks.remove(&id);
        }
    }

    /// Clear all shared object locks.
    /// Useful for test isolation or resetting the simulation state.
    pub fn clear_shared_locks(&mut self) {
        self.shared_locks.clear();
    }

    /// Get all current shared object locks.
    pub fn get_shared_locks(&self) -> Vec<SharedObjectLock> {
        self.shared_locks.values().cloned().collect()
    }

    /// Check if a shared object is currently locked.
    pub fn is_shared_object_locked(&self, object_id: AccountAddress) -> bool {
        self.shared_locks.contains_key(&object_id)
    }

    /// Check if a shared object has a mutable lock.
    pub fn has_mutable_lock(&self, object_id: AccountAddress) -> bool {
        self.shared_locks
            .get(&object_id)
            .map(|lock| lock.is_mutable)
            .unwrap_or(false)
    }

    // ============================================================================
    // Configuration and Consensus Simulation
    // ============================================================================

    /// Get the current simulation configuration.
    pub fn config(&self) -> &crate::benchmark::vm::SimulationConfig {
        &self.config
    }

    /// Get mutable access to the simulation configuration.
    pub fn config_mut(&mut self) -> &mut crate::benchmark::vm::SimulationConfig {
        &mut self.config
    }

    /// Set the simulation configuration.
    pub fn set_config(&mut self, config: crate::benchmark::vm::SimulationConfig) {
        self.config = config;
    }

    /// Get the current lamport clock value.
    /// The lamport clock is incremented on every transaction that touches shared objects.
    pub fn lamport_clock(&self) -> u64 {
        self.lamport_clock
    }

    /// Advance the lamport clock and return the new value.
    /// This is called automatically during shared object transactions.
    pub fn advance_lamport_clock(&mut self) -> u64 {
        self.lamport_clock += 1;
        self.lamport_clock
    }

    /// Advance the epoch by a given amount.
    pub fn advance_epoch(&mut self, by: u64) {
        self.config.advance_epoch(by);
    }

    /// Get the current epoch number.
    pub fn epoch(&self) -> u64 {
        self.config.epoch
    }

    /// Set the random seed for deterministic random number generation.
    pub fn set_random_seed(&mut self, seed: [u8; 32]) {
        self.config.random_seed = seed;
    }

    /// Set the gas budget for transaction execution.
    /// None means unlimited gas.
    pub fn set_gas_budget(&mut self, budget: Option<u64>) {
        self.config.gas_budget = budget;
    }

    /// Enable or disable immutability enforcement.
    /// When enabled, mutations to immutable objects will fail.
    pub fn set_enforce_immutability(&mut self, enforce: bool) {
        self.config.enforce_immutability = enforce;
    }

    // ============================================================================
    // Consensus Ordering and Serialization Validation
    // ============================================================================

    /// Get the current consensus sequence number.
    pub fn consensus_sequence(&self) -> u64 {
        self.consensus_sequence
    }

    /// Get the consensus history (recent transaction orderings).
    pub fn consensus_history(&self) -> &[ConsensusOrderEntry] {
        &self.consensus_history
    }

    /// Clear consensus history (useful for test isolation).
    pub fn clear_consensus_history(&mut self) {
        self.consensus_history.clear();
    }

    /// Record a transaction in the consensus ordering history.
    ///
    /// This should be called after a transaction executes successfully.
    /// It records the read/write versions for future conflict detection.
    pub fn record_consensus_entry(
        &mut self,
        transaction_id: String,
        read_versions: BTreeMap<AccountAddress, u64>,
        write_versions: BTreeMap<AccountAddress, u64>,
    ) {
        self.consensus_sequence += 1;
        let entry = ConsensusOrderEntry {
            sequence: self.consensus_sequence,
            transaction_id,
            read_versions,
            write_versions,
            timestamp_ms: self.timestamp_ms.unwrap_or(DEFAULT_CLOCK_BASE_MS),
        };
        self.consensus_history.push(entry);

        // Keep history bounded (last 1000 transactions)
        if self.consensus_history.len() > 1000 {
            self.consensus_history.remove(0);
        }
    }

    /// Validate that a new transaction's access pattern is serializable
    /// with respect to the consensus history.
    ///
    /// Returns `ConsensusValidation::Valid` if the transaction can proceed,
    /// or a conflict description if serialization would be violated.
    pub fn validate_consensus_order(
        &self,
        intended_reads: &BTreeMap<AccountAddress, u64>,
        intended_writes: &BTreeMap<AccountAddress, u64>,
    ) -> ConsensusValidation {
        // First check for stale reads against current object state
        for (object_id, read_version) in intended_reads {
            if let Some(obj) = self.objects.get(object_id) {
                if obj.version > *read_version {
                    return ConsensusValidation::StaleRead {
                        object_id: *object_id,
                        read_version: *read_version,
                        current_version: obj.version,
                    };
                }
            }
        }

        // Then check against recent transaction history for serialization conflicts
        for entry in self.consensus_history.iter().rev() {
            // Check read-write conflicts: we're reading something they wrote
            for (object_id, our_read_version) in intended_reads {
                if let Some(their_write_version) = entry.write_versions.get(object_id) {
                    if our_read_version < their_write_version {
                        return ConsensusValidation::SerializationConflict {
                            object_id: *object_id,
                            our_version: *our_read_version,
                            their_version: *their_write_version,
                            conflicting_tx: entry.transaction_id.clone(),
                            reason: format!(
                                "Read version {} is stale; object was written at version {} by {}",
                                our_read_version, their_write_version, entry.transaction_id
                            ),
                        };
                    }
                }
            }

            // Check write-read conflicts: we're writing something they read
            for (object_id, our_write_version) in intended_writes {
                if let Some(their_read_version) = entry.read_versions.get(object_id) {
                    // If we're writing at or before their read version, conflict
                    if our_write_version <= their_read_version {
                        return ConsensusValidation::SerializationConflict {
                            object_id: *object_id,
                            our_version: *our_write_version,
                            their_version: *their_read_version,
                            conflicting_tx: entry.transaction_id.clone(),
                            reason: format!(
                                "Write version {} conflicts with read at version {} by {}",
                                our_write_version, their_read_version, entry.transaction_id
                            ),
                        };
                    }
                }
            }

            // Check write-write conflicts: we're both writing the same object
            for (object_id, our_write_version) in intended_writes {
                if let Some(their_write_version) = entry.write_versions.get(object_id) {
                    if our_write_version <= their_write_version {
                        return ConsensusValidation::SerializationConflict {
                            object_id: *object_id,
                            our_version: *our_write_version,
                            their_version: *their_write_version,
                            conflicting_tx: entry.transaction_id.clone(),
                            reason: format!(
                                "Write version {} conflicts with write at version {} by {}",
                                our_write_version, their_write_version, entry.transaction_id
                            ),
                        };
                    }
                }
            }
        }

        ConsensusValidation::Valid
    }

    /// Bump the version of shared objects after a transaction mutates them.
    ///
    /// Call this after a transaction that writes to shared objects.
    /// The lamport clock is used as the new version.
    pub fn bump_shared_object_versions(&mut self, object_ids: &[AccountAddress]) {
        let new_version = self.lamport_clock;
        for object_id in object_ids {
            if let Some(obj) = self.objects.get_mut(object_id) {
                if obj.is_shared {
                    obj.version = new_version;
                }
            }
        }
    }

    // ============================================================================
    // State Checkpoint/Restore
    // ============================================================================

    /// Create a checkpoint of the current simulation state.
    ///
    /// Captures object state, dynamic fields, shared locks, and counters
    /// at the current point in time for later restoration.
    pub fn create_checkpoint(&self) -> StateCheckpoint {
        StateCheckpoint {
            objects: self.objects.clone(),
            dynamic_fields: self.dynamic_fields.clone(),
            shared_locks: self.shared_locks.clone(),
            lamport_clock: self.lamport_clock,
            consensus_sequence: self.consensus_sequence,
            tx_counter: self.tx_counter,
            id_counter: self.id_counter,
        }
    }

    /// Restore the simulation state from a checkpoint.
    ///
    /// Rolls back all object state, dynamic fields, shared locks, and counters
    /// to the state captured in the checkpoint.
    pub fn restore_checkpoint(&mut self, checkpoint: StateCheckpoint) {
        self.objects = checkpoint.objects;
        self.dynamic_fields = checkpoint.dynamic_fields;
        self.shared_locks = checkpoint.shared_locks;
        self.lamport_clock = checkpoint.lamport_clock;
        self.consensus_sequence = checkpoint.consensus_sequence;
        self.tx_counter = checkpoint.tx_counter;
        self.id_counter = checkpoint.id_counter;
    }

    // ============================================================================
    // Dynamic Field Iteration
    // ============================================================================

    /// List all dynamic field keys for a parent object.
    ///
    /// Returns (child_id, type_tag) pairs for all dynamic fields.
    pub fn list_dynamic_fields(&self, parent_id: AccountAddress) -> Vec<(AccountAddress, TypeTag)> {
        self.dynamic_fields
            .iter()
            .filter(|((p, _), _)| *p == parent_id)
            .map(|((_, child_id), (type_tag, _))| (*child_id, type_tag.clone()))
            .collect()
    }

    /// Count dynamic fields for a parent object.
    ///
    /// Equivalent to `table::length()` or `bag::length()`.
    pub fn count_dynamic_fields(&self, parent_id: AccountAddress) -> usize {
        self.dynamic_fields
            .keys()
            .filter(|(p, _)| *p == parent_id)
            .count()
    }

    /// Iterate over dynamic fields with a closure.
    ///
    /// Useful for aggregating data from Table/Bag contents.
    pub fn fold_dynamic_fields<T, F>(&self, parent_id: AccountAddress, initial: T, mut f: F) -> T
    where
        F: FnMut(T, AccountAddress, &TypeTag, &[u8]) -> T,
    {
        let mut acc = initial;
        for ((p, child_id), (type_tag, bytes)) in &self.dynamic_fields {
            if *p == parent_id {
                acc = f(acc, *child_id, type_tag, bytes);
            }
        }
        acc
    }

    /// Execute a PTB with automatic shared object locking.
    ///
    /// Note: This is now an alias for `execute_ptb()` since locking is integrated
    /// directly into the main execution path. All PTB executions automatically
    /// handle shared object locking.
    #[deprecated(note = "Use execute_ptb() directly - locking is now automatic")]
    pub fn execute_ptb_with_locking(
        &mut self,
        inputs: Vec<InputValue>,
        commands: Vec<Command>,
    ) -> ExecutionResult {
        self.execute_ptb(inputs, commands)
    }

    /// Parse an error string into a structured SimulationError.
    /// Returns errors matching mainnet format without suggestions.
    fn parse_error(&self, error: &str) -> SimulationError {
        // Check for LINKER_ERROR (missing module)
        if error.contains("LINKER_ERROR") || error.contains("Cannot find ModuleId") {
            if let Some(addr) = Self::extract_address(error) {
                return SimulationError::MissingPackage {
                    address: addr,
                    module: Self::extract_module_name(error),
                };
            }
        }

        // Check for ABORTED (contract abort)
        if error.contains("ABORTED") {
            if let Some(code) = Self::extract_abort_code(error) {
                let (module, function) = Self::extract_abort_location(error);
                return SimulationError::ContractAbort {
                    module: module.unwrap_or_else(|| "unknown".to_string()),
                    function: function.unwrap_or_else(|| "unknown".to_string()),
                    abort_code: code,
                    message: Self::extract_abort_message(error),
                };
            }
        }

        // Check for FAILED_TO_DESERIALIZE_ARGUMENT
        if error.contains("FAILED_TO_DESERIALIZE_ARGUMENT") {
            return SimulationError::DeserializationFailed {
                argument_index: Self::extract_argument_index(error).unwrap_or(0),
                expected_type: Self::extract_expected_type(error)
                    .unwrap_or_else(|| "unknown".to_string()),
            };
        }

        // Check for FUNCTION_RESOLUTION_FAILURE
        if error.contains("FUNCTION_RESOLUTION_FAILURE") {
            if let Some(addr) = Self::extract_address(error) {
                return SimulationError::MissingPackage {
                    address: addr,
                    module: Self::extract_module_name(error),
                };
            }
        }

        // Default: pass through the raw error
        SimulationError::ExecutionError {
            message: error.to_string(),
        }
    }

    /// Extract argument index from error message.
    fn extract_argument_index(error: &str) -> Option<usize> {
        // Look for patterns like "argument 0" or "arg[0]"
        if let Some(start) = error.find("argument ") {
            let rest = &error[start + 9..];
            if let Some(end) = rest.find(|c: char| !c.is_ascii_digit()) {
                return rest[..end].parse().ok();
            }
        }
        None
    }

    /// Extract expected type from error message.
    fn extract_expected_type(error: &str) -> Option<String> {
        // Look for patterns like "expected type: X" or "as X"
        if let Some(start) = error.find("expected ") {
            let rest = &error[start + 9..];
            if let Some(end) = rest.find([',', ')', '\n']) {
                return Some(rest[..end].trim().to_string());
            }
        }
        None
    }

    /// Extract an address from an error message.
    fn extract_address(error: &str) -> Option<String> {
        // Look for patterns like "address: abc123" or "0x..."
        if let Some(start) = error.find("address: ") {
            let rest = &error[start + 9..];
            if let Some(end) = rest.find(|c: char| c == ',' || c == '}' || c.is_whitespace()) {
                let addr = &rest[..end];
                return Some(format!("0x{}", addr));
            }
        }
        None
    }

    /// Extract a module name from an error message.
    fn extract_module_name(error: &str) -> Option<String> {
        if let Some(start) = error.find("Identifier(\"") {
            let rest = &error[start + 12..];
            if let Some(end) = rest.find("\"") {
                return Some(rest[..end].to_string());
            }
        }
        None
    }

    /// Extract abort code from error message.
    fn extract_abort_code(error: &str) -> Option<u64> {
        if let Some(start) = error.find("sub_status: Some(") {
            let rest = &error[start + 17..];
            if let Some(end) = rest.find(")") {
                return rest[..end].parse().ok();
            }
        }
        None
    }

    /// Extract abort location (module::function).
    fn extract_abort_location(error: &str) -> (Option<String>, Option<String>) {
        // Look for pattern like "0x...::module::function at offset"
        if let Some(start) = error.find("message: Some(\"") {
            let rest = &error[start + 15..];
            if let Some(end) = rest.find(" at offset") {
                let location = &rest[..end];
                let parts: Vec<&str> = location.split("::").collect();
                if parts.len() >= 3 {
                    return (Some(parts[1].to_string()), Some(parts[2].to_string()));
                }
            }
        }
        (None, None)
    }

    /// Extract abort message.
    fn extract_abort_message(error: &str) -> Option<String> {
        if let Some(start) = error.find("message: Some(\"") {
            let rest = &error[start + 15..];
            if let Some(end) = rest.find("\")") {
                return Some(rest[..end].to_string());
            }
        }
        None
    }

    /// Inspect an object's state for debugging.
    /// Returns a human-readable representation of the object.
    pub fn inspect_object(&self, id: &AccountAddress) -> Option<String> {
        let obj = self.objects.get(id)?;

        let mut output = format!(
            "Object {}\n\
             Type: {:?}\n\
             Shared: {}\n\
             Immutable: {}\n\
             Version: {}\n\
             BCS bytes ({} bytes): ",
            id.to_hex_literal(),
            obj.type_tag,
            obj.is_shared,
            obj.is_immutable,
            obj.version,
            obj.bcs_bytes.len()
        );

        // Show first 64 bytes in hex, or all if shorter
        let preview_len = std::cmp::min(obj.bcs_bytes.len(), 64);
        for byte in &obj.bcs_bytes[..preview_len] {
            output.push_str(&format!("{:02x}", byte));
        }
        if obj.bcs_bytes.len() > 64 {
            output.push_str("...");
        }

        Some(output)
    }

    /// List all available packages with their modules.
    pub fn list_available_packages(&self) -> Vec<(AccountAddress, Vec<String>)> {
        // Get packages from resolver
        let package_addrs = self.resolver.list_packages();
        let mut result = Vec::new();

        for addr in package_addrs {
            // Get module names for this package
            let modules = self.resolver.get_package_modules(&addr);
            result.push((addr, modules));
        }
        result
    }

    // ====================================================================
    // Source Compilation Support
    // ====================================================================

    /// Compile a Move project using the Sui CLI.
    ///
    /// This wraps `sui move build` to compile Move source code into bytecode.
    /// The project must have a standard Move.toml structure.
    ///
    /// # Arguments
    /// * `project_path` - Path to the Move project directory (containing Move.toml)
    ///
    /// # Returns
    /// * `Ok(CompileResult)` - Compilation succeeded, bytecode is in build directory
    /// * `Err(CompileError)` - Compilation failed with structured error info
    pub fn compile_source(
        &self,
        project_path: &std::path::Path,
    ) -> Result<CompileResult, CompileError> {
        use std::process::Command;

        // Verify Move.toml exists
        let manifest = project_path.join("Move.toml");
        if !manifest.exists() {
            return Err(CompileError {
                errors: vec![CompileErrorDetail {
                    file: None,
                    line: None,
                    column: None,
                    message: "Move.toml not found in project directory".to_string(),
                }],
                raw_output: "Move.toml not found".to_string(),
            });
        }

        // Run sui move build
        let output = Command::new("sui")
            .args(["move", "build", "--path"])
            .arg(project_path)
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(CompileError {
                    errors: vec![CompileErrorDetail {
                        file: None,
                        line: None,
                        column: None,
                        message: format!("Failed to run 'sui move build': {}", e),
                    }],
                    raw_output: e.to_string(),
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let raw_output = format!("{}\n{}", stdout, stderr);

        if output.status.success() {
            // Find compiled bytecode in build directory
            let build_dir = project_path.join("build");
            let bytecode_files = Self::find_compiled_modules(&build_dir);

            Ok(CompileResult {
                build_dir,
                modules: bytecode_files,
                warnings: Self::parse_compile_warnings(&stdout),
            })
        } else {
            // Parse compile errors
            let errors = Self::parse_compile_errors(&stderr);
            Err(CompileError { errors, raw_output })
        }
    }

    /// Compile Move source and deploy the resulting package.
    ///
    /// This is a convenience method that:
    /// 1. Compiles the Move project
    /// 2. Reads the compiled bytecode
    /// 3. Deploys it to the simulation environment
    ///
    /// # Arguments
    /// * `project_path` - Path to the Move project directory
    ///
    /// # Returns
    /// * `Ok((package_id, module_names))` - Deployment succeeded
    /// * `Err` - Either compilation or deployment failed
    pub fn compile_and_deploy(
        &mut self,
        project_path: &std::path::Path,
    ) -> Result<(AccountAddress, Vec<String>)> {
        // Compile
        let compile_result = self
            .compile_source(project_path)
            .map_err(|e| anyhow!("Compilation failed:\n{}", e.format_errors()))?;

        // Read bytecode from compiled modules
        let mut modules_bytecode = Vec::new();
        for module_path in &compile_result.modules {
            let bytecode = std::fs::read(module_path)
                .map_err(|e| anyhow!("Failed to read compiled module {:?}: {}", module_path, e))?;
            modules_bytecode.push(bytecode);
        }

        if modules_bytecode.is_empty() {
            return Err(anyhow!("No compiled modules found in build directory"));
        }

        // Generate a new package address
        let package_id = self.fresh_id();

        // Parse modules and prepare for deployment
        let mut modules_with_names: Vec<(String, Vec<u8>)> = Vec::new();
        let mut module_names = Vec::new();

        for bytecode in modules_bytecode {
            if let Ok(module) =
                move_binary_format::CompiledModule::deserialize_with_defaults(&bytecode)
            {
                let name = module.name().to_string();
                module_names.push(name.clone());
                modules_with_names.push((name, bytecode));
            }
        }

        // Deploy to resolver with address aliasing
        self.resolver
            .add_package_modules_at(modules_with_names, Some(package_id))?;

        Ok((package_id, module_names))
    }

    /// Find all compiled .mv files in a build directory.
    fn find_compiled_modules(build_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut modules = Vec::new();

        if !build_dir.exists() {
            return modules;
        }

        // Walk the build directory looking for .mv files
        if let Ok(entries) = std::fs::read_dir(build_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Look in package_name/bytecode_modules/*.mv
                    let bytecode_dir = path.join("bytecode_modules");
                    if bytecode_dir.exists() {
                        if let Ok(module_entries) = std::fs::read_dir(&bytecode_dir) {
                            for module_entry in module_entries.flatten() {
                                let module_path = module_entry.path();
                                if module_path.extension().is_some_and(|e| e == "mv") {
                                    modules.push(module_path);
                                }
                            }
                        }
                    }
                }
            }
        }

        modules
    }

    /// Parse compile warnings from stdout.
    fn parse_compile_warnings(stdout: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        for line in stdout.lines() {
            if line.contains("warning") || line.contains("Warning") {
                warnings.push(line.to_string());
            }
        }
        warnings
    }

    // ====================================================================
    // Sandbox Execution Interface Methods
    // ====================================================================

    /// Create an object from JSON field values.
    /// Used by the sandbox_exec module for LLM-driven object creation.
    pub fn create_object_from_json(
        &mut self,
        object_type: &str,
        fields: &std::collections::HashMap<String, serde_json::Value>,
        specific_id: Option<[u8; 32]>,
    ) -> Result<AccountAddress> {
        // Parse the object type
        let type_tag = crate::benchmark::tx_replay::parse_type_tag(object_type)?;

        // Generate or use specific ID
        let id = if let Some(bytes) = specific_id {
            AccountAddress::new(bytes)
        } else {
            self.fresh_id()
        };

        // Build BCS bytes from fields
        // For now, we support common patterns:
        // - Objects with UID as first field need the ID prepended
        // - Coin types need balance encoding
        let bcs_bytes = self.encode_object_from_json(&type_tag, &id, fields)?;

        let obj = SimulatedObject {
            id,
            type_tag,
            bcs_bytes,
            is_shared: false,
            is_immutable: false,
            version: 1,
        };
        self.objects.insert(id, obj);
        Ok(id)
    }

    /// Encode object fields from JSON to BCS.
    fn encode_object_from_json(
        &self,
        _type_tag: &TypeTag,
        id: &AccountAddress,
        fields: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();

        // Most Sui objects start with a UID field containing the object ID
        bytes.extend_from_slice(id.as_ref());

        // Encode remaining fields in order
        // Note: This is a simplified encoding that handles common cases
        // A full implementation would need type information to properly order fields
        for value in fields.values() {
            match value {
                serde_json::Value::Number(n) => {
                    if let Some(v) = n.as_u64() {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    } else if let Some(v) = n.as_i64() {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                }
                serde_json::Value::Bool(b) => {
                    bytes.push(if *b { 1 } else { 0 });
                }
                serde_json::Value::String(s) => {
                    // Try to parse as hex address
                    if s.starts_with("0x") {
                        if let Ok(addr) = AccountAddress::from_hex_literal(s) {
                            bytes.extend_from_slice(addr.as_ref());
                            continue;
                        }
                    }
                    // Otherwise encode as vector<u8>
                    let s_bytes = s.as_bytes();
                    // ULEB128 length prefix
                    bytes.extend(leb128_encode(s_bytes.len() as u64));
                    bytes.extend_from_slice(s_bytes);
                }
                serde_json::Value::Array(arr) => {
                    // Encode as vector with ULEB128 length prefix
                    bytes.extend(leb128_encode(arr.len() as u64));
                    for elem in arr {
                        if let Some(v) = elem.as_u64() {
                            bytes.push(v as u8);
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(bytes)
    }

    /// Get an object prepared for PTB execution.
    pub fn get_object_for_ptb(&self, object_id: &str) -> Result<ObjectInput> {
        self.get_object_for_ptb_with_mode(object_id, None)
    }

    /// Get an object prepared for PTB execution with explicit access mode.
    /// Modes: "immutable", "mutable", "owned", "shared" (default: inferred from object)
    ///
    /// If the object is not found locally and mainnet fetching is enabled,
    /// this will NOT auto-fetch. Use `get_or_fetch_object_for_ptb` for auto-fetch behavior.
    pub fn get_object_for_ptb_with_mode(
        &self,
        object_id: &str,
        mode: Option<&str>,
    ) -> Result<ObjectInput> {
        let addr = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow!("Invalid object ID: {}", e))?;

        let obj = self
            .objects
            .get(&addr)
            .ok_or_else(|| anyhow!("ObjectNotFound: {}", object_id))?;

        // Use explicit mode if provided, otherwise infer from object properties
        let type_tag = Some(obj.type_tag.clone());
        match mode {
            Some("mutable") | Some("mut") => Ok(ObjectInput::MutRef {
                id: addr,
                bytes: obj.bcs_bytes.clone(),
                type_tag,
            }),
            Some("immutable") | Some("imm") => Ok(ObjectInput::ImmRef {
                id: addr,
                bytes: obj.bcs_bytes.clone(),
                type_tag,
            }),
            Some("owned") | Some("value") => Ok(ObjectInput::Owned {
                id: addr,
                bytes: obj.bcs_bytes.clone(),
                type_tag,
            }),
            Some("shared") => Ok(ObjectInput::Shared {
                id: addr,
                bytes: obj.bcs_bytes.clone(),
                type_tag,
            }),
            // Default: infer from object properties
            None | Some(_) => {
                if obj.is_shared {
                    Ok(ObjectInput::Shared {
                        id: addr,
                        bytes: obj.bcs_bytes.clone(),
                        type_tag,
                    })
                } else if obj.is_immutable {
                    Ok(ObjectInput::ImmRef {
                        id: addr,
                        bytes: obj.bcs_bytes.clone(),
                        type_tag,
                    })
                } else {
                    // Default to mutable reference for non-shared, non-immutable objects
                    Ok(ObjectInput::MutRef {
                        id: addr,
                        bytes: obj.bcs_bytes.clone(),
                        type_tag,
                    })
                }
            }
        }
    }

    /// Get an object for PTB execution, auto-fetching from mainnet if not found locally.
    /// This is the recommended method when mainnet fetching is enabled.
    pub fn get_or_fetch_object_for_ptb(
        &mut self,
        object_id: &str,
        mode: Option<&str>,
    ) -> Result<ObjectInput> {
        let addr = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow!("Invalid object ID: {}", e))?;

        // Try local first
        if self.objects.contains_key(&addr) {
            return self.get_object_for_ptb_with_mode(object_id, mode);
        }

        // Not found locally - try to fetch from mainnet if enabled
        if self.fetcher.is_some() {
            self.fetch_object_from_mainnet(object_id)?;
            return self.get_object_for_ptb_with_mode(object_id, mode);
        }

        // No fetcher - return not found error
        Err(anyhow!("ObjectNotFound: {}", object_id))
    }

    /// Check if an object exists in the local store.
    pub fn has_object(&self, object_id: &str) -> bool {
        if let Ok(addr) = AccountAddress::from_hex_literal(object_id) {
            self.objects.contains_key(&addr)
        } else {
            false
        }
    }

    /// Delete an object from the store.
    pub fn delete_object(&mut self, object_id: &str) -> Result<()> {
        let addr = AccountAddress::from_hex_literal(object_id)
            .map_err(|e| anyhow!("Invalid object ID: {}", e))?;
        self.objects
            .remove(&addr)
            .ok_or_else(|| anyhow!("ObjectNotFound: {}", object_id))?;
        Ok(())
    }

    /// Get the count of objects in the store.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Create a gas coin (Coin<SUI>) with the given balance.
    /// Used for PTB gas payment simulation.
    pub fn create_gas_coin(&mut self, balance: u64) -> Result<ObjectInput> {
        // Create a SUI coin
        let coin_id = self.create_coin("0x2::sui::SUI", balance)?;

        // Get it as an owned object (gas coin is consumed by value)
        let obj = self
            .objects
            .get(&coin_id)
            .ok_or_else(|| anyhow!("Failed to retrieve created gas coin"))?;

        Ok(ObjectInput::Owned {
            id: coin_id,
            bytes: obj.bcs_bytes.clone(),
            type_tag: Some(obj.type_tag.clone()),
        })
    }

    /// Get struct definitions from loaded modules.
    pub fn get_struct_definitions(
        &self,
        package: &str,
        module_filter: Option<&str>,
        struct_filter: Option<&str>,
    ) -> Result<Vec<StructDefinition>> {
        let pkg_addr = AccountAddress::from_hex_literal(package)
            .map_err(|e| anyhow!("Invalid package address: {}", e))?;

        let mut result = Vec::new();

        // Get modules for this package
        let module_names = self.resolver.get_package_modules(&pkg_addr);

        for module_name in module_names {
            // Apply module filter
            if let Some(filter) = module_filter {
                if !module_name.contains(filter) {
                    continue;
                }
            }

            // Get struct definitions from this module
            if let Some(structs) = self.resolver.get_module_structs(&pkg_addr, &module_name) {
                for (struct_name, struct_info) in structs {
                    // Apply struct filter
                    if let Some(filter) = struct_filter {
                        if !struct_name.contains(filter) {
                            continue;
                        }
                    }

                    result.push(StructDefinition {
                        package: package.to_string(),
                        module: module_name.clone(),
                        name: struct_name,
                        abilities: struct_info.abilities,
                        type_params: struct_info
                            .type_params
                            .into_iter()
                            .map(|tp| TypeParamDef {
                                name: tp.name,
                                constraints: tp.constraints,
                            })
                            .collect(),
                        fields: struct_info
                            .fields
                            .into_iter()
                            .map(|f| FieldDefinition {
                                name: f.name,
                                field_type: f.field_type,
                            })
                            .collect(),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Get a summary of the sandbox state.
    pub fn get_state_summary(&self) -> StateSummary {
        let loaded_packages: Vec<String> = self
            .resolver
            .list_packages()
            .into_iter()
            .map(|a| a.to_hex_literal())
            .collect();

        let loaded_modules: Vec<(String, String)> = self
            .resolver
            .list_packages()
            .into_iter()
            .flat_map(|addr| {
                self.resolver
                    .get_package_modules(&addr)
                    .into_iter()
                    .map(move |name| (addr.to_hex_literal(), name))
            })
            .collect();

        StateSummary {
            loaded_packages,
            loaded_modules,
            object_count: self.objects.len(),
            sender: self.sender.to_hex_literal(),
            timestamp_ms: self.timestamp_ms.unwrap_or(0),
        }
    }

    /// Reset the sandbox to initial state (keep only framework).
    pub fn reset(&mut self) -> Result<()> {
        self.resolver = LocalModuleResolver::with_sui_framework()?;
        self.objects.clear();
        self.id_counter = 0;
        self.sender = AccountAddress::ZERO;
        self.timestamp_ms = None;
        Ok(())
    }

    /// Call a Move function directly (for testing).
    pub fn call_function(
        &mut self,
        package: &str,
        module: &str,
        function: &str,
        type_args: &[String],
        args: &[serde_json::Value],
    ) -> Result<FunctionCallResult> {
        let pkg_addr = AccountAddress::from_hex_literal(package)?;
        let module_id = Identifier::new(module)?;
        let function_id = Identifier::new(function)?;

        // Parse type args
        let parsed_type_args: Vec<TypeTag> = type_args
            .iter()
            .map(|s| crate::benchmark::tx_replay::parse_type_tag(s))
            .collect::<Result<Vec<_>, _>>()?;

        // Build the MoveCall command
        let command = Command::MoveCall {
            package: pkg_addr,
            module: module_id,
            function: function_id,
            type_args: parsed_type_args,
            args: (0..args.len())
                .map(|i| crate::benchmark::ptb::Argument::Input(i as u16))
                .collect(),
        };

        // Convert args to inputs
        let inputs: Vec<InputValue> = args
            .iter()
            .map(|v| InputValue::Pure(serde_json::to_vec(v).unwrap_or_default()))
            .collect();

        // Execute
        let result = self.execute_ptb(inputs, vec![command]);

        if result.success {
            // Extract return values from the first (and only) command's results
            let return_values = result
                .effects
                .as_ref()
                .and_then(|effects| effects.return_values.first())
                .cloned()
                .unwrap_or_default();

            // Extract gas used from effects
            let gas_used = result
                .effects
                .as_ref()
                .map(|effects| effects.gas_used)
                .unwrap_or(0);

            Ok(FunctionCallResult {
                return_values,
                gas_used,
            })
        } else {
            Err(anyhow!(
                "{}",
                result
                    .raw_error
                    .unwrap_or_else(|| "Unknown error".to_string())
            ))
        }
    }

    /// Parse compile errors from stderr into structured form.
    pub fn parse_compile_errors(stderr: &str) -> Vec<CompileErrorDetail> {
        let mut errors = Vec::new();
        let mut current_error: Option<CompileErrorDetail> = None;

        for line in stderr.lines() {
            // Look for error patterns like:
            // error[E01234]: message
            // --> sources/module.move:10:5
            if line.starts_with("error") {
                // Save previous error
                if let Some(err) = current_error.take() {
                    errors.push(err);
                }

                // Start new error
                let message = line.trim_start_matches("error").trim();
                current_error = Some(CompileErrorDetail {
                    file: None,
                    line: None,
                    column: None,
                    message: message.to_string(),
                });
            } else if line.contains("-->") && current_error.is_some() {
                // Parse location: --> path/to/file.move:line:column
                if let Some(ref mut err) = current_error {
                    let location = line
                        .trim_start_matches(|c: char| c == '-' || c == '>' || c.is_whitespace());
                    let parts: Vec<&str> = location.split(':').collect();
                    if !parts.is_empty() {
                        err.file = Some(parts[0].to_string());
                    }
                    if parts.len() > 1 {
                        err.line = parts[1].parse().ok();
                    }
                    if parts.len() > 2 {
                        err.column = parts[2].parse().ok();
                    }
                }
            }
            // Note: We intentionally do not capture 'help:' or 'suggestion' lines
            // to keep error output neutral and non-prescriptive
        }

        // Don't forget the last error
        if let Some(err) = current_error {
            errors.push(err);
        }

        // If no structured errors found, create a generic one
        if errors.is_empty() && !stderr.trim().is_empty() {
            errors.push(CompileErrorDetail {
                file: None,
                line: None,
                column: None,
                message: stderr.lines().next().unwrap_or("Unknown error").to_string(),
            });
        }

        errors
    }

    // ========================================================================
    // State Persistence
    // ========================================================================

    /// Export the current state to a serializable format.
    pub fn export_state(&self) -> PersistentState {
        use base64::Engine;

        // Export objects
        let objects: Vec<SerializedObject> = self
            .objects
            .values()
            .map(|obj| SerializedObject {
                id: obj.id.to_hex_literal(),
                type_tag: format!("{}", obj.type_tag),
                bcs_bytes_b64: base64::engine::general_purpose::STANDARD.encode(&obj.bcs_bytes),
                is_shared: obj.is_shared,
                is_immutable: obj.is_immutable,
                version: obj.version,
            })
            .collect();

        // Export non-framework modules
        // We skip 0x1, 0x2, 0x3 as those are always loaded from bundled framework
        let framework_addrs: std::collections::BTreeSet<AccountAddress> = [
            AccountAddress::from_hex_literal("0x1").unwrap(),
            AccountAddress::from_hex_literal("0x2").unwrap(),
            AccountAddress::from_hex_literal("0x3").unwrap(),
        ]
        .into_iter()
        .collect();

        let modules: Vec<SerializedModule> = self
            .resolver
            .iter_modules()
            .filter(|m| !framework_addrs.contains(m.self_id().address()))
            .filter_map(|m| {
                // Get bytecode from resolver
                let id = m.self_id();
                match self.resolver.get_module(&id) {
                    Ok(Some(bytes)) => Some(SerializedModule {
                        id: format!("{}::{}", id.address().to_hex_literal(), id.name()),
                        bytecode_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    }),
                    _ => None,
                }
            })
            .collect();

        // Export coin registry
        let coin_registry: std::collections::HashMap<String, CoinMetadata> = self
            .coin_registry
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Export dynamic fields (Table/Bag entries)
        let dynamic_fields: Vec<SerializedDynamicField> = self
            .dynamic_fields
            .iter()
            .map(
                |((parent_id, child_id), (type_tag, bytes))| SerializedDynamicField {
                    parent_id: parent_id.to_hex_literal(),
                    child_id: child_id.to_hex_literal(),
                    type_tag: format!("{}", type_tag),
                    value_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
            )
            .collect();

        // Export pending receives (send-to-object pattern)
        let pending_receives: Vec<SerializedPendingReceive> = self
            .pending_receives
            .iter()
            .map(
                |((recipient_id, sent_id), (bytes, type_tag))| SerializedPendingReceive {
                    recipient_id: recipient_id.to_hex_literal(),
                    sent_id: sent_id.to_hex_literal(),
                    type_tag: format!("{}", type_tag),
                    object_bytes_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
            )
            .collect();

        // Get current timestamp for metadata
        let now = chrono::Utc::now().to_rfc3339();

        PersistentState {
            version: PersistentState::CURRENT_VERSION,
            objects,
            modules,
            coin_registry,
            sender: self.sender.to_hex_literal(),
            id_counter: self.id_counter,
            timestamp_ms: self.timestamp_ms,
            dynamic_fields,
            pending_receives,
            config: Some(self.config.clone()),
            metadata: Some(StateMetadata {
                description: None,
                created_at: Some(now.clone()),
                modified_at: Some(now),
                tags: Vec::new(),
            }),
            fetcher_config: if self.fetcher_config.enabled {
                Some(self.fetcher_config.clone())
            } else {
                None
            },
        }
    }

    /// Export state with custom metadata.
    pub fn export_state_with_metadata(
        &self,
        description: Option<String>,
        tags: Vec<String>,
    ) -> PersistentState {
        let mut state = self.export_state();
        if let Some(ref mut metadata) = state.metadata {
            metadata.description = description;
            metadata.tags = tags;
        }
        state
    }

    /// Save the current state to a file.
    pub fn save_state(&self, path: &std::path::Path) -> Result<()> {
        let state = self.export_state();
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| anyhow!("Failed to serialize state: {}", e))?;
        std::fs::write(path, json).map_err(|e| anyhow!("Failed to write state file: {}", e))?;
        Ok(())
    }

    /// Save the current state to a file with custom metadata.
    pub fn save_state_with_metadata(
        &self,
        path: &std::path::Path,
        description: Option<String>,
        tags: Vec<String>,
    ) -> Result<()> {
        let state = self.export_state_with_metadata(description, tags);
        let json = serde_json::to_string_pretty(&state)
            .map_err(|e| anyhow!("Failed to serialize state: {}", e))?;
        std::fs::write(path, json).map_err(|e| anyhow!("Failed to write state file: {}", e))?;
        Ok(())
    }

    /// Load state from a file, merging with current state.
    pub fn load_state(&mut self, path: &std::path::Path) -> Result<()> {
        use base64::Engine;

        let json = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read state file: {}", e))?;
        let state: PersistentState = serde_json::from_str(&json)
            .map_err(|e| anyhow!("Failed to parse state file: {}", e))?;

        // Check version compatibility
        if state.version > PersistentState::CURRENT_VERSION {
            return Err(anyhow!(
                "State file version {} is newer than supported version {}",
                state.version,
                PersistentState::CURRENT_VERSION
            ));
        }

        // Load modules
        for module in &state.modules {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&module.bytecode_b64)
                .map_err(|e| anyhow!("Failed to decode module {}: {}", module.id, e))?;
            self.resolver.add_module_bytes(bytes)?;
        }

        // Load objects
        for obj in &state.objects {
            let id = AccountAddress::from_hex_literal(&obj.id)
                .map_err(|e| anyhow!("Invalid object ID {}: {}", obj.id, e))?;
            let type_tag = crate::benchmark::tx_replay::parse_type_tag(&obj.type_tag)
                .map_err(|e| anyhow!("Invalid type tag {}: {}", obj.type_tag, e))?;
            let bcs_bytes = base64::engine::general_purpose::STANDARD
                .decode(&obj.bcs_bytes_b64)
                .map_err(|e| anyhow!("Failed to decode object {}: {}", obj.id, e))?;

            let sim_obj = SimulatedObject {
                id,
                type_tag,
                bcs_bytes,
                is_shared: obj.is_shared,
                is_immutable: obj.is_immutable,
                version: obj.version,
            };
            self.objects.insert(id, sim_obj);
        }

        // Load coin registry
        for (k, v) in state.coin_registry {
            self.coin_registry.insert(k, v);
        }

        // Load sender
        if !state.sender.is_empty() && state.sender != "0x0" {
            self.sender =
                AccountAddress::from_hex_literal(&state.sender).unwrap_or(AccountAddress::ZERO);
        }

        // Load id counter (use max to avoid collisions)
        self.id_counter = self.id_counter.max(state.id_counter);

        // Load timestamp
        if state.timestamp_ms.is_some() {
            self.timestamp_ms = state.timestamp_ms;
        }

        // Load dynamic fields (Table/Bag entries) - v2+
        for df in &state.dynamic_fields {
            let parent_id = AccountAddress::from_hex_literal(&df.parent_id)
                .map_err(|e| anyhow!("Invalid parent ID {}: {}", df.parent_id, e))?;
            let child_id = AccountAddress::from_hex_literal(&df.child_id)
                .map_err(|e| anyhow!("Invalid child ID {}: {}", df.child_id, e))?;
            let type_tag = crate::benchmark::tx_replay::parse_type_tag(&df.type_tag)
                .map_err(|e| anyhow!("Invalid type tag {}: {}", df.type_tag, e))?;
            let value = base64::engine::general_purpose::STANDARD
                .decode(&df.value_b64)
                .map_err(|e| anyhow!("Failed to decode dynamic field value: {}", e))?;

            self.dynamic_fields
                .insert((parent_id, child_id), (type_tag, value));
        }

        // Load pending receives (send-to-object pattern) - v2+
        for pr in &state.pending_receives {
            let recipient_id = AccountAddress::from_hex_literal(&pr.recipient_id)
                .map_err(|e| anyhow!("Invalid recipient ID {}: {}", pr.recipient_id, e))?;
            let sent_id = AccountAddress::from_hex_literal(&pr.sent_id)
                .map_err(|e| anyhow!("Invalid sent ID {}: {}", pr.sent_id, e))?;
            let type_tag = crate::benchmark::tx_replay::parse_type_tag(&pr.type_tag)
                .map_err(|e| anyhow!("Invalid type tag {}: {}", pr.type_tag, e))?;
            let object_bytes = base64::engine::general_purpose::STANDARD
                .decode(&pr.object_bytes_b64)
                .map_err(|e| anyhow!("Failed to decode pending receive bytes: {}", e))?;

            self.pending_receives
                .insert((recipient_id, sent_id), (object_bytes, type_tag));
        }

        // Load simulation config - v3+
        if let Some(config) = state.config {
            self.config = config;
        }

        // Load and restore fetcher config - v4+
        // If the saved state had fetching enabled, we auto-reconnect
        if let Some(fetcher_config) = state.fetcher_config {
            if fetcher_config.enabled {
                self.fetcher_config = fetcher_config.clone();
                self.fetcher = Self::create_fetcher_from_config(&fetcher_config);
            }
        }

        Ok(())
    }

    /// Create a new environment from a saved state file.
    pub fn from_state_file(path: &std::path::Path) -> Result<Self> {
        let mut env = Self::new()?;
        env.load_state(path)?;
        Ok(env)
    }

    // ========================================================================
    // LLM Agent Tools - Introspection and search methods
    // ========================================================================

    /// List all loaded module paths (e.g., "0x2::coin").
    pub fn list_modules(&self) -> Vec<String> {
        self.resolver.list_modules()
    }

    /// List all functions in a module.
    pub fn list_functions(&self, module_path: &str) -> Option<Vec<String>> {
        self.resolver.list_functions(module_path)
    }

    /// List all structs in a module.
    pub fn list_structs(&self, module_path: &str) -> Option<Vec<String>> {
        self.resolver.list_structs(module_path)
    }

    /// Get detailed function info.
    pub fn get_function_info(
        &self,
        module_path: &str,
        function_name: &str,
    ) -> Option<serde_json::Value> {
        self.resolver.get_function_info(module_path, function_name)
    }

    /// Find all constructors (functions that return the given type).
    pub fn find_constructors(&self, type_path: &str) -> Vec<serde_json::Value> {
        self.resolver.find_constructors(type_path)
    }

    /// Search for types matching a pattern.
    pub fn search_types(
        &self,
        pattern: &str,
        ability_filter: Option<&str>,
    ) -> Vec<serde_json::Value> {
        self.resolver.search_types(pattern, ability_filter)
    }

    /// Search for functions matching a pattern.
    pub fn search_functions(&self, pattern: &str, entry_only: bool) -> Vec<serde_json::Value> {
        self.resolver.search_functions(pattern, entry_only)
    }

    /// Disassemble a function to bytecode.
    pub fn disassemble_function(&self, module_path: &str, function_name: &str) -> Option<String> {
        self.resolver
            .disassemble_function(module_path, function_name)
    }

    /// Get struct type information.
    pub fn get_struct_info(&self, type_path: &str) -> Option<serde_json::Value> {
        self.resolver.get_struct_info(type_path)
    }

    /// Create a test object with the given type and value.
    /// This is a simplified API that converts JSON values to field maps and delegates
    /// to create_object_from_json. Supports:
    /// - JSON objects: used directly as field map
    /// - JSON primitives (number, string, bool): wrapped as {"value": ...}
    /// - JSON arrays: wrapped as {"elements": [...]}
    pub fn create_test_object(
        &mut self,
        type_tag: &str,
        value: serde_json::Value,
    ) -> Result<AccountAddress> {
        use std::collections::HashMap;

        // Convert JSON value to a field map
        let fields: HashMap<String, serde_json::Value> = match value {
            serde_json::Value::Object(map) => {
                // If the value is already an object, convert the map to HashMap
                map.into_iter().collect()
            }
            serde_json::Value::Number(_)
            | serde_json::Value::Bool(_)
            | serde_json::Value::String(_) => {
                // Wrap primitives in a "value" field (common pattern for wrapper types)
                let mut fields = HashMap::new();
                fields.insert("value".to_string(), value);
                fields
            }
            serde_json::Value::Array(arr) => {
                // Wrap arrays in an "elements" field
                let mut fields = HashMap::new();
                fields.insert("elements".to_string(), serde_json::Value::Array(arr));
                fields
            }
            serde_json::Value::Null => {
                // Empty object
                HashMap::new()
            }
        };

        // Delegate to the full create_object_from_json
        self.create_object_from_json(type_tag, &fields, None)
    }

    /// Get module dependencies.
    /// Returns a list of (address, module_name) pairs that the module imports.
    pub fn get_module_dependencies(
        &self,
        address: &AccountAddress,
        module_name: &str,
    ) -> Result<Vec<(AccountAddress, String)>> {
        use move_binary_format::CompiledModule;

        let module_id = move_core_types::language_storage::ModuleId::new(
            *address,
            move_core_types::identifier::Identifier::new(module_name)
                .map_err(|e| anyhow::anyhow!("Invalid module name: {}", e))?,
        );

        let module_bytes = self
            .resolver
            .get_module(&module_id)
            .map_err(|e| anyhow::anyhow!("Module not found: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("Module not found: {}", module_id))?;

        let module = CompiledModule::deserialize_with_defaults(&module_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize module: {:?}", e))?;

        let deps: Vec<(AccountAddress, String)> = module
            .immediate_dependencies()
            .into_iter()
            .map(|dep| (*dep.address(), dep.name().to_string()))
            .collect();

        Ok(deps)
    }

    /// Disassemble an entire module to bytecode instructions.
    pub fn disassemble_module(
        &self,
        address: &AccountAddress,
        module_name: &str,
    ) -> Result<String> {
        use move_binary_format::CompiledModule;
        use move_command_line_common::files::FileHash;
        use move_disassembler::disassembler::Disassembler;
        use move_ir_types::location::Loc;

        let module_id = move_core_types::language_storage::ModuleId::new(
            *address,
            move_core_types::identifier::Identifier::new(module_name)
                .map_err(|e| anyhow::anyhow!("Invalid module name: {}", e))?,
        );

        let module_bytes = self
            .resolver
            .get_module(&module_id)
            .map_err(|e| anyhow::anyhow!("Module not found: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("Module not found: {}", module_id))?;

        let module = CompiledModule::deserialize_with_defaults(&module_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize module: {:?}", e))?;

        let disasm = Disassembler::from_module(&module, Loc::new(FileHash::empty(), 0, 0))
            .map_err(|e| anyhow::anyhow!("Failed to create disassembler: {:?}", e))?;

        Ok(disasm
            .disassemble()
            .unwrap_or_else(|_| "Disassembly failed".to_string()))
    }

    /// Get a human-readable summary of a module.
    pub fn get_module_summary(
        &self,
        address: &AccountAddress,
        module_name: &str,
    ) -> Result<String> {
        use move_binary_format::CompiledModule;

        let module_id = move_core_types::language_storage::ModuleId::new(
            *address,
            move_core_types::identifier::Identifier::new(module_name)
                .map_err(|e| anyhow::anyhow!("Invalid module name: {}", e))?,
        );

        let module_bytes = self
            .resolver
            .get_module(&module_id)
            .map_err(|e| anyhow::anyhow!("Module not found: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("Module not found: {}", module_id))?;

        let module = CompiledModule::deserialize_with_defaults(&module_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize module: {:?}", e))?;

        let mut summary = String::new();
        summary.push_str(&format!(
            "Module: {}::{}\n",
            address.to_hex_literal(),
            module_name
        ));
        summary.push_str(&format!("Structs: {}\n", module.struct_defs().len()));
        summary.push_str(&format!("Functions: {}\n", module.function_defs().len()));

        // List struct names
        if !module.struct_defs().is_empty() {
            summary.push_str("\nStructs:\n");
            for def in module.struct_defs() {
                let handle = module.datatype_handle_at(def.struct_handle);
                let name = module.identifier_at(handle.name);
                summary.push_str(&format!("  - {}\n", name));
            }
        }

        // List function names
        if !module.function_defs().is_empty() {
            summary.push_str("\nFunctions:\n");
            for def in module.function_defs() {
                let handle = module.function_handle_at(def.function);
                let name = module.identifier_at(handle.name);
                let vis = if def.is_entry { "entry " } else { "" };
                summary.push_str(&format!("  - {}{}\n", vis, name));
            }
        }

        Ok(summary)
    }

    // ========================================================================
    // Event Query APIs
    // ========================================================================

    /// Get all events emitted during this session.
    pub fn get_all_events(&self) -> &[EmittedEvent] {
        &self.all_events
    }

    /// Get events from the last PTB execution.
    pub fn get_last_tx_events(&self) -> &[EmittedEvent] {
        &self.last_tx_events
    }

    /// Get events filtered by type prefix.
    /// The prefix is matched against the event type string (e.g., "0x2::display::").
    pub fn get_events_by_type(&self, type_prefix: &str) -> Vec<&EmittedEvent> {
        self.all_events
            .iter()
            .filter(|e| e.type_tag.to_string().contains(type_prefix))
            .collect()
    }

    /// Clear all captured events.
    /// Useful for isolating tests or starting a fresh event capture.
    pub fn clear_events(&mut self) {
        self.all_events.clear();
        self.last_tx_events.clear();
    }

    /// Get the total count of events captured during this session.
    pub fn event_count(&self) -> usize {
        self.all_events.len()
    }

    // ========================================================================
    // Shared Object Lock Query APIs
    // ========================================================================

    /// Get the lock information for a specific object, if any.
    pub fn get_lock_for_object(&self, object_id: &AccountAddress) -> Option<SharedObjectLock> {
        self.shared_locks.get(object_id).cloned()
    }

    /// List all currently held shared object locks.
    pub fn list_shared_locks(&self) -> Vec<SharedObjectLock> {
        self.shared_locks.values().cloned().collect()
    }

    /// Check if a specific object has a lock held.
    pub fn is_object_locked(&self, object_id: &AccountAddress) -> bool {
        self.shared_locks.contains_key(object_id)
    }

    /// Get the count of currently held locks.
    pub fn lock_count(&self) -> usize {
        self.shared_locks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_environment() {
        let env = SimulationEnvironment::new();
        assert!(env.is_ok());
    }

    #[test]
    fn test_create_coin() {
        let mut env = SimulationEnvironment::new().unwrap();
        let coin_id = env.create_coin("0x2::sui::SUI", 1_000_000_000);
        assert!(coin_id.is_ok());

        let id = coin_id.unwrap();
        let obj = env.get_object(&id);
        assert!(obj.is_some());
        assert_eq!(obj.unwrap().bcs_bytes.len(), 40); // 32 (UID) + 8 (balance)
    }

    #[test]
    fn test_parse_linker_error() {
        let env = SimulationEnvironment::new().unwrap();
        let error = "Cannot find ModuleId { address: dba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7, name: Identifier(\"usdc\") }";
        let parsed = env.parse_error(error);

        match parsed {
            SimulationError::MissingPackage {
                address, module, ..
            } => {
                assert!(address.contains("dba34672"));
                assert_eq!(module, Some("usdc".to_string()));
            }
            _ => panic!("Expected MissingPackage error"),
        }
    }

    #[test]
    fn test_parse_abort_error() {
        let env = SimulationEnvironment::new().unwrap();
        let error = "VMError { major_status: ABORTED, sub_status: Some(202), message: Some(\"0xf825::sq::csst at offset 13\") }";
        let parsed = env.parse_error(error);

        match parsed {
            SimulationError::ContractAbort {
                abort_code,
                module,
                function,
                ..
            } => {
                assert_eq!(abort_code, 202);
                assert_eq!(module, "sq");
                assert_eq!(function, "csst");
            }
            _ => panic!("Expected ContractAbort error"),
        }
    }

    #[test]
    fn test_parse_type_string_primitives() {
        assert!(matches!(
            SimulationEnvironment::parse_type_string("u8"),
            Some(TypeTag::U8)
        ));
        assert!(matches!(
            SimulationEnvironment::parse_type_string("u64"),
            Some(TypeTag::U64)
        ));
        assert!(matches!(
            SimulationEnvironment::parse_type_string("bool"),
            Some(TypeTag::Bool)
        ));
        assert!(matches!(
            SimulationEnvironment::parse_type_string("address"),
            Some(TypeTag::Address)
        ));
    }

    #[test]
    fn test_parse_type_string_simple_struct() {
        let result = SimulationEnvironment::parse_type_string("0x2::sui::SUI");
        assert!(result.is_some());
        if let Some(TypeTag::Struct(s)) = result {
            assert_eq!(s.module.as_str(), "sui");
            assert_eq!(s.name.as_str(), "SUI");
            assert!(s.type_params.is_empty());
        } else {
            panic!("Expected struct type");
        }
    }

    #[test]
    fn test_parse_type_string_single_generic() {
        let result = SimulationEnvironment::parse_type_string("0x2::coin::Coin<0x2::sui::SUI>");
        assert!(result.is_some());
        if let Some(TypeTag::Struct(s)) = result {
            assert_eq!(s.module.as_str(), "coin");
            assert_eq!(s.name.as_str(), "Coin");
            assert_eq!(s.type_params.len(), 1);
        } else {
            panic!("Expected struct type");
        }
    }

    #[test]
    fn test_parse_type_string_multiple_generics() {
        let result = SimulationEnvironment::parse_type_string(
            "0xabc::pool::Pool<0x2::sui::SUI, 0x2::usdc::USDC>",
        );
        assert!(result.is_some());
        if let Some(TypeTag::Struct(s)) = result {
            assert_eq!(s.module.as_str(), "pool");
            assert_eq!(s.name.as_str(), "Pool");
            assert_eq!(s.type_params.len(), 2);
        } else {
            panic!("Expected struct type");
        }
    }

    #[test]
    fn test_parse_type_string_nested_generics() {
        let result = SimulationEnvironment::parse_type_string(
            "0x2::option::Option<0x2::coin::Coin<0x2::sui::SUI>>",
        );
        assert!(result.is_some());
        if let Some(TypeTag::Struct(s)) = result {
            assert_eq!(s.name.as_str(), "Option");
            assert_eq!(s.type_params.len(), 1);
            // Check nested type
            if let TypeTag::Struct(inner) = &s.type_params[0] {
                assert_eq!(inner.name.as_str(), "Coin");
                assert_eq!(inner.type_params.len(), 1);
            } else {
                panic!("Expected nested struct type");
            }
        } else {
            panic!("Expected struct type");
        }
    }

    #[test]
    fn test_parse_type_string_vector() {
        let result = SimulationEnvironment::parse_type_string("vector<u8>");
        assert!(matches!(result, Some(TypeTag::Vector(_))));

        let result = SimulationEnvironment::parse_type_string("vector<0x2::sui::SUI>");
        assert!(matches!(result, Some(TypeTag::Vector(_))));
    }

    #[test]
    fn test_format_type_tag_roundtrip() {
        let type_str = "0x2::coin::Coin<0x2::sui::SUI>";
        if let Some(parsed) = SimulationEnvironment::parse_type_string(type_str) {
            let formatted = SimulationEnvironment::format_type_tag(&parsed);
            assert!(formatted.contains("coin::Coin"));
            assert!(formatted.contains("sui::SUI"));
        }
    }
}
