//! Analyze the JSON checkpoint data from Walrus
//!
//! Since BCS decoding fails, let's see what the JSON endpoint gives us

use anyhow::Result;
use sui_transport::walrus::WalrusClient;

fn main() -> Result<()> {
    println!("=== Walrus JSON Data Analysis ===\n");

    let client = WalrusClient::mainnet();

    // Get latest checkpoint
    let latest = client.get_latest_checkpoint()?;
    println!("Latest checkpoint: {}\n", latest);

    // Fetch via JSON endpoint
    println!("Fetching checkpoint {} via JSON endpoint...\n", latest);
    let content = client.get_checkpoint_with_content(latest)?;

    // Analyze the structure
    println!("=== Checkpoint Structure ===\n");

    if let Some(summary) = content.get("checkpoint_summary") {
        println!("✓ Has checkpoint_summary");
        if let Some(data) = summary.get("data") {
            println!("  - epoch: {}", data.get("epoch").unwrap_or(&serde_json::json!(null)));
            println!("  - sequence_number: {}", data.get("sequence_number").unwrap_or(&serde_json::json!(null)));
            println!("  - timestamp_ms: {}", data.get("timestamp_ms").unwrap_or(&serde_json::json!(null)));
        }
    }

    if let Some(contents) = content.get("checkpoint_contents") {
        println!("\n✓ Has checkpoint_contents");
        if let Some(v1) = contents.get("V1") {
            if let Some(transactions) = v1.get("transactions") {
                if let Some(arr) = transactions.as_array() {
                    println!("  - transactions: {} entries", arr.len());

                    if !arr.is_empty() {
                        println!("\n  First transaction:");
                        let first = &arr[0];
                        println!("    transaction digest: {}", first.get("transaction").unwrap_or(&serde_json::json!("?")));
                        println!("    effects digest: {}", first.get("effects").unwrap_or(&serde_json::json!("?")));
                    }
                }
            }
        }
    }

    if let Some(transactions) = content.get("transactions") {
        println!("\n✓ Has transactions array");
        if let Some(arr) = transactions.as_array() {
            println!("  - Full transaction data: {} entries", arr.len());

            if !arr.is_empty() {
                println!("\n  First full transaction:");
                let first = &arr[0];

                // Check what fields are available
                if let Some(obj) = first.as_object() {
                    println!("    Available fields:");
                    for key in obj.keys() {
                        println!("      - {}", key);
                    }

                    // Inspect transaction field
                    if let Some(tx) = obj.get("transaction") {
                        println!("\n    Transaction structure:");
                        if let Some(tx_obj) = tx.as_object() {
                            for key in tx_obj.keys() {
                                println!("      - {}", key);
                            }
                        }
                    }

                    // Inspect effects field
                    if let Some(effects) = obj.get("effects") {
                        println!("\n    Effects structure:");
                        if let Some(eff_obj) = effects.as_object() {
                            for key in eff_obj.keys() {
                                println!("      - {}", key);
                            }
                        }
                    }

                    // Check for input/output objects
                    if let Some(input_objs) = obj.get("input_objects") {
                        if let Some(arr) = input_objs.as_array() {
                            println!("\n    Input objects: {} entries", arr.len());
                            if !arr.is_empty() {
                                println!("      First object fields:");
                                if let Some(first_obj) = arr[0].as_object() {
                                    for key in first_obj.keys() {
                                        println!("        - {}", key);
                                    }
                                }
                            }
                        }
                    }

                    if let Some(output_objs) = obj.get("output_objects") {
                        if let Some(arr) = output_objs.as_array() {
                            println!("\n    Output objects: {} entries", arr.len());
                        }
                    }
                }
            }
        }
    } else {
        println!("\n✗ No transactions array found");
        println!("  This means the JSON endpoint only returns digests, not full data");
    }

    println!("\n=== Assessment ===\n");

    let has_full_data = content.get("transactions").and_then(|t| t.as_array()).is_some();

    if has_full_data {
        println!("✓ JSON endpoint provides FULL transaction data");
        println!("  - We can use this for PTB replay!");
        println!("  - Includes transaction commands, effects, and objects");
    } else {
        println!("✗ JSON endpoint only provides transaction DIGESTS");
        println!("  - We would need to fetch individual transactions separately");
        println!("  - This defeats the purpose of using Walrus");
    }

    // Save full JSON to file for inspection
    println!("\nSaving full JSON to checkpoint_data.json for inspection...");
    std::fs::write(
        "checkpoint_data.json",
        serde_json::to_string_pretty(&content)?
    )?;
    println!("✓ Saved to checkpoint_data.json");

    Ok(())
}
//! Moved to `examples/walrus_checkpoint/attic/` (not part of the single-entry Walrus demo).
