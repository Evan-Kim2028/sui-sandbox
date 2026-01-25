//! Storage I/O tracking for accurate gas metering.
//!
//! This module tracks storage operations (reads, writes, deletes) and calculates
//! the associated gas costs. On Sui, storage operations account for a significant
//! portion of transaction gas costs (~40%).
//!
//! # Storage Cost Model
//!
//! Sui charges for storage in several ways:
//!
//! 1. **Read Cost**: `bytes * obj_access_cost_read_per_byte`
//! 2. **Mutate Cost**: `bytes * obj_access_cost_mutate_per_byte`
//! 3. **Delete Cost**: `bytes * obj_access_cost_delete_per_byte`
//! 4. **New Storage**: `bytes * obj_data_cost_refundable` (rebatable)
//! 5. **Metadata**: `obj_metadata_cost_non_refundable` (non-rebatable)
//!
//! # Storage Rebates
//!
//! When objects are deleted, the sender receives a rebate:
//! ```text
//! rebate = (storage_cost * storage_rebate_rate + 5000) / 10000
//! ```
//!
//! The rebate rate is typically 99% (9900 basis points), meaning most of the
//! storage cost is refunded when objects are deleted.

use super::GasParameters;

/// Tracks storage I/O operations and calculates associated gas costs.
///
/// This is designed to be attached to the ObjectRuntime during execution
/// to track all object read/write/delete operations.
#[derive(Debug, Clone)]
pub struct StorageTracker {
    // ========== Protocol Parameters ==========
    /// Cost per byte for reading an object
    obj_access_cost_read_per_byte: u64,
    /// Cost per byte for mutating an object
    obj_access_cost_mutate_per_byte: u64,
    /// Cost per byte for deleting an object
    obj_access_cost_delete_per_byte: u64,
    /// Cost per byte for type verification
    obj_access_cost_verify_per_byte: u64,
    /// Refundable cost per byte of object data
    obj_data_cost_refundable: u64,
    /// Non-refundable cost per object (metadata)
    obj_metadata_cost_non_refundable: u64,
    /// Storage rebate rate (basis points)
    storage_rebate_rate: u64,

    // ========== Accumulated Costs ==========
    /// Total read cost accumulated
    read_cost: u64,
    /// Total mutate cost accumulated
    mutate_cost: u64,
    /// Total delete cost accumulated
    delete_cost: u64,
    /// Total new storage cost (for new objects)
    new_storage_cost: u64,
    /// Total storage rebate accumulated (from deletions)
    storage_rebate: u64,

    // ========== Statistics ==========
    /// Number of objects read
    objects_read: u64,
    /// Total bytes read
    bytes_read: u64,
    /// Number of objects mutated
    objects_mutated: u64,
    /// Total bytes mutated
    bytes_mutated: u64,
    /// Number of objects created
    objects_created: u64,
    /// Total bytes of new objects
    bytes_created: u64,
    /// Number of objects deleted
    objects_deleted: u64,
    /// Total bytes deleted
    bytes_deleted: u64,
}

impl StorageTracker {
    /// Create a new storage tracker with parameters from ProtocolConfig.
    pub fn new(params: &GasParameters) -> Self {
        Self {
            obj_access_cost_read_per_byte: params.obj_access_cost_read_per_byte,
            obj_access_cost_mutate_per_byte: params.obj_access_cost_mutate_per_byte,
            obj_access_cost_delete_per_byte: params.obj_access_cost_delete_per_byte,
            obj_access_cost_verify_per_byte: params.obj_access_cost_verify_per_byte,
            obj_data_cost_refundable: params.obj_data_cost_refundable,
            obj_metadata_cost_non_refundable: params.obj_metadata_cost_non_refundable,
            storage_rebate_rate: params.storage_rebate_rate,

            read_cost: 0,
            mutate_cost: 0,
            delete_cost: 0,
            new_storage_cost: 0,
            storage_rebate: 0,

            objects_read: 0,
            bytes_read: 0,
            objects_mutated: 0,
            bytes_mutated: 0,
            objects_created: 0,
            bytes_created: 0,
            objects_deleted: 0,
            bytes_deleted: 0,
        }
    }

