//! Tiered instruction cost tables for accurate gas metering.
//!
//! This module provides access to Sui's tiered cost tables, which charge
//! progressively more gas as execution grows (instructions executed,
//! stack height, stack size).
//!
//! # Tiering System
//!
//! Sui uses a tiered pricing model where costs increase as resources are consumed:
//!
//! - **Instructions**: Cost multiplier increases after 20K, 50K, 100K, 200K, 10M instructions
//! - **Stack Height**: Cost multiplier increases after 1K, 10K stack frames
//! - **Stack Size**: Cost multiplier increases after 100K, 500K, 1M, 100M bytes
//!
//! This prevents DoS attacks while keeping simple transactions cheap.

use sui_types::gas_model::units_types::CostTable;

/// Get the appropriate cost table for a gas model version.
///
/// Different protocol versions use different cost schedules:
/// - v1-3: initial_cost_schedule_v1
/// - v4: initial_cost_schedule_v2
/// - v5: initial_cost_schedule_v3
/// - v6-7: initial_cost_schedule_v4
/// - v8+: initial_cost_schedule_v5
///
/// This mirrors `sui_types::gas_model::gas_predicates::cost_table_for_version`.
pub fn cost_table_for_version(gas_model_version: u64) -> CostTable {
    // Use the same logic as Sui's gas_predicates module
    sui_types::gas_model::gas_predicates::cost_table_for_version(gas_model_version)
}

/// Get a zero-cost schedule for unmetered execution.
pub fn zero_cost_schedule() -> CostTable {
    sui_types::gas_model::tables::ZERO_COST_SCHEDULE.clone()
}

/// Get the default (initial) cost schedule.
pub fn default_cost_schedule() -> CostTable {
    sui_types::gas_model::tables::INITIAL_COST_SCHEDULE.clone()
}

/// Instruction tier information for a given instruction count.
#[derive(Debug, Clone, Copy)]
pub struct TierInfo {
    /// Current cost multiplier
    pub multiplier: u64,
    /// Next tier threshold (None if at max tier)
    pub next_tier_start: Option<u64>,
}

/// Get tier information for a given instruction count.
pub fn instruction_tier_info(cost_table: &CostTable, instruction_count: u64) -> TierInfo {
    let (multiplier, next_tier_start) = cost_table.instruction_tier(instruction_count);
    TierInfo {
        multiplier,
        next_tier_start,
    }
}

/// Get tier information for a given stack height.
pub fn stack_height_tier_info(cost_table: &CostTable, stack_height: u64) -> TierInfo {
    let (multiplier, next_tier_start) = cost_table.stack_height_tier(stack_height);
    TierInfo {
        multiplier,
        next_tier_start,
    }
}

/// Get tier information for a given stack size.
pub fn stack_size_tier_info(cost_table: &CostTable, stack_size: u64) -> TierInfo {
    let (multiplier, next_tier_start) = cost_table.stack_size_tier(stack_size);
    TierInfo {
        multiplier,
        next_tier_start,
    }
}

/// Summary of cost table tiers for debugging/display.
#[derive(Debug, Clone)]
pub struct CostTableSummary {
    pub gas_model_version: u64,
    pub instruction_tiers: Vec<(u64, u64)>,
    pub stack_height_tiers: Vec<(u64, u64)>,
    pub stack_size_tiers: Vec<(u64, u64)>,
}

impl CostTableSummary {
    /// Create a summary for a given gas model version.
    pub fn for_version(gas_model_version: u64) -> Self {
        let table = cost_table_for_version(gas_model_version);
        Self {
            gas_model_version,
            instruction_tiers: table.instruction_tiers.into_iter().collect(),
            stack_height_tiers: table.stack_height_tiers.into_iter().collect(),
            stack_size_tiers: table.stack_size_tiers.into_iter().collect(),
        }
    }

    /// Format the tiers as a human-readable string.
    pub fn format_tiers(&self) -> String {
        let mut s = format!("Cost Table for Gas Model v{}\n", self.gas_model_version);
        s.push_str("\nInstruction Tiers:\n");
        for (threshold, multiplier) in &self.instruction_tiers {
            s.push_str(&format!("  {} instructions -> {}x cost\n", threshold, multiplier));
        }
        s.push_str("\nStack Height Tiers:\n");
        for (threshold, multiplier) in &self.stack_height_tiers {
            s.push_str(&format!("  {} frames -> {}x cost\n", threshold, multiplier));
        }
        s.push_str("\nStack Size Tiers:\n");
        for (threshold, multiplier) in &self.stack_size_tiers {
            s.push_str(&format!("  {} bytes -> {}x cost\n", threshold, multiplier));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_table_for_version() {
        // Test different gas model versions get different tables
        let v1 = cost_table_for_version(1);
        let v8 = cost_table_for_version(8);

        // v8 should have the 10M instruction tier that v1 doesn't
        assert!(
            v8.instruction_tiers.contains_key(&10_000_000),
            "v8 should have 10M instruction tier"
        );
    }

    #[test]
    fn test_instruction_tiers_v8() {
        let table = cost_table_for_version(8);

        // Test tier progression for v8 (uses v5 cost schedule)
        let tier_0 = instruction_tier_info(&table, 0);
        assert_eq!(tier_0.multiplier, 1);
        assert_eq!(tier_0.next_tier_start, Some(20_000));

        let tier_20k = instruction_tier_info(&table, 20_000);
        assert_eq!(tier_20k.multiplier, 2);
        assert_eq!(tier_20k.next_tier_start, Some(50_000));

        let tier_50k = instruction_tier_info(&table, 50_000);
        assert_eq!(tier_50k.multiplier, 10);

        let tier_100k = instruction_tier_info(&table, 100_000);
        assert_eq!(tier_100k.multiplier, 50);

        let tier_200k = instruction_tier_info(&table, 200_000);
        assert_eq!(tier_200k.multiplier, 100);

        let tier_10m = instruction_tier_info(&table, 10_000_000);
        assert_eq!(tier_10m.multiplier, 1000);
    }

    #[test]
    fn test_zero_cost_schedule() {
        let table = zero_cost_schedule();

        let tier = instruction_tier_info(&table, 1_000_000);
        assert_eq!(tier.multiplier, 0, "Zero cost schedule should have 0 multiplier");
    }

    #[test]
    fn test_cost_table_summary() {
        let summary = CostTableSummary::for_version(8);

        assert_eq!(summary.gas_model_version, 8);
        assert!(!summary.instruction_tiers.is_empty());
        assert!(!summary.stack_height_tiers.is_empty());
        assert!(!summary.stack_size_tiers.is_empty());

        // Test formatting doesn't panic
        let formatted = summary.format_tiers();
        assert!(formatted.contains("Gas Model v8"));
    }
}
