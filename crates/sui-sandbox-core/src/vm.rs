//! # VMHarness: Local Bytecode Sandbox Execution Engine
//!
//! This module provides the core execution infrastructure for the Local Bytecode Sandbox.
//! It wraps the Move VM to enable offline type inhabitation testing.
//!
//! ## Key Types
//!
//! - [`VMHarness`]: Main entry point for executing Move functions
//! - [`InMemoryStorage`]: Module resolver with execution tracing
//! - [`ExecutionTrace`]: Records which modules were accessed during execution
//! - [`SimulationConfig`]: Configuration for sandbox behavior
//!
//! ## How It Works
//!
//! 1. Load bytecode from [`LocalModuleResolver`] (framework + target + helper packages)
//! 2. Register native functions via [`build_native_function_table`]
//! 3. Create VM session with [`ObjectRuntime`] extension for dynamic fields
//! 4. Execute functions and capture execution trace
//!
//! The execution trace proves that target package modules were actually loaded,
//! validating that the LLM-generated code exercised the intended code paths.

use anyhow::{anyhow, Result};
use move_binary_format::errors::{PartialVMError, PartialVMResult};
use move_core_types::account_address::AccountAddress;
use move_core_types::gas_algebra::{InternalGas, NumArgs, NumBytes};
use move_core_types::language_storage::{ModuleId, TypeTag};
use move_core_types::resolver::{LinkageResolver, ModuleResolver};
use move_core_types::vm_status::StatusCode;
use move_vm_runtime::move_vm::MoveVM;
use move_vm_runtime::native_extensions::NativeContextExtensions;
use move_vm_types::gas::{GasMeter, SimpleInstruction, UnmeteredGasMeter};
use move_vm_types::views::{TypeView, ValueView};
use parking_lot::Mutex;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::gas::{
    bucketize_computation, AccurateGasMeter, GasParameters, GasSummary, GasSummaryBuilder,
    StorageTracker,
};
use crate::natives::{build_native_function_table, EmittedEvent, MockNativeState};
use crate::object_runtime::{
    ChildFetcherFn, KeyBasedChildFetcherFn, ObjectRuntimeState, SharedObjectRuntime,
    VersionedChildFetcherFn,
};
use crate::resolver::LocalModuleResolver;
use crate::sui_object_runtime;
use sui_protocol_config::ProtocolConfig;

// =============================================================================
// Default Configuration Constants
// =============================================================================

/// Default clock base timestamp: 2024-01-01 00:00:00 UTC
/// Used as the starting point for mock clock in simulations.
const DEFAULT_CLOCK_BASE_MS: u64 = 1_704_067_200_000;

/// Default epoch for simulations.
/// This is an arbitrary value that provides a reasonable starting epoch.
const DEFAULT_EPOCH: u64 = 100;

/// Default gas budget when unlimited gas is requested (50 SUI).
/// Used by strict() configuration and as a reference value.
const DEFAULT_GAS_BUDGET: u64 = 50_000_000_000;

// =============================================================================
// SimulationConfig
// =============================================================================

/// Configuration for the Move VM simulation sandbox.
///
/// `SimulationConfig` controls how the sandbox executes Move code, including
/// transaction context, gas metering, cryptographic verification, and protocol
/// behavior. Each configuration represents a single transaction's execution context.
///
/// # Transaction Identity
///
/// Each transaction needs a unique identity for object ID generation. The sandbox
/// uses `tx_hash` combined with an internal counter to derive globally unique
/// object IDs via `hash(tx_hash || ids_created)`. This matches Sui mainnet behavior
/// where every object created on-chain has a unique address.
///
/// # Gas Model
///
/// The sandbox models Sui's gas system with three configurable parameters:
/// - `reference_gas_price`: The epoch-wide base price set by validators
/// - `gas_price`: The actual price paid (reference + optional tip)
/// - `gas_budget`: Maximum gas units allowed for the transaction
///
/// # Protocol Version
///
/// Feature flags and protocol-specific behavior are gated by `protocol_version`.
/// Code that checks `sui::protocol_config::protocol_version()` will receive this value.
///
/// # Example
///
/// ```rust,ignore
/// use sui_sandbox_core::vm::SimulationConfig;
///
/// // Create a config for transaction replay
/// let config = SimulationConfig::default()
///     .with_sender_address(sender)
///     .with_tx_hash(transaction_digest)
///     .with_epoch(current_epoch)
///     .with_tx_timestamp(timestamp_ms)
///     .with_gas_price(1000)
///     .with_gas_budget(Some(10_000_000_000));
///
/// // Or use strict mode for more realistic behavior
/// let strict_config = SimulationConfig::strict()
///     .with_sender_address(sender);
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SimulationConfig {
    /// Mock crypto natives always pass verification (default: true).
    ///
    /// When true, signature verification, hash checks, etc. always succeed.
    /// Set to false for realistic cryptographic validation.
    pub mock_crypto_pass: bool,

    /// Use an advancing clock (default: true).
    ///
    /// When true, each call to `Clock::timestamp_ms()` returns an incrementing value.
    /// For transaction replay, set `tx_timestamp_ms` and the clock will be frozen
    /// at that value (matching on-chain behavior where clock is fixed per transaction).
    pub advancing_clock: bool,

    /// Use deterministic random values (default: true).
    ///
    /// When true, `sui::random::Random` produces predictable values based on `random_seed`.
    /// This enables reproducible testing of randomness-dependent code.
    pub deterministic_random: bool,

    /// Permissive ownership checks (default: true).
    ///
    /// When true, ownership validations are relaxed for testing.
    /// Set to false for strict ownership enforcement matching mainnet.
    pub permissive_ownership: bool,

    /// Base timestamp for the mock clock in milliseconds.
    ///
    /// Default: 1704067200000 (2024-01-01 00:00:00 UTC)
    pub clock_base_ms: u64,

    /// Seed for deterministic random number generation.
    ///
    /// When `deterministic_random` is true, this seed controls the random sequence.
    /// Same seed produces same random values across executions.
    pub random_seed: [u8; 32],

    /// Transaction sender address.
    ///
    /// This address is returned by `tx_context::sender()` and used for
    /// ownership validation when `permissive_ownership` is false.
    pub sender_address: [u8; 32],

    /// Transaction timestamp in milliseconds (optional).
    ///
    /// If set, this overrides `clock_base_ms` for `TxContext.epoch_timestamp_ms`
    /// and freezes the clock at this value. Essential for accurate transaction replay.
    pub tx_timestamp_ms: Option<u64>,

    /// Current epoch number (default: 100).
    ///
    /// Returned by `tx_context::epoch()`. Can be advanced between transactions
    /// using `advance_epoch()` to simulate epoch progression.
    pub epoch: u64,

    /// Gas budget for execution (default: 50 billion gas units).
    ///
    /// Gas metering is enabled by default to catch gas-sensitive bugs and
    /// match real Sui network behavior. When set, execution will fail with
    /// OutOfGas if the budget is exceeded.
    ///
    /// Use `without_gas_metering()` to disable gas metering for unlimited
    /// execution (e.g., for exploratory testing or debugging).
    ///
    /// Returned by `tx_context::gas_budget()`.
    pub gas_budget: Option<u64>,

    /// Enforce immutable object constraints (default: false).
    ///
    /// When true, mutations to immutable (frozen) objects will fail.
    /// Enable for stricter validation matching mainnet behavior.
    pub enforce_immutability: bool,

    /// Use Sui's actual native implementations (default: false).
    ///
    /// When true, uses sui-move-natives for dynamic field operations,
    /// providing 1:1 parity with on-chain behavior.
    pub use_sui_natives: bool,

    /// Transaction hash/digest for object ID derivation.
    ///
    /// Object IDs are derived using `hash(tx_hash || ids_created)`, ensuring
    /// globally unique addresses across all transactions. Each `SimulationConfig`
    /// instance gets a unique `tx_hash` by default.
    ///
    /// For transaction replay, set this to the actual transaction digest to
    /// generate matching object IDs.
    pub tx_hash: [u8; 32],

    /// Reference gas price for this epoch in MIST (default: 750).
    ///
    /// In Sui, this is determined by validator consensus once per epoch.
    /// Returned by `tx_context::reference_gas_price()`.
    ///
    /// 1 SUI = 1,000,000,000 MIST
    pub reference_gas_price: u64,

    /// Gas price for this transaction in MIST (default: reference_gas_price).
    ///
    /// This is the actual price paid: `reference_gas_price + tip`.
    /// Returned by `tx_context::gas_price()`.
    pub gas_price: u64,

    /// Protocol version (default: 73, matching recent mainnet).
    ///
    /// Controls feature flags and protocol-specific behavior.
    /// Returned by `sui::protocol_config::protocol_version()`.
    /// Features are generally enabled when `protocol_version >= 60`.
    pub protocol_version: u64,

    /// Storage price per unit in MIST (default: 76).
    ///
    /// Used to calculate storage rebates when objects are deleted.
    /// In Sui: storage_units = object_bytes * 100, cost = storage_units * storage_price.
    /// 99% of the storage cost is refundable as rebate when the object is deleted.
    pub storage_price: u64,

    /// Enable object version tracking (default: false).
    ///
    /// When true, the executor will track input object versions and compute
    /// output versions using lamport timestamps. This enables accurate
    /// `TransactionEffects.object_versions` with version change information.
    ///
    /// For this to work properly, object inputs must include version information
    /// (via `ObjectInput` variants with `version: Some(v)`).
    pub track_versions: bool,

    /// Use accurate gas metering (default: true).
    ///
    /// When true, uses Sui's actual gas cost tables with:
    /// - Tiered instruction costs that increase with execution size
    /// - Protocol-accurate native function costs
    /// - Storage I/O tracking (read/write/delete costs)
    /// - Computation bucketization
    ///
    /// This provides ~95%+ accuracy compared to mainnet gas costs.
    /// When false, uses simple hardcoded costs (~30-40% accuracy).
    #[serde(default)]
    pub accurate_gas: bool,
}

/// Default protocol version (mainnet v73 as of late 2025)
pub const DEFAULT_PROTOCOL_VERSION: u64 = 73;

/// Default storage price per unit in MIST (mainnet value as of epoch ~500+)
pub const DEFAULT_STORAGE_PRICE: u64 = 76;

/// Default reference gas price in MIST
pub const DEFAULT_REFERENCE_GAS_PRICE: u64 = 750;

impl Default for SimulationConfig {
    fn default() -> Self {
        // Generate a random tx_hash for each new config to ensure unique object IDs
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut tx_hash = [0u8; 32];
        // Use time-based entropy for uniqueness
        tx_hash[0..16].copy_from_slice(&nanos.to_le_bytes());
        tx_hash[16..32].copy_from_slice(&(nanos.wrapping_mul(31337)).to_le_bytes());

        Self {
            mock_crypto_pass: true,
            advancing_clock: true,
            deterministic_random: true,
            permissive_ownership: true,
            clock_base_ms: DEFAULT_CLOCK_BASE_MS,
            random_seed: [0u8; 32],
            sender_address: [0u8; 32],
            tx_timestamp_ms: None,
            epoch: DEFAULT_EPOCH,
            gas_budget: Some(DEFAULT_GAS_BUDGET), // Enable gas metering by default for Sui parity
            enforce_immutability: false,          // Backwards compatible default
            use_sui_natives: false,               // Use custom natives by default
            tx_hash,                              // Random per instance for unique IDs
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE, // No tip by default
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            storage_price: DEFAULT_STORAGE_PRICE,
            track_versions: false, // Opt-in for backwards compatibility
            accurate_gas: true,    // Default to accurate gas for improved fidelity
        }
    }
}

impl SimulationConfig {
    /// Create a new default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a config with Sui's actual natives enabled.
    /// This provides 1:1 parity with on-chain behavior for dynamic field operations.
    pub fn with_sui_natives() -> Self {
        Self {
            use_sui_natives: true,
            ..Default::default()
        }
    }

    /// Create a strict configuration (more realistic behavior).
    pub fn strict() -> Self {
        let default = Self::default();
        Self {
            mock_crypto_pass: false,
            advancing_clock: true,
            deterministic_random: true,
            permissive_ownership: false,
            clock_base_ms: DEFAULT_CLOCK_BASE_MS,
            random_seed: [0u8; 32],
            sender_address: [0u8; 32],
            tx_timestamp_ms: None,
            epoch: DEFAULT_EPOCH,
            gas_budget: Some(DEFAULT_GAS_BUDGET),
            enforce_immutability: true, // Strict mode enforces immutability
            use_sui_natives: false,     // Backwards compatible
            tx_hash: default.tx_hash,   // Keep unique tx_hash
            reference_gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            gas_price: DEFAULT_REFERENCE_GAS_PRICE,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            storage_price: DEFAULT_STORAGE_PRICE,
            track_versions: false, // Opt-in feature
            accurate_gas: true,    // Strict mode uses accurate gas
        }
    }

    /// Builder method: set mock_crypto_pass.
    pub fn with_mock_crypto(mut self, pass: bool) -> Self {
        self.mock_crypto_pass = pass;
        self
    }

    /// Builder method: set clock base time.
    pub fn with_clock_base(mut self, ms: u64) -> Self {
        self.clock_base_ms = ms;
        self
    }