    /// Create a storage tracker with default parameters.
    pub fn with_defaults() -> Self {
        Self::new(&GasParameters::default())
    }

    /// Track an object read operation.
    ///
    /// # Arguments
    /// * `bytes` - Size of the object in bytes
    pub fn charge_read(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        let cost = bytes.saturating_mul(self.obj_access_cost_read_per_byte);

        self.read_cost = self.read_cost.saturating_add(cost);
        self.objects_read = self.objects_read.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(bytes);

        tracing::trace!(
            bytes = bytes,
            cost = cost,
            total_read_cost = self.read_cost,
            "storage: charged read"
        );
    }

    /// Track an object mutation operation.
    ///
    /// # Arguments
    /// * `old_bytes` - Original size of the object
    /// * `new_bytes` - New size of the object after mutation
    pub fn charge_mutate(&mut self, old_bytes: usize, new_bytes: usize) {
        let old_bytes = old_bytes as u64;
        let new_bytes = new_bytes as u64;

        // Base mutation cost
        let mutate_cost = new_bytes.saturating_mul(self.obj_access_cost_mutate_per_byte);
        self.mutate_cost = self.mutate_cost.saturating_add(mutate_cost);

        // If object grew, charge for additional storage
        if new_bytes > old_bytes {
            let growth = new_bytes - old_bytes;
            let growth_cost = growth.saturating_mul(self.obj_data_cost_refundable);
            self.new_storage_cost = self.new_storage_cost.saturating_add(growth_cost);

            tracing::trace!(
                old_bytes = old_bytes,
                new_bytes = new_bytes,
                growth = growth,
                growth_cost = growth_cost,
                "storage: object grew"
            );
        } else if new_bytes < old_bytes {
            // Object shrank - calculate rebate for freed storage
            let shrink = old_bytes - new_bytes;
            let shrink_storage_cost = shrink.saturating_mul(self.obj_data_cost_refundable);
            let rebate = self.calculate_sender_rebate(shrink_storage_cost);
            self.storage_rebate = self.storage_rebate.saturating_add(rebate);

            tracing::trace!(
                old_bytes = old_bytes,
                new_bytes = new_bytes,
                shrink = shrink,
                rebate = rebate,
                "storage: object shrank"
            );
        }

        self.objects_mutated = self.objects_mutated.saturating_add(1);
        self.bytes_mutated = self.bytes_mutated.saturating_add(new_bytes);

        tracing::trace!(
            old_bytes = old_bytes,
            new_bytes = new_bytes,
            mutate_cost = mutate_cost,
            total_mutate_cost = self.mutate_cost,
            "storage: charged mutate"
        );
    }

    /// Track a new object creation.
    ///
    /// # Arguments
    /// * `bytes` - Size of the new object in bytes
    pub fn charge_create(&mut self, bytes: usize) {
        let bytes = bytes as u64;

        // Storage cost for new data (refundable portion)
        let data_cost = bytes.saturating_mul(self.obj_data_cost_refundable);
        self.new_storage_cost = self.new_storage_cost.saturating_add(data_cost);

        // Metadata cost (non-refundable)
        self.new_storage_cost = self
            .new_storage_cost
            .saturating_add(self.obj_metadata_cost_non_refundable);

        self.objects_created = self.objects_created.saturating_add(1);
        self.bytes_created = self.bytes_created.saturating_add(bytes);

        tracing::trace!(
            bytes = bytes,
            data_cost = data_cost,
            metadata_cost = self.obj_metadata_cost_non_refundable,
            total_new_storage = self.new_storage_cost,
            "storage: charged create"
        );
    }

