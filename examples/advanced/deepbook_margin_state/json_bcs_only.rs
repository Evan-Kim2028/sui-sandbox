//! JSON-to-BCS Reconstruction Example
//!
//! Demonstrates reconstructing BCS bytes from decoded OBJECT_JSON
//! using struct layouts from Move bytecode.
//!
//! ## Usage
//!
//! ```bash
//! # Default manifest scenario
//! cargo run --example deepbook_json_bcs_only
//!
//! # Or supply a custom OBJECT_JSON file
//! DATA_FILE=./path/to/object_json.json cargo run --example deepbook_json_bcs_only
//! ```

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use serde::Deserialize;
use std::path::PathBuf;
use sui_sandbox_core::utilities::{
    validate_json_bcs_reconstruction, JsonBcsValidationObject, JsonBcsValidationPlan,
    JsonBcsValidationStatus,
};
#[path = "../../deepbook_scenarios.rs"]
mod deepbook_scenarios;

const DEFAULT_FALLBACK_PACKAGE_ROOTS: [&str; 2] = [
    "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497",
];

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

    let scenario = deepbook_scenarios::resolve_scenario(Some("position_a_json_bcs"))
        .and_then(|scenario| deepbook_scenarios::require_kind(&scenario, "json_bcs").cloned())
        .map_err(|err| anyhow!("{}", err))?;
    let scenario_description = scenario
        .description
        .unwrap_or_else(|| "no description".to_string());
    let data_path = std::env::var("DATA_FILE")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            scenario
                .object_json_file
                .as_deref()
                .map(deepbook_scenarios::scenario_data_path)
        })
        .ok_or_else(|| {
            anyhow!(
                "Scenario '{}' is missing 'object_json_file' (and DATA_FILE is unset)",
                scenario.id
            )
        })?;
    let package_roots = scenario.package_roots.clone().unwrap_or_else(|| {
        DEFAULT_FALLBACK_PACKAGE_ROOTS
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    });

    println!();
    println!(
        "DeepBook JSON/BCS scenario: {} - {}",
        scenario.id, scenario_description
    );

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║        JSON-to-BCS Reconstruction Demo                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // 1. Load test data
    let data = match load_object_json_data(&data_path) {
        Ok(d) => d,
        Err(e) => {
            println!("Data file not found: {}\n", e);
            println!("This example requires OBJECT_JSON data.");
            println!("Scenario '{}': {}", scenario.id, scenario_description);
            println!("Expected fixture file: {}", data_path.display());
            println!("Provide your own data file:");
            println!(
                "  DATA_FILE=./my_object_json.json cargo run --example deepbook_json_bcs_only\n"
            );
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
        package_roots: package_roots
            .into_iter()
            .map(|root| AccountAddress::from_hex_literal(root.as_str()))
            .collect::<std::result::Result<Vec<_>, _>>()?,
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

fn load_object_json_data(path: &PathBuf) -> Result<ObjectJsonData> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse {}: {}", path.display(), e))
}
