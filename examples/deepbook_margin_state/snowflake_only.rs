//! Snowflake-Only BCS Reconstruction Example
//!
//! Demonstrates reconstructing BCS bytes from Snowflake's OBJECT_JSON
//! using struct layouts from Move bytecode.
//!
//! ## Usage
//!
//! ```bash
//! # Default test data
//! cargo run --example deepbook_snowflake_only
//!
//! # Custom data file
//! DATA_FILE="./path/to/data.json" cargo run --example deepbook_snowflake_only
//! ```

mod common;
mod json_to_bcs;

use anyhow::{anyhow, Result};
use json_to_bcs::JsonToBcsConverter;
use move_core_types::account_address::AccountAddress;
use serde::Deserialize;
use std::collections::HashMap;
use sui_state_fetcher::HistoricalStateProvider;

// Package addresses to fetch bytecode from
const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";
const DEEPBOOK_PACKAGE: &str = "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809";
const DEFAULT_DATA_FILE: &str = "./examples/deepbook_margin_state/data/snowflake_object_json.json";

// =============================================================================
// Data Structures
// =============================================================================

#[derive(Debug, Deserialize)]
struct SnowflakeData {
    #[allow(dead_code)]
    description: String,
    #[serde(alias = "checkpoint_range")]
    checkpoint: Option<serde_json::Value>,
    objects: Vec<ObjectData>,
}

#[derive(Debug, Deserialize)]
struct ObjectData {
    object_id: String,
    version: u64,
    #[serde(rename = "type")]
    object_type: String,
    #[allow(dead_code)]
    bcs_length: Option<u64>,
    #[allow(dead_code)]
    category: Option<String>,
    object_json: serde_json::Value,
}

// =============================================================================
// Main
// =============================================================================

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║        Snowflake BCS Reconstruction Demo                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // 1. Load test data
    let data_file = std::env::var("DATA_FILE").unwrap_or_else(|_| DEFAULT_DATA_FILE.to_string());
    let data = load_snowflake_data(&data_file)?;
    let checkpoint = data.checkpoint.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "N/A".to_string());
    println!("Loaded {} objects from checkpoint {}\n", data.objects.len(), checkpoint);

    // 2. Fetch bytecode and build converter
    let rt = tokio::runtime::Runtime::new()?;
    let provider = rt.block_on(async { HistoricalStateProvider::mainnet().await })?;

    let package_ids = vec![
        AccountAddress::from_hex_literal(MARGIN_PACKAGE)?,
        AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE)?,
    ];
    let packages = rt.block_on(async { provider.fetch_packages_with_deps(&package_ids, None, None).await })?;

    let mut converter = JsonToBcsConverter::new();
    let mut total_modules = 0;
    for (_, pkg) in &packages {
        let bytecode: Vec<Vec<u8>> = pkg.modules.iter().map(|(_, b)| b.clone()).collect();
        converter.add_modules_from_bytes(&bytecode)?;
        total_modules += bytecode.len();
    }
    println!("Loaded {} modules from {} packages\n", total_modules, packages.len());

    // 3. Fetch actual BCS for validation
    let grpc = provider.grpc();
    let mut actual_bcs: HashMap<String, Vec<u8>> = HashMap::new();
    for obj in &data.objects {
        if let Ok(Some(grpc_obj)) = rt.block_on(async { grpc.get_object_at_version(&obj.object_id, Some(obj.version)).await }) {
            if let Some(bcs) = grpc_obj.bcs {
                actual_bcs.insert(obj.object_id.clone(), bcs);
            }
        }
    }

    // 4. Test BCS reconstruction
    println!("Results:");
    println!("─────────────────────────────────────────────────────────────────");

    let mut passed = 0;
    let mut failed = 0;

    for obj in &data.objects {
        let type_short = obj.object_type.split("::").last().unwrap_or(&obj.object_type);
        let id_short = &obj.object_id[..16];

        match converter.convert(&obj.object_type, &obj.object_json) {
            Ok(reconstructed) => {
                if let Some(actual) = actual_bcs.get(&obj.object_id) {
                    if reconstructed == *actual {
                        println!("  ✓ {}... {} ({} bytes)", id_short, type_short, reconstructed.len());
                        passed += 1;
                    } else {
                        let diff_at = reconstructed.iter().zip(actual.iter()).position(|(a, b)| a != b).unwrap_or(0);
                        println!("  ✗ {}... {} (mismatch at byte {})", id_short, type_short, diff_at);
                        failed += 1;
                    }
                } else {
                    println!("  ? {}... {} (no baseline)", id_short, type_short);
                }
            }
            Err(e) => {
                println!("  ✗ {}... {} - {}", id_short, type_short, e);
                failed += 1;
            }
        }
    }

    println!("─────────────────────────────────────────────────────────────────");
    println!("Total: {} passed, {} failed\n", passed, failed);

    Ok(())
}

fn load_snowflake_data(path: &str) -> Result<SnowflakeData> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("Failed to read {}: {}", path, e))?;
    serde_json::from_str(&content)
        .map_err(|e| anyhow!("Failed to parse {}: {}", path, e))
}
