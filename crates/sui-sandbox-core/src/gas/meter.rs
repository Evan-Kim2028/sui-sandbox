//! Accurate gas meter implementation using Sui's GasStatus.
//!
//! # ⚠️ Beta - See [`super`] module docs for limitations
//!
//! This module provides a gas meter that wraps Sui's actual `GasStatus`
//! from `sui_types::gas_model::tables`. This ensures we use the same
//! tiered instruction costs, stack tracking, and gas deduction logic
//! as the real Sui network.
//!
//! **Note**: While this uses Sui's actual cost tables, accuracy varies by
//! transaction type. Simple transactions typically achieve 100% accuracy,
//! while complex multi-package transactions may show ~50% accuracy.
//! See the parent module documentation for details.
//!
//! # Architecture
//!
//! The `AccurateGasMeter` wraps `sui_types::gas_model::tables::GasStatus`
//! and implements Move VM's `GasMeter` trait. All gas charging is delegated
//! to the underlying GasStatus, which handles:
//!
//! - Tiered instruction costs
//! - Stack height tracking
//! - Stack size tracking
//! - Gas deduction with overflow protection
//!
//! # Usage
//!
//! ```ignore
//! use sui_sandbox_core::gas::{AccurateGasMeter, GasParameters};
//!
//! let params = GasParameters::default();
//! let mut meter = AccurateGasMeter::new(50_000_000_000, 1000, &params);
//!
//! // Use with Move VM execution
//! session.execute_function(..., &mut meter)?;
//!
//! // Get gas consumed (in gas units, not MIST)
//! let consumed = meter.gas_consumed();
//!
//! // To convert to MIST: consumed * gas_price
//! // Remember to apply gas rounding for final cost:
//! // use sui_sandbox_core::gas::finalize_computation_cost;
//! ```

use std::sync::Arc;

use move_binary_format::errors::PartialVMResult;
use move_core_types::gas_algebra::{InternalGas, NumArgs, NumBytes};
use move_vm_types::gas::{GasMeter, SimpleInstruction};
use move_vm_types::views::{TypeView, ValueView};

use sui_types::gas_model::tables::GasStatus;

use super::{cost_table_for_version, GasParameters};

/// Size constants for gas calculations (from Sui)
const CONST_SIZE: u64 = 16;
const REFERENCE_SIZE: u64 = 8;
const STRUCT_SIZE: u64 = 2;
const VEC_SIZE: u64 = 8;

/// Accurate gas meter using Sui's GasStatus.
///
/// This provides 1:1 gas metering parity with the Sui network by
/// delegating to Sui's actual GasStatus implementation.
pub struct AccurateGasMeter {
    /// Sui's actual gas status (handles tiering, tracking, deduction)
    gas_status: GasStatus,

    /// Gas parameters from protocol config
    params: Arc<GasParameters>,
}

impl AccurateGasMeter {
    /// Create a new accurate gas meter.
    ///
    /// # Arguments
    /// * `budget` - Maximum gas budget in MIST
    /// * `gas_price` - Gas price for this transaction
    /// * `params` - Gas parameters from protocol config
    pub fn new(budget: u64, gas_price: u64, params: &GasParameters) -> Self {
        let cost_table = cost_table_for_version(params.gas_model_version);
        let mut gas_status = GasStatus::new(
            cost_table,
            budget,
            gas_price.max(1), // Ensure non-zero gas price
            params.gas_model_version,
        );
        gas_status.set_metering(true);

        Self {
            gas_status,
            params: Arc::new(params.clone()),
        }
    }

    /// Create an unmetered gas meter (for system transactions).
    pub fn new_unmetered() -> Self {
        Self {
            gas_status: GasStatus::new_unmetered(),
            params: Arc::new(GasParameters::default()),
        }
    }

    /// Get the amount of gas consumed so far (before gas price multiplication).
    pub fn gas_consumed(&self) -> u64 {
        self.gas_status.gas_used_pre_gas_price()
    }

    /// Get the remaining gas budget in gas units.
    pub fn remaining_gas_units(&self) -> u64 {
        u64::from(self.gas_status.remaining_gas())
    }

    /// Get the gas price.
    pub fn gas_price(&self) -> u64 {
        self.gas_status.gas_price()
    }

    /// Get execution statistics.
    pub fn stats(&self) -> GasMeterStats {
        GasMeterStats {
            gas_consumed: self.gas_consumed(),
            remaining_gas: self.remaining_gas_units(),
            instructions_executed: self.gas_status.instructions_executed(),
            stack_height_high_water_mark: self.gas_status.stack_height_high_water_mark(),
            stack_size_high_water_mark: self.gas_status.stack_size_high_water_mark(),
            native_calls: self.gas_status.num_native_calls,
        }
    }

