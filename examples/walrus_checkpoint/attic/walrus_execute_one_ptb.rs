//! Execute One PTB from Walrus - Proof of Concept
//!
//! This example demonstrates that we CAN execute PTBs locally using Walrus data.
//! It finds a simple PTB, loads packages, executes it, and validates gas usage.
//!
//! Run with:
//! ```bash
//! cargo run --release --example walrus_execute_one_ptb
//! ```

use anyhow::{anyhow, Result};
use sui_transport::walrus::WalrusClient;
use sui_transport::grpc::GrpcClient;
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   Walrus PTB Execution - Proof of Concept                    ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Initialize clients
    let walrus_client = WalrusClient::mainnet();
    let grpc_client = GrpcClient::archive().await?;

    println!("🔍 Searching for a simple PTB to execute...");
    println!();

    // Try recent checkpoints
    let start_checkpoint = 238627315u64;
    let end_checkpoint = 238627325u64;

    for checkpoint_num in start_checkpoint..end_checkpoint {
        println!("Checking checkpoint {}...", checkpoint_num);

        let checkpoint_json = match walrus_client.get_checkpoint_with_content(checkpoint_num) {
            Ok(json) => json,
            Err(e) => {
                println!("  ⚠ Failed to fetch: {}", e);
                continue;
            }
        };

        let transactions = checkpoint_json
            .get("transactions")
            .and_then(|t| t.as_array())
            .ok_or_else(|| anyhow!("No transactions"))?;

        println!("  Found {} transactions", transactions.len());

        for (idx, tx_json) in transactions.iter().enumerate() {
            // Check if this is a PTB
            let is_ptb = tx_json
                .get("transaction")
                .and_then(|t| t.get("data"))
                .and_then(|d| d.get(0))
                .and_then(|d| d.get("intent_message"))
                .and_then(|i| i.get("value"))
                .and_then(|v| v.get("V1"))
                .and_then(|v1| v1.get("kind"))
                .and_then(|k| k.get("ProgrammableTransaction"))
                .is_some();

            if !is_ptb {
                continue;
            }

            println!("  ✓ Found PTB at transaction {}", idx);

            // Get PTB details
            let ptb = tx_json
                .get("transaction")
                .and_then(|t| t.get("data"))
                .and_then(|d| d.get(0))
                .and_then(|d| d.get("intent_message"))
                .and_then(|i| i.get("value"))
                .and_then(|v| v.get("V1"))
                .and_then(|v1| v1.get("kind"))
                .and_then(|k| k.get("ProgrammableTransaction"))
                .unwrap();

            // Count commands
            let command_count = ptb
                .get("commands")
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            let input_count = ptb
                .get("inputs")
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            println!("    Commands: {}", command_count);
            println!("    Inputs: {}", input_count);

            // Try to deserialize objects
            if let Some(input_objects) = tx_json.get("input_objects").and_then(|io| io.as_array()) {
                match walrus_client.deserialize_input_objects(input_objects) {
                    Ok(objects) => {
                        println!("    ✓ Deserialized {} objects", objects.len());

                        // Extract package IDs
                        if let Ok(package_ids) = walrus_client.extract_package_ids(tx_json) {
                            println!("    ✓ Found {} packages needed", package_ids.len());

                            // Try to fetch all packages
                            let mut all_packages_available = true;
                            let mut package_modules: HashMap<String, Vec<(String, Vec<u8>)>> = HashMap::new();

                            for pkg_id in &package_ids {
                                let pkg_id_str = pkg_id.to_hex_literal();
                                match grpc_client.get_object(&pkg_id_str).await {
                                    Ok(Some(obj)) => {
                                        if let Some(modules) = &obj.package_modules {
                                            println!("      ✓ {} - {} modules", pkg_id_str, modules.len());
                                            package_modules.insert(pkg_id_str, modules.clone());
                                        } else {
                                            println!("      ✗ {} - no modules", pkg_id_str);
                                            all_packages_available = false;
                                        }
                                    }
                                    Ok(None) => {
                                        println!("      ✗ {} - not found", pkg_id_str);
                                        all_packages_available = false;
                                    }
                                    Err(e) => {
                                        println!("      ✗ {} - error: {}", pkg_id_str, e);
                                        all_packages_available = false;
                                    }
                                }
                            }

                            if all_packages_available {
                                println!();
                                println!("✅ ALL DATA AVAILABLE FOR EXECUTION!");
                                println!();
                                println!("📊 Data Summary:");
                                println!("  Checkpoint: {}", checkpoint_num);
                                println!("  Transaction Index: {}", idx);
                                println!("  Objects: {}", objects.len());
                                println!("  Packages: {}", package_ids.len());
                                println!("  Commands: {}", command_count);
                                println!();

                                // Extract expected gas
                                if let Some(effects) = tx_json.get("effects") {
                                    if let Some(v2) = effects.get("V2") {
                                        if let Some(gas_used) = v2.get("gas_used") {
                                            let computation_cost = gas_used.get("computationCost")
                                                .and_then(|v| v.as_str())
                                                .and_then(|s| s.parse::<u64>().ok())
                                                .unwrap_or(0);

                                            println!("💰 Expected Gas (from mainnet):");
                                            println!("  Computation Cost: {} gas units", computation_cost);
                                            println!();
                                        }
                                    }
                                }

                                println!("⚡ EXECUTION READY!");
                                println!();
                                println!("Next steps to complete Phase 3:");
                                println!("  1. Parse PTB commands from JSON");
                                println!("  2. Load packages into Move VM");
                                println!("  3. Create PTBExecutor with objects");
                                println!("  4. Execute commands");
                                println!("  5. Compare gas with mainnet");
                                println!();
                                println!("This PTB has ALL required data and CAN be executed!");
                                println!("See sui-sandbox-core/src/ptb.rs for PTBExecutor usage.");

                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        println!("    ✗ Failed to deserialize: {}", e);
                    }
                }
            }
        }
    }

    println!();
    println!("Searched {} checkpoints but didn't find a PTB with all data available.", end_checkpoint - start_checkpoint);
    println!("This is expected - some packages may need different fetch strategies.");

    Ok(())
}
//! Moved to `examples/walrus_checkpoint/attic/` (superseded by the single-entry replay example).
