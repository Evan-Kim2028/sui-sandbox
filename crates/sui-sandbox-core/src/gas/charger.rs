//! Unified gas charger that orchestrates all gas operations.
//!
//! The `AccurateGasCharger` is the main entry point for accurate gas metering.
//! It combines:
//! - Protocol configuration loading
//! - Tiered instruction cost tracking via `AccurateGasMeter`
//! - Storage I/O tracking via `StorageTracker`
//! - Computation bucketization
//! - Final gas summary generation
//!
//! # Usage
//!
//! ```ignore
//! use sui_sandbox_core::gas::{AccurateGasCharger, GasSummary};
//!
//! // Create charger with budget and protocol version
//! let mut charger = AccurateGasCharger::new(
//!     50_000_000_000,  // budget
//!     1000,            // gas_price
//!     1000,            // reference_gas_price
//!     68,              // protocol_version
//! );
//!
//! // Use gas_meter() with Move VM execution
//! // Use storage_tracker() with ObjectRuntime
//!
//! // Finalize and get summary
//! let summary = charger.finalize();
//! ```

use std::sync::Arc;

use super::{
    bucketize_computation, load_protocol_config, AccurateGasMeter, GasParameters, GasSummary,
    GasSummaryBuilder, StorageTracker,
};

/// Unified gas charger that orchestrates all gas operations.
///
/// This is the main entry point for accurate gas metering. It manages
/// both computation gas (via AccurateGasMeter) and storage gas (via StorageTracker),
/// then combines them into a final GasSummary.
pub struct AccurateGasCharger {
    /// Gas parameters from protocol config
    params: Arc<GasParameters>,

    /// Computation gas meter (implements GasMeter trait)
    gas_meter: AccurateGasMeter,

    /// Storage I/O tracker
    storage_tracker: StorageTracker,

    /// Gas price for this transaction
    gas_price: u64,

    /// Reference gas price for this epoch
    reference_gas_price: u64,

    /// Maximum computation bucket for bucketization
    max_computation_bucket: u64,

    /// Whether to apply computation bucketization
    enable_bucketization: bool,
}

impl AccurateGasCharger {
    /// Create a new gas charger with the specified parameters.
    ///
    /// # Arguments
    /// * `budget` - Maximum gas budget in MIST
    /// * `gas_price` - Gas price for this transaction
    /// * `reference_gas_price` - Reference gas price for this epoch
    /// * `protocol_version` - Protocol version (determines cost tables)
    pub fn new(
        budget: u64,
        gas_price: u64,
        reference_gas_price: u64,
        protocol_version: u64,
    ) -> Self {
        let config = load_protocol_config(protocol_version);
        let params = Arc::new(GasParameters::from_protocol_config(&config));

        let gas_meter = AccurateGasMeter::new(budget, gas_price, &params);
        let storage_tracker = StorageTracker::new(&params);

        Self {
            max_computation_bucket: params.max_gas_computation_bucket,
            params,
            gas_meter,
            storage_tracker,
            gas_price,
            reference_gas_price,
            enable_bucketization: true,
        }
    }

    /// Create a gas charger with pre-loaded parameters.
    pub fn with_params(
        budget: u64,
        gas_price: u64,
        reference_gas_price: u64,
        params: Arc<GasParameters>,
    ) -> Self {
        let gas_meter = AccurateGasMeter::new(budget, gas_price, &params);
        let storage_tracker = StorageTracker::new(&params);

        Self {
            max_computation_bucket: params.max_gas_computation_bucket,
            params,
            gas_meter,
            storage_tracker,
            gas_price,
            reference_gas_price,
            enable_bucketization: true,
        }
    }

    /// Create an unmetered gas charger (for system transactions).
    pub fn new_unmetered(protocol_version: u64) -> Self {
        let config = load_protocol_config(protocol_version);
        let params = Arc::new(GasParameters::from_protocol_config(&config));

        let gas_meter = AccurateGasMeter::new_unmetered();
        let storage_tracker = StorageTracker::new(&params);

        Self {
            max_computation_bucket: params.max_gas_computation_bucket,
            params,
            gas_meter,
            storage_tracker,
            gas_price: 1,
            reference_gas_price: 1,
            enable_bucketization: false,
        }
    }

    /// Get a mutable reference to the gas meter.
    ///
    /// Use this with Move VM execution:
    /// ```ignore
    /// session.execute_function(..., charger.gas_meter())?;
    /// ```
    pub fn gas_meter(&mut self) -> &mut AccurateGasMeter {
        &mut self.gas_meter
    }

    /// Get a reference to the gas meter (immutable).
    pub fn gas_meter_ref(&self) -> &AccurateGasMeter {
        &self.gas_meter
    }

    /// Get a mutable reference to the storage tracker.
    ///
    /// Use this with ObjectRuntime for storage operations:
    /// ```ignore
    /// charger.storage_tracker().charge_read(bytes);
    /// charger.storage_tracker().charge_create(bytes);
    /// ```
    pub fn storage_tracker(&mut self) -> &mut StorageTracker {
        &mut self.storage_tracker
    }

