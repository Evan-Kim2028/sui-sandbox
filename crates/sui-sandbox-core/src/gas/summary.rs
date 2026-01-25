//! Gas summary and output structures.
//!
//! This module provides structures for representing the final gas costs
//! of a transaction, including computation, storage, and rebates.

use super::StorageSummary;

/// Complete gas summary for a transaction.
///
/// This matches the structure of Sui's on-chain gas cost reporting,
/// breaking down costs into computation, storage, and rebates.
#[derive(Debug, Clone, Default)]
pub struct GasSummary {
    /// Computation gas cost (bytecode execution, native functions)
    pub computation_cost: u64,

    /// Computation cost before bucketization (for debugging)
    pub computation_cost_pre_bucket: u64,

    /// Storage gas cost (object reads, writes, new storage)
    pub storage_cost: u64,

    /// Storage rebate (from object deletions)
    pub storage_rebate: u64,

    /// Non-refundable storage fee (burned)
    pub non_refundable_storage_fee: u64,

    /// Total gas cost (computation + storage - rebate)
    pub total_cost: u64,

    /// Gas price used for this transaction
    pub gas_price: u64,

    /// Reference gas price (for conversion)
    pub reference_gas_price: u64,

    /// Detailed storage breakdown
    pub storage_details: StorageSummary,

    /// Gas model version used
    pub gas_model_version: u64,

    /// Whether computation was bucketized
    pub bucketized: bool,
}

impl GasSummary {
    /// Create a new gas summary.
    pub fn new(
        computation_cost: u64,
        storage_cost: u64,
        storage_rebate: u64,
        gas_price: u64,
        reference_gas_price: u64,
    ) -> Self {
        let total_cost = computation_cost
            .saturating_add(storage_cost)
            .saturating_sub(storage_rebate);

        Self {
            computation_cost,
            computation_cost_pre_bucket: computation_cost,
            storage_cost,
            storage_rebate,
            non_refundable_storage_fee: 0,
            total_cost,
            gas_price,
            reference_gas_price,
            storage_details: StorageSummary::default(),
            gas_model_version: 0,
            bucketized: false,
        }
    }

    /// Create an empty/zero gas summary.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Get the effective gas used (total_cost in gas units).
    pub fn gas_used(&self) -> u64 {
        self.total_cost
    }

    /// Get the computation cost in MIST (multiplied by gas price).
    pub fn computation_cost_mist(&self) -> u64 {
        if self.reference_gas_price > 0 {
            self.computation_cost.saturating_mul(self.gas_price) / self.reference_gas_price
        } else {
            self.computation_cost.saturating_mul(self.gas_price)
        }
    }

    /// Get the storage cost in MIST (multiplied by gas price).
    pub fn storage_cost_mist(&self) -> u64 {
        if self.reference_gas_price > 0 {
            self.storage_cost.saturating_mul(self.gas_price) / self.reference_gas_price
        } else {
            self.storage_cost.saturating_mul(self.gas_price)
        }
    }

    /// Get the total cost in MIST.
    pub fn total_cost_mist(&self) -> u64 {
        if self.reference_gas_price > 0 {
            self.total_cost.saturating_mul(self.gas_price) / self.reference_gas_price
        } else {
            self.total_cost.saturating_mul(self.gas_price)
        }
    }

    /// Set detailed storage information.
    pub fn with_storage_details(mut self, details: StorageSummary) -> Self {
        self.storage_details = details;
        self
    }

    /// Set gas model version.
    pub fn with_gas_model_version(mut self, version: u64) -> Self {
        self.gas_model_version = version;
        self
    }

    /// Mark as bucketized.
    pub fn with_bucketized(mut self, pre_bucket_cost: u64) -> Self {
        self.computation_cost_pre_bucket = pre_bucket_cost;
        self.bucketized = true;
        self
    }
}

/// Builder for constructing GasSummary.
#[derive(Debug, Default)]
pub struct GasSummaryBuilder {
    computation_cost: u64,
    storage_cost: u64,
    storage_rebate: u64,
    non_refundable_storage_fee: u64,
    gas_price: u64,
    reference_gas_price: u64,
    storage_details: Option<StorageSummary>,
    gas_model_version: u64,
    pre_bucket_computation: Option<u64>,
}

