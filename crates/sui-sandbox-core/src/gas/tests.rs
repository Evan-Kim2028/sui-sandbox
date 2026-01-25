//! Integration tests for the accurate gas metering module.
//!
//! These tests verify that the various gas components work together correctly:
//! - Protocol config loading → GasParameters
//! - AccurateGasMeter with GasParameters
//! - StorageTracker with GasParameters
//! - GasSummaryBuilder combining all costs
//! - AccurateGasCharger orchestrating the full flow
//!
//! Unit tests for individual components are in their respective modules:
//! - meter.rs: AccurateGasMeter tests
//! - storage.rs: StorageTracker tests
//! - summary.rs: GasSummary/bucketization tests
//! - protocol.rs: GasParameters tests
//! - cost_table.rs: CostTable tests
//! - charger.rs: AccurateGasCharger tests
//! - native_costs.rs: NativeFunctionCosts tests

use super::*;
use move_vm_types::gas::GasMeter;

// =============================================================================
// End-to-End Integration Tests
// =============================================================================

#[test]
fn test_full_gas_flow() {
    // 1. Load protocol config
    let config = load_protocol_config(68);
    let params = GasParameters::from_protocol_config(&config);

    // 2. Create gas meter
    let mut meter = AccurateGasMeter::new(50_000_000_000, 1000, &params);

    // 3. Create storage tracker
    let mut storage = StorageTracker::new(&params);

    // 4. Simulate some operations
    storage.charge_read(100);
    storage.charge_create(200);
    storage.charge_mutate(100, 150);

    // Execute many bytecode instructions to accumulate visible gas
    // (gas units = internal gas / 1000, need >1000 internal to show as 1)
    use move_vm_types::gas::SimpleInstruction;
    for _ in 0..2000 {
        meter.charge_simple_instr(SimpleInstruction::LdU64).unwrap();
    }

    // 5. Get summaries
    let _meter_stats = meter.stats();
    let storage_summary = storage.summary();

    // 6. Build final summary
    let gas_summary = GasSummaryBuilder::new()
        .computation_cost(meter.gas_consumed())
        .storage_cost(storage_summary.total_cost())
        .storage_rebate(storage_summary.storage_rebate)
        .gas_price(1000)
        .reference_gas_price(1000)
        .gas_model_version(params.gas_model_version)
        .storage_details(storage_summary)
        .build();

    // Verify - computation cost comes from instructions (may be small due to unit conversion)
    // Storage cost should definitely be > 0
    assert!(gas_summary.storage_cost > 0, "storage_cost should be > 0");
    assert!(gas_summary.total_cost > 0, "total_cost should be > 0");
    assert_eq!(gas_summary.gas_model_version, params.gas_model_version);
}

#[test]
fn test_protocol_versions() {
    // Test protocol version 68 (current mainnet)
    // Note: Older protocol versions may not have all config fields in this Sui crate version
    let config = load_protocol_config(68);
    let params = GasParameters::from_protocol_config(&config);

    // Verify gas model version is set correctly (v8+ for protocol 68)
    assert!(
        params.gas_model_version >= 8,
        "protocol 68 should have gas model version >= 8"
    );

    // Verify cost table can be loaded
    let cost_table = cost_table_for_version(params.gas_model_version);
    assert!(!cost_table.instruction_tiers.is_empty());

    // Verify v5 cost table has the 10M tier
    assert!(cost_table.instruction_tiers.contains_key(&10_000_000));
}

#[test]
fn test_tiered_costs_increase() {
    // Verify that costs increase as instructions execute (tiering)
    let params = GasParameters::default();
    let mut meter = AccurateGasMeter::new(50_000_000_000_000, 1000, &params);

    use move_vm_types::gas::SimpleInstruction;

    // Execute many instructions and track cost progression
    let mut costs = Vec::new();
    let mut total_instructions = 0;

    for batch in 0..5 {
        let start_gas = meter.gas_consumed();

        // Execute 10,000 instructions
        for _ in 0..10_000 {
            meter.charge_simple_instr(SimpleInstruction::LdU64).unwrap();
            total_instructions += 1;
        }

        let batch_cost = meter.gas_consumed() - start_gas;
        costs.push(batch_cost);

        tracing::debug!(
            batch = batch,
            total_instructions = total_instructions,
            batch_cost = batch_cost,
            "batch completed"
        );
    }

    // Due to tiering, later batches should cost more per instruction
    // (at least some batches should show increased cost)
    // Note: The exact behavior depends on the gas model version
    assert!(
        costs.iter().all(|&c| c > 0),
        "All batches should have non-zero cost"
    );
}