    /// Builder method: set random seed.
    pub fn with_random_seed(mut self, seed: [u8; 32]) -> Self {
        self.random_seed = seed;
        self
    }

    /// Builder method: set epoch number.
    pub fn with_epoch(mut self, epoch: u64) -> Self {
        self.epoch = epoch;
        self
    }

    /// Builder method: set gas budget.
    ///
    /// Gas metering is enabled by default with a budget of 50 billion gas units.
    /// Use `without_gas_metering()` to disable gas metering entirely.
    pub fn with_gas_budget(mut self, budget: Option<u64>) -> Self {
        self.gas_budget = budget;
        self
    }

    /// Builder method: disable gas metering entirely.
    ///
    /// This allows unlimited gas consumption, useful for:
    /// - Testing without gas constraints
    /// - Running exploratory simulations
    /// - Debugging gas-unrelated issues
    ///
    /// Note: Disabling gas metering means gas-sensitive bugs won't be caught.
    /// For production testing, prefer keeping gas metering enabled.
    pub fn without_gas_metering(mut self) -> Self {
        self.gas_budget = None;
        self
    }

    /// Builder method: enable/disable immutability enforcement.
    pub fn with_immutability_enforcement(mut self, enforce: bool) -> Self {
        self.enforce_immutability = enforce;
        self
    }

    /// Builder method: set sender address for transaction context.
    /// This address is used in TxContext and for ownership validation.
    pub fn with_sender(mut self, sender: [u8; 32]) -> Self {
        self.sender_address = sender;
        self
    }

    /// Builder method: set sender address from AccountAddress.
    pub fn with_sender_address(mut self, sender: AccountAddress) -> Self {
        self.sender_address = sender.into_bytes();
        self
    }

    /// Builder method: set transaction timestamp in milliseconds.
    pub fn with_tx_timestamp(mut self, timestamp_ms: u64) -> Self {
        self.tx_timestamp_ms = Some(timestamp_ms);
        self
    }

    /// Builder method: set transaction hash/digest.
    /// This should be unique per transaction to ensure globally unique object IDs.
    pub fn with_tx_hash(mut self, tx_hash: [u8; 32]) -> Self {
        self.tx_hash = tx_hash;
        self
    }

    /// Builder method: set reference gas price.
    pub fn with_reference_gas_price(mut self, rgp: u64) -> Self {
        self.reference_gas_price = rgp;
        self
    }

    /// Builder method: set gas price (reference + tip).
    pub fn with_gas_price(mut self, price: u64) -> Self {
        self.gas_price = price;
        self
    }

    /// Builder method: set protocol version.
    pub fn with_protocol_version(mut self, version: u64) -> Self {
        self.protocol_version = version;
        self
    }

    /// Builder method: set storage price per unit.
    ///
    /// This is used to calculate storage rebates when objects are deleted.
    /// Default is 76 MIST per storage unit (mainnet value as of epoch ~500+).
    pub fn with_storage_price(mut self, price: u64) -> Self {
        self.storage_price = price;
        self
    }

    /// Builder method: enable accurate gas metering.
    ///
    /// When enabled, uses Sui's actual gas cost tables with:
    /// - Tiered instruction costs that increase with execution size
    /// - Protocol-accurate native function costs
    /// - Storage I/O tracking (read/write/delete costs)
    /// - Computation bucketization
    ///
    /// This provides ~95%+ accuracy compared to mainnet gas costs.
    pub fn with_accurate_gas(mut self, enabled: bool) -> Self {
        self.accurate_gas = enabled;
        self
    }

    /// Enable or disable Sui's actual native implementations.
    ///
    /// When enabled, uses sui-move-natives for dynamic field operations,
    /// providing 1:1 parity with on-chain behavior. Required for accurate
    /// gas metering of native function calls.
    pub fn with_use_sui_natives(mut self, enabled: bool) -> Self {
        self.use_sui_natives = enabled;
        self
    }

    /// Advance the epoch by a given amount (mutates in place).
    pub fn advance_epoch(&mut self, by: u64) {
        self.epoch = self.epoch.saturating_add(by);
    }
}

/// Tracks which modules are accessed during VM execution.
/// This allows us to verify that target package modules were actually loaded/executed.
#[derive(Debug, Clone, Default)]
pub struct ExecutionTrace {
    /// Module IDs that were accessed during execution (via get_module calls)
    pub modules_accessed: BTreeSet<ModuleId>,
}

impl ExecutionTrace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a specific package was accessed during execution.
    ///
    /// This is useful for verifying that target package code was actually executed
    /// (not just framework calls). Used in trace analysis for benchmarking.
    pub fn accessed_package(&self, addr: &AccountAddress) -> bool {
        self.modules_accessed.iter().any(|id| id.address() == addr)
    }

    /// Get all modules accessed from a specific package.
    ///
    /// Returns the subset of accessed modules that belong to the given package address.
    /// Useful for detailed trace analysis during debugging.
    pub fn modules_from_package(&self, addr: &AccountAddress) -> Vec<&ModuleId> {
        self.modules_accessed
            .iter()
            .filter(|id| id.address() == addr)
            .collect()
    }
}

/// A return value with its type information.
#[derive(Debug, Clone, Default)]
pub struct TypedReturnValue {
    /// The BCS-serialized return value bytes.
    pub bytes: Vec<u8>,
    /// The type tag of this return value (if known).
    pub type_tag: Option<TypeTag>,
}

impl TypedReturnValue {
    /// Create a new typed return value.
    pub fn new(bytes: Vec<u8>, type_tag: Option<TypeTag>) -> Self {
        Self { bytes, type_tag }
    }
}

/// Result of executing a Move function, including return values and mutable reference outputs.
#[derive(Debug, Clone, Default)]
pub struct ExecutionOutput {
    /// Return values from the function (BCS bytes with optional type info).
    pub return_values: Vec<TypedReturnValue>,
    /// Mutable reference outputs: (argument_index, new_bytes, optional_type).
    /// These are the updated values for arguments passed as &mut.
    /// The argument index is u8 (LocalIndex from Move VM).
    pub mutable_ref_outputs: Vec<(u8, Vec<u8>, Option<TypeTag>)>,
    /// Estimated gas used for this execution.
    /// This is a simplified estimation based on:
    /// - Base cost per function call
    /// - Cost per byte of arguments
    /// - Cost per byte of return values
    pub gas_used: u64,
}

// =============================================================================
// Structured Error Types
// =============================================================================

/// Structured abort information extracted directly from VMError.
///
/// This provides precise error information without fragile string parsing:
/// - `abort_code`: The exact abort code from `VMError::sub_status()`
/// - `location`: Module where abort occurred from `VMError::location()`
/// - `offsets`: Function and instruction offset from `VMError::offsets()`
/// - `stack_trace`: Full call stack from `VMError::exec_state()`
///
/// This mirrors Sui's `ExecutionFailureStatus::MoveAbort` structure.
#[derive(Debug, Clone)]
pub struct StructuredAbortInfo {
    /// The abort code (from `VMError::sub_status()`).
    pub abort_code: u64,
    /// Module ID where the abort occurred (from `VMError::location()`).
    pub module_id: Option<ModuleId>,
    /// Function definition index within the module.
    pub function_index: u16,
    /// Bytecode instruction offset within the function.
    pub instruction_offset: u16,
    /// Resolved function name (looked up from bytecode).
    pub function_name: Option<String>,
    /// Full stack trace if available (from `VMError::exec_state()`).
    /// Each entry is (module_id, function_index, instruction_offset).
    pub stack_trace: Vec<(ModuleId, u16, u16)>,
}

impl StructuredAbortInfo {
    /// Create from a VMError, extracting all structured information.
    ///
    /// Returns None if the error is not an abort (i.e., major_status != ABORTED).
    pub fn from_vm_error(error: &move_binary_format::errors::VMError) -> Option<Self> {
        use move_binary_format::errors::Location;
        use move_core_types::vm_status::StatusCode;

        // Only extract abort info if this is actually an abort
        if error.major_status() != StatusCode::ABORTED {
            return None;
        }

        let abort_code = error.sub_status()?;

        // Extract module ID from location
        let module_id = match error.location() {
            Location::Module(id) => Some(id.clone()),
            Location::Undefined => None,
        };

        // Extract function and instruction offsets
        let (function_index, instruction_offset) = error
            .offsets()
            .first()
            .map(|(f, i)| (f.0, *i))
            .unwrap_or((0, 0));

        // Extract stack trace if available
        let stack_trace = error
            .exec_state()
            .map(|state| {
                state
                    .stack_trace()
                    .iter()
                    .map(|(mod_id, func_idx, offset)| (mod_id.clone(), func_idx.0, *offset))
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            abort_code,
            module_id,
            function_index,
            instruction_offset,
            function_name: None, // Resolved later via bytecode lookup
            stack_trace,
        })
    }

    /// Resolve the function name from bytecode.
    ///
    /// Call this after creation to look up the function name from the compiled module.
    pub fn resolve_function_name<R, E>(&mut self, resolver: &R)
    where
        R: ModuleResolver<Error = E>,
        E: std::fmt::Debug,
    {
        let Some(module_id) = &self.module_id else {
            return;
        };

        // Try to load the module and look up the function name
        // Note: get_module returns Result<Option<Vec<u8>>, E>
        if let Ok(Some(module_bytes)) = resolver.get_module(module_id) {
            if let Ok(module) =
                move_binary_format::CompiledModule::deserialize_with_defaults(&module_bytes)
            {
                // Look up function definition
                if let Some(func_def) = module.function_defs.get(self.function_index as usize) {
                    let func_handle = &module.function_handles[func_def.function.0 as usize];
                    self.function_name =
                        Some(module.identifiers[func_handle.name.0 as usize].to_string());
                }
            }
        }
    }
}

/// Structured error information from VM execution.
///
/// This captures all error details from the Move VM in a structured form,
/// avoiding the need for fragile string parsing.
#[derive(Debug, Clone)]
pub struct StructuredVMError {
    /// The major status code (e.g., ABORTED, OUT_OF_GAS, TYPE_MISMATCH).
    pub major_status: move_core_types::vm_status::StatusCode,
    /// Optional sub-status (abort code for ABORTED status).
    pub sub_status: Option<u64>,
    /// Optional error message from the VM.
    pub message: Option<String>,
    /// Abort-specific information (only populated if major_status == ABORTED).
    pub abort_info: Option<StructuredAbortInfo>,
}

impl StructuredVMError {
    /// Create from a VMError, extracting all structured information.
    pub fn from_vm_error(error: &move_binary_format::errors::VMError) -> Self {
        Self {
            major_status: error.major_status(),
            sub_status: error.sub_status(),
            message: error.message().cloned(),
            abort_info: StructuredAbortInfo::from_vm_error(error),
        }
    }
}

/// Result of executing a Move function, which can succeed or fail.
///
/// Unlike the previous approach that immediately converted errors to strings,
/// this preserves the full structured error information from the VM.
#[derive(Debug, Clone)]
pub enum ExecutionResult {
    /// Execution succeeded with output values.
    Success(ExecutionOutput),
    /// Execution failed with structured error information.
    Failure {
        /// Structured error details from the VM.
        error: StructuredVMError,
        /// String representation for backwards compatibility and display.
        error_message: String,
    },
}

impl ExecutionResult {
    /// Returns true if execution succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self, ExecutionResult::Success(_))
    }

    /// Returns the output if successful, None otherwise.
    pub fn output(&self) -> Option<&ExecutionOutput> {
        match self {
            ExecutionResult::Success(output) => Some(output),
            ExecutionResult::Failure { .. } => None,
        }
    }

    /// Returns the structured error if failed, None otherwise.
    pub fn error(&self) -> Option<&StructuredVMError> {
        match self {
            ExecutionResult::Success(_) => None,
            ExecutionResult::Failure { error, .. } => Some(error),
        }
    }

    /// Returns the abort info if this was an abort, None otherwise.
    pub fn abort_info(&self) -> Option<&StructuredAbortInfo> {
        self.error().and_then(|e| e.abort_info.as_ref())
    }

    /// Converts to a Result, consuming self.
    pub fn into_result(self) -> Result<ExecutionOutput> {
        match self {
            ExecutionResult::Success(output) => Ok(output),
            ExecutionResult::Failure { error_message, .. } => Err(anyhow!("{}", error_message)),
        }
    }
}

/// Gas cost constants and utilities for estimation.
///
/// This module provides both hardcoded defaults and integration with Sui's
/// `ProtocolConfig` for accurate gas cost estimation.
///
/// ## Gas Model Overview
///
/// Sui's gas model (as of v2) charges for:
/// - **Computation**: CPU cycles (function calls, bytecode execution, natives)
/// - **Storage**: Object reads, writes, and deletions (per-byte costs)
/// - **Transaction base cost**: Fixed overhead per transaction
///
/// ## Using ProtocolConfig
///
/// For mainnet-accurate gas estimation, use `GasCostTable::from_protocol_config()`:
/// ```no_run
/// use sui_protocol_config::ProtocolConfig;
/// use sui_sandbox_core::vm::gas_costs::GasCostTable;
///
/// let config = ProtocolConfig::get_for_min_version();
/// let costs = GasCostTable::from_protocol_config(&config);
/// let object_size: u64 = 100;
/// let read_cost = costs.obj_access_cost_read_per_byte * object_size;
/// ```
pub mod gas_costs {
    use sui_protocol_config::ProtocolConfig;

