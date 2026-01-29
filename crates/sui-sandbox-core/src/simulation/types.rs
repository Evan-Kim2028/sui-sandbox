//! Core types for the simulation environment.
//!
//! This module contains the fundamental data structures used throughout
//! the simulation: objects, execution results, and serialization types.

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::BTreeMap;

use crate::ptb::TransactionEffects;

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

// ============================================================================
// Coin Metadata
// ============================================================================

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
    pub error: Option<super::SimulationError>,

    /// Raw error message (for debugging).
    pub raw_error: Option<String>,

    /// Index of the command that failed (0-based), if execution failed.
    pub failed_command_index: Option<usize>,

    /// Description of the failed command (e.g., "MoveCall 0x2::coin::split").
    pub failed_command_description: Option<String>,

    /// Number of commands that succeeded before the failure.
    pub commands_succeeded: usize,

    /// Detailed error context for debugging (optional, populated for complex failures).
    pub error_context: Option<crate::error_context::CommandErrorContext>,

    /// Snapshot of execution state at failure time (optional).
    pub state_at_failure: Option<crate::error_context::ExecutionSnapshot>,
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

    /// Optional runtime ownership metadata.
    /// Used for building full Sui objects when available.
    pub owner: Option<crate::object_runtime::Owner>,
}

/// Result of a direct function call.
#[derive(Debug, Clone)]
pub struct FunctionCallResult {
    pub return_values: Vec<Vec<u8>>,
    pub gas_used: u64,
}

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

// ============================================================================
// State Checkpoint Types
// ============================================================================

/// A checkpoint of simulation state for rollback support.
#[derive(Debug, Clone)]
pub struct StateCheckpoint {
    /// Snapshot of all objects.
    pub objects: BTreeMap<AccountAddress, SimulatedObject>,
    /// Snapshot of dynamic fields.
    pub dynamic_fields: BTreeMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)>,
    /// Snapshot of shared locks.
    pub shared_locks: BTreeMap<AccountAddress, super::consensus::SharedObjectLock>,
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
pub fn leb128_encode(mut val: u64) -> Vec<u8> {
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
