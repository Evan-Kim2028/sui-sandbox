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
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{ModuleId, TypeTag};
use move_core_types::resolver::{LinkageResolver, ModuleResolver};
use move_vm_runtime::move_vm::MoveVM;
use move_vm_runtime::native_extensions::NativeContextExtensions;
use move_vm_types::gas::UnmeteredGasMeter;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::benchmark::natives::{build_native_function_table, EmittedEvent, MockNativeState};
use crate::benchmark::object_runtime::{ObjectRuntimeState, SharedObjectRuntime};
use crate::benchmark::resolver::LocalModuleResolver;

/// Configuration for the simulation sandbox.
///
/// This allows customization of how the sandbox behaves, particularly
/// for mocked natives and system state.
#[derive(Debug, Clone)]
pub struct SimulationConfig {
    /// Mock crypto natives always pass verification (default: true).
    /// When true, signature verification, hash checks, etc. always succeed.
    pub mock_crypto_pass: bool,

    /// Use an advancing clock (default: true).
    /// When true, `Clock::timestamp_ms()` returns advancing values.
    pub advancing_clock: bool,

    /// Use deterministic random values (default: true).
    /// When true, `Random` produces predictable values based on seed.
    pub deterministic_random: bool,

    /// Permissive ownership checks (default: true).
    /// When true, ownership validations are relaxed for testing.
    pub permissive_ownership: bool,

    /// Base timestamp for the mock clock in milliseconds (default: 1704067200000 = 2024-01-01).
    pub clock_base_ms: u64,

    /// Seed for deterministic random number generation.
    pub random_seed: [u8; 32],

    /// Transaction sender address (default: 0x0).
    /// This is used when synthesizing TxContext for entry function calls.
    pub sender_address: [u8; 32],

    /// Transaction timestamp in milliseconds (default: None, uses clock_base_ms).
    /// If set, this overrides clock_base_ms for TxContext.epoch_timestamp_ms.
    pub tx_timestamp_ms: Option<u64>,

    /// Current epoch number (default: 100).
    /// This is used in TxContext.epoch and can be advanced between transactions.
    pub epoch: u64,

    /// Gas budget for execution (default: None = unlimited).
    /// When set, execution will fail with OutOfGas if budget is exceeded.
    pub gas_budget: Option<u64>,

    /// Enforce immutable object constraints (default: false for backwards compat).
    /// When true, mutations to immutable objects will fail.
    pub enforce_immutability: bool,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            mock_crypto_pass: true,
            advancing_clock: true,
            deterministic_random: true,
            permissive_ownership: true,
            clock_base_ms: 1704067200000, // 2024-01-01 00:00:00 UTC
            random_seed: [0u8; 32],
            sender_address: [0u8; 32],
            tx_timestamp_ms: None,
            epoch: 100, // Default epoch
            gas_budget: None, // Unlimited by default
            enforce_immutability: false, // Backwards compatible default
        }
    }
}