    // ========== Default Constants (fallback values) ==========

    /// Base cost per function call (covers stack frame, dispatch, etc.)
    pub const FUNCTION_CALL_BASE: u64 = 1000;
    /// Cost per byte of input arguments
    pub const INPUT_BYTE: u64 = 10;
    /// Cost per byte of output (return values)
    pub const OUTPUT_BYTE: u64 = 10;
    /// Cost per type argument
    pub const TYPE_ARG: u64 = 100;
    /// Cost for native function call
    pub const NATIVE_CALL: u64 = 500;
    /// Cost per byte for object storage
    pub const STORAGE_BYTE: u64 = 50;
    /// Cost for object creation
    pub const OBJECT_CREATE: u64 = 2000;
    /// Cost for object mutation
    pub const OBJECT_MUTATE: u64 = 1000;
    /// Cost for object deletion
    pub const OBJECT_DELETE: u64 = 500;

    // ========== ProtocolConfig-based Gas Cost Table ==========

    /// Gas cost table derived from Sui's ProtocolConfig.
    ///
    /// This provides mainnet-accurate gas costs when initialized from
    /// the current protocol version's config.
    #[derive(Debug, Clone)]
    pub struct GasCostTable {
        /// Base transaction cost (fixed overhead)
        pub base_tx_cost_fixed: u64,
        /// Additional fixed cost for package publish
        pub package_publish_cost_fixed: u64,
        /// Cost per byte of transaction data
        pub base_tx_cost_per_byte: u64,
        /// Cost per byte for package publish data
        pub package_publish_cost_per_byte: u64,
        /// Cost per byte for reading an object
        pub obj_access_cost_read_per_byte: u64,
        /// Cost per byte for mutating an object
        pub obj_access_cost_mutate_per_byte: u64,
        /// Cost per byte for deleting an object
        pub obj_access_cost_delete_per_byte: u64,
        /// Cost per byte for type verification
        pub obj_access_cost_verify_per_byte: u64,
        /// Refundable cost per byte of object data
        pub obj_data_cost_refundable: u64,
        /// Non-refundable cost per object (metadata)
        pub obj_metadata_cost_non_refundable: u64,
        /// Storage rebate rate (basis points, e.g., 9900 = 99%)
        pub storage_rebate_rate: u64,
        /// Maximum gas budget for a transaction
        pub max_tx_gas: u64,
        /// Maximum gas price
        pub max_gas_price: u64,
        /// Maximum computation gas bucket
        pub max_gas_computation_bucket: u64,
        /// Gas model version (affects charging behavior)
        pub gas_model_version: u64,
    }

    impl Default for GasCostTable {
        fn default() -> Self {
            // These defaults match Sui's initial protocol config (v1)
            Self {
                base_tx_cost_fixed: 110_000,
                package_publish_cost_fixed: 1_000,
                base_tx_cost_per_byte: 0,
                package_publish_cost_per_byte: 80,
                obj_access_cost_read_per_byte: 15,
                obj_access_cost_mutate_per_byte: 40,
                obj_access_cost_delete_per_byte: 40,
                obj_access_cost_verify_per_byte: 200,
                obj_data_cost_refundable: 100,
                obj_metadata_cost_non_refundable: 50,
                storage_rebate_rate: 9900, // 99%
                max_tx_gas: super::DEFAULT_GAS_BUDGET,
                max_gas_price: 100_000,
                max_gas_computation_bucket: 5_000_000,
                gas_model_version: 1,
            }
        }
    }

    impl GasCostTable {
        /// Create a gas cost table from Sui's ProtocolConfig.
        ///
        /// This ensures gas estimates match mainnet behavior for the
        /// specified protocol version.
        pub fn from_protocol_config(config: &ProtocolConfig) -> Self {
            // Note: ProtocolConfig getters return u64 directly, not Option<u64>
            Self {
                base_tx_cost_fixed: config.base_tx_cost_fixed(),
                package_publish_cost_fixed: config.package_publish_cost_fixed(),
                base_tx_cost_per_byte: config.base_tx_cost_per_byte(),
                package_publish_cost_per_byte: config.package_publish_cost_per_byte(),
                obj_access_cost_read_per_byte: config.obj_access_cost_read_per_byte(),
                obj_access_cost_mutate_per_byte: config.obj_access_cost_mutate_per_byte(),
                obj_access_cost_delete_per_byte: config.obj_access_cost_delete_per_byte(),
                obj_access_cost_verify_per_byte: config.obj_access_cost_verify_per_byte(),
                obj_data_cost_refundable: config.obj_data_cost_refundable(),
                obj_metadata_cost_non_refundable: config.obj_metadata_cost_non_refundable(),
                storage_rebate_rate: config.storage_rebate_rate(),
                max_tx_gas: config.max_tx_gas(),
                max_gas_price: config.max_gas_price(),
                max_gas_computation_bucket: config.max_gas_computation_bucket(),
                gas_model_version: config.gas_model_version(),
            }
        }

        /// Estimate gas cost for reading an object.
        pub fn estimate_read_cost(&self, object_size_bytes: u64) -> u64 {
            object_size_bytes * self.obj_access_cost_read_per_byte
        }

        /// Estimate gas cost for mutating an object.
        pub fn estimate_mutate_cost(&self, object_size_bytes: u64) -> u64 {
            object_size_bytes * self.obj_access_cost_mutate_per_byte
        }

        /// Estimate gas cost for deleting an object.
        pub fn estimate_delete_cost(&self, object_size_bytes: u64) -> u64 {
            object_size_bytes * self.obj_access_cost_delete_per_byte
        }

        /// Estimate storage cost for a new object (rebatable portion).
        pub fn estimate_storage_cost(&self, object_size_bytes: u64) -> u64 {
            object_size_bytes * self.obj_data_cost_refundable
                + self.obj_metadata_cost_non_refundable
        }

        /// Calculate storage rebate for deleting an object.
        ///
        /// The rebate is a percentage of the original storage cost.
        pub fn calculate_storage_rebate(&self, original_storage_cost: u64) -> u64 {
            // storage_rebate_rate is in basis points (e.g., 9900 = 99%)
            (original_storage_cost * self.storage_rebate_rate) / 10_000
        }

        /// Estimate total transaction cost.
        ///
        /// This combines the fixed base cost with per-byte costs and
        /// object access costs.
        pub fn estimate_transaction_cost(
            &self,
            tx_data_size: u64,
            input_objects_read_bytes: u64,
            output_objects_mutate_bytes: u64,
            deleted_objects_bytes: u64,
            computation_cost: u64,
        ) -> u64 {
            let base = self.base_tx_cost_fixed + tx_data_size * self.base_tx_cost_per_byte;
            let read = self.estimate_read_cost(input_objects_read_bytes);
            let mutate = self.estimate_mutate_cost(output_objects_mutate_bytes);
            let delete = self.estimate_delete_cost(deleted_objects_bytes);

            base + read + mutate + delete + computation_cost
        }
    }

    /// Get the current mainnet gas cost table.
    ///
    /// This uses the maximum protocol version's config to get
    /// the most current gas costs.
    ///
    /// # Safety
    /// This uses `get_for_max_version_UNSAFE` which is intended for testing
    /// and simulation, not for production validator use.
    pub fn mainnet_gas_costs() -> GasCostTable {
        let config = ProtocolConfig::get_for_max_version_UNSAFE();
        GasCostTable::from_protocol_config(&config)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_gas_cost_table_default() {
            let table = GasCostTable::default();
            assert_eq!(table.base_tx_cost_fixed, 110_000);
            assert_eq!(table.storage_rebate_rate, 9900);
        }

        #[test]
        fn test_gas_cost_table_from_protocol_config() {
            let table = mainnet_gas_costs();
            // Verify it has reasonable values
            assert!(table.base_tx_cost_fixed > 0);
            assert!(table.max_tx_gas > 0);
            assert!(table.storage_rebate_rate > 0);
        }

        #[test]
        fn test_estimate_read_cost() {
            let table = GasCostTable::default();
            let cost = table.estimate_read_cost(1000); // 1KB object
            assert_eq!(cost, 1000 * 15); // 15 gas per byte
        }

        #[test]
        fn test_estimate_storage_cost() {
            let table = GasCostTable::default();
            let cost = table.estimate_storage_cost(1000); // 1KB object
                                                          // 1000 * 100 (refundable) + 50 (metadata)
            assert_eq!(cost, 100_050);
        }

        #[test]
        fn test_calculate_storage_rebate() {
            let table = GasCostTable::default();
            let original_cost = 100_000;
            let rebate = table.calculate_storage_rebate(original_cost);
            // 99% rebate (9900 basis points)
            assert_eq!(rebate, 99_000);
        }

        #[test]
        fn test_estimate_transaction_cost() {
            let table = GasCostTable::default();
            let cost = table.estimate_transaction_cost(
                100,    // tx data size
                1000,   // input objects read bytes
                500,    // output objects mutate bytes
                200,    // deleted objects bytes
                50_000, // computation cost
            );
            // base: 110_000 + 0*100 = 110_000
            // read: 1000 * 15 = 15_000
            // mutate: 500 * 40 = 20_000
            // delete: 200 * 40 = 8_000
            // computation: 50_000
            // total: 203_000
            assert_eq!(cost, 203_000);
        }
    }
}

/// Estimate gas for a function execution.
fn estimate_gas(
    args: &[Vec<u8>],
    type_args: &[TypeTag],
    return_values: &[Vec<u8>],
    mutable_ref_outputs: &[(u8, Vec<u8>)],
) -> u64 {
    let mut gas = gas_costs::FUNCTION_CALL_BASE;

    // Input bytes
    for arg in args {
        gas += arg.len() as u64 * gas_costs::INPUT_BYTE;
    }

    // Type arguments
    gas += type_args.len() as u64 * gas_costs::TYPE_ARG;

    // Output bytes (return values)
    for ret in return_values {
        gas += ret.len() as u64 * gas_costs::OUTPUT_BYTE;
    }

    // Mutable reference outputs (mutations)
    for (_, bytes) in mutable_ref_outputs {
        gas += bytes.len() as u64 * gas_costs::OUTPUT_BYTE;
        gas += gas_costs::OBJECT_MUTATE;
    }

    gas
}

/// A gas meter that tracks actual gas consumption with a budget.
/// Returns OutOfGas error if budget is exceeded.
pub struct MeteredGasMeter {
    /// Total gas budget
    budget: u64,
    /// Gas consumed so far
    consumed: u64,
}

impl MeteredGasMeter {
    /// Create a new metered gas meter with the given budget.
    pub fn new(budget: u64) -> Self {
        Self {
            budget,
            consumed: 0,
        }
    }

    /// Get the amount of gas consumed.
    pub fn gas_consumed(&self) -> u64 {
        self.consumed
    }

    /// Charge gas, returning OutOfGas if budget exceeded.
    fn charge(&mut self, amount: u64) -> PartialVMResult<()> {
        self.consumed = self.consumed.saturating_add(amount);
        if self.consumed > self.budget {
            Err(PartialVMError::new(StatusCode::OUT_OF_GAS))
        } else {
            Ok(())
        }
    }
}

impl GasMeter for MeteredGasMeter {
    fn charge_simple_instr(&mut self, instr: SimpleInstruction) -> PartialVMResult<()> {
        let cost = match instr {
            // Arithmetic operations - cheap
            SimpleInstruction::Add
            | SimpleInstruction::Sub
            | SimpleInstruction::Mul
            | SimpleInstruction::Div
            | SimpleInstruction::Mod
            | SimpleInstruction::BitOr
            | SimpleInstruction::BitAnd
            | SimpleInstruction::Xor
            | SimpleInstruction::Shl
            | SimpleInstruction::Shr
            | SimpleInstruction::Or
            | SimpleInstruction::And
            | SimpleInstruction::Not
            | SimpleInstruction::Lt
            | SimpleInstruction::Gt
            | SimpleInstruction::Le
            | SimpleInstruction::Ge => 10,

            // Control flow - very cheap
            SimpleInstruction::Nop
            | SimpleInstruction::Ret
            | SimpleInstruction::BrTrue
            | SimpleInstruction::BrFalse
            | SimpleInstruction::Branch => 5,

            // Load constants - varies by size
            SimpleInstruction::LdU8 => 5,
            SimpleInstruction::LdU16 => 5,
            SimpleInstruction::LdU32 => 5,
            SimpleInstruction::LdU64 => 8,
            SimpleInstruction::LdU128 => 10,
            SimpleInstruction::LdU256 => 15,
            SimpleInstruction::LdTrue | SimpleInstruction::LdFalse => 5,

            // Casts - cheap
            SimpleInstruction::CastU8
            | SimpleInstruction::CastU16
            | SimpleInstruction::CastU32
            | SimpleInstruction::CastU64
            | SimpleInstruction::CastU128
            | SimpleInstruction::CastU256 => 8,

            // Reference operations
            SimpleInstruction::FreezeRef => 5,
            SimpleInstruction::MutBorrowLoc | SimpleInstruction::ImmBorrowLoc => 10,
            SimpleInstruction::ImmBorrowField
            | SimpleInstruction::MutBorrowField
            | SimpleInstruction::ImmBorrowFieldGeneric
            | SimpleInstruction::MutBorrowFieldGeneric => 20,

            // Abort is free (execution will stop anyway)
            SimpleInstruction::Abort => 0,
        };
        self.charge(cost)
    }