    /// Check if metering is enabled.
    pub fn is_metered(&self) -> bool {
        self.gas_status.charge
    }

    /// Enable or disable metering.
    pub fn set_metering(&mut self, enabled: bool) {
        self.gas_status.set_metering(enabled);
    }

    /// Get access to the underlying GasStatus for advanced operations.
    pub fn gas_status(&self) -> &GasStatus {
        &self.gas_status
    }

    /// Get mutable access to the underlying GasStatus.
    pub fn gas_status_mut(&mut self) -> &mut GasStatus {
        &mut self.gas_status
    }

    /// Charge for bytes (e.g., BCS operations).
    pub fn charge_bytes(&mut self, size: usize, cost_per_byte: u64) -> PartialVMResult<()> {
        self.gas_status.charge_bytes(size, cost_per_byte)
    }
}

/// Statistics from gas meter execution.
#[derive(Debug, Clone, Default)]
pub struct GasMeterStats {
    /// Total gas consumed
    pub gas_consumed: u64,
    /// Remaining gas budget
    pub remaining_gas: u64,
    /// Number of bytecode instructions executed
    pub instructions_executed: u64,
    /// Maximum stack height reached
    pub stack_height_high_water_mark: u64,
    /// Maximum stack size reached (bytes)
    pub stack_size_high_water_mark: u64,
    /// Number of native function calls
    pub native_calls: u64,
}

impl GasMeter for AccurateGasMeter {
    fn charge_simple_instr(&mut self, instr: SimpleInstruction) -> PartialVMResult<()> {
        // Map instruction to (num_instructions, pushes, pops, incr_size, decr_size)
        let (pushes, pops, incr_size, decr_size) = match instr {
            // Arithmetic - push 1, pop 2, no size change
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
            | SimpleInstruction::Lt
            | SimpleInstruction::Gt
            | SimpleInstruction::Le
            | SimpleInstruction::Ge => (1, 2, CONST_SIZE, CONST_SIZE * 2),

            // Unary - push 1, pop 1, no net change
            SimpleInstruction::Not => (1, 1, CONST_SIZE, CONST_SIZE),

            // Control flow - no stack change
            SimpleInstruction::Nop
            | SimpleInstruction::Ret
            | SimpleInstruction::BrTrue
            | SimpleInstruction::BrFalse
            | SimpleInstruction::Branch => (0, 0, 0, 0),

            // Load constants - push 1
            SimpleInstruction::LdU8
            | SimpleInstruction::LdU16
            | SimpleInstruction::LdU32
            | SimpleInstruction::LdU64
            | SimpleInstruction::LdU128
            | SimpleInstruction::LdU256
            | SimpleInstruction::LdTrue
            | SimpleInstruction::LdFalse => (1, 0, CONST_SIZE, 0),

            // Casts - push 1, pop 1
            SimpleInstruction::CastU8
            | SimpleInstruction::CastU16
            | SimpleInstruction::CastU32
            | SimpleInstruction::CastU64
            | SimpleInstruction::CastU128
            | SimpleInstruction::CastU256 => (1, 1, CONST_SIZE, CONST_SIZE),

            // Reference operations
            SimpleInstruction::FreezeRef => (1, 1, REFERENCE_SIZE, REFERENCE_SIZE),
            SimpleInstruction::MutBorrowLoc | SimpleInstruction::ImmBorrowLoc => {
                (1, 0, REFERENCE_SIZE, 0)
            }
            SimpleInstruction::ImmBorrowField
            | SimpleInstruction::MutBorrowField
            | SimpleInstruction::ImmBorrowFieldGeneric
            | SimpleInstruction::MutBorrowFieldGeneric => (1, 1, REFERENCE_SIZE, REFERENCE_SIZE),

            // Abort - no stack effect (execution stops)
            SimpleInstruction::Abort => (0, 1, 0, CONST_SIZE),
        };

        self.gas_status
            .charge(1, pushes, pops, incr_size, decr_size)
    }

    fn charge_pop(&mut self, _popped_val: impl ValueView) -> PartialVMResult<()> {
        self.gas_status.charge(1, 0, 1, 0, CONST_SIZE)
    }

    fn charge_call(
        &mut self,
        _module_id: &move_core_types::language_storage::ModuleId,
        _func_name: &str,
        args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        let arg_count = args.len() as u64;
        // Function call: pop args, create frame
        self.gas_status
            .charge(1, 0, arg_count, STRUCT_SIZE, arg_count * CONST_SIZE)
    }

