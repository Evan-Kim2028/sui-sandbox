//! Cetus DLMM (Discretized Liquidity Market Maker) calculation utilities.
//!
//! This module provides data structures and functions for:
//! - Loading DLMM position data from Snowflake JSON exports
//! - Calculating position token amounts from bin reserves
//! - Historical snapshot analysis for position tracking
//!
//! ## Key Concepts
//!
//! DLMM uses discrete bins instead of continuous ticks:
//! - Each bin has a fixed price and holds both coin_a and coin_b reserves
//! - Position liquidity is distributed across a range of bins
//! - When price moves out of range, position converts to 100% of one token
//!
//! ## Position Amount Calculation
//!
//! For each bin in the position's range:
//! ```text
//! position_amount = (position_liquidity_share / bin_total_liquidity_share) * bin_reserve
//! ```

use anyhow::{anyhow, Result};
use move_core_types::language_storage::TypeTag;
use serde::Deserialize;
use std::collections::HashMap;

use super::parse_type_tag;

// =============================================================================
// Snowflake Data Structures
// =============================================================================

/// Raw Snowflake export data containing pool and position objects.
#[derive(Debug, Deserialize)]
pub struct SnowflakeData {
    pub description: String,
    pub checkpoint: u64,
    pub objects: Vec<ObjectData>,
}

/// Individual object from Snowflake OBJECT table.
#[derive(Debug, Deserialize)]
pub struct ObjectData {
    pub object_id: String,
    pub version: u64,
    #[serde(rename = "type")]
    pub object_type: String,
    pub owner_type: String,
    pub initial_shared_version: Option<u64>,
    pub object_json: serde_json::Value,
}

// =============================================================================
// DLMM Position Data Structures
// =============================================================================

/// Extended position data with bin reserves for amount calculation.
#[derive(Debug, Deserialize)]
pub struct ExtendedPositionData {
    pub position_id: String,
    pub pool_id: String,
    pub lower_bin: i32,
    pub upper_bin: i32,
    /// Bin groups containing the bin reserves
    pub bin_groups: Vec<BinGroupData>,
    /// PositionInfo containing per-bin liquidity shares
    pub position_stats: Vec<PositionBinStat>,
}

/// Group of bins from a single dynamic field object.
#[derive(Debug, Deserialize)]
pub struct BinGroupData {
    #[allow(dead_code)]
    pub group_idx: i32,
    pub bins: Vec<BinData>,
}

/// Individual bin data with reserves and liquidity.
#[derive(Debug, Deserialize)]
pub struct BinData {
    pub bin_id: i32,
    /// Raw amount of coin_a (e.g., USDC with 6 decimals)
    pub amount_a: String,
    /// Raw amount of coin_b (e.g., SUI with 9 decimals)
    pub amount_b: String,
    /// Total liquidity share in this bin
    pub liquidity_share: String,
}

/// Position's per-bin liquidity share.
#[derive(Debug, Deserialize)]
pub struct PositionBinStat {
    pub bin_id: i32,
    pub liquidity_share: String,
}

/// Calculated position token amounts.
#[derive(Debug, Default)]
pub struct PositionAmounts {
    pub total_usdc: f64,
    pub total_sui: f64,
    pub per_bin: Vec<BinAmount>,
}

/// Per-bin breakdown of position amounts.
#[derive(Debug)]
pub struct BinAmount {
    pub bin_id: i32,
    pub usdc_amount: f64,
    pub sui_amount: f64,
    pub position_share_pct: f64,
}

// =============================================================================
// Historical Snapshot Structures
// =============================================================================

/// Historical data with daily snapshots for position tracking.
#[derive(Debug, Deserialize)]
pub struct HistoricalData {
    pub position_id: String,
    pub pool_id: String,
    pub lower_bin: i32,
    pub upper_bin: i32,
    pub position_stats: Vec<PositionBinStat>,
    pub daily_snapshots: Vec<DailySnapshot>,
}

/// Single day's snapshot of pool and position state.
#[derive(Debug, Deserialize)]
pub struct DailySnapshot {
    pub date: String,
    #[allow(dead_code)]
    pub checkpoint: u64,
    pub active_bin: i32,
    pub range_status: String,
    #[allow(dead_code)]
    pub pool_usdc: f64,
    #[allow(dead_code)]
    pub pool_sui: f64,
    pub bin_groups: Vec<BinGroupData>,
}

// =============================================================================
// Calculation Functions
// =============================================================================