    fn charge_pop(&mut self, _popped_val: impl ValueView) -> PartialVMResult<()> {
        self.charge(5)
    }

    fn charge_call(
        &mut self,
        _module_id: &ModuleId,
        _func_name: &str,
        args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        let arg_count = args.len() as u64;
        self.charge(gas_costs::FUNCTION_CALL_BASE + arg_count * 50)
    }

    fn charge_call_generic(
        &mut self,
        _module_id: &ModuleId,
        _func_name: &str,
        ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        let ty_count = ty_args.len() as u64;
        let arg_count = args.len() as u64;
        self.charge(gas_costs::FUNCTION_CALL_BASE + ty_count * gas_costs::TYPE_ARG + arg_count * 50)
    }

    fn charge_ld_const(&mut self, size: NumBytes) -> PartialVMResult<()> {
        self.charge(size.into())
    }

    fn charge_ld_const_after_deserialization(
        &mut self,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(20)
    }

    fn charge_copy_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(20)
    }

    fn charge_move_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(10)
    }

    fn charge_store_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(10)
    }

    fn charge_pack(
        &mut self,
        _is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(50 + args.len() as u64 * 10)
    }

    fn charge_unpack(
        &mut self,
        _is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(50 + args.len() as u64 * 10)
    }

    fn charge_variant_switch(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(20)
    }

    fn charge_read_ref(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.charge(30)
    }

    fn charge_write_ref(
        &mut self,
        _new_val: impl ValueView,
        _old_val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(50)
    }

    fn charge_eq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        self.charge(30)
    }

    fn charge_neq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        self.charge(30)
    }

    fn charge_vec_pack<'a>(
        &mut self,
        _ty: impl TypeView + 'a,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(100 + args.len() as u64 * 20)
    }

    fn charge_vec_len(&mut self, _ty: impl TypeView) -> PartialVMResult<()> {
        self.charge(10)
    }

    fn charge_vec_borrow(
        &mut self,
        _is_mut: bool,
        _ty: impl TypeView,
        _is_success: bool,
    ) -> PartialVMResult<()> {
        self.charge(30)
    }

    fn charge_vec_push_back(
        &mut self,
        _ty: impl TypeView,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        self.charge(50)
    }

    fn charge_vec_pop_back(
        &mut self,
        _ty: impl TypeView,
        _val: Option<impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(30)
    }

    fn charge_vec_unpack(
        &mut self,
        _ty: impl TypeView,
        _expect_num_elements: NumArgs,
        elems: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        self.charge(50 + elems.len() as u64 * 10)
    }

    fn charge_vec_swap(&mut self, _ty: impl TypeView) -> PartialVMResult<()> {
        self.charge(40)
    }

    fn charge_native_function(
        &mut self,
        amount: InternalGas,
        _ret_vals: Option<impl ExactSizeIterator<Item = impl ValueView>>,
    ) -> PartialVMResult<()> {
        // Native function cost passed in from the native impl
        self.charge(amount.into())
    }

    fn charge_native_function_before_execution(
        &mut self,
        ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let ty_count = ty_args.len() as u64;
        let arg_count = args.len() as u64;
        self.charge(gas_costs::NATIVE_CALL + ty_count * 50 + arg_count * 30)
    }

    fn charge_drop_frame(
        &mut self,
        locals: impl Iterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let count = locals.count() as u64;
        self.charge(20 + count * 5)
    }

    fn remaining_gas(&self) -> InternalGas {
        InternalGas::new(self.budget.saturating_sub(self.consumed))
    }
}

/// Enum to hold either a metered or unmetered gas meter.
#[allow(clippy::large_enum_variant)]
pub enum GasMeterImpl {
    /// Simple metered gas meter with hardcoded costs (~30-40% accuracy)
    Metered(MeteredGasMeter),
    /// Unmetered (infinite gas)
    Unmetered(UnmeteredGasMeter),
    /// Accurate gas meter using Sui's actual cost tables (~95%+ accuracy)
    Accurate(AccurateGasMeter),
}

impl GasMeterImpl {
    /// Create from config - metered if budget is set, unmetered otherwise.
    pub fn from_config(config: &SimulationConfig) -> Self {
        match (config.gas_budget, config.accurate_gas) {
            (Some(budget), true) => {
                // Use accurate gas metering
                let params = GasParameters::from_protocol_config(
                    &crate::gas::load_protocol_config(config.protocol_version),
                );
                GasMeterImpl::Accurate(AccurateGasMeter::new(budget, config.gas_price, &params))
            }
            (Some(budget), false) => {
                // Use simple metered gas
                GasMeterImpl::Metered(MeteredGasMeter::new(budget))
            }
            (None, _) => {
                // No budget = unmetered
                GasMeterImpl::Unmetered(UnmeteredGasMeter)
            }
        }
    }

    /// Get gas consumed (0 for unmetered).
    pub fn gas_consumed(&self) -> u64 {
        match self {
            GasMeterImpl::Metered(m) => m.gas_consumed(),
            GasMeterImpl::Unmetered(_) => 0,
            GasMeterImpl::Accurate(m) => m.gas_consumed(),
        }
    }

    /// Check if this is using accurate gas metering.
    pub fn is_accurate(&self) -> bool {
        matches!(self, GasMeterImpl::Accurate(_))
    }

    /// Check if this is unmetered (no gas tracking).
    pub fn is_unmetered(&self) -> bool {
        matches!(self, GasMeterImpl::Unmetered(_))
    }
}

impl GasMeter for GasMeterImpl {
    fn charge_simple_instr(&mut self, instr: SimpleInstruction) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_simple_instr(instr),
            GasMeterImpl::Unmetered(m) => m.charge_simple_instr(instr),
            GasMeterImpl::Accurate(m) => m.charge_simple_instr(instr),
        }
    }

    fn charge_pop(&mut self, popped_val: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_pop(popped_val),
            GasMeterImpl::Unmetered(m) => m.charge_pop(popped_val),
            GasMeterImpl::Accurate(m) => m.charge_pop(popped_val),
        }
    }

    fn charge_call(
        &mut self,
        module_id: &ModuleId,
        func_name: &str,
        args: impl ExactSizeIterator<Item = impl ValueView>,
        num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_call(module_id, func_name, args, num_locals),
            GasMeterImpl::Unmetered(m) => m.charge_call(module_id, func_name, args, num_locals),
            GasMeterImpl::Accurate(m) => m.charge_call(module_id, func_name, args, num_locals),
        }
    }

    fn charge_call_generic(
        &mut self,
        module_id: &ModuleId,
        func_name: &str,
        ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        args: impl ExactSizeIterator<Item = impl ValueView>,
        num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => {
                m.charge_call_generic(module_id, func_name, ty_args, args, num_locals)
            }
            GasMeterImpl::Unmetered(m) => {
                m.charge_call_generic(module_id, func_name, ty_args, args, num_locals)
            }
            GasMeterImpl::Accurate(m) => {
                m.charge_call_generic(module_id, func_name, ty_args, args, num_locals)
            }
        }
    }

    fn charge_ld_const(&mut self, size: NumBytes) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_ld_const(size),
            GasMeterImpl::Unmetered(m) => m.charge_ld_const(size),
            GasMeterImpl::Accurate(m) => m.charge_ld_const(size),
        }
    }

    fn charge_ld_const_after_deserialization(
        &mut self,
        val: impl ValueView,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_ld_const_after_deserialization(val),
            GasMeterImpl::Unmetered(m) => m.charge_ld_const_after_deserialization(val),
            GasMeterImpl::Accurate(m) => m.charge_ld_const_after_deserialization(val),
        }
    }

    fn charge_copy_loc(&mut self, val: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_copy_loc(val),
            GasMeterImpl::Unmetered(m) => m.charge_copy_loc(val),
            GasMeterImpl::Accurate(m) => m.charge_copy_loc(val),
        }
    }

    fn charge_move_loc(&mut self, val: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_move_loc(val),
            GasMeterImpl::Unmetered(m) => m.charge_move_loc(val),
            GasMeterImpl::Accurate(m) => m.charge_move_loc(val),
        }
    }

    fn charge_store_loc(&mut self, val: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_store_loc(val),
            GasMeterImpl::Unmetered(m) => m.charge_store_loc(val),
            GasMeterImpl::Accurate(m) => m.charge_store_loc(val),
        }
    }

    fn charge_pack(
        &mut self,
        is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_pack(is_generic, args),
            GasMeterImpl::Unmetered(m) => m.charge_pack(is_generic, args),
            GasMeterImpl::Accurate(m) => m.charge_pack(is_generic, args),
        }
    }

    fn charge_unpack(
        &mut self,
        is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_unpack(is_generic, args),
            GasMeterImpl::Unmetered(m) => m.charge_unpack(is_generic, args),
            GasMeterImpl::Accurate(m) => m.charge_unpack(is_generic, args),
        }
    }

    fn charge_variant_switch(&mut self, val: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_variant_switch(val),
            GasMeterImpl::Unmetered(m) => m.charge_variant_switch(val),
            GasMeterImpl::Accurate(m) => m.charge_variant_switch(val),
        }
    }

    fn charge_read_ref(&mut self, val: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_read_ref(val),
            GasMeterImpl::Unmetered(m) => m.charge_read_ref(val),
            GasMeterImpl::Accurate(m) => m.charge_read_ref(val),
        }
    }

    fn charge_write_ref(
        &mut self,
        new_val: impl ValueView,
        old_val: impl ValueView,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_write_ref(new_val, old_val),
            GasMeterImpl::Unmetered(m) => m.charge_write_ref(new_val, old_val),
            GasMeterImpl::Accurate(m) => m.charge_write_ref(new_val, old_val),
        }
    }

    fn charge_eq(&mut self, lhs: impl ValueView, rhs: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_eq(lhs, rhs),
            GasMeterImpl::Unmetered(m) => m.charge_eq(lhs, rhs),
            GasMeterImpl::Accurate(m) => m.charge_eq(lhs, rhs),
        }
    }

    fn charge_neq(&mut self, lhs: impl ValueView, rhs: impl ValueView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_neq(lhs, rhs),
            GasMeterImpl::Unmetered(m) => m.charge_neq(lhs, rhs),
            GasMeterImpl::Accurate(m) => m.charge_neq(lhs, rhs),
        }
    }

    fn charge_vec_pack<'a>(
        &mut self,
        ty: impl TypeView + 'a,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_pack(ty, args),
            GasMeterImpl::Unmetered(m) => m.charge_vec_pack(ty, args),
            GasMeterImpl::Accurate(m) => m.charge_vec_pack(ty, args),
        }
    }

    fn charge_vec_len(&mut self, ty: impl TypeView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_len(ty),
            GasMeterImpl::Unmetered(m) => m.charge_vec_len(ty),
            GasMeterImpl::Accurate(m) => m.charge_vec_len(ty),
        }
    }

    fn charge_vec_borrow(
        &mut self,
        is_mut: bool,
        ty: impl TypeView,
        is_success: bool,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_borrow(is_mut, ty, is_success),
            GasMeterImpl::Unmetered(m) => m.charge_vec_borrow(is_mut, ty, is_success),
            GasMeterImpl::Accurate(m) => m.charge_vec_borrow(is_mut, ty, is_success),
        }
    }

    fn charge_vec_push_back(
        &mut self,
        ty: impl TypeView,
        val: impl ValueView,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_push_back(ty, val),
            GasMeterImpl::Unmetered(m) => m.charge_vec_push_back(ty, val),
            GasMeterImpl::Accurate(m) => m.charge_vec_push_back(ty, val),
        }
    }

    fn charge_vec_pop_back(
        &mut self,
        ty: impl TypeView,
        val: Option<impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_pop_back(ty, val),
            GasMeterImpl::Unmetered(m) => m.charge_vec_pop_back(ty, val),
            GasMeterImpl::Accurate(m) => m.charge_vec_pop_back(ty, val),
        }
    }

    fn charge_vec_unpack(
        &mut self,
        ty: impl TypeView,
        expect_num_elements: NumArgs,
        elems: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_unpack(ty, expect_num_elements, elems),
            GasMeterImpl::Unmetered(m) => m.charge_vec_unpack(ty, expect_num_elements, elems),
            GasMeterImpl::Accurate(m) => m.charge_vec_unpack(ty, expect_num_elements, elems),
        }
    }

    fn charge_vec_swap(&mut self, ty: impl TypeView) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_vec_swap(ty),
            GasMeterImpl::Unmetered(m) => m.charge_vec_swap(ty),
            GasMeterImpl::Accurate(m) => m.charge_vec_swap(ty),
        }
    }

    fn charge_native_function(
        &mut self,
        amount: InternalGas,
        ret_vals: Option<impl ExactSizeIterator<Item = impl ValueView>>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_native_function(amount, ret_vals),
            GasMeterImpl::Unmetered(m) => m.charge_native_function(amount, ret_vals),
            GasMeterImpl::Accurate(m) => m.charge_native_function(amount, ret_vals),
        }
    }

    fn charge_native_function_before_execution(
        &mut self,
        ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_native_function_before_execution(ty_args, args),
            GasMeterImpl::Unmetered(m) => m.charge_native_function_before_execution(ty_args, args),
            GasMeterImpl::Accurate(m) => m.charge_native_function_before_execution(ty_args, args),
        }
    }

    fn charge_drop_frame(
        &mut self,
        locals: impl Iterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        match self {
            GasMeterImpl::Metered(m) => m.charge_drop_frame(locals),
            GasMeterImpl::Unmetered(m) => m.charge_drop_frame(locals),
            GasMeterImpl::Accurate(m) => m.charge_drop_frame(locals),
        }
    }

    fn remaining_gas(&self) -> InternalGas {
        match self {
            GasMeterImpl::Metered(m) => m.remaining_gas(),
            GasMeterImpl::Unmetered(m) => m.remaining_gas(),
            GasMeterImpl::Accurate(m) => m.remaining_gas(),
        }
    }
}