    /// Track an object deletion.
    ///
    /// # Arguments
    /// * `bytes` - Size of the deleted object in bytes
    /// * `previous_storage_cost` - The storage cost that was paid when the object was created
    ///                             (if known, otherwise use bytes * obj_data_cost_refundable)
    pub fn charge_delete(&mut self, bytes: usize, previous_storage_cost: Option<u64>) {
        let bytes = bytes as u64;

        // Deletion access cost
        let delete_cost = bytes.saturating_mul(self.obj_access_cost_delete_per_byte);
        self.delete_cost = self.delete_cost.saturating_add(delete_cost);

        // Calculate storage rebate
        let storage_cost = previous_storage_cost.unwrap_or_else(|| {
            bytes.saturating_mul(self.obj_data_cost_refundable)
                + self.obj_metadata_cost_non_refundable
        });
        let rebate = self.calculate_sender_rebate(storage_cost);
        self.storage_rebate = self.storage_rebate.saturating_add(rebate);

        self.objects_deleted = self.objects_deleted.saturating_add(1);
        self.bytes_deleted = self.bytes_deleted.saturating_add(bytes);

        tracing::trace!(
            bytes = bytes,
            delete_cost = delete_cost,
            storage_cost = storage_cost,
            rebate = rebate,
            total_rebate = self.storage_rebate,
            "storage: charged delete"
        );
    }

    /// Track type verification cost.
    ///
    /// # Arguments
    /// * `bytes` - Size of the type being verified
    pub fn charge_verify(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        let cost = bytes.saturating_mul(self.obj_access_cost_verify_per_byte);

        // Verification cost is added to read cost
        self.read_cost = self.read_cost.saturating_add(cost);

        tracing::trace!(
            bytes = bytes,
            cost = cost,
            "storage: charged verify"
        );
    }

    /// Calculate the sender rebate for a given storage cost.
    ///
    /// Uses Sui's formula: `(storage_cost * rebate_rate + 5000) / 10000`
    fn calculate_sender_rebate(&self, storage_cost: u64) -> u64 {
        let numerator = (storage_cost as u128)
            .saturating_mul(self.storage_rebate_rate as u128)
            .saturating_add(5000);
        (numerator / 10000) as u64
    }

    /// Get the summary of storage costs.
    pub fn summary(&self) -> StorageSummary {
        StorageSummary {
            read_cost: self.read_cost,
            mutate_cost: self.mutate_cost,
            delete_cost: self.delete_cost,
            new_storage_cost: self.new_storage_cost,
            storage_rebate: self.storage_rebate,

            objects_read: self.objects_read,
            bytes_read: self.bytes_read,
            objects_mutated: self.objects_mutated,
            bytes_mutated: self.bytes_mutated,
            objects_created: self.objects_created,
            bytes_created: self.bytes_created,
            objects_deleted: self.objects_deleted,
            bytes_deleted: self.bytes_deleted,
        }
    }

    /// Get total storage-related gas cost (excluding rebate).
    pub fn total_storage_cost(&self) -> u64 {
        self.read_cost
            .saturating_add(self.mutate_cost)
            .saturating_add(self.delete_cost)
            .saturating_add(self.new_storage_cost)
    }

    /// Get net storage cost after rebate.
    pub fn net_storage_cost(&self) -> i64 {
        (self.total_storage_cost() as i64).saturating_sub(self.storage_rebate as i64)
    }

    /// Reset all accumulated costs (for reuse).
    pub fn reset(&mut self) {
        self.read_cost = 0;
        self.mutate_cost = 0;
        self.delete_cost = 0;
        self.new_storage_cost = 0;
        self.storage_rebate = 0;
        self.objects_read = 0;
        self.bytes_read = 0;
        self.objects_mutated = 0;
        self.bytes_mutated = 0;
        self.objects_created = 0;
        self.bytes_created = 0;
        self.objects_deleted = 0;
        self.bytes_deleted = 0;
    }
}

/// Summary of storage costs and statistics.
#[derive(Debug, Clone, Default)]
pub struct StorageSummary {
    /// Total read cost
    pub read_cost: u64,
    /// Total mutation cost
    pub mutate_cost: u64,
    /// Total deletion access cost
    pub delete_cost: u64,
    /// Total new storage cost (for created/grown objects)
    pub new_storage_cost: u64,
    /// Total storage rebate (from deleted/shrunk objects)
    pub storage_rebate: u64,

    /// Number of objects read
    pub objects_read: u64,
    /// Total bytes read
    pub bytes_read: u64,
    /// Number of objects mutated
    pub objects_mutated: u64,
    /// Total bytes mutated
    pub bytes_mutated: u64,
    /// Number of objects created
    pub objects_created: u64,
    /// Total bytes of new objects
    pub bytes_created: u64,
    /// Number of objects deleted
    pub objects_deleted: u64,
    /// Total bytes deleted
    pub bytes_deleted: u64,
}

