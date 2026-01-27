//! Moved to `examples/walrus_checkpoint/attic/` (not part of the single-entry Walrus demo).
//! Inspect Walrus Data - Diagnostic Tool
//!
//! This script fetches data from Walrus and inspects it to understand
//! what we're actually getting.

use anyhow::Result;
use sui_transport::walrus::WalrusClient;

fn main() -> Result<()> {
    println!("=== Walrus Data Inspection ===\n");

    let client = WalrusClient::mainnet();

    // Step 1: Get latest checkpoint
    println!("Step 1: Fetching latest checkpoint number...");
    let latest = client.get_latest_checkpoint()?;
    println!("✓ Latest: {}\n", latest);

    // Step 2: Get checkpoint metadata
    println!("Step 2: Fetching metadata for checkpoint {}...", latest);
    let metadata = client.get_checkpoint_metadata(latest)?;
    println!("✓ Metadata:");
    println!("  Checkpoint: {}", metadata.checkpoint_number);
    println!("  Blob ID: {}", metadata.blob_id);
    println!("  Object ID: {}", metadata.object_id);
    println!("  Index: {}", metadata.index);
    println!("  Offset: {} bytes", metadata.offset);
    println!("  Length: {} bytes", metadata.length);
    println!();

    // Step 3: Try to fetch raw bytes
    println!("Step 3: Fetching raw checkpoint bytes...");
    match client.fetch_checkpoint_bytes(&metadata.blob_id, metadata.offset, metadata.length) {
        Ok(bytes) => {
            println!("✓ Successfully fetched {} bytes", bytes.len());
            println!("  First 32 bytes (hex): {}", hex::encode(&bytes[..32.min(bytes.len())]));
            println!();

            // Step 4: Try to decode
            println!("Step 4: Attempting BCS decode...");
            match bcs::from_bytes::<sui_types::full_checkpoint_content::CheckpointData>(&bytes) {
                Ok(checkpoint) => {
                    println!("✓ Successfully decoded checkpoint!");
                    println!("  Sequence: {}", checkpoint.checkpoint_summary.sequence_number);
                    println!("  Epoch: {}", checkpoint.checkpoint_summary.epoch);
                    println!("  Transactions: {}", checkpoint.transactions.len());
                    println!();

                    // Inspect first transaction
                    if !checkpoint.transactions.is_empty() {
                        let tx = &checkpoint.transactions[0];
                        println!("First transaction:");
                        println!("  Digest: {}", tx.transaction.digest());
                        println!("  Input objects: {}", tx.input_objects.len());
                        println!("  Output objects: {}", tx.output_objects.len());

                        use sui_types::transaction::TransactionDataAPI;
                        let tx_data = tx.transaction.transaction_data();
                        println!("  Kind: {:?}", tx_data.kind());
                    }
                }
                Err(e) => {
                    println!("✗ BCS decode failed: {}", e);
                    println!("  This might be due to:");
                    println!("  1. Version mismatch between sui-types and archived data");
                    println!("  2. Incorrect offset/length");
                    println!("  3. Data corruption");
                }
            }
        }
        Err(e) => {
            println!("✗ Failed to fetch bytes: {}", e);
        }
    }

    println!("\n=== Alternative: Try via JSON endpoint ===\n");

    // Try the show_content endpoint which returns JSON
    println!("Fetching checkpoint via JSON endpoint...");
    match client.get_checkpoint_with_content(latest) {
        Ok(content) => {
            println!("✓ Got JSON content");

            // Pretty print a sample
            let pretty = serde_json::to_string_pretty(&content)?;
            let lines: Vec<&str> = pretty.lines().collect();
            println!("First 50 lines of JSON:");
            for (i, line) in lines.iter().take(50).enumerate() {
                println!("{:4}: {}", i + 1, line);
            }

            if lines.len() > 50 {
                println!("  ... ({} more lines)", lines.len() - 50);
            }
        }
        Err(e) => {
            println!("✗ Failed: {}", e);
        }
    }

    Ok(())
}
