//! Cetus DLMM Position Inspector with Historical Data
//!
//! This example demonstrates the end-to-end flow of:
//! 1. Loading OBJECT_JSON from a data file
//! 2. Reconstructing BCS from JSON using Move bytecode layouts
//! 3. Loading objects into the Move VM simulation
//! 4. Executing DLMM view functions via PTB
//! 5. Calculating position token amounts from bin data
//!
//! ## Usage
//!
//! ```bash
//! # Use default test position
//! cargo run --example cetus_dlmm_position
//!
//! # Use custom data file
//! DLMM_DATA_FILE="./my_data.json" cargo run --example cetus_dlmm_position
//! ```

mod common;

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use std::collections::HashSet;

use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput};
use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_sandbox_core::utilities::is_framework_package;
use sui_state_fetcher::HistoricalStateProvider;

use common::json_bcs::JsonToBcsConverter;
use common::{
    calculate_position_amounts, create_child_fetcher, display_historical_view,
    display_position_amounts, extract_coin_types_from_pool, extract_package_ids_from_type,
    load_extended_position_data, load_historical_data, load_object_json_data, parse_type_tag,
};

// =============================================================================
// Constants
// =============================================================================

const CETUS_DLMM_POOL_PACKAGE: &str =
    "0x5664f9d3fd82c84023870cfbda8ea84e14c8dd56ce557ad2116e0668581a682b";

const DEFAULT_DATA_FILE: &str =
    "./examples/cetus_historical_position_fees/data/dlmm_objects.json";

const DEFAULT_EXTENDED_DATA_FILE: &str =
    "./examples/cetus_historical_position_fees/data/dlmm_extended_data.json";

const DEFAULT_HISTORICAL_DATA_FILE: &str =
    "./examples/cetus_historical_position_fees/data/dlmm_historical_snapshots.json";

// =============================================================================
// Data Structures
// =============================================================================

/// Converted object data with BCS bytes ready for VM loading.
struct ConvertedObject {
    object_id: String,
    object_type: String,
    bcs: Vec<u8>,
    is_shared: bool,
    version: u64,
    #[allow(dead_code)]
    initial_shared_version: Option<u64>,
}