impl StorageSummary {
    /// Get total storage-related gas cost (excluding rebate).
    pub fn total_cost(&self) -> u64 {
        self.read_cost
            .saturating_add(self.mutate_cost)
            .saturating_add(self.delete_cost)
            .saturating_add(self.new_storage_cost)
    }

    /// Get net storage cost after rebate.
    pub fn net_cost(&self) -> i64 {
        (self.total_cost() as i64).saturating_sub(self.storage_rebate as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_params() -> GasParameters {
        GasParameters {
            obj_access_cost_read_per_byte: 15,
            obj_access_cost_mutate_per_byte: 40,
            obj_access_cost_delete_per_byte: 40,
            obj_access_cost_verify_per_byte: 200,
            obj_data_cost_refundable: 100,
            obj_metadata_cost_non_refundable: 50,
            storage_rebate_rate: 9900, // 99%
            ..Default::default()
        }
    }

    #[test]
    fn test_charge_read() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_read(100);

        let summary = tracker.summary();
        assert_eq!(summary.read_cost, 100 * 15);
        assert_eq!(summary.objects_read, 1);
        assert_eq!(summary.bytes_read, 100);
    }

    #[test]
    fn test_charge_create() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_create(100);

        let summary = tracker.summary();
        // data_cost = 100 * 100 = 10_000
        // metadata_cost = 50
        // total = 10_050
        assert_eq!(summary.new_storage_cost, 10_050);
        assert_eq!(summary.objects_created, 1);
        assert_eq!(summary.bytes_created, 100);
    }

    #[test]
    fn test_charge_mutate_grow() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_mutate(100, 150);

        let summary = tracker.summary();
        // mutate_cost = 150 * 40 = 6_000
        // growth_cost = 50 * 100 = 5_000
        assert_eq!(summary.mutate_cost, 6_000);
        assert_eq!(summary.new_storage_cost, 5_000);
    }

    #[test]
    fn test_charge_mutate_shrink() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_mutate(150, 100);

        let summary = tracker.summary();
        // mutate_cost = 100 * 40 = 4_000
        // shrink = 50 bytes
        // shrink_storage_cost = 50 * 100 = 5_000
        // rebate = (5_000 * 9900 + 5000) / 10000 = 4_950
        assert_eq!(summary.mutate_cost, 4_000);
        assert_eq!(summary.storage_rebate, 4_950);
    }

    #[test]
    fn test_charge_delete() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_delete(100, None);

        let summary = tracker.summary();
        // delete_cost = 100 * 40 = 4_000
        // storage_cost = 100 * 100 + 50 = 10_050
        // rebate = (10_050 * 9900 + 5000) / 10000 = 9_950
        assert_eq!(summary.delete_cost, 4_000);
        assert_eq!(summary.storage_rebate, 9_950);
    }

    #[test]
    fn test_rebate_calculation() {
        let tracker = StorageTracker::new(&test_params());

        // Test Sui's exact formula: (storage_cost * rate + 5000) / 10000
        let rebate = tracker.calculate_sender_rebate(10_000);
        assert_eq!(rebate, 9_900); // (10_000 * 9900 + 5000) / 10000 = 9900
    }

    #[test]
    fn test_total_and_net_cost() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_read(100);     // 1_500
        tracker.charge_create(100);   // 10_050
        tracker.charge_delete(50, None); // delete: 2_000, rebate: 4_975

        let summary = tracker.summary();
        let total = summary.total_cost();
        let net = summary.net_cost();

        assert_eq!(total, 1_500 + 10_050 + 2_000);
        assert_eq!(net, (total as i64) - (summary.storage_rebate as i64));
    }

    #[test]
    fn test_reset() {
        let mut tracker = StorageTracker::new(&test_params());

        tracker.charge_read(100);
        tracker.charge_create(100);
        tracker.reset();

        let summary = tracker.summary();
        assert_eq!(summary.read_cost, 0);
        assert_eq!(summary.new_storage_cost, 0);
        assert_eq!(summary.objects_read, 0);
    }
}