/// A dynamic field entry that was created or modified during execution.
/// Used to track Table/Bag entries for TransactionEffects.
#[derive(Debug, Clone)]
pub struct DynamicFieldEntry {
    /// Parent object ID (the Table/Bag UID)
    pub parent_id: AccountAddress,
    /// Child object ID (derived from hash of parent + key)
    pub child_id: AccountAddress,
    /// Type of the stored value
    pub value_type: TypeTag,
    /// Whether this is a new field (vs. modified)
    pub is_new: bool,
}

/// Snapshot of dynamic field state from ObjectRuntime.
/// Used to extract child objects after PTB execution.
#[derive(Debug, Clone, Default)]
pub struct DynamicFieldSnapshot {
    /// Child objects stored in the runtime: (parent_id, child_id) -> (type_tag, is_new)
    pub children: Vec<((AccountAddress, AccountAddress), TypeTag)>,
}

/// Create BCS-serialized bytes for a synthetic TxContext.
/// TxContext has the following structure (from sui-framework):
/// ```move
/// struct TxContext has drop {
///     sender: address,
///     tx_hash: vector<u8>,
///     epoch: u64,
///     epoch_timestamp_ms: u64,
///     ids_created: u64,
/// }
/// ```
fn create_synthetic_tx_context_bytes() -> Vec<u8> {
    create_tx_context_bytes_with_config(&SimulationConfig::default())
}

/// Create TxContext bytes with custom configuration for sender and timestamp.
fn create_tx_context_bytes_with_config(config: &SimulationConfig) -> Vec<u8> {
    let mut bytes = Vec::new();
    // sender: address (32 bytes)
    bytes.extend_from_slice(&config.sender_address);
    // tx_hash: vector<u8> (length prefix + 32 bytes)
    bytes.push(32); // ULEB128 length = 32
    bytes.extend_from_slice(&config.tx_hash); // Use actual tx_hash from config
                                              // epoch: u64 (8 bytes, little-endian) - use configured epoch
    bytes.extend_from_slice(&config.epoch.to_le_bytes());
    // epoch_timestamp_ms: u64 (8 bytes, little-endian)
    let timestamp = config.tx_timestamp_ms.unwrap_or(config.clock_base_ms);
    bytes.extend_from_slice(&timestamp.to_le_bytes());
    // ids_created: u64 (8 bytes, little-endian)
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes
}

pub struct InMemoryStorage<'a> {
    module_resolver: &'a LocalModuleResolver,
    /// Shared trace to record module accesses during execution
    trace: Arc<Mutex<ExecutionTrace>>,
}

impl<'a> InMemoryStorage<'a> {
    pub fn new(module_resolver: &'a LocalModuleResolver, restricted: bool) -> Self {
        Self::with_trace(
            module_resolver,
            restricted,
            Arc::new(Mutex::new(ExecutionTrace::new())),
        )
    }

    pub fn with_trace(
        module_resolver: &'a LocalModuleResolver,
        restricted: bool,
        trace: Arc<Mutex<ExecutionTrace>>,
    ) -> Self {
        let mut storage = Self {
            module_resolver,
            trace,
        };

        if restricted {
            storage.populate_restricted_state();
        }

        storage
    }

    /// Populate storage with minimal mock objects for restricted Tier B execution.
    /// These provide deterministic, pre-populated state for common Sui types.
    ///
    /// Note: This is intentionally a no-op. In the current architecture:
    /// - System objects (Clock, TxContext) are synthesized on-demand via `synthesize_clock()`
    ///   and `create_synthetic_tx_context_bytes()`
    /// - Actual object storage happens in `SimulationEnvironment`, not here
    /// - `InMemoryStorage` only handles module resolution for the Move VM
    ///
    /// If future Tier B execution needs pre-populated objects accessible through
    /// the Move VM's resource/object APIs, those would need to be added to the
    /// NativeContextExtensions via SharedObjectRuntime or a similar mechanism.
    fn populate_restricted_state(&mut self) {
        // Intentional no-op - see doc comment above for rationale
    }

    /// Get a reference to the underlying module resolver.
    /// This is useful for looking up function signatures for type resolution.
    pub fn module_resolver(&self) -> &LocalModuleResolver {
        self.module_resolver
    }
}

impl<'a> LinkageResolver for InMemoryStorage<'a> {
    type Error = anyhow::Error;

    fn link_context(&self) -> AccountAddress {
        AccountAddress::ZERO
    }

    fn relocate(&self, module_id: &ModuleId) -> Result<ModuleId, Self::Error> {
        // Check if this address has an alias (for package upgrades)
        if let Some(aliased_addr) = self.module_resolver.get_alias(module_id.address()) {
            let relocated = ModuleId::new(aliased_addr, module_id.name().to_owned());
            return Ok(relocated);
        }
        Ok(module_id.clone())
    }
}

impl<'a> ModuleResolver for InMemoryStorage<'a> {
    type Error = anyhow::Error;

    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        // Track module access (parking_lot::Mutex doesn't poison, so lock() is infallible)
        self.trace.lock().modules_accessed.insert(id.clone());
        self.module_resolver.get_module(id)
    }
}

pub struct VMHarness<'a> {
    vm: MoveVM,
    storage: InMemoryStorage<'a>,
    /// Mock native state for Sui-specific natives (events, clock, etc.)
    native_state: Arc<MockNativeState>,
    /// Shared execution trace for tracking module access
    trace: Arc<Mutex<ExecutionTrace>>,
    /// Simulation configuration (gas settings, clock base, crypto mocks, etc.)
    config: SimulationConfig,
    /// Shared dynamic field state that persists across VM sessions.
    /// Used to track Table/Bag entries throughout PTB execution.
    shared_df_state: Arc<Mutex<ObjectRuntimeState>>,
    /// Optional callback for on-demand child object fetching (ID-based).
    /// Used for fetching dynamic field children from network/archive when not preloaded.
    child_fetcher: Option<Arc<ChildFetcherFn>>,
    /// Optional callback for key-based child object fetching.
    /// Used as fallback when ID-based lookup fails due to package upgrade type mismatches.
    key_based_child_fetcher: Option<Arc<KeyBasedChildFetcherFn>>,
    /// Track all child object IDs accessed during execution (for tracing).
    /// This persists across multiple sessions for the lifetime of the harness.
    accessed_children: Arc<Mutex<std::collections::HashSet<AccountAddress>>>,
    /// Address aliases for package upgrades (bytecode address -> runtime/storage address).
    /// These are passed to SharedObjectRuntime for type tag rewriting in dynamic field ops.
    address_aliases: std::collections::HashMap<AccountAddress, AccountAddress>,
    /// Package versions for version-aware reverse alias selection.
    /// Maps storage_id (normalized hex string) -> version number.
    package_versions: std::collections::HashMap<String, u64>,
    /// Protocol config for Sui natives mode (cached to avoid recreating)
    #[allow(dead_code)]
    protocol_config: Option<ProtocolConfig>,
    /// Sui native extensions (only used when use_sui_natives is true)
    /// This is created lazily when a child fetcher is set.
    sui_extensions: Option<sui_object_runtime::SuiNativeExtensions>,
    /// Optional storage tracker for accurate gas metering.
    /// When enabled, tracks object read/write/delete costs.
    storage_tracker: Option<StorageTracker>,
}

impl<'a> VMHarness<'a> {
    /// Create a new VMHarness with default configuration.
    pub fn new(resolver: &'a LocalModuleResolver, restricted: bool) -> Result<Self> {
        Self::with_config(resolver, restricted, SimulationConfig::default())
    }

    /// Create a new VMHarness with custom configuration.
    pub fn with_config(
        resolver: &'a LocalModuleResolver,
        restricted: bool,
        config: SimulationConfig,
    ) -> Result<Self> {
        // Create mock native state for Sui natives with configured sender
        let mut native_state = MockNativeState::new();
        native_state.sender = AccountAddress::new(config.sender_address);
        native_state.epoch = config.epoch;
        let clock_base = config.tx_timestamp_ms.unwrap_or(config.clock_base_ms);
        native_state.epoch_timestamp_ms = clock_base;
        // Also set the MockClock's base to the configured timestamp
        // This ensures clock::timestamp_ms() returns the correct time
        native_state.clock = crate::natives::MockClock::with_base(clock_base);

        // If accurate gas is enabled, set native function costs
        if config.accurate_gas {
            let protocol_config = crate::gas::load_protocol_config(config.protocol_version);
            let native_costs =
                crate::gas::NativeFunctionCosts::from_protocol_config(&protocol_config);
            native_state.native_costs = Some(native_costs);
        }

        let native_state = Arc::new(native_state);

        // Build native function table based on configuration
        let protocol_config = if config.use_sui_natives {
            Some(ProtocolConfig::get_for_max_version_UNSAFE())
        } else {
            None
        };

        let natives = if let Some(ref pc) = protocol_config {
            // Use Sui's actual native implementations for 1:1 parity
            // Note: This provides correct dynamic field behavior but requires
            // extensions to be set up per-session (ObjectRuntime, TransactionContext, etc.)
            sui_object_runtime::build_sui_native_function_table(pc, false)
        } else {
            // Use our custom mock natives (default, backwards compatible)
            build_native_function_table(native_state.clone())
        };

        let vm = MoveVM::new(natives).map_err(|e| anyhow!("failed to create VM: {:?}", e))?;
        let trace = Arc::new(Mutex::new(ExecutionTrace::new()));
        // Create storage tracker if accurate gas is enabled
        let storage_tracker = if config.accurate_gas {
            let params = GasParameters::from_protocol_config(&crate::gas::load_protocol_config(
                config.protocol_version,
            ));
            Some(StorageTracker::new(&params))
        } else {
            None
        };

        // If using Sui natives, eagerly initialize Sui native extensions so native calls that
        // depend on extensions (e.g. tx_context, object_runtime) don't fail even before a child
        // fetcher is installed.
        //
        // We start with a no-op child fetcher; callers can later override it via
        // `set_child_fetcher` / `set_versioned_child_fetcher`.
        let sui_extensions = if config.use_sui_natives {
            let noop_fetcher: sui_object_runtime::ChildFetchFn =
                std::sync::Arc::new(|_child_id: sui_types::base_types::ObjectID| None);
            let sui_config = sui_object_runtime::SuiRuntimeConfig {
                sender: AccountAddress::new(config.sender_address),
                epoch: config.epoch,
                epoch_timestamp_ms: config.tx_timestamp_ms.unwrap_or(config.clock_base_ms),
                gas_price: config.gas_price,
                gas_budget: config.gas_budget.unwrap_or(DEFAULT_GAS_BUDGET),
                sponsor: None,
                is_metered: config.accurate_gas,
            };
            Some(sui_object_runtime::SuiNativeExtensions::new(
                noop_fetcher,
                sui_config,
            ))
        } else {
            None
        };

        Ok(Self {
            vm,
            storage: InMemoryStorage::with_trace(resolver, restricted, trace.clone()),
            native_state,
            trace,
            config,
            shared_df_state: Arc::new(Mutex::new(ObjectRuntimeState::new())),
            child_fetcher: None,
            key_based_child_fetcher: None,
            accessed_children: Arc::new(Mutex::new(std::collections::HashSet::new())),
            address_aliases: std::collections::HashMap::new(),
            package_versions: std::collections::HashMap::new(),
            protocol_config,
            sui_extensions,
            storage_tracker,
        })
    }

    /// Set address aliases for package upgrades.
    /// Maps bytecode addresses to runtime/storage addresses, enabling correct
    /// type tag rewriting in dynamic field operations for upgraded packages.
    pub fn set_address_aliases(
        &mut self,
        aliases: std::collections::HashMap<AccountAddress, AccountAddress>,
    ) {
        self.address_aliases = aliases;
    }

    /// Generate a fresh object ID using Sui's tx_context derivation.
    /// This uses hash(tx_hash || ids_created) via the MockNativeState.
    pub fn fresh_object_id(&self) -> AccountAddress {
        self.native_state.fresh_id()
    }

    /// Set the ids_created counter used for object ID derivation.
    /// Useful for deterministic replay/testing.
    pub fn set_ids_created(&self, value: u64) {
        self.native_state.set_ids_created(value);
    }

    /// Get the current ids_created counter.
    pub fn ids_created(&self) -> u64 {
        self.native_state.ids_created()
    }

    // ========== Storage Tracking Methods ==========