// =============================================================================
// Main
// =============================================================================

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║          Cetus DLMM Position - Historical PTB                         ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    // -------------------------------------------------------------------------
    // Step 1: Load object JSON data
    // -------------------------------------------------------------------------
    let data_file =
        std::env::var("DLMM_DATA_FILE").unwrap_or_else(|_| DEFAULT_DATA_FILE.to_string());

    println!("Step 1: Load object JSON data from {}", data_file);

    let data = match load_object_json_data(&data_file) {
        Ok(d) => d,
        Err(e) => {
            println!("  ! Data file not found: {}", e);
            println!("  ! Creating sample data file...");
            create_sample_data_file(&data_file)?;
            println!("  ! Please populate the data file with OBJECT_JSON and re-run.");
            return Ok(());
        }
    };

    println!("  Description: {}", data.description);
    println!("  Checkpoint: {}", data.checkpoint);
    println!("  Objects: {}", data.objects.len());

    for obj in &data.objects {
        let type_short = obj
            .object_type
            .split("::")
            .last()
            .unwrap_or(&obj.object_type);
        println!(
            "    - {}... {} (v{})",
            &obj.object_id[..16],
            type_short,
            obj.version
        );
    }

    // -------------------------------------------------------------------------
    // Step 2: Fetch bytecode packages
    // -------------------------------------------------------------------------
    println!("\nStep 2: Fetch Move bytecode for struct layouts");

    let rt = tokio::runtime::Runtime::new()?;
    let provider = rt.block_on(async { HistoricalStateProvider::mainnet().await })?;

    // Collect all unique package IDs from object types
    let mut package_ids: HashSet<String> = HashSet::new();
    package_ids.insert(CETUS_DLMM_POOL_PACKAGE.to_string());

    for obj in &data.objects {
        for pkg in extract_package_ids_from_type(&obj.object_type) {
            if !is_framework_package(&pkg) {
                package_ids.insert(pkg);
            }
        }
    }

    let package_addrs: Vec<AccountAddress> = package_ids
        .iter()
        .filter_map(|p| AccountAddress::from_hex_literal(p).ok())
        .collect();

    println!(
        "  Fetching {} packages with dependencies...",
        package_addrs.len()
    );

    let packages = rt.block_on(async {
        provider
            .fetch_packages_with_deps(&package_addrs, None, None)
            .await
    })?;

    // Build the JSON to BCS converter
    let mut converter = JsonToBcsConverter::new();
    let mut total_modules = 0;
    for (pkg_id, pkg) in &packages {
        let bytecode: Vec<Vec<u8>> = pkg.modules.iter().map(|(_, b)| b.clone()).collect();
        converter.add_modules_from_bytes(&bytecode)?;
        total_modules += bytecode.len();
        println!("    Loaded {} ({} modules)", pkg_id, bytecode.len());
    }
    println!(
        "  Total: {} modules from {} packages",
        total_modules,
        packages.len()
    );

    // -------------------------------------------------------------------------
    // Step 3: Convert OBJECT_JSON to BCS
    // -------------------------------------------------------------------------
    println!("\nStep 3: Reconstruct BCS from OBJECT_JSON");

    let mut converted_objects: Vec<ConvertedObject> = Vec::new();

    for obj in &data.objects {
        let type_short = obj
            .object_type
            .split("::")
            .last()
            .unwrap_or(&obj.object_type);

        match converter.convert(&obj.object_type, &obj.object_json) {
            Ok(bcs) => {
                let is_shared = obj.owner_type == "Shared";
                converted_objects.push(ConvertedObject {
                    object_id: obj.object_id.clone(),
                    object_type: obj.object_type.clone(),
                    bcs: bcs.clone(),
                    is_shared,
                    version: obj.version,
                    initial_shared_version: obj.initial_shared_version,
                });
                println!(
                    "  ✓ {}... {} ({} bytes)",
                    &obj.object_id[..16],
                    type_short,
                    bcs.len()
                );
            }
            Err(e) => {
                println!("  ✗ {}... {} - {}", &obj.object_id[..16], type_short, e);
            }
        }
    }

    if converted_objects.is_empty() {
        return Err(anyhow!("No objects converted successfully"));
    }

    // -------------------------------------------------------------------------
    // Step 4: Initialize Move VM simulation
    // -------------------------------------------------------------------------
    println!("\nStep 4: Initialize Move VM simulation");

    let mut env = SimulationEnvironment::new()?;
    env.set_sender(AccountAddress::ZERO);

    // Deploy packages
    for (pkg_id, pkg) in &packages {
        let modules: Vec<(String, Vec<u8>)> = pkg
            .modules
            .iter()
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect();
        env.deploy_package_at_address(&pkg_id.to_hex_literal(), modules)?;
    }
    println!("  Deployed {} packages", packages.len());

    // Load objects from reconstructed BCS
    for obj in &converted_objects {
        env.load_object_from_data(
            &obj.object_id,
            obj.bcs.clone(),
            Some(obj.object_type.as_str()),
            obj.is_shared,
            false, // not immutable
            obj.version,
        )?;
    }
    println!("  Loaded {} objects", converted_objects.len());

    // Set up child fetcher for dynamic field access
    let child_grpc = rt.block_on(async { sui_transport::grpc::GrpcClient::mainnet().await })?;
    let child_fetcher = create_child_fetcher(child_grpc, Default::default(), None);
    env.set_child_fetcher(child_fetcher);
    println!("  Child fetcher enabled for dynamic fields");

    // -------------------------------------------------------------------------
    // Step 5: Find the pool and position objects
    // -------------------------------------------------------------------------
    println!("\nStep 5: Identify pool and position");

    let pool_obj = converted_objects
        .iter()
        .find(|o| o.object_type.contains("::pool::Pool"))
        .ok_or_else(|| anyhow!("No Pool object found in data"))?;

    let position_obj = converted_objects
        .iter()
        .find(|o| o.object_type.contains("::position::Position"))
        .ok_or_else(|| anyhow!("No Position object found in data"))?;

    println!("  Pool: {} (v{})", pool_obj.object_id, pool_obj.version);
    println!(
        "  Position: {} (v{})",
        position_obj.object_id, position_obj.version
    );

    // Extract coin types from pool type
    let (coin_a, coin_b) = extract_coin_types_from_pool(&pool_obj.object_type)?;
    println!("  Coin A: {}", coin_a);
    println!("  Coin B: {}", coin_b);

    // -------------------------------------------------------------------------
    // Step 6: Display pool and position state from JSON
    // -------------------------------------------------------------------------
    println!("\nStep 6: Analyze position data from object JSON");

    let (active_bin, lower_bin, upper_bin) = extract_position_info(&data)?;

    let range_status = common::get_range_status(active_bin, lower_bin, upper_bin);

    println!("  Position Range: bins {} to {}", lower_bin, upper_bin);
    println!("  Current Active Bin: {}", active_bin);
    println!("  Range Status: {}", range_status);

    // -------------------------------------------------------------------------
    // Step 7: Attempt PTB call (may fail due to dynamic field access)
    // -------------------------------------------------------------------------
    println!("\nStep 7: Call pool::get_position_amounts (PTB attempt)");

    let grpc = provider.grpc();
    let fresh_pool = rt.block_on(async { grpc.get_object(&pool_obj.object_id).await })?;
    let position_id_str = &position_obj.object_id;

    if let Some(pool_grpc) = fresh_pool {
        if let Some(pool_bcs) = &pool_grpc.bcs {
            println!("  Pool: v{} ({} bytes)", pool_grpc.version, pool_bcs.len());

            // Re-load pool with fresh BCS
            env.load_object_from_data(
                &pool_obj.object_id,
                pool_bcs.clone(),
                pool_grpc.type_string.as_deref(),
                true,
                false,
                pool_grpc.version,
            )?;

            // Build PTB
            let mut inputs: Vec<InputValue> = Vec::new();

            let pool_addr = AccountAddress::from_hex_literal(&pool_obj.object_id)?;
            let pool_type_tag = parse_type_tag(&format!(
                "{}::pool::Pool<{}, {}>",
                CETUS_DLMM_POOL_PACKAGE, coin_a, coin_b
            ));

            let pool_input = ObjectInput::Shared {
                id: pool_addr,
                bytes: pool_bcs.clone(),
                type_tag: pool_type_tag,
                version: Some(pool_grpc.version),
                mutable: true,
            };
            let pool_idx = inputs.len() as u16;
            inputs.push(InputValue::Object(pool_input));

            let position_addr = AccountAddress::from_hex_literal(position_id_str)?;
            let position_id_bcs = bcs::to_bytes(&position_addr)?;
            let position_id_idx = inputs.len() as u16;
            inputs.push(InputValue::Pure(position_id_bcs));

            let dlmm_addr = AccountAddress::from_hex_literal(CETUS_DLMM_POOL_PACKAGE)?;
            let command = Command::MoveCall {
                package: dlmm_addr,
                module: Identifier::new("pool")?,
                function: Identifier::new("get_position_amounts")?,
                type_args: vec![coin_a.clone(), coin_b.clone()],
                args: vec![Argument::Input(pool_idx), Argument::Input(position_id_idx)],
            };

            let result = env.execute_ptb(inputs, vec![command]);

            if result.success {
                println!("  ✓ PTB executed successfully!");
            } else {
                let err_msg = result
                    .error
                    .map(|e| format!("{:?}", e))
                    .or(result.raw_error)
                    .unwrap_or_else(|| "Unknown error".to_string());
                println!(
                    "  ✗ PTB failed (expected - dynamic field access): {}",
                    err_msg
                );
                println!("  → Using bin data for calculation instead");
            }
        }
    }

    // -------------------------------------------------------------------------
    // Step 8: Calculate position amounts from bin data
    // -------------------------------------------------------------------------
    println!("\nStep 8: Calculate position amounts from bin data");

    let extended_data_file = std::env::var("DLMM_EXTENDED_DATA")
        .unwrap_or_else(|_| DEFAULT_EXTENDED_DATA_FILE.to_string());

    match load_extended_position_data(&extended_data_file) {
        Ok(ext_data) => {
            println!("  Loaded extended data from: {}", extended_data_file);
            let amounts = calculate_position_amounts(&ext_data);
            display_position_amounts(
                &ext_data.position_id,
                &ext_data.pool_id,
                ext_data.lower_bin,
                ext_data.upper_bin,
                active_bin,
                &amounts,
            );
        }
        Err(e) => {
            println!("  Extended data not available: {}", e);
            print_query_help();
        }
    }

    // -------------------------------------------------------------------------
    // Step 9: Display historical 7-day view
    // -------------------------------------------------------------------------
    println!("\nStep 9: Historical Position View (7-day summary)");

    let historical_data_file = std::env::var("DLMM_HISTORICAL_DATA")
        .unwrap_or_else(|_| DEFAULT_HISTORICAL_DATA_FILE.to_string());

    match load_historical_data(&historical_data_file) {
        Ok(hist_data) => {
            display_historical_view(&hist_data);
        }
        Err(e) => {
            println!("  Historical data not available: {}", e);
            println!(
                "  Create {} with daily_snapshots for historical view",
                historical_data_file
            );
        }
    }

    // -------------------------------------------------------------------------
    // Step 10: Validate BCS reconstruction
    // -------------------------------------------------------------------------
    println!("\nStep 10: Validate BCS reconstruction");

    for obj in &converted_objects {
        let type_short = obj
            .object_type
            .split("::")
            .last()
            .unwrap_or(&obj.object_type);

        match rt.block_on(async {
            grpc.get_object_at_version(&obj.object_id, Some(obj.version))
                .await
        }) {
            Ok(Some(grpc_obj)) => {
                if let Some(actual_bcs) = grpc_obj.bcs {
                    if obj.bcs == actual_bcs {
                        println!(
                            "  ✓ {}... {} - BCS matches! ({} bytes)",
                            &obj.object_id[..16],
                            type_short,
                            obj.bcs.len()
                        );
                    } else {
                        let diff_at = obj
                            .bcs
                            .iter()
                            .zip(actual_bcs.iter())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                        println!(
                            "  ✗ {}... {} - BCS mismatch at byte {}",
                            &obj.object_id[..16],
                            type_short,
                            diff_at
                        );
                    }
                } else {
                    println!(
                        "  ? {}... {} - No BCS in gRPC response",
                        &obj.object_id[..16],
                        type_short
                    );
                }
            }
            Ok(None) => {
                println!(
                    "  ? {}... {} - Object not found at version {}",
                    &obj.object_id[..16],
                    type_short,
                    obj.version
                );
            }
            Err(e) => {
                println!(
                    "  ! {}... {} - gRPC error: {}",
                    &obj.object_id[..16],
                    type_short,
                    e
                );
            }
        }
    }

    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                              Done                                    ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Extract position info from object JSON data.