impl SimulationConfig {
    /// Create a new default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a strict configuration (more realistic behavior).
    pub fn strict() -> Self {
        Self {
            mock_crypto_pass: false,
            advancing_clock: true,
            deterministic_random: true,
            permissive_ownership: false,
            clock_base_ms: 1704067200000,
            random_seed: [0u8; 32],
            sender_address: [0u8; 32],
            tx_timestamp_ms: None,
            epoch: 100,
            gas_budget: Some(50_000_000_000), // 50 SUI default budget
            enforce_immutability: true, // Strict mode enforces immutability
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
    pub fn with_gas_budget(mut self, budget: Option<u64>) -> Self {
        self.gas_budget = budget;
        self
    }

    /// Builder method: enable/disable immutability enforcement.
    pub fn with_immutability_enforcement(mut self, enforce: bool) -> Self {
        self.enforce_immutability = enforce;
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

    /// Check if a specific package was accessed during execution
    #[allow(dead_code)]
    pub fn accessed_package(&self, addr: &AccountAddress) -> bool {
        self.modules_accessed.iter().any(|id| id.address() == addr)
    }

    /// Get all modules accessed from a specific package
    #[allow(dead_code)]
    pub fn modules_from_package(&self, addr: &AccountAddress) -> Vec<&ModuleId> {
        self.modules_accessed
            .iter()
            .filter(|id| id.address() == addr)
            .collect()
    }
}

/// Result of executing a Move function, including return values and mutable reference outputs.
#[derive(Debug, Clone, Default)]
pub struct ExecutionOutput {
    /// Return values from the function (BCS bytes).
    pub return_values: Vec<Vec<u8>>,
    /// Mutable reference outputs: (argument_index, new_bytes).
    /// These are the updated values for arguments passed as &mut.
    /// The argument index is u8 (LocalIndex from Move VM).
    pub mutable_ref_outputs: Vec<(u8, Vec<u8>)>,
    /// Estimated gas used for this execution.
    /// This is a simplified estimation based on:
    /// - Base cost per function call
    /// - Cost per byte of arguments
    /// - Cost per byte of return values
    pub gas_used: u64,
}

/// Gas cost constants for estimation.
/// These are simplified approximations of Sui's actual gas costs.
pub mod gas_costs {
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
    bytes.extend_from_slice(&[0u8; 32]);
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
        // Track module access
        if let Ok(mut trace) = self.trace.lock() {
            trace.modules_accessed.insert(id.clone());
        }
        self.module_resolver.get_module(id)
    }
}

pub struct VMHarness<'a> {
    vm: MoveVM,
    storage: InMemoryStorage<'a>,
    #[allow(dead_code)]
    native_state: Arc<MockNativeState>,
    /// Shared execution trace
    trace: Arc<Mutex<ExecutionTrace>>,
    /// Simulation configuration
    #[allow(dead_code)]
    config: SimulationConfig,
    /// Shared dynamic field state that persists across VM sessions.
    /// Used to track Table/Bag entries throughout PTB execution.
    shared_df_state: Arc<Mutex<ObjectRuntimeState>>,
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
        // Create mock native state for Sui natives
        let native_state = Arc::new(MockNativeState::new());

        // Build native function table with move-stdlib + mock Sui natives
        let natives = build_native_function_table(native_state.clone());

        let vm = MoveVM::new(natives).map_err(|e| anyhow!("failed to create VM: {:?}", e))?;
        let trace = Arc::new(Mutex::new(ExecutionTrace::new()));
        Ok(Self {
            vm,
            storage: InMemoryStorage::with_trace(resolver, restricted, trace.clone()),
            native_state,
            trace,
            config,
            shared_df_state: Arc::new(Mutex::new(ObjectRuntimeState::new())),
        })
    }

    /// Get the current simulation configuration.
    pub fn config(&self) -> &SimulationConfig {
        &self.config
    }

    /// Get the execution trace showing which modules were accessed
    pub fn get_trace(&self) -> ExecutionTrace {
        self.trace.lock().map(|t| t.clone()).unwrap_or_default()
    }

    /// Clear the execution trace (call before each new execution)
    pub fn clear_trace(&self) {
        if let Ok(mut trace) = self.trace.lock() {
            trace.modules_accessed.clear();
        }
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
    pub fn preload_dynamic_fields(&self, fields: Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)>) {
        if let Ok(mut state) = self.shared_df_state.lock() {
            for ((parent, child), type_tag, bytes) in fields {
                state.add_child(parent, child, type_tag, bytes);
                state.preloaded_children.insert((parent, child));
            }
        }
    }