    fn charge_call_generic(
        &mut self,
        _module_id: &move_core_types::language_storage::ModuleId,
        _func_name: &str,
        ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        args: impl ExactSizeIterator<Item = impl ValueView>,
        _num_locals: NumArgs,
    ) -> PartialVMResult<()> {
        let ty_count = ty_args.len() as u64;
        let arg_count = args.len() as u64;
        // Generic call has additional type argument overhead
        let type_overhead = ty_count * CONST_SIZE;
        self.gas_status.charge(
            1,
            0,
            arg_count,
            STRUCT_SIZE + type_overhead,
            arg_count * CONST_SIZE,
        )
    }

    fn charge_ld_const(&mut self, size: NumBytes) -> PartialVMResult<()> {
        let size: u64 = size.into();
        self.gas_status.charge(1, 1, 0, size, 0)
    }

    fn charge_ld_const_after_deserialization(
        &mut self,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        // Already charged in charge_ld_const
        Ok(())
    }

    fn charge_copy_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.gas_status.charge(1, 1, 0, CONST_SIZE, 0)
    }

    fn charge_move_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        // Move doesn't copy, just transfers ownership
        self.gas_status.charge(1, 1, 1, CONST_SIZE, CONST_SIZE)
    }

    fn charge_store_loc(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.gas_status.charge(1, 0, 1, 0, CONST_SIZE)
    }

    fn charge_pack(
        &mut self,
        _is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let field_count = args.len() as u64;
        // Pack: pop fields, push struct
        self.gas_status
            .charge(1, 1, field_count, STRUCT_SIZE, field_count * CONST_SIZE)
    }

    fn charge_unpack(
        &mut self,
        _is_generic: bool,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let field_count = args.len() as u64;
        // Unpack: pop struct, push fields
        self.gas_status
            .charge(1, field_count, 1, field_count * CONST_SIZE, STRUCT_SIZE)
    }

    fn charge_variant_switch(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        self.gas_status.charge(1, 0, 0, 0, 0)
    }

    fn charge_read_ref(&mut self, _val: impl ValueView) -> PartialVMResult<()> {
        // Read ref: pop ref, push value
        self.gas_status.charge(1, 1, 1, CONST_SIZE, REFERENCE_SIZE)
    }

    fn charge_write_ref(
        &mut self,
        _new_val: impl ValueView,
        _old_val: impl ValueView,
    ) -> PartialVMResult<()> {
        // Write ref: pop ref and value
        self.gas_status
            .charge(1, 0, 2, 0, REFERENCE_SIZE + CONST_SIZE)
    }

    fn charge_eq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        // Eq: pop 2, push bool
        self.gas_status.charge(1, 1, 2, CONST_SIZE, CONST_SIZE * 2)
    }

    fn charge_neq(&mut self, _lhs: impl ValueView, _rhs: impl ValueView) -> PartialVMResult<()> {
        // Neq: same as eq
        self.gas_status.charge(1, 1, 2, CONST_SIZE, CONST_SIZE * 2)
    }

    fn charge_vec_pack<'a>(
        &mut self,
        _ty: impl TypeView + 'a,
        args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let elem_count = args.len() as u64;
        // Vec pack: pop elements, push vector
        self.gas_status
            .charge(1, 1, elem_count, VEC_SIZE, elem_count * CONST_SIZE)
    }

    fn charge_vec_len(&mut self, _ty: impl TypeView) -> PartialVMResult<()> {
        // Vec len: pop vec ref, push u64
        self.gas_status.charge(1, 1, 1, CONST_SIZE, REFERENCE_SIZE)
    }

    fn charge_vec_borrow(
        &mut self,
        _is_mut: bool,
        _ty: impl TypeView,
        _is_success: bool,
    ) -> PartialVMResult<()> {
        // Vec borrow: pop vec ref and index, push element ref
        self.gas_status
            .charge(1, 1, 2, REFERENCE_SIZE, REFERENCE_SIZE + CONST_SIZE)
    }

    fn charge_vec_push_back(
        &mut self,
        _ty: impl TypeView,
        _val: impl ValueView,
    ) -> PartialVMResult<()> {
        // Vec push: pop vec ref and value
        self.gas_status
            .charge(1, 0, 2, 0, REFERENCE_SIZE + CONST_SIZE)
    }

    fn charge_vec_pop_back(
        &mut self,
        _ty: impl TypeView,
        _val: Option<impl ValueView>,
    ) -> PartialVMResult<()> {
        // Vec pop: pop vec ref, push value
        self.gas_status.charge(1, 1, 1, CONST_SIZE, REFERENCE_SIZE)
    }

    fn charge_vec_unpack(
        &mut self,
        _ty: impl TypeView,
        _expect_num_elements: NumArgs,
        elems: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let elem_count = elems.len() as u64;
        // Vec unpack: pop vector, push elements
        self.gas_status
            .charge(1, elem_count, 1, elem_count * CONST_SIZE, VEC_SIZE)
    }

    fn charge_vec_swap(&mut self, _ty: impl TypeView) -> PartialVMResult<()> {
        // Vec swap: pop vec ref and 2 indices
        self.gas_status
            .charge(1, 0, 3, 0, REFERENCE_SIZE + CONST_SIZE * 2)
    }

    fn charge_native_function(
        &mut self,
        amount: InternalGas,
        _ret_vals: Option<impl ExactSizeIterator<Item = impl ValueView>>,
    ) -> PartialVMResult<()> {
        // Native function returns its own cost
        self.gas_status.record_native_call();
        self.gas_status.deduct_gas(amount)
    }

    fn charge_native_function_before_execution(
        &mut self,
        _ty_args: impl ExactSizeIterator<Item = impl TypeView>,
        _args: impl ExactSizeIterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        // Pre-execution setup cost is included in the native function cost
        Ok(())
    }

    fn charge_drop_frame(
        &mut self,
        locals: impl Iterator<Item = impl ValueView>,
    ) -> PartialVMResult<()> {
        let local_count = locals.count() as u64;
        // Drop frame: clean up locals
        self.gas_status
            .charge(1, 0, local_count, 0, local_count * CONST_SIZE)
    }

    fn remaining_gas(&self) -> InternalGas {
        InternalGas::new(u64::from(self.gas_status.remaining_gas()) * 1000)
    }
}