fn extract_position_info(data: &common::ObjectJsonData) -> Result<(i32, i32, i32)> {
    let pool_data = data
        .objects
        .iter()
        .find(|o| o.object_type.contains("::pool::Pool"))
        .ok_or_else(|| anyhow!("Pool not found"))?;

    let active_bin = pool_data
        .object_json
        .get("active_id")
        .and_then(|v| v.get("bits"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    let pos_data = data
        .objects
        .iter()
        .find(|o| o.object_type.contains("::position::Position"))
        .ok_or_else(|| anyhow!("Position not found"))?;

    let lower_bin = pos_data
        .object_json
        .get("lower_bin_id")
        .and_then(|v| v.get("bits"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    let upper_bin = pos_data
        .object_json
        .get("upper_bin_id")
        .and_then(|v| v.get("bits"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    Ok((active_bin, lower_bin, upper_bin))
}

/// Create a sample data file for new users.
fn create_sample_data_file(path: &str) -> Result<()> {
    let sample = serde_json::json!({
        "description": "DLMM USDC/SUI position - POPULATE WITH OBJECT_JSON",
        "checkpoint": 241670271,
        "objects": [
            {
                "object_id": "0x64e590b0e4d4f7dfc7ae9fae8e9983cd80ad83b658d8499bf550a9d4f6667076",
                "version": 775632036,
                "type": "0x5664f9d3fd82c84023870cfbda8ea84e14c8dd56ce557ad2116e0668581a682b::pool::Pool<0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC, 0x2::sui::SUI>",
                "owner_type": "Shared",
                "initial_shared_version": 643563051,
                "object_json": { "_comment": "PASTE OBJECT_JSON HERE" }
            },
            {
                "object_id": "0x33f5514521220478d3b3e141c7a67f766fd6b4150e25148a13171b4b68089417",
                "version": 768470949,
                "type": "0x5664f9d3fd82c84023870cfbda8ea84e14c8dd56ce557ad2116e0668581a682b::position::Position",
                "owner_type": "AddressOwner",
                "initial_shared_version": null,
                "object_json": { "_comment": "PASTE OBJECT_JSON HERE" }
            }
        ]
    });

    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, serde_json::to_string_pretty(&sample)?)?;
    Ok(())
}

/// Print help for queries to get bin data.
fn print_query_help() {
    println!("  To calculate exact position amounts, query for:");
    println!();
    println!("  1. Bin groups (contains reserves):");
    println!("     Query objects owned by <bin_manager.bins.id>");
    println!("     Filter by group index (e.g., 27810, 27811)");
    println!();
    println!("  2. PositionInfo (contains per-bin liquidity shares):");
    println!("     Query objects owned by <position_manager.positions.id>");
    println!("     Filter by position index");
}
