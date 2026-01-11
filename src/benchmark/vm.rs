use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::{ModuleId, TypeTag};
use move_core_types::resolver::{LinkageResolver, ModuleResolver};
use move_vm_runtime::move_vm::MoveVM;
use move_vm_types::gas::UnmeteredGasMeter;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::benchmark::natives::{build_native_function_table, MockNativeState};
use crate::benchmark::resolver::LocalModuleResolver;

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
        self.modules_accessed.iter().filter(|id| id.address() == addr).collect()
    }
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
    let mut bytes = Vec::new();
    // sender: address (32 bytes, all zeros)
    bytes.extend_from_slice(&[0u8; 32]);
    // tx_hash: vector<u8> (length prefix + 32 bytes)
    bytes.push(32); // ULEB128 length = 32
    bytes.extend_from_slice(&[0u8; 32]);
    // epoch: u64 (8 bytes, little-endian)
    bytes.extend_from_slice(&0u64.to_le_bytes());
    // epoch_timestamp_ms: u64 (8 bytes, little-endian)
    bytes.extend_from_slice(&0u64.to_le_bytes());
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
        Self::with_trace(module_resolver, restricted, Arc::new(Mutex::new(ExecutionTrace::new())))
    }
    
    pub fn with_trace(module_resolver: &'a LocalModuleResolver, restricted: bool, trace: Arc<Mutex<ExecutionTrace>>) -> Self {
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
    fn populate_restricted_state(&mut self) {
        // NOTE: Currently these mocks are created but not stored in a reachable way.
        // This is kept as a placeholder for future state-dependent Tier B logic.
    }
}

impl<'a> LinkageResolver for InMemoryStorage<'a> {
    type Error = anyhow::Error;

    fn link_context(&self) -> AccountAddress {
        AccountAddress::ZERO
    }

    fn relocate(&self, module_id: &ModuleId) -> Result<ModuleId, Self::Error> {
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
}

impl<'a> VMHarness<'a> {
    pub fn new(resolver: &'a LocalModuleResolver, restricted: bool) -> Result<Self> {
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
        })
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

    pub fn execute_entry_function(
        &mut self,
        module: &ModuleId,
        function_name: &move_core_types::identifier::IdentStr,
        ty_args: Vec<TypeTag>,
        args: Vec<Vec<u8>>,
    ) -> Result<()> {
        let mut session = self.vm.new_session(&self.storage);

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
        let function_name = move_core_types::identifier::Identifier::new(function_name)?;
        let mut session = self.vm.new_session(&self.storage);

        let mut loaded_ty_args = Vec::new();
        for tag in ty_args {
            let ty = session
                .load_type(&tag)
                .map_err(|e| anyhow!("load type failed: {:?}", e))?;
            loaded_ty_args.push(ty);
        }

        let mut gas_meter = UnmeteredGasMeter;
        
        let return_values = session
            .execute_function_bypass_visibility(
                module,
                function_name.as_ident_str(),
                loaded_ty_args,
                args,
                &mut gas_meter,
                None,
            )
            .map_err(|e| anyhow!("execution failed: {:?}", e))?;

        let (result, _store) = session.finish();
        let _changes = result.map_err(|e| anyhow!("session finish failed: {:?}", e))?;
        
        // Extract just the bytes from return values
        let returns: Vec<Vec<u8>> = return_values.return_values
            .into_iter()
            .map(|(bytes, _layout)| bytes)
            .collect();
        
        Ok(returns)
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
                    // For now, skip Clock synthesis - it requires object storage support.
                    return Err(anyhow!("Clock synthesis not yet implemented - requires object storage"));
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
        Ok(create_synthetic_tx_context_bytes())
    }
    
    /// Synthesize Clock bytes (placeholder - returns minimal valid structure)
    pub fn synthesize_clock(&self) -> Result<Vec<u8>> {
        // Clock struct: { id: UID, timestamp_ms: u64 }
        // UID is a 32-byte object ID
        let mut bytes = Vec::new();
        // id: UID (32 bytes)
        bytes.extend_from_slice(&[0u8; 32]);
        // timestamp_ms: u64
        bytes.extend_from_slice(&0u64.to_le_bytes());
        Ok(bytes)
    }
}