    /// Get a reference to the storage tracker (immutable).
    pub fn storage_tracker_ref(&self) -> &StorageTracker {
        &self.storage_tracker
    }

    /// Get the gas parameters.
    pub fn params(&self) -> &GasParameters {
        &self.params
    }

    /// Disable computation bucketization.
    pub fn disable_bucketization(&mut self) {
        self.enable_bucketization = false;
    }

    /// Enable computation bucketization.
    pub fn enable_bucketization(&mut self) {
        self.enable_bucketization = true;
    }

    /// Check if the gas budget has been exceeded.
    pub fn is_out_of_gas(&self) -> bool {
        self.gas_meter.remaining_gas_units() == 0
    }

    /// Get current computation gas consumed (before bucketization).
    pub fn computation_gas_consumed(&self) -> u64 {
        self.gas_meter.gas_consumed()
    }

    /// Get current storage gas consumed.
    pub fn storage_gas_consumed(&self) -> u64 {
        self.storage_tracker.total_storage_cost()
    }

    /// Get current storage rebate accumulated.
    pub fn storage_rebate(&self) -> u64 {
        self.storage_tracker.summary().storage_rebate
    }

    /// Get total gas consumed so far (computation + storage - rebate).
    pub fn total_gas_consumed(&self) -> u64 {
        let computation = self.computation_gas_consumed();
        let storage = self.storage_gas_consumed();
        let rebate = self.storage_rebate();

        computation.saturating_add(storage).saturating_sub(rebate)
    }

    /// Finalize the gas charger and produce a summary.
    ///
    /// This consumes the charger and returns the final gas costs,
    /// applying bucketization if enabled.
    pub fn finalize(self) -> GasSummary {
        let computation_pre_bucket = self.gas_meter.gas_consumed();
        let storage_summary = self.storage_tracker.summary();

        // Apply bucketization if enabled
        let computation_cost = if self.enable_bucketization {
            bucketize_computation(computation_pre_bucket, self.max_computation_bucket)
        } else {
            computation_pre_bucket
        };

        // Calculate storage cost (read + mutate + delete + new storage)
        let storage_cost = storage_summary.total_cost();
        let storage_rebate = storage_summary.storage_rebate;

        // Calculate non-refundable portion (burned)
        // In Sui: non_refundable = storage_cost - storage_rebate
        let non_refundable = storage_cost.saturating_sub(storage_rebate);

        GasSummaryBuilder::new()
            .computation_cost(computation_cost)
            .pre_bucket_computation(computation_pre_bucket)
            .storage_cost(storage_cost)
            .storage_rebate(storage_rebate)
            .non_refundable_storage_fee(non_refundable)
            .gas_price(self.gas_price)
            .reference_gas_price(self.reference_gas_price)
            .gas_model_version(self.params.gas_model_version)
            .storage_details(storage_summary)
            .build()
    }

    /// Get a summary without consuming the charger.
    ///
    /// Useful for intermediate gas checks during execution.
    pub fn current_summary(&self) -> GasSummary {
        let computation_pre_bucket = self.gas_meter.gas_consumed();
        let storage_summary = self.storage_tracker.summary();

        let computation_cost = if self.enable_bucketization {
            bucketize_computation(computation_pre_bucket, self.max_computation_bucket)
        } else {
            computation_pre_bucket
        };

        let storage_cost = storage_summary.total_cost();
        let storage_rebate = storage_summary.storage_rebate;
        let non_refundable = storage_cost.saturating_sub(storage_rebate);

        GasSummaryBuilder::new()
            .computation_cost(computation_cost)
            .pre_bucket_computation(computation_pre_bucket)
            .storage_cost(storage_cost)
            .storage_rebate(storage_rebate)
            .non_refundable_storage_fee(non_refundable)
            .gas_price(self.gas_price)
            .reference_gas_price(self.reference_gas_price)
            .gas_model_version(self.params.gas_model_version)
            .storage_details(storage_summary)
            .build()
    }

    /// Reset the charger for reuse (keeps parameters, clears accumulated costs).
    pub fn reset(&mut self, budget: u64) {
        self.gas_meter = AccurateGasMeter::new(budget, self.gas_price, &self.params);
        self.storage_tracker.reset();
    }
}

impl std::fmt::Debug for AccurateGasCharger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccurateGasCharger")
            .field("computation_consumed", &self.computation_gas_consumed())
            .field("storage_consumed", &self.storage_gas_consumed())
            .field("storage_rebate", &self.storage_rebate())
            .field("gas_price", &self.gas_price)
            .field("gas_model_version", &self.params.gas_model_version)
            .finish()
    }
}

/// Gas charging mode for the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GasChargingMode {
    /// Use accurate gas metering with protocol-accurate costs.
    Accurate,
    /// Use simple metering with hardcoded costs (legacy behavior).
    #[default]
    Simple,
    /// No gas metering (unlimited gas).
    Unmetered,
}