/// Calculate position token amounts from extended position data.
///
/// Uses the formula:
/// `position_amount = (position_share / bin_total_share) * bin_reserve`
///
/// Returns amounts in human-readable units (USDC with 6 decimals, SUI with 9).
pub fn calculate_position_amounts(data: &ExtendedPositionData) -> PositionAmounts {
    calculate_amounts_from_bins(
        &data.bin_groups,
        &data.position_stats,
        data.lower_bin,
        data.upper_bin,
    )
}

/// Calculate position token amounts for a specific snapshot.
pub fn calculate_snapshot_amounts(
    snapshot: &DailySnapshot,
    position_stats: &[PositionBinStat],
    lower_bin: i32,
    upper_bin: i32,
) -> PositionAmounts {
    calculate_amounts_from_bins(&snapshot.bin_groups, position_stats, lower_bin, upper_bin)
}

/// Internal function to calculate amounts from bin data.
fn calculate_amounts_from_bins(
    bin_groups: &[BinGroupData],
    position_stats: &[PositionBinStat],
    lower_bin: i32,
    upper_bin: i32,
) -> PositionAmounts {
    let mut result = PositionAmounts::default();

    // Build a map of bin_id -> (amount_a, amount_b, liquidity_share)
    let mut bin_reserves: HashMap<i32, (u128, u128, u128)> = HashMap::new();
    for group in bin_groups {
        for bin in &group.bins {
            let amount_a = bin.amount_a.parse::<u128>().unwrap_or(0);
            let amount_b = bin.amount_b.parse::<u128>().unwrap_or(0);
            let liq_share = bin.liquidity_share.parse::<u128>().unwrap_or(0);
            bin_reserves.insert(bin.bin_id, (amount_a, amount_b, liq_share));
        }
    }

    // Build a map of bin_id -> position_liquidity_share
    let mut position_shares: HashMap<i32, u128> = HashMap::new();
    for stat in position_stats {
        let share = stat.liquidity_share.parse::<u128>().unwrap_or(0);
        position_shares.insert(stat.bin_id, share);
    }

    // Calculate position amounts for each bin in range
    for bin_id in lower_bin..=upper_bin {
        if let (Some(&(amount_a, amount_b, bin_total_share)), Some(&pos_share)) =
            (bin_reserves.get(&bin_id), position_shares.get(&bin_id))
        {
            if bin_total_share == 0 {
                continue;
            }

            // position_amount = position_share / bin_total_share * bin_reserve
            let share_ratio = pos_share as f64 / bin_total_share as f64;
            let pos_amount_a = share_ratio * amount_a as f64;
            let pos_amount_b = share_ratio * amount_b as f64;

            // Convert to human-readable amounts (USDC=6 decimals, SUI=9 decimals)
            let usdc_amount = pos_amount_a / 1_000_000.0;
            let sui_amount = pos_amount_b / 1_000_000_000.0;

            result.total_usdc += usdc_amount;
            result.total_sui += sui_amount;

            result.per_bin.push(BinAmount {
                bin_id,
                usdc_amount,
                sui_amount,
                position_share_pct: share_ratio * 100.0,
            });
        }
    }

    result
}

// =============================================================================
// Display Functions
// =============================================================================

/// Display current position amounts with per-bin breakdown.
pub fn display_position_amounts(
    position_id: &str,
    pool_id: &str,
    lower_bin: i32,
    upper_bin: i32,
    active_bin: i32,
    amounts: &PositionAmounts,
) {
    let range_status = get_range_status(active_bin, lower_bin, upper_bin);

    println!("\n  ═══════════════════════════════════════════════════════════════");
    println!("  POSITION TOKEN AMOUNTS (calculated from Snowflake bin data)");
    println!("  ═══════════════════════════════════════════════════════════════");
    println!("  Position: {}", position_id);
    println!("  Pool: {}", pool_id);
    println!(
        "  Bin Range: {} to {} ({} bins)",
        lower_bin,
        upper_bin,
        upper_bin - lower_bin + 1
    );
    println!();
    println!("  Total USDC (coin_a): {:.6} USDC", amounts.total_usdc);
    println!("  Total SUI  (coin_b): {:.9} SUI", amounts.total_sui);
    println!();
    println!("  Active Bin: {} ({})", active_bin, range_status);
    println!("  ═══════════════════════════════════════════════════════════════");

    // Per-bin breakdown
    if !amounts.per_bin.is_empty() {
        println!("\n  Per-bin breakdown:");
        println!(
            "  {:>6}  {:>14}  {:>14}  {:>10}",
            "Bin", "USDC", "SUI", "Share %"
        );
        println!(
            "  {}  {}  {}  {}",
            "-".repeat(6),
            "-".repeat(14),
            "-".repeat(14),
            "-".repeat(10)
        );

        for bin_amt in &amounts.per_bin {
            println!(
                "  {:>6}  {:>14.6}  {:>14.9}  {:>9.4}%",
                bin_amt.bin_id, bin_amt.usdc_amount, bin_amt.sui_amount, bin_amt.position_share_pct
            );
        }
    }
}