    /// Extract dynamic field state after PTB execution.
    /// Returns all child objects that were created/modified during execution.
    pub fn extract_dynamic_fields(&self) -> Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)> {
        if let Ok(state) = self.shared_df_state.lock() {
            state.all_children()
        } else {
            Vec::new()
        }
    }

    /// Extract only new dynamic fields (created during this PTB, not preloaded).
    pub fn extract_new_dynamic_fields(&self) -> Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)> {
        if let Ok(state) = self.shared_df_state.lock() {
            state.new_children()
        } else {
            Vec::new()
        }
    }

    /// Clear dynamic field state (call between transactions if needed).
    pub fn clear_dynamic_fields(&self) {
        if let Ok(mut state) = self.shared_df_state.lock() {
            state.clear();
        }
    }

    /// Create VM extensions with a SharedObjectRuntime that syncs with our persistent state.
    /// This allows dynamic field operations to persist across multiple MoveCall executions.
    fn create_extensions(&self) -> NativeContextExtensions<'static> {
        let mut extensions = NativeContextExtensions::default();
        // Use SharedObjectRuntime to sync with our persistent dynamic field state
        let shared_runtime = SharedObjectRuntime::new(self.shared_df_state.clone());
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

        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session
                .load_type(&tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = UnmeteredGasMeter;

        session
            .execute_entry_function(module, function_name, loaded_ty_args, args, &mut gas_meter)
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
        Ok(output.return_values)
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

        let mut loaded_ty_args = Vec::new();
        for tag in &ty_args {
            let ty = session
                .load_type(tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = UnmeteredGasMeter;

        let serialized_return = session
            .execute_function_bypass_visibility(
                module,
                function_name.as_ident_str(),
                loaded_ty_args,
                args.clone(),
                &mut gas_meter,
                None,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;

        // Extract return values
        let return_values: Vec<Vec<u8>> = serialized_return
            .return_values
            .into_iter()
            .map(|(bytes, _layout)| bytes)
            .collect();

        // Extract mutable reference outputs (argument index -> new bytes)
        let mutable_ref_outputs: Vec<(u8, Vec<u8>)> = serialized_return
            .mutable_reference_outputs
            .into_iter()
            .map(|(idx, bytes, _layout)| (idx, bytes))
            .collect();

        // Estimate gas used
        let gas_used = estimate_gas(&args, &ty_args, &return_values, &mutable_ref_outputs);

        Ok(ExecutionOutput {
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
/// ```ignore
/// let mut session = PTBSession::new(&mut harness);
///
/// // First call creates a Table and adds an entry
/// session.execute_function(&module_id, "create_table", vec![], vec![])?;
///
/// // Second call can access the Table entries created above
/// session.execute_function(&module_id, "read_table", vec![], vec![])?;
///
/// // Extract dynamic field state for TransactionEffects
/// let df_state = session.finish();
/// ```
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
            + args.iter().map(|a| a.len() as u64 * gas_costs::INPUT_BYTE).sum::<u64>()
            + ty_args.len() as u64 * gas_costs::TYPE_ARG;

        // Create a SharedObjectRuntime that references our shared state
        let shared_runtime = SharedObjectRuntime::new(self.shared_state.clone());
        let mut extensions = NativeContextExtensions::default();
        extensions.add(shared_runtime);

        let mut session = self.harness.vm()
            .new_session_with_extensions(self.harness.storage(), extensions);

        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session
                .load_type(&tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = UnmeteredGasMeter;

        let serialized_return = session
            .execute_function_bypass_visibility(
                module,
                function_name.as_ident_str(),
                loaded_ty_args,
                args,
                &mut gas_meter,
                None,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        // Finish the session
        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;

        // Note: The SharedObjectRuntime has been dropped at this point, but the
        // native functions have been syncing state to self.shared_state throughout
        // execution. So any dynamic field operations are preserved.

        // Extract return values
        let return_values: Vec<Vec<u8>> = serialized_return
            .return_values
            .into_iter()
            .map(|(bytes, _layout)| bytes)
            .collect();

        // Extract mutable reference outputs
        let mutable_ref_outputs: Vec<(u8, Vec<u8>)> = serialized_return
            .mutable_reference_outputs
            .into_iter()
            .map(|(idx, bytes, _layout)| (idx, bytes))
            .collect();

        // Calculate output gas and combine with input gas
        let output_gas: u64 = return_values.iter().map(|r| r.len() as u64 * gas_costs::OUTPUT_BYTE).sum::<u64>()
            + mutable_ref_outputs.iter().map(|(_, bytes)| {
                bytes.len() as u64 * gas_costs::OUTPUT_BYTE + gas_costs::OBJECT_MUTATE
            }).sum::<u64>();
        let gas_used = input_gas + output_gas;

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
        let state = self.shared_state.lock().ok();

        let children = state
            .map(|s| {
                s.children
                    .iter()
                    .map(|(k, (t, _))| (*k, t.clone()))
                    .collect()
            })
            .unwrap_or_default();

        DynamicFieldSnapshot { children }
    }

    /// Finish and return both the snapshot and all child bytes.
    /// Use this when you need to sync state back to SimulationEnvironment.
    pub fn finish_with_bytes(self) -> (DynamicFieldSnapshot, Vec<((AccountAddress, AccountAddress), TypeTag, Vec<u8>)>) {
        let state = self.shared_state.lock().ok();

        let (children, all_bytes) = state
            .map(|s| {
                let children: Vec<_> = s.children
                    .iter()
                    .map(|(k, (t, _))| (*k, t.clone()))
                    .collect();
                let all: Vec<_> = s.children
                    .iter()
                    .map(|(k, (t, b))| (*k, t.clone(), b.clone()))
                    .collect();
                (children, all)
            })
            .unwrap_or_default();

        (DynamicFieldSnapshot { children }, all_bytes)
    }
}