/// Callback interface for storage tracking during ObjectRuntime operations.
///
/// Implement this trait to receive notifications when objects are read,
/// created, mutated, or deleted during Move execution.
pub trait StorageCallback: Send + Sync {
    /// Called when an object is read.
    fn on_object_read(&self, object_id: &[u8; 32], bytes: usize);

    /// Called when a new object is created.
    fn on_object_created(&self, object_id: &[u8; 32], bytes: usize);

    /// Called when an object is mutated.
    fn on_object_mutated(&self, object_id: &[u8; 32], old_bytes: usize, new_bytes: usize);

    /// Called when an object is deleted.
    fn on_object_deleted(
        &self,
        object_id: &[u8; 32],
        bytes: usize,
        previous_storage_cost: Option<u64>,
    );
}

/// A storage callback that forwards events to a StorageTracker.
///
/// This provides the integration between ObjectRuntime operations and
/// the accurate gas metering system.
pub struct StorageTrackerCallback {
    /// The storage tracker to forward events to.
    /// Uses interior mutability since callbacks may be called from immutable context.
    tracker: parking_lot::Mutex<StorageTracker>,
}

impl StorageTrackerCallback {
    /// Create a new callback wrapping a storage tracker.
    pub fn new(tracker: StorageTracker) -> Self {
        Self {
            tracker: parking_lot::Mutex::new(tracker),
        }
    }

    /// Get the accumulated storage summary.
    pub fn summary(&self) -> super::StorageSummary {
        self.tracker.lock().summary()
    }

    /// Reset the tracker.
    pub fn reset(&self) {
        self.tracker.lock().reset();
    }

    /// Get total storage cost accumulated.
    pub fn total_storage_cost(&self) -> u64 {
        self.tracker.lock().total_storage_cost()
    }
}

impl StorageCallback for StorageTrackerCallback {
    fn on_object_read(&self, _object_id: &[u8; 32], bytes: usize) {
        self.tracker.lock().charge_read(bytes);
    }

    fn on_object_created(&self, _object_id: &[u8; 32], bytes: usize) {
        self.tracker.lock().charge_create(bytes);
    }

    fn on_object_mutated(&self, _object_id: &[u8; 32], old_bytes: usize, new_bytes: usize) {
        self.tracker.lock().charge_mutate(old_bytes, new_bytes);
    }

    fn on_object_deleted(
        &self,
        _object_id: &[u8; 32],
        bytes: usize,
        previous_storage_cost: Option<u64>,
    ) {
        self.tracker
            .lock()
            .charge_delete(bytes, previous_storage_cost);
    }
}

/// Helper to create a storage callback from protocol parameters.
impl StorageTrackerCallback {
    /// Create a callback with parameters from a GasParameters struct.
    pub fn from_params(params: &super::GasParameters) -> Self {
        Self::new(StorageTracker::new(params))
    }

    /// Create a callback with default parameters.
    pub fn with_defaults() -> Self {
        Self::new(StorageTracker::with_defaults())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use move_vm_types::gas::{GasMeter, SimpleInstruction};

    #[test]
    fn test_charger_creation() {
        let charger = AccurateGasCharger::new(50_000_000_000, 1000, 1000, 68);

        assert!(!charger.is_out_of_gas());
        assert_eq!(charger.computation_gas_consumed(), 0);
        assert_eq!(charger.storage_gas_consumed(), 0);
    }

    #[test]
    fn test_charger_with_operations() {
        let mut charger = AccurateGasCharger::new(50_000_000_000, 1000, 1000, 68);

        // Simulate computation
        for _ in 0..2000 {
            charger
                .gas_meter()
                .charge_simple_instr(SimpleInstruction::LdU64)
                .unwrap();
        }

        // Simulate storage
        charger.storage_tracker().charge_read(100);
        charger.storage_tracker().charge_create(200);

        // Check costs accumulated
        assert!(charger.storage_gas_consumed() > 0);
        assert!(charger.total_gas_consumed() > 0);
    }

    #[test]
    fn test_charger_finalize() {
        let mut charger = AccurateGasCharger::new(50_000_000_000, 1000, 1000, 68);

        // Add some operations
        charger.storage_tracker().charge_create(100);

        let summary = charger.finalize();

        assert!(summary.storage_cost > 0);
        assert!(summary.gas_model_version >= 8);
    }

    #[test]
    fn test_charger_reset() {
        let mut charger = AccurateGasCharger::new(50_000_000_000, 1000, 1000, 68);

        charger.storage_tracker().charge_create(100);
        assert!(charger.storage_gas_consumed() > 0);

        charger.reset(50_000_000_000);
        assert_eq!(charger.storage_gas_consumed(), 0);
    }

    #[test]
    fn test_unmetered_charger() {
        let charger = AccurateGasCharger::new_unmetered(68);

        assert!(!charger.gas_meter_ref().is_metered());
    }
}