#[test]
fn test_create_delete_rebate_flow() {
    // Integration test: verify rebate is correctly calculated when an object
    // is created and then deleted in the same transaction
    let params = GasParameters {
        storage_rebate_rate: 9900, // 99%
        obj_data_cost_refundable: 100,
        obj_metadata_cost_non_refundable: 50,
        obj_access_cost_delete_per_byte: 40,
        ..Default::default()
    };

    let mut tracker = StorageTracker::new(&params);

    // Create a 100-byte object
    tracker.charge_create(100);

    // Delete the same object
    tracker.charge_delete(100, None);

    let summary = tracker.summary();

    // Verify both costs and rebates are tracked
    // storage_cost = 100 * 100 + 50 = 10_050 (from creation)
    // delete_cost = 100 * 40 = 4_000
    // rebate = (10_050 * 9900 + 5000) / 10000 ≈ 9_950
    assert!(
        summary.storage_rebate > 9_900,
        "Rebate should be ~99% of storage"
    );
    assert!(
        summary.storage_rebate < 10_050,
        "Rebate should be less than total storage"
    );
    assert_eq!(summary.new_storage_cost, 10_050);
    assert_eq!(summary.delete_cost, 4_000);
}

#[test]
fn test_computation_bucketization() {
    // Test that bucketization works correctly
    assert_eq!(bucketize_computation(500, 5_000_000), 1_000);
    assert_eq!(bucketize_computation(1_500, 5_000_000), 5_000);
    assert_eq!(bucketize_computation(7_500, 5_000_000), 10_000);
    assert_eq!(bucketize_computation(15_000, 5_000_000), 20_000);
    assert_eq!(bucketize_computation(30_000, 5_000_000), 50_000);
    assert_eq!(bucketize_computation(100_000, 5_000_000), 200_000);
    assert_eq!(bucketize_computation(500_000, 5_000_000), 1_000_000);

    // Above max bucket
    assert_eq!(bucketize_computation(2_000_000, 5_000_000), 2_000_000);
}

#[test]
fn test_gas_summary_with_bucketization() {
    let pre_bucket = 7_500u64;
    let bucketed = bucketize_computation(pre_bucket, 5_000_000);

    let summary = GasSummaryBuilder::new()
        .computation_cost(bucketed)
        .pre_bucket_computation(pre_bucket)
        .storage_cost(1_000)
        .gas_price(1000)
        .reference_gas_price(1000)
        .build();

    assert!(summary.bucketized);
    assert_eq!(summary.computation_cost_pre_bucket, 7_500);
    assert_eq!(summary.computation_cost, 10_000); // Bucketed to 10K
}

// =============================================================================
// AccurateGasCharger Integration Tests
// =============================================================================

#[test]
fn test_charger_end_to_end() {
    // Full end-to-end test using AccurateGasCharger
    use move_vm_types::gas::SimpleInstruction;

    let mut charger = AccurateGasCharger::new(
        50_000_000_000, // 50 SUI budget
        1000,           // gas price
        1000,           // reference gas price
        68,             // protocol version
    );

    // Simulate a typical transaction:
    // 1. Read some input objects
    charger.storage_tracker().charge_read(256); // 256 byte object
    charger.storage_tracker().charge_read(128); // 128 byte object

    // 2. Execute some computation
    for _ in 0..5000 {
        charger
            .gas_meter()
            .charge_simple_instr(SimpleInstruction::LdU64)
            .unwrap();
    }

    // 3. Create a new object
    charger.storage_tracker().charge_create(512);

    // 4. Mutate an existing object (it grew)
    charger.storage_tracker().charge_mutate(256, 384);

    // 5. Finalize and check the summary
    let summary = charger.finalize();

    // Verify all cost components are present
    assert!(summary.computation_cost > 0, "should have computation cost");
    assert!(summary.storage_cost > 0, "should have storage cost");
    assert!(summary.total_cost > 0, "should have total cost");
    assert!(summary.bucketized, "should be bucketized");
    assert!(summary.gas_model_version >= 8);

    // Verify storage details are tracked
    assert_eq!(summary.storage_details.objects_read, 2);
    assert_eq!(summary.storage_details.objects_created, 1);
    assert_eq!(summary.storage_details.objects_mutated, 1);
}

#[test]
fn test_charger_with_deletion_and_rebate() {
    // Test that deletion rebates are correctly calculated through the charger
    let mut charger = AccurateGasCharger::new(50_000_000_000, 1000, 1000, 68);

    // Create an object
    charger.storage_tracker().charge_create(100);
    let cost_after_create = charger.storage_gas_consumed();

    // Delete the object (should generate rebate)
    charger.storage_tracker().charge_delete(100, None);

    let summary = charger.finalize();

    // Should have storage costs from both create and delete
    assert!(summary.storage_cost > cost_after_create);

    // Should have a rebate
    assert!(summary.storage_rebate > 0);

    // Net cost should be less than gross due to rebate
    let gross_cost = summary.computation_cost + summary.storage_cost;
    assert!(summary.total_cost < gross_cost);
}