/// Display historical position view with daily snapshots.
pub fn display_historical_view(data: &HistoricalData) {
    println!("\n  ═══════════════════════════════════════════════════════════════════════════");
    println!("  HISTORICAL POSITION VIEW - 7 DAY SUMMARY");
    println!("  ═══════════════════════════════════════════════════════════════════════════");
    println!("  Position: {}", data.position_id);
    println!("  Pool: {}", data.pool_id);
    println!("  Bin Range: {} to {}", data.lower_bin, data.upper_bin);
    println!();
    println!(
        "  {:>12}  {:>6}  {:>12}  {:>14}  {:>14}  {:>14}",
        "Date", "Active", "Status", "USDC", "SUI", "Total Value*"
    );
    println!(
        "  {}  {}  {}  {}  {}  {}",
        "-".repeat(12),
        "-".repeat(6),
        "-".repeat(12),
        "-".repeat(14),
        "-".repeat(14),
        "-".repeat(14)
    );

    for snapshot in &data.daily_snapshots {
        let amounts = calculate_snapshot_amounts(
            snapshot,
            &data.position_stats,
            data.lower_bin,
            data.upper_bin,
        );

        let status_short = match snapshot.range_status.as_str() {
            "IN_RANGE" => "IN_RANGE",
            "ABOVE_RANGE" => "ABOVE",
            _ => "BELOW",
        };

        println!(
            "  {:>12}  {:>6}  {:>12}  {:>14.6}  {:>14.9}  {:>14}",
            snapshot.date,
            snapshot.active_bin,
            status_short,
            amounts.total_usdc,
            amounts.total_sui,
            "-" // Price oracle needed for USD value
        );
    }

    println!();
    println!("  * Total Value requires external price oracle (e.g., Pyth) for SUI/USD price");
    println!("  ═══════════════════════════════════════════════════════════════════════════");
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Get range status based on active bin position.
pub fn get_range_status(active_bin: i32, lower_bin: i32, upper_bin: i32) -> &'static str {
    if active_bin > upper_bin {
        "ABOVE_RANGE (100% SUI)"
    } else if active_bin < lower_bin {
        "BELOW_RANGE (100% USDC)"
    } else {
        "IN_RANGE (mixed)"
    }
}

/// Extract coin type parameters from a Pool type string.
///
/// Given a type like `0x123::pool::Pool<0xabc::usdc::USDC, 0x2::sui::SUI>`,
/// returns the two coin TypeTags.
pub fn extract_coin_types_from_pool(pool_type: &str) -> Result<(TypeTag, TypeTag)> {
    let tag = parse_type_tag(pool_type).ok_or_else(|| anyhow!("Failed to parse pool type"))?;
    let TypeTag::Struct(struct_tag) = tag else {
        return Err(anyhow!("Pool type is not a struct"));
    };
    if struct_tag.type_params.len() < 2 {
        return Err(anyhow!(
            "Pool type does not contain coin type params (got {})",
            struct_tag.type_params.len()
        ));
    }
    Ok((
        struct_tag.type_params[0].clone(),
        struct_tag.type_params[1].clone(),
    ))
}

/// Load Snowflake data from a JSON file.
pub fn load_snowflake_data(path: &str) -> Result<SnowflakeData> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("Failed to read {}: {}", path, e))?;
    serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse {}: {}", path, e))
}

/// Load extended position data from a JSON file.
pub fn load_extended_position_data(path: &str) -> Result<ExtendedPositionData> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("Failed to read {}: {}", path, e))?;
    serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse {}: {}", path, e))
}

/// Load historical data from a JSON file.
pub fn load_historical_data(path: &str) -> Result<HistoricalData> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("Failed to read {}: {}", path, e))?;
    serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse {}: {}", path, e))
}