impl GasSummaryBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set computation cost.
    pub fn computation_cost(mut self, cost: u64) -> Self {
        self.computation_cost = cost;
        self
    }

    /// Set storage cost.
    pub fn storage_cost(mut self, cost: u64) -> Self {
        self.storage_cost = cost;
        self
    }

    /// Set storage rebate.
    pub fn storage_rebate(mut self, rebate: u64) -> Self {
        self.storage_rebate = rebate;
        self
    }

    /// Set non-refundable storage fee.
    pub fn non_refundable_storage_fee(mut self, fee: u64) -> Self {
        self.non_refundable_storage_fee = fee;
        self
    }

    /// Set gas price.
    pub fn gas_price(mut self, price: u64) -> Self {
        self.gas_price = price;
        self
    }

    /// Set reference gas price.
    pub fn reference_gas_price(mut self, price: u64) -> Self {
        self.reference_gas_price = price;
        self
    }

    /// Set storage details.
    pub fn storage_details(mut self, details: StorageSummary) -> Self {
        self.storage_details = Some(details);
        self
    }

    /// Set gas model version.
    pub fn gas_model_version(mut self, version: u64) -> Self {
        self.gas_model_version = version;
        self
    }

    /// Set pre-bucket computation cost (for bucketization tracking).
    pub fn pre_bucket_computation(mut self, cost: u64) -> Self {
        self.pre_bucket_computation = Some(cost);
        self
    }

    /// Build the GasSummary.
    pub fn build(self) -> GasSummary {
        let total_cost = self
            .computation_cost
            .saturating_add(self.storage_cost)
            .saturating_sub(self.storage_rebate);

        GasSummary {
            computation_cost: self.computation_cost,
            computation_cost_pre_bucket: self
                .pre_bucket_computation
                .unwrap_or(self.computation_cost),
            storage_cost: self.storage_cost,
            storage_rebate: self.storage_rebate,
            non_refundable_storage_fee: self.non_refundable_storage_fee,
            total_cost,
            gas_price: self.gas_price,
            reference_gas_price: self.reference_gas_price,
            storage_details: self.storage_details.unwrap_or_default(),
            gas_model_version: self.gas_model_version,
            bucketized: self.pre_bucket_computation.is_some(),
        }
    }
}

/// Computation buckets used by Sui for gas bucketing.
///
/// Sui rounds computation costs to discrete buckets to reduce
/// transaction cost variability.
pub const COMPUTATION_BUCKETS: &[u64] = &[1_000, 5_000, 10_000, 20_000, 50_000, 200_000, 1_000_000];

/// Bucketize a computation cost to the nearest bucket.
///
/// This implements Sui's computation bucketing logic which rounds
/// costs up to predefined bucket values for more predictable fees.
pub fn bucketize_computation(cost: u64, max_bucket: u64) -> u64 {
    for &bucket in COMPUTATION_BUCKETS {
        if cost <= bucket {
            return bucket;
        }
    }
    // Above all buckets - use the actual cost or max_bucket
    cost.min(max_bucket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_summary_new() {
        let summary = GasSummary::new(1000, 500, 100, 1000, 1000);

        assert_eq!(summary.computation_cost, 1000);
        assert_eq!(summary.storage_cost, 500);
        assert_eq!(summary.storage_rebate, 100);
        assert_eq!(summary.total_cost, 1400); // 1000 + 500 - 100
    }

    #[test]
    fn test_gas_summary_mist_conversion() {
        let summary = GasSummary::new(1000, 500, 100, 2000, 1000);

        // With 2x gas price, costs in MIST should be doubled
        assert_eq!(summary.computation_cost_mist(), 2000);
        assert_eq!(summary.storage_cost_mist(), 1000);
        assert_eq!(summary.total_cost_mist(), 2800);
    }

    #[test]
    fn test_gas_summary_builder() {
        let summary = GasSummaryBuilder::new()
            .computation_cost(1000)
            .storage_cost(500)
            .storage_rebate(100)
            .gas_price(1000)
            .reference_gas_price(1000)
            .gas_model_version(8)
            .build();

        assert_eq!(summary.computation_cost, 1000);
        assert_eq!(summary.gas_model_version, 8);
        assert_eq!(summary.total_cost, 1400);
    }

    // Note: bucketize_computation is tested more thoroughly in gas::tests::test_computation_bucketization
}
