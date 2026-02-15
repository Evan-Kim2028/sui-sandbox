//! JSON-to-BCS Reconstruction Example
//!
//! Demonstrates reconstructing BCS bytes from decoded OBJECT_JSON
//! using struct layouts from Move bytecode.
//!
//! ## Usage
//!
//! ```bash
//! # Default test data
//! cargo run --example deepbook_json_bcs_only
//!
//! # Custom data file
//! DATA_FILE="./path/to/data.json" cargo run --example deepbook_json_bcs_only
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use serde::Deserialize;
use sui_sandbox_core::utilities::{
    validate_json_bcs_reconstruction, JsonBcsValidationObject, JsonBcsValidationPlan,
    JsonBcsValidationStatus,
};

// Package addresses to fetch bytecode from
const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";
const DEFAULT_DATA_FILE: &str = "./examples/data/deepbook_margin_state/object_json.json";

// =============================================================================
// Data Structures
// =============================================================================

#[derive(Debug, Deserialize)]
struct ObjectJsonData {
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
    println!("║        JSON-to-BCS Reconstruction Demo                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // 1. Load test data
    let data_file = std::env::var("DATA_FILE").unwrap_or_else(|_| DEFAULT_DATA_FILE.to_string());
    let data = match load_object_json_data(&data_file) {
        Ok(d) => d,
        Err(e) => {
            println!("Data file not found: {}\n", e);
            println!("This example requires OBJECT_JSON data.");
            println!("Provide your own data file:");
            println!("  DATA_FILE=\"./my_data.json\" cargo run --example deepbook_json_bcs_only\n");
            println!("Expected JSON format:");
            println!("  {{");
            println!("    \"description\": \"...\",");
            println!("    \"checkpoint\": 240733000,");
            println!("    \"objects\": [");
            println!("      {{");
            println!("        \"object_id\": \"0x...\",");
            println!("        \"version\": 123456,");
            println!("        \"type\": \"0xpkg::module::Type\",");
            println!("        \"object_json\": {{ ... }}");
            println!("      }}");
            println!("    ]");
            println!("  }}");
            return Ok(());
        }
    };
    let checkpoint = data
        .checkpoint
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "N/A".to_string());
    println!(
        "Loaded {} objects from checkpoint {}\n",
        data.objects.len(),
        checkpoint
    );

    // 2. Run generic JSON->BCS reconstruction validator
    let rt = tokio::runtime::Runtime::new()?;
    let report = rt.block_on(validate_json_bcs_reconstruction(&JsonBcsValidationPlan {
        package_roots: vec![AccountAddress::from_hex_literal(MARGIN_PACKAGE)?],
        type_refs: data
            .objects
            .iter()
            .map(|object| object.object_type.clone())
            .collect(),
        objects: data
            .objects
            .iter()
            .map(|object| JsonBcsValidationObject {
                object_id: object.object_id.clone(),
                version: object.version,
                object_type: object.object_type.clone(),
                object_json: object.object_json.clone(),
            })
            .collect(),
        historical_mode: data.checkpoint.is_some(),
    }))?;
    println!("Using gRPC endpoint: {}", report.grpc_endpoint);
    println!(
        "Resolved {} package roots from explicit + type-inferred dependencies",
        report.resolved_package_roots
    );
    println!(
        "Loaded {} modules from {} packages\n",
        report.module_count, report.package_count
    );

    // 3. Report validation outcomes
    println!("Results:");
    println!("─────────────────────────────────────────────────────────────────");

    for entry in &report.entries {
        let type_short = entry
            .object_type
            .split("::")
            .last()
            .unwrap_or(&entry.object_type);
        let id_short_len = 16.min(entry.object_id.len());
        let id_short = &entry.object_id[..id_short_len];

        match entry.status {
            JsonBcsValidationStatus::Match => {
                println!(
                    "  ✓ {}... {} ({} bytes)",
                    id_short,
                    type_short,
                    entry.reconstructed_len.unwrap_or_default()
                );
            }
            JsonBcsValidationStatus::Mismatch => {
                println!(
                    "  ✗ {}... {} (mismatch at byte {})",
                    id_short,
                    type_short,
                    entry.mismatch_offset.unwrap_or_default()
                );
            }
            JsonBcsValidationStatus::MissingBaseline => {
                println!("  ? {}... {} (no baseline)", id_short, type_short);
            }
            JsonBcsValidationStatus::ConversionError => {
                println!(
                    "  ✗ {}... {} - {}",
                    id_short,
                    type_short,
                    entry.error.as_deref().unwrap_or("conversion error")
                );
            }
        }
    }

    println!("─────────────────────────────────────────────────────────────────");
    let failed = report.summary.mismatched + report.summary.conversion_errors;
    println!(
        "Total: {} passed, {} failed, {} no baseline\n",
        report.summary.matched, failed, report.summary.missing_baseline
    );

    Ok(())
}

fn load_object_json_data(path: &str) -> Result<ObjectJsonData> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("Failed to read {}: {}", path, e))?;
    serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse {}: {}", path, e))
}