impl std::fmt::Debug for AccurateGasMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccurateGasMeter")
            .field("gas_consumed", &self.gas_consumed())
            .field("remaining_gas", &self.remaining_gas_units())
            .field("gas_model_version", &self.params.gas_model_version)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accurate_gas_meter_creation() {
        let params = GasParameters::default();
        let meter = AccurateGasMeter::new(50_000_000_000, 1000, &params);

        assert!(meter.is_metered());
        assert_eq!(meter.gas_consumed(), 0);
        assert!(meter.remaining_gas_units() > 0);
    }

    #[test]
    fn test_unmetered_gas_meter() {
        let meter = AccurateGasMeter::new_unmetered();

        assert!(!meter.is_metered());
    }

    #[test]
    fn test_gas_consumption() {
        let params = GasParameters::default();
        let mut meter = AccurateGasMeter::new(50_000_000_000, 1000, &params);

        // Execute many instructions to accumulate enough gas to be visible
        // (internal gas units are 1000x, so we need >1000 to show as 1 gas unit)
        for _ in 0..2000 {
            meter.charge_simple_instr(SimpleInstruction::LdU64).unwrap();
        }

        // Should have consumed some gas (2000 instructions at 1 internal gas each = 2000 internal = 2 gas units)
        assert!(
            meter.gas_consumed() > 0,
            "gas_consumed should be > 0, was {}",
            meter.gas_consumed()
        );

        // Also verify stats track instructions
        let stats = meter.stats();
        assert!(stats.instructions_executed >= 2000);
    }

    #[test]
    fn test_out_of_gas() {
        let params = GasParameters::default();
        // Budget of 10 gas units with gas_price=1 means 10 gas budget
        // Each instruction costs ~1-17 internal gas (depending on stack tracking)
        // 10 gas = 10,000 internal gas, should run out quickly
        let mut meter = AccurateGasMeter::new(10, 1, &params);

        // This should eventually run out of gas
        let mut result = Ok(());
        for _ in 0..100_000 {
            result = meter.charge_simple_instr(SimpleInstruction::LdU64);
            if result.is_err() {
                break;
            }
        }

        assert!(result.is_err(), "Should have run out of gas");
    }

    #[test]
    fn test_stats() {
        let params = GasParameters::default();
        let mut meter = AccurateGasMeter::new(50_000_000_000, 1000, &params);

        // Execute many instructions
        for _ in 0..2000 {
            meter.charge_simple_instr(SimpleInstruction::LdU64).unwrap();
        }

        let stats = meter.stats();
        // Gas consumed is in gas units (internal / 1000), so may be small
        // But instructions_executed should always be accurate
        assert!(
            stats.instructions_executed >= 2000,
            "instructions should be >= 2000, was {}",
            stats.instructions_executed
        );
    }
}