    /// Track an object read for storage gas metering.
    ///
    /// Call this when an object is loaded as input to a transaction.
    /// The `bytes` parameter should be the BCS-serialized size of the object.
    ///
    /// Returns the computation gas cost for reading this object (in gas units).
    /// This should be added to the PTB's gas_used for accurate computation gas tracking.
    /// On Sui, object reads are charged as computation cost, not storage cost.
    pub fn track_object_read(&mut self, bytes: usize) -> u64 {
        // Track in storage tracker for storage-related accounting
        if let Some(ref mut tracker) = self.storage_tracker {
            tracker.charge_read(bytes);
        }

        // Return computation gas cost for object reads (only when accurate gas is enabled)
        // On Sui, object reads are charged via GasStatus.charge_bytes() which counts as computation
        if self.config.accurate_gas {
            let protocol_config = crate::gas::load_protocol_config(self.config.protocol_version);
            let params = GasParameters::from_protocol_config(&protocol_config);
            // charge_bytes uses: size * cost_per_byte, then divides by 1000 to get gas units
            // The cost is in internal gas units, convert to gas units
            let internal_cost = (bytes as u64).saturating_mul(params.obj_access_cost_read_per_byte);
            // Convert internal gas to gas units (divide by 1000)
            internal_cost / 1000
        } else {
            0
        }
    }

    /// Track an object creation for storage gas metering.
    ///
    /// Call this when a new object is created during execution.
    /// The `bytes` parameter should be the BCS-serialized size of the new object.
    pub fn track_object_create(&mut self, bytes: usize) {
        if let Some(ref mut tracker) = self.storage_tracker {
            tracker.charge_create(bytes);
        }
    }

    /// Track an object mutation for storage gas metering.
    ///
    /// Call this when an existing object is modified during execution.
    /// - `old_bytes`: BCS-serialized size before mutation
    /// - `new_bytes`: BCS-serialized size after mutation
    pub fn track_object_mutate(&mut self, old_bytes: usize, new_bytes: usize) {
        if let Some(ref mut tracker) = self.storage_tracker {
            tracker.charge_mutate(old_bytes, new_bytes);
        }
    }

    /// Track an object deletion for storage gas metering.
    ///
    /// Call this when an object is deleted during execution.
    /// - `bytes`: BCS-serialized size of the deleted object
    /// - `old_storage_cost`: The storage cost that was paid when the object was created (for rebate)
    pub fn track_object_delete(&mut self, bytes: usize, old_storage_cost: Option<u64>) {
        if let Some(ref mut tracker) = self.storage_tracker {
            tracker.charge_delete(bytes, old_storage_cost);
        }
    }

    /// Get the current storage cost summary.
    ///
    /// Returns None if storage tracking is not enabled.
    pub fn storage_summary(&self) -> Option<crate::gas::StorageSummary> {
        self.storage_tracker.as_ref().map(|t| t.summary())
    }

    /// Reset the storage tracker (e.g., between PTB commands).
    pub fn reset_storage_tracker(&mut self) {
        if let Some(ref mut tracker) = self.storage_tracker {
            tracker.reset();
        }
    }

    /// Check if storage tracking is enabled.
    pub fn has_storage_tracking(&self) -> bool {
        self.storage_tracker.is_some()
    }

    /// Get a complete gas summary combining computation and storage costs.
    ///
    /// This method is useful for PTB-level gas accumulation where you want
    /// the total gas cost after executing all commands.
    ///
    /// Returns None if accurate gas metering is not enabled.
    ///
    /// # Arguments
    /// * `gas_meter` - The gas meter used during execution (for computation costs)
    ///
    /// # Example
    /// ```ignore
    /// let mut gas_meter = GasMeterImpl::from_config(&config);
    /// // ... execute PTB commands ...
    /// if let Some(summary) = harness.get_gas_summary(&gas_meter) {
    ///     println!("Total gas: {}", summary.total_cost);
    /// }
    /// ```
    pub fn get_gas_summary(&self, gas_meter: &GasMeterImpl) -> Option<GasSummary> {
        // Only works with accurate gas metering
        if !self.config.accurate_gas {
            return None;
        }

        let computation_gas = gas_meter.gas_consumed();
        let storage_summary = self.storage_tracker.as_ref()?.summary();

        // Apply bucketization based on protocol config
        let protocol_config = crate::gas::load_protocol_config(self.config.protocol_version);
        let params = GasParameters::from_protocol_config(&protocol_config);
        let computation_cost =
            bucketize_computation(computation_gas, params.max_gas_computation_bucket);

        let storage_cost = storage_summary.total_cost();
        let storage_rebate = storage_summary.storage_rebate;
        let non_refundable = storage_cost.saturating_sub(storage_rebate);

        Some(
            GasSummaryBuilder::new()
                .computation_cost(computation_cost)
                .pre_bucket_computation(computation_gas)
                .storage_cost(storage_cost)
                .storage_rebate(storage_rebate)
                .non_refundable_storage_fee(non_refundable)
                .gas_price(self.config.gas_price)
                .reference_gas_price(self.config.gas_price) // Use gas_price as reference for simulation
                .gas_model_version(params.gas_model_version)
                .storage_details(storage_summary)
                .build(),
        )
    }

    /// Get gas consumed so far from a gas meter.
    ///
    /// Returns 0 if the gas meter is unmetered.
    pub fn get_computation_gas(&self, gas_meter: &GasMeterImpl) -> u64 {
        gas_meter.gas_consumed()
    }

    /// Check if accurate gas metering is enabled.
    pub fn has_accurate_gas(&self) -> bool {
        self.config.accurate_gas
    }

    /// Set address aliases with version hints for accurate reverse mapping.
    ///
    /// Prefer this over `set_address_aliases` when replaying transactions involving
    /// upgraded packages with dynamic fields.
    ///
    /// The `aliases` map is storage_id -> original_id (for module resolution).
    /// The `versions` map is storage_id (normalized hex string) -> version number.
    ///
    /// When multiple storage addresses map to the same original (common with package
    /// upgrade chains like v7 -> v12 -> v17), version hints ensure the correct
    /// storage address is used for dynamic field hash computation.
    pub fn set_address_aliases_with_versions(
        &mut self,
        aliases: std::collections::HashMap<AccountAddress, AccountAddress>,
        versions: std::collections::HashMap<String, u64>,
    ) {
        self.address_aliases = aliases;
        self.package_versions = versions;
    }

    /// Set a callback for on-demand child object fetching.
    /// This callback is called when a dynamic field child is requested but not found
    /// in the preloaded set. It receives the child object ID and should return
    /// the object's type and BCS bytes if available.
    pub fn set_child_fetcher(&mut self, fetcher: ChildFetcherFn) {
        // Wrap the fetcher in an Arc upfront so we can share it
        let fetcher_arc = Arc::new(fetcher);
        self.child_fetcher = Some(Arc::clone(&fetcher_arc));

        // If using Sui natives mode, also set up Sui extensions
        if self.config.use_sui_natives {
            // Convert the ChildFetcherFn to a ChildFetchFn for Sui runtime
            // The Sui version needs (type_tag, bytes, version, parent_id)
            // Use the fetcher_arc we created above directly (no unwrap needed)
            let sui_fetcher: sui_object_runtime::ChildFetchFn =
                std::sync::Arc::new(move |child_id: sui_types::base_types::ObjectID| {
                    let addr = AccountAddress::new(child_id.into_bytes());
                    fetcher_arc(addr, addr).map(|(type_tag, bytes)| {
                        // Use default version and parent
                        (type_tag, bytes, 1u64, child_id)
                    })
                });

            // Create Sui runtime config from simulation config
            let sui_config = sui_object_runtime::SuiRuntimeConfig {
                sender: AccountAddress::new(self.config.sender_address),
                epoch: self.config.epoch,
                epoch_timestamp_ms: self
                    .config
                    .tx_timestamp_ms
                    .unwrap_or(self.config.clock_base_ms),
                gas_price: self.config.gas_price,
                gas_budget: self.config.gas_budget.unwrap_or(DEFAULT_GAS_BUDGET),
                sponsor: None,
                // Enable gas metering for native functions when accurate_gas is enabled
                is_metered: self.config.accurate_gas,
            };

            self.sui_extensions = Some(sui_object_runtime::SuiNativeExtensions::new(
                sui_fetcher,
                sui_config,
            ));
        }
    }

    /// Clear the child fetcher callback.
    pub fn clear_child_fetcher(&mut self) {
        self.child_fetcher = None;
    }

    /// Set a versioned child fetcher for transaction replay.
    ///
    /// This is preferred over `set_child_fetcher` for replay scenarios as it provides
    /// correct version information for each child object. The fetcher should return
    /// (type_tag, bcs_bytes, version) for each child.
    pub fn set_versioned_child_fetcher(&mut self, fetcher: VersionedChildFetcherFn) {
        // Create a non-versioned wrapper for the existing child_fetcher field
        let fetcher_arc = Arc::new(fetcher);
        let fetcher_for_legacy = fetcher_arc.clone();
        let legacy_fetcher: ChildFetcherFn = Box::new(move |parent_id, child_id| {
            fetcher_for_legacy(parent_id, child_id)
                .map(|(type_tag, bytes, _version)| (type_tag, bytes))
        });
        self.child_fetcher = Some(Arc::new(legacy_fetcher));

        // If using Sui natives mode, set up extensions with proper versioning
        if self.config.use_sui_natives {
            let sui_fetcher: sui_object_runtime::ChildFetchFn =
                std::sync::Arc::new(move |child_id: sui_types::base_types::ObjectID| {
                    let addr = AccountAddress::new(child_id.into_bytes());
                    fetcher_arc(addr, addr)
                        .map(|(type_tag, bytes, version)| (type_tag, bytes, version, child_id))
                });

            let sui_config = sui_object_runtime::SuiRuntimeConfig {
                sender: AccountAddress::new(self.config.sender_address),
                epoch: self.config.epoch,
                epoch_timestamp_ms: self
                    .config
                    .tx_timestamp_ms
                    .unwrap_or(self.config.clock_base_ms),
                gas_price: self.config.gas_price,
                gas_budget: self.config.gas_budget.unwrap_or(DEFAULT_GAS_BUDGET),
                sponsor: None,
                // Enable gas metering for native functions when accurate_gas is enabled
                is_metered: self.config.accurate_gas,
            };

            self.sui_extensions = Some(sui_object_runtime::SuiNativeExtensions::new(
                sui_fetcher,
                sui_config,
            ));
        }
    }

    /// Set a key-based child fetcher for handling package upgrade type mismatches.
    /// This callback is called when ID-based lookup fails, allowing lookup by
    /// dynamic field key content instead of computed hash.
    pub fn set_key_based_child_fetcher(&mut self, fetcher: KeyBasedChildFetcherFn) {
        self.key_based_child_fetcher = Some(Arc::new(fetcher));
    }

    /// Register an input object for Sui natives mode.
    ///
    /// This is required for objects whose children will be accessed via dynamic fields.
    /// The `owner` must match the object's actual ownership:
    /// - `Owner::AddressOwner(sender)` for owned objects (enables transfers)
    /// - `Owner::Shared { initial_shared_version }` for shared objects
    /// - `Owner::ObjectOwner(parent)` for child objects
    ///
    /// Incorrect ownership causes transfer failures ("sender does not own it").
    pub fn add_sui_input_object(
        &self,
        object_id: AccountAddress,
        version: u64,
        owner: sui_types::object::Owner,
    ) {
        if let Some(ref sui_ext) = self.sui_extensions {
            sui_ext.add_input_object(
                sui_types::base_types::ObjectID::from(object_id),
                version,
                owner,
            );
        }
    }

    /// Register multiple input objects for Sui natives mode.
    pub fn add_sui_input_objects(
        &self,
        objects: &[(AccountAddress, u64, sui_types::object::Owner)],
    ) {
        if let Some(ref sui_ext) = self.sui_extensions {
            for (id, version, owner) in objects {
                sui_ext.add_input_object(
                    sui_types::base_types::ObjectID::from(*id),
                    *version,
                    owner.clone(),
                );
            }
        }
    }

    /// Get all child object IDs that were accessed during execution.
    /// This is useful for tracing which children need to be fetched for replay.
    pub fn get_accessed_children(&self) -> Vec<AccountAddress> {
        self.accessed_children.lock().iter().cloned().collect()
    }

    /// Clear the accessed children tracking (call before a new trace run).
    pub fn clear_accessed_children(&mut self) {
        self.accessed_children.lock().clear();
    }

    /// Get all objects created during MoveCall execution (via transfer/share/freeze natives).
    /// Returns Vec<(object_id, type_tag, bytes, owner)>.
    /// This is used to sync created objects to the PTB executor after each MoveCall.
    pub fn get_created_objects(
        &self,
    ) -> Vec<(
        AccountAddress,
        move_core_types::language_storage::TypeTag,
        Vec<u8>,
        crate::object_runtime::Owner,
    )> {
        self.shared_df_state.lock().all_created_objects()
    }

    /// Get and drain all objects created during MoveCall execution.
    /// Returns Vec<(object_id, type_tag, bytes, owner)> and clears the created objects map.
    /// Use this after each MoveCall to sync created objects to the PTB executor.
    pub fn drain_created_objects(
        &self,
    ) -> Vec<(
        AccountAddress,
        move_core_types::language_storage::TypeTag,
        Vec<u8>,
        crate::object_runtime::Owner,
    )> {
        self.shared_df_state.lock().drain_created_objects()
    }

