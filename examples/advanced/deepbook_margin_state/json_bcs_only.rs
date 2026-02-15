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
use std::collections::HashMap;
use sui_sandbox_core::bootstrap::create_mainnet_provider;
use sui_sandbox_core::utilities::collect_required_package_roots_from_type_strings;
use sui_sandbox_core::utilities::json_to_bcs::JsonToBcsConverter;

// Package addresses to fetch bytecode from
const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";
const DEFAULT_DATA_FILE: &str = "./examples/advanced/deepbook_margin_state/data/object_json.json";

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

    // 2. Fetch bytecode and build converter
    let rt = tokio::runtime::Runtime::new()?;
    let provider = rt.block_on(create_mainnet_provider(data.checkpoint.is_some()))?;
    println!("Using gRPC endpoint: {}", provider.grpc_endpoint());

    let explicit_roots = vec![AccountAddress::from_hex_literal(MARGIN_PACKAGE)?];
    let type_roots: Vec<String> = data
        .objects
        .iter()
        .map(|object| object.object_type.clone())
        .collect();
    let package_ids: Vec<AccountAddress> =
        collect_required_package_roots_from_type_strings(&explicit_roots, &type_roots)?
            .into_iter()
            .collect();

    println!(
        "Resolved {} package roots from explicit + type-inferred dependencies",
        package_ids.len()
    );
    let packages = rt.block_on(async {
        provider
            .fetch_packages_with_deps(&package_ids, None, None)
            .await
    })?;

    let mut converter = JsonToBcsConverter::new();
    let mut total_modules = 0;
    for (_, pkg) in &packages {
        let bytecode: Vec<Vec<u8>> = pkg.modules.iter().map(|(_, b)| b.clone()).collect();
        converter.add_modules_from_bytes(&bytecode)?;
        total_modules += bytecode.len();
    }
    println!(
        "Loaded {} modules from {} packages\n",
        total_modules,
        packages.len()
    );

    // 3. Fetch actual BCS for validation
    let grpc = provider.grpc();
    let mut actual_bcs: HashMap<String, Vec<u8>> = HashMap::new();
    for obj in &data.objects {
        if let Ok(Some(grpc_obj)) = rt.block_on(async {
            grpc.get_object_at_version(&obj.object_id, Some(obj.version))
                .await
        }) {
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
        let type_short = obj
            .object_type
            .split("::")
            .last()
            .unwrap_or(&obj.object_type);
        let id_short = &obj.object_id[..16];

        match converter.convert(&obj.object_type, &obj.object_json) {
            Ok(reconstructed) => {
                if let Some(actual) = actual_bcs.get(&obj.object_id) {
                    if reconstructed == *actual {
                        println!(
                            "  ✓ {}... {} ({} bytes)",
                            id_short,
                            type_short,
                            reconstructed.len()
                        );
                        passed += 1;
                    } else {
                        let diff_at = reconstructed
                            .iter()
                            .zip(actual.iter())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                        println!(
                            "  ✗ {}... {} (mismatch at byte {})",
                            id_short, type_short, diff_at
                        );
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

fn load_object_json_data(path: &str) -> Result<ObjectJsonData> {
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("Failed to read {}: {}", path, e))?;
    serde_json::from_str(&content).map_err(|e| anyhow!("Failed to parse {}: {}", path, e))
}