    /// Get the current simulation configuration.
    pub fn config(&self) -> &SimulationConfig {
        &self.config
    }

    /// Get the execution trace showing which modules were accessed
    pub fn get_trace(&self) -> ExecutionTrace {
        self.trace.lock().clone()
    }

    /// Clear the execution trace (call before each new execution)
    pub fn clear_trace(&self) {
        self.trace.lock().modules_accessed.clear();
    }

    /// Get all events emitted during execution.
    pub fn get_events(&self) -> Vec<EmittedEvent> {
        self.native_state.get_events()
    }

    /// Get events of a specific type emitted during execution.
    pub fn get_events_by_type(&self, type_prefix: &str) -> Vec<EmittedEvent> {
        self.native_state.get_events_by_type(type_prefix)
    }

    /// Clear all emitted events (call between transactions).
    pub fn clear_events(&self) {
        self.native_state.clear_events()
    }

    /// Preload dynamic field state from SimulationEnvironment.
    /// Call this before executing a PTB to provide Table/Bag state.
    pub fn preload_dynamic_fields(
        &self,
        fields: Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)>,
    ) {
        let mut state = self.shared_df_state.lock();
        for ((parent, child), type_tag, bytes) in fields {
            state.add_child(parent, child, type_tag, bytes);
            state.preloaded_children.insert((parent, child));
        }
    }

    /// Extract dynamic field state after PTB execution.
    /// Returns all child objects that were created/modified during execution.
    pub fn extract_dynamic_fields(
        &self,
    ) -> Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)> {
        self.shared_df_state.lock().all_children()
    }

    /// Extract only new dynamic fields (created during this PTB, not preloaded).
    pub fn extract_new_dynamic_fields(
        &self,
    ) -> Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)> {
        self.shared_df_state.lock().new_children()
    }

    /// Clear dynamic field state (call between transactions if needed).
    pub fn clear_dynamic_fields(&self) {
        self.shared_df_state.lock().clear();
    }

    /// Preload pending receives from SimulationEnvironment.
    /// Call this before executing a PTB to enable transfer::receive in Move code.
    pub fn preload_pending_receives(
        &self,
        receives: Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)>,
    ) {
        let mut state = self.shared_df_state.lock();
        for ((recipient, sent), type_tag, bytes) in receives {
            state.add_pending_receive(recipient, sent, type_tag, bytes);
        }
    }

    /// Create VM extensions with a SharedObjectRuntime that syncs with our persistent state.
    /// This allows dynamic field operations to persist across multiple MoveCall executions.
    fn create_extensions(&self) -> NativeContextExtensions<'static> {
        let mut extensions = NativeContextExtensions::default();

        // If using Sui natives mode and we have Sui extensions, use those
        if self.config.use_sui_natives {
            if let Some(ref sui_ext) = self.sui_extensions {
                sui_ext.add_to_extensions(&mut extensions);
                return extensions;
            }
            // Fall through to custom runtime if no Sui extensions set up yet
        }

        // Use SharedObjectRuntime with shared access tracking (default path)
        let mut shared_runtime = SharedObjectRuntime::with_access_tracking(
            self.shared_df_state.clone(),
            self.accessed_children.clone(),
        );

        // If we have a child fetcher, clone the Arc and wrap it in a new Box for the runtime
        if let Some(fetcher_arc) = &self.child_fetcher {
            let fetcher_clone = fetcher_arc.clone();
            shared_runtime.set_child_fetcher(Box::new(move |parent_id, child_id| {
                fetcher_clone(parent_id, child_id)
            }));
        }

        // If we have a key-based child fetcher, set it up as fallback
        if let Some(fetcher_arc) = &self.key_based_child_fetcher {
            let fetcher_clone = fetcher_arc.clone();
            shared_runtime.set_key_based_child_fetcher(Box::new(
                move |parent_id, child_id, key_type, key_bytes| {
                    fetcher_clone(parent_id, child_id, key_type, key_bytes)
                },
            ));
        }

        // Pass address aliases to enable type tag rewriting for upgraded packages.
        // This is critical for correct dynamic field hash computation.
        if !self.address_aliases.is_empty() {
            shared_runtime.set_address_aliases_with_versions(
                self.address_aliases.clone(),
                self.package_versions.clone(),
            );
        }

        extensions.add(shared_runtime);
        extensions
    }

    pub fn execute_entry_function(
        &mut self,
        module: &ModuleId,
        function_name: &move_core_types::identifier::IdentStr,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<()> {
        let extensions = self.create_extensions();
        let mut session = self
            .vm
            .new_session_with_extensions(&self.storage, extensions);

        // Relocate module ID if there's an address alias
        let relocated_module = self
            .storage
            .relocate(module)
            .unwrap_or_else(|_| module.clone());

        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session
                .load_type(&tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = GasMeterImpl::from_config(&self.config);

        session
            .execute_entry_function(
                &relocated_module,
                function_name,
                loaded_ty_args,
                args,
                &mut gas_meter,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;

        Ok(())
    }

    /// Execute a function and return its serialized return values.
    /// This enables constructor chaining where we call a constructor,
    /// capture its return value, and pass it to subsequent functions.
    pub fn execute_function_with_return(
        &mut self,
        module: &ModuleId,
        function_name: &str,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>> {
        let output = self.execute_function_full(module, function_name, ty_args, args)?;
        // Extract just the bytes from TypedReturnValue
        Ok(output.return_values.into_iter().map(|v| v.bytes).collect())
    }

    /// Execute a function and return full output including mutable reference changes.
    /// Use this when you need to track mutations to &mut arguments.
    pub fn execute_function_full(
        &mut self,
        module: &ModuleId,
        function_name: &str,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<ExecutionOutput> {
        let function_name = move_core_types::identifier::Identifier::new(function_name)?;
        let extensions = self.create_extensions();
        let mut session = self
            .vm
            .new_session_with_extensions(&self.storage, extensions);

        // Relocate module ID if there's an address alias
        let relocated_module = self
            .storage
            .relocate(module)
            .unwrap_or_else(|_| module.clone());

        let mut loaded_ty_args = Vec::new();
        for tag in &ty_args {
            let ty = session
                .load_type(tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = GasMeterImpl::from_config(&self.config);

        let serialized_return = session
            .execute_function_bypass_visibility(
                &relocated_module,
                function_name.as_ident_str(),
                loaded_ty_args,
                args.clone(),
                &mut gas_meter,
                None,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        // Get actual gas consumed from the meter
        let metered_gas = gas_meter.gas_consumed();

        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;

        // Extract return values (type tracking is done at PTB level via get_type_from_arg)
        // Note: MoveTypeLayout from the VM contains structural info but not struct names,
        // so we cannot convert it to TypeTag directly.
        let return_values: Vec<TypedReturnValue> = serialized_return
            .return_values
            .into_iter()
            .map(|(bytes, _layout)| TypedReturnValue::new(bytes, None))
            .collect();

        // Extract mutable reference outputs (argument index -> new bytes)
        let mutable_ref_outputs: Vec<(u8, Vec<u8>, Option<TypeTag>)> = serialized_return
            .mutable_reference_outputs
            .into_iter()
            .map(|(idx, bytes, _layout)| (idx, bytes, None))
            .collect();

        // Use metered gas when metering is enabled, otherwise fall back to estimation.
        // IMPORTANT: When using accurate gas metering, we trust the meter's value even if 0.
        // Heuristic estimation is only used for unmetered execution (e.g., when no gas budget).
        let gas_used = if gas_meter.is_unmetered() {
            // No gas metering - use heuristic estimation for backwards compatibility
            let return_bytes: Vec<Vec<u8>> =
                return_values.iter().map(|v| v.bytes.clone()).collect();
            let ref_bytes: Vec<(u8, Vec<u8>)> = mutable_ref_outputs
                .iter()
                .map(|(idx, bytes, _)| (*idx, bytes.clone()))
                .collect();
            estimate_gas(&args, &ty_args, &return_bytes, &ref_bytes)
        } else {
            // Gas metering is enabled - use the metered value (accurate or simple)
            metered_gas
        };

        Ok(ExecutionOutput {
            return_values,
            mutable_ref_outputs,
            gas_used,
        })
    }

    /// Execute a function and return structured error information on failure.
    ///
    /// Unlike `execute_function_full` which converts errors to anyhow strings,
    /// this method preserves the full `VMError` structure, enabling precise
    /// abort code extraction without string parsing.
    ///
    /// Use this when you need:
    /// - Exact abort codes (from `VMError::sub_status()`)
    /// - Precise abort locations (module, function, instruction offset)
    /// - Full stack traces
    ///
    /// # Returns
    /// - `ExecutionResult::Success(output)` on successful execution
    /// - `ExecutionResult::Failure { error, error_message }` on failure
    pub fn execute_function_with_structured_error(
        &mut self,
        module: &ModuleId,
        function_name: &str,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> ExecutionResult {
        let function_name_ident = match move_core_types::identifier::Identifier::new(function_name)
        {
            Ok(id) => id,
            Err(e) => {
                return ExecutionResult::Failure {
                    error: StructuredVMError {
                        major_status: StatusCode::UNKNOWN_INVARIANT_VIOLATION_ERROR,
                        sub_status: None,
                        message: Some(format!("invalid function name: {}", e)),
                        abort_info: None,
                    },
                    error_message: format!("invalid function name '{}': {}", function_name, e),
                }
            }
        };

        let extensions = self.create_extensions();
        let mut session = self
            .vm
            .new_session_with_extensions(&self.storage, extensions);

        // Relocate module ID if there's an address alias.
        // This is necessary because the VM's function resolution doesn't use LinkageResolver
        // for the target module, only for dependencies during module loading.
        let relocated_module = self
            .storage
            .relocate(module)
            .unwrap_or_else(|_| module.clone());

        // Load type arguments
        let mut loaded_ty_args = Vec::new();
        for tag in &ty_args {
            match session.load_type(tag) {
                Ok(ty) => loaded_ty_args.push(ty),
                Err(e) => {
                    let structured = StructuredVMError::from_vm_error(&e);
                    return ExecutionResult::Failure {
                        error: structured,
                        error_message: format!("load type failed: {:?}", e),
                    };
                }
            }
        }

        let mut gas_meter = GasMeterImpl::from_config(&self.config);

        // Execute the function - this is where we capture VMError directly
        let serialized_return = match session.execute_function_bypass_visibility(
            &relocated_module,
            function_name_ident.as_ident_str(),
            loaded_ty_args,
            args.clone(),
            &mut gas_meter,
            None,
        ) {
            Ok(result) => result,
            Err(vm_error) => {
                // Extract structured error BEFORE converting to string
                let mut structured = StructuredVMError::from_vm_error(&vm_error);

                // Try to resolve function name if we have abort info
                if let Some(ref mut abort_info) = structured.abort_info {
                    abort_info.resolve_function_name(&self.storage);
                }

                let error_message = format!("execution failed: {:?}", vm_error);
                return ExecutionResult::Failure {
                    error: structured,
                    error_message,
                };
            }
        };

        // Get actual gas consumed from the meter
        let metered_gas = gas_meter.gas_consumed();

        // Finish session
        let (result, _store) = session.finish();
        if let Err(vm_error) = result {
            let structured = StructuredVMError::from_vm_error(&vm_error);
            return ExecutionResult::Failure {
                error: structured,
                error_message: format!("session finish failed: {:?}", vm_error),
            };
        }

        // Extract return values
        let return_values: Vec<TypedReturnValue> = serialized_return
            .return_values
            .into_iter()
            .map(|(bytes, _layout)| TypedReturnValue::new(bytes, None))
            .collect();

        // Extract mutable reference outputs
        let mutable_ref_outputs: Vec<(u8, Vec<u8>, Option<TypeTag>)> = serialized_return
            .mutable_reference_outputs
            .into_iter()
            .map(|(idx, bytes, _layout)| (idx, bytes, None))
            .collect();

        // Use metered gas when metering is enabled, otherwise fall back to estimation.
        // IMPORTANT: When using accurate gas metering, we trust the meter's value even if 0.
        // Heuristic estimation is only used for unmetered execution (e.g., when no gas budget).
        let gas_used = if gas_meter.is_unmetered() {
            // No gas metering - use heuristic estimation for backwards compatibility
            let return_bytes: Vec<Vec<u8>> =
                return_values.iter().map(|v| v.bytes.clone()).collect();
            let ref_bytes: Vec<(u8, Vec<u8>)> = mutable_ref_outputs
                .iter()
                .map(|(idx, bytes, _)| (*idx, bytes.clone()))
                .collect();
            estimate_gas(&args, &ty_args, &return_bytes, &ref_bytes)
        } else {
            // Gas metering is enabled - use the metered value (accurate or simple)
            metered_gas
        };

        ExecutionResult::Success(ExecutionOutput {
            return_values,
            mutable_ref_outputs,
            gas_used,
        })
    }

    pub fn execute_function(
        &mut self,
        module: &ModuleId,
        function_name: &str,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<()> {
        self.execute_function_with_return(module, function_name, ty_args, args)?;
        Ok(())
    }

    /// Execute an entry function with support for synthesizable Sui system params.
    ///
    /// This handles TxContext, Clock, and other system types that can be synthesized
    /// without real on-chain state.
    pub fn execute_entry_function_with_synth(
        &mut self,
        module: &ModuleId,
        function_name: &move_core_types::identifier::IdentStr,
        ty_args: Vec<TypeTag>,
        mut args: Vec<Vec<u8>>,
        synthesizable_params: &[&str],
    ) -> Result<()> {
        // The Sui runtime normally handles TxContext injection automatically for entry functions.
        // We serialize synthetic values and append them to args.
        // Entry functions expect TxContext as the last param (by Sui convention).

        for synth_type in synthesizable_params {
            match *synth_type {
                "TxContext" => {
                    // Append BCS-serialized synthetic TxContext
                    args.push(create_synthetic_tx_context_bytes());
                }
                "Clock" => {
                    // Clock is typically passed by immutable reference (&Clock).
                    // We synthesize Clock bytes with the current timestamp.
                    // Note: This works because Move VM can deserialize the object
                    // from BCS bytes even for reference parameters in entry functions.
                    args.push(self.synthesize_clock()?);
                }
                other => {
                    return Err(anyhow!("unknown synthesizable param type: {}", other));
                }
            }
        }

        // Now execute with the augmented args
        self.execute_entry_function(module, function_name, ty_args, args)
    }

    /// Synthesize TxContext bytes for constructor arg building
    pub fn synthesize_tx_context(&self) -> Result<Vec<u8>> {
        Ok(create_tx_context_bytes_with_config(&self.config))
    }

    /// Synthesize Clock bytes with advancing timestamp from MockClock.
    ///
    /// The Clock struct has: { id: UID, timestamp_ms: u64 }
    /// Each call to this function advances the mock clock.
    pub fn synthesize_clock(&self) -> Result<Vec<u8>> {
        // Clock struct: { id: UID, timestamp_ms: u64 }
        let mut bytes = Vec::new();
        // id: UID (32 bytes) - Clock has a well-known ID on mainnet (0x6)
        let mut clock_id = [0u8; 32];
        clock_id[31] = 0x06; // 0x0...06 is the Clock object ID
        bytes.extend_from_slice(&clock_id);
        // timestamp_ms: u64 - get from MockClock (advances each call)
        let timestamp = self.native_state.clock_timestamp_ms();
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        Ok(bytes)
    }

    /// Get access to the internal MoveVM for advanced session management.
    pub fn vm(&self) -> &MoveVM {
        &self.vm
    }

    /// Get access to the storage for creating sessions.
    pub fn storage(&self) -> &InMemoryStorage<'a> {
        &self.storage
    }
}

/// A PTB execution session that maintains persistent ObjectRuntime state across
/// multiple Move function calls. This enables proper dynamic field support where
/// Table/Bag operations persist within a transaction.
///
/// ## How It Works
///
/// The session uses `SharedObjectRuntime` which wraps the state in `Arc<Mutex>`.
/// For each Move function call, a new VM session is created with a fresh
/// `SharedObjectRuntime` extension pointing to the same shared state. The native
/// functions automatically sync with this shared state.
///
/// ## Usage
///
/// See `examples/` for complete VM session usage patterns.
pub struct PTBSession<'a, 'b> {
    harness: &'a mut VMHarness<'b>,
    /// Shared state that persists across VM sessions
    shared_state: Arc<Mutex<ObjectRuntimeState>>,
}

impl<'a, 'b> PTBSession<'a, 'b> {
    /// Create a new PTB session with a fresh ObjectRuntime.
    pub fn new(harness: &'a mut VMHarness<'b>) -> Self {
        Self {
            harness,
            shared_state: Arc::new(Mutex::new(ObjectRuntimeState::new())),
        }
    }

    /// Create a PTB session with pre-loaded dynamic field state.
    /// This is used to continue with existing Table/Bag state from SimulationEnvironment.
    pub fn with_preloaded_state(
        harness: &'a mut VMHarness<'b>,
        preloaded: Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)>,
    ) -> Result<Self> {
        let mut state = ObjectRuntimeState::new();

        for ((parent, child_id), type_tag, bytes) in preloaded {
            state.add_child(parent, child_id, type_tag, bytes.clone());
            state.preloaded_children.insert((parent, child_id));
        }

        Ok(Self {
            harness,
            shared_state: Arc::new(Mutex::new(state)),
        })
    }

    /// Execute a Move function within this PTB session.
    /// The ObjectRuntime state persists across calls via the shared state.
    pub fn execute_function(
        &mut self,
        module: &ModuleId,
        function_name: &str,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<ExecutionOutput> {
        let function_name = move_core_types::identifier::Identifier::new(function_name)?;

        // Pre-calculate input gas before args/ty_args are consumed
        let input_gas = gas_costs::FUNCTION_CALL_BASE
            + args
                .iter()
                .map(|a| a.len() as u64 * gas_costs::INPUT_BYTE)
                .sum::<u64>()
            + ty_args.len() as u64 * gas_costs::TYPE_ARG;

        // Create a SharedObjectRuntime that references our shared state
        let shared_runtime = SharedObjectRuntime::new(self.shared_state.clone());
        let mut extensions = NativeContextExtensions::default();
        extensions.add(shared_runtime);

        let mut session = self
            .harness
            .vm()
            .new_session_with_extensions(self.harness.storage(), extensions);

        // Relocate module ID if there's an address alias
        let relocated_module = self
            .harness
            .storage()
            .relocate(module)
            .unwrap_or_else(|_| module.clone());

        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session
                .load_type(&tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = GasMeterImpl::from_config(self.harness.config());

        let serialized_return = session
            .execute_function_bypass_visibility(
                &relocated_module,
                function_name.as_ident_str(),
                loaded_ty_args,
                args,
                &mut gas_meter,
                None,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        // Get actual gas consumed
        let metered_gas = gas_meter.gas_consumed();

        // Finish the session
        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;

        // Note: The SharedObjectRuntime has been dropped at this point, but the
        // native functions have been syncing state to self.shared_state throughout
        // execution. So any dynamic field operations are preserved.

        // Extract return values (type tracking is done at PTB level via get_type_from_arg)
        // Note: MoveTypeLayout from the VM contains structural info but not struct names,
        // so we cannot convert it to TypeTag directly.
        let return_values: Vec<TypedReturnValue> = serialized_return
            .return_values
            .into_iter()
            .map(|(bytes, _layout)| TypedReturnValue::new(bytes, None))
            .collect();

        // Extract mutable reference outputs
        let mutable_ref_outputs: Vec<(u8, Vec<u8>, Option<TypeTag>)> = serialized_return
            .mutable_reference_outputs
            .into_iter()
            .map(|(idx, bytes, _layout)| (idx, bytes, None))
            .collect();

        // Use metered gas when metering is enabled, otherwise fall back to estimation.
        // IMPORTANT: When using accurate gas metering, we trust the meter's value even if 0.
        // Heuristic estimation is only used for unmetered execution (e.g., when no gas budget).
        let gas_used: u64 = if gas_meter.is_unmetered() {
            // No gas metering - use heuristic estimation for backwards compatibility
            let output_gas = return_values
                .iter()
                .map(|r| r.bytes.len() as u64 * gas_costs::OUTPUT_BYTE)
                .sum::<u64>()
                + mutable_ref_outputs
                    .iter()
                    .map(|(_, bytes, _)| {
                        bytes.len() as u64 * gas_costs::OUTPUT_BYTE + gas_costs::OBJECT_MUTATE
                    })
                    .sum::<u64>();
            input_gas + output_gas
        } else {
            // Gas metering is enabled - use the metered value (accurate or simple)
            metered_gas
        };

        Ok(ExecutionOutput {
            return_values,
            mutable_ref_outputs,
            gas_used,
        })
    }

    /// Get a reference to the shared state (for inspection during execution).
    pub fn shared_state(&self) -> &Arc<Mutex<ObjectRuntimeState>> {
        &self.shared_state
    }

    /// Finish the PTB session and extract the dynamic field state.
    /// Returns all child objects that were created during this session.
    pub fn finish(self) -> DynamicFieldSnapshot {
        let state = self.shared_state.lock();
        let children = state
            .children
            .iter()
            .map(|(k, (t, _))| (*k, t.clone()))
            .collect();

        DynamicFieldSnapshot { children }
    }

    /// Finish and return both the snapshot and all child bytes.
    /// Use this when you need to sync state back to SimulationEnvironment.
    pub fn finish_with_bytes(
        self,
    ) -> (
        DynamicFieldSnapshot,
        Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)>,
    ) {
        let state = self.shared_state.lock();
        let children: Vec<_> = state
            .children
            .iter()
            .map(|(k, (t, _))| (*k, t.clone()))
            .collect();
        let all_bytes: Vec<_> = state
            .children
            .iter()
            .map(|(k, (t, b))| (*k, t.clone(), b.clone()))
            .collect();

        (DynamicFieldSnapshot { children }, all_bytes)
    }
}

#[cfg(test)]
mod structured_error_tests {
    use super::*;
    use move_binary_format::errors::{Location, PartialVMError};
    use move_core_types::identifier::Identifier;
    use move_core_types::vm_status::StatusCode;

    #[test]
    fn test_structured_abort_info_from_abort_error() {
        // Create a VMError with ABORTED status and sub_status
        let partial = PartialVMError::new(StatusCode::ABORTED)
            .with_sub_status(42)
            .with_message("test abort".to_string());

        let module_id = ModuleId::new(
            AccountAddress::from_hex_literal("0x2").unwrap(),
            Identifier::new("coin").unwrap(),
        );
        let vm_error = partial.finish(Location::Module(module_id.clone()));

        let abort_info = StructuredAbortInfo::from_vm_error(&vm_error);

        assert!(abort_info.is_some());
        let abort_info = abort_info.unwrap();
        assert_eq!(abort_info.abort_code, 42);
        assert_eq!(abort_info.module_id, Some(module_id));
    }

    #[test]
    fn test_structured_abort_info_not_abort() {
        // Create a VMError with non-abort status
        let partial = PartialVMError::new(StatusCode::TYPE_MISMATCH);
        let vm_error = partial.finish(Location::Undefined);

        let abort_info = StructuredAbortInfo::from_vm_error(&vm_error);

        // Should return None for non-abort errors
        assert!(abort_info.is_none());
    }

    #[test]
    fn test_structured_vm_error_from_abort() {
        let partial = PartialVMError::new(StatusCode::ABORTED)
            .with_sub_status(1234)
            .with_message("insufficient balance".to_string());

        let module_id = ModuleId::new(
            AccountAddress::from_hex_literal("0x2").unwrap(),
            Identifier::new("balance").unwrap(),
        );
        let vm_error = partial.finish(Location::Module(module_id));

        let structured = StructuredVMError::from_vm_error(&vm_error);

        assert_eq!(structured.major_status, StatusCode::ABORTED);
        assert_eq!(structured.sub_status, Some(1234));
        assert_eq!(structured.message.as_deref(), Some("insufficient balance"));
        assert!(structured.abort_info.is_some());
        assert_eq!(structured.abort_info.as_ref().unwrap().abort_code, 1234);
    }

    #[test]
    fn test_structured_vm_error_from_non_abort() {
        let partial = PartialVMError::new(StatusCode::OUT_OF_GAS);
        let vm_error = partial.finish(Location::Undefined);

        let structured = StructuredVMError::from_vm_error(&vm_error);

        assert_eq!(structured.major_status, StatusCode::OUT_OF_GAS);
        assert_eq!(structured.sub_status, None);
        assert!(structured.abort_info.is_none());
    }

    #[test]
    fn test_execution_result_success() {
        let output = ExecutionOutput {
            return_values: vec![TypedReturnValue::new(vec![1, 2, 3], None)],
            mutable_ref_outputs: vec![],
            gas_used: 100,
        };

        let result = ExecutionResult::Success(output);

        assert!(result.is_success());
        assert!(result.output().is_some());
        assert!(result.error().is_none());
        assert!(result.abort_info().is_none());
    }

    #[test]
    fn test_execution_result_failure() {
        let partial = PartialVMError::new(StatusCode::ABORTED).with_sub_status(999);
        let vm_error = partial.finish(Location::Undefined);
        let structured = StructuredVMError::from_vm_error(&vm_error);

        let result = ExecutionResult::Failure {
            error: structured,
            error_message: "abort 999".to_string(),
        };

        assert!(!result.is_success());
        assert!(result.output().is_none());
        assert!(result.error().is_some());
        assert!(result.abort_info().is_some());
        assert_eq!(result.abort_info().unwrap().abort_code, 999);
    }

    #[test]
    fn test_execution_result_into_result_success() {
        let output = ExecutionOutput {
            return_values: vec![],
            mutable_ref_outputs: vec![],
            gas_used: 50,
        };

        let result = ExecutionResult::Success(output);
        let converted = result.into_result();

        assert!(converted.is_ok());
        assert_eq!(converted.unwrap().gas_used, 50);
    }

    #[test]
    fn test_execution_result_into_result_failure() {
        let partial = PartialVMError::new(StatusCode::ABORTED).with_sub_status(1);
        let vm_error = partial.finish(Location::Undefined);
        let structured = StructuredVMError::from_vm_error(&vm_error);

        let result = ExecutionResult::Failure {
            error: structured,
            error_message: "test error".to_string(),
        };

        let converted = result.into_result();

        assert!(converted.is_err());
        assert!(converted.unwrap_err().to_string().contains("test error"));
    }
}
