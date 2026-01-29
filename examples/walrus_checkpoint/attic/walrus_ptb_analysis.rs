//! Moved to `examples/walrus_checkpoint/attic/` (not part of the single-entry Walrus demo).
//! Walrus PTB Analysis - What Can We Simulate?
//!
//! This example fetches a checkpoint from Walrus and analyzes
//! what would be needed to simulate its PTBs.
//!
//! Run with:
//! ```bash
//! cargo run --example walrus_ptb_analysis
//! ```

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use sui_transport::walrus::WalrusClient;
use sui_types::base_types::{ObjectID, SequenceNumber};
use sui_types::effects::TransactionEffectsAPI;
use sui_types::transaction::TransactionKind;

fn main() -> Result<()> {
    println!("=== Walrus PTB Analysis ===\n");

    let client = WalrusClient::mainnet();

    // Fetch latest checkpoint
    println!("Fetching latest checkpoint...");
    let latest = client.get_latest_checkpoint()?;
    let checkpoint = client.get_checkpoint(latest)?;
    println!("✓ Checkpoint {}\n", latest);

    // Extract PTBs
    let mut ptb_count = 0;
    let mut required_objects: HashMap<ObjectID, SequenceNumber> = HashMap::new();
    let mut required_packages: HashSet<ObjectID> = HashSet::new();
    let mut available_objects: HashMap<ObjectID, SequenceNumber> = HashMap::new();

    println!("Analyzing PTBs and their requirements...\n");

    for (idx, tx_data) in checkpoint.transactions.iter().enumerate() {
        if let TransactionKind::ProgrammableTransaction(ptb) = tx_data.transaction.transaction_data().kind() {
            ptb_count += 1;

            if ptb_count <= 3 {
                // Analyze first few PTBs in detail
                println!("PTB #{} (digest: {})", ptb_count, tx_data.transaction.digest());
                println!("  Commands: {}", ptb.commands.len());
                println!("  Inputs: {}", ptb.inputs.len());

                // Extract required objects from inputs
                let tx_data_inner = tx_data.transaction.transaction_data();
                for input in tx_data_inner.input_objects()? {
                    match input {
                        sui_types::transaction::InputObject::ImmOrOwnedMoveObject((id, version, _)) => {
                            required_objects.insert(*id, *version);
                            println!("    Requires: {}@v{}", id, version.value());
                        }
                        sui_types::transaction::InputObject::SharedMoveObject { id, initial_shared_version, .. } => {
                            required_objects.insert(*id, *initial_shared_version);
                            println!("    Requires (shared): {}@v{}", id, initial_shared_version.value());
                        }
                        sui_types::transaction::InputObject::MovePackage(id) => {
                            required_packages.insert(*id);
                            println!("    Requires package: {}", id);
                        }
                        _ => {}
                    }
                }

                // Extract packages from MoveCall commands
                for cmd in &ptb.commands {
                    if let sui_types::transaction::Command::MoveCall(call) = cmd {
                        required_packages.insert(call.package);
                        println!("    Calls: {}::{}::{}", call.package, call.module, call.function);
                    }
                }

                println!();
            }
        }

        // Track objects created/modified in this checkpoint (available for later txs)
        for obj_ref in tx_data.effects.created() {
            available_objects.insert(obj_ref.0, obj_ref.1);
        }
        for obj_ref in tx_data.effects.mutated() {
            available_objects.insert(obj_ref.0, obj_ref.1);
        }
    }

    println!("=== Summary ===\n");
    println!("Total PTBs: {}", ptb_count);
    println!("Unique objects required: {}", required_objects.len());
    println!("Unique packages required: {}", required_packages.len());
    println!("Objects available in checkpoint: {}", available_objects.len());

    // Check overlap
    let mut available_count = 0;
    let mut missing_count = 0;

    for (obj_id, version) in &required_objects {
        if let Some(avail_version) = available_objects.get(obj_id) {
            if avail_version == version {
                available_count += 1;
            } else {
                missing_count += 1;
            }
        } else {
            missing_count += 1;
        }
    }

    println!("\nObject availability:");
    println!("  Available in checkpoint: {} ({:.1}%)",
        available_count,
        (available_count as f64 / required_objects.len() as f64) * 100.0
    );
    println!("  Needs historical fetch: {} ({:.1}%)",
        missing_count,
        (missing_count as f64 / required_objects.len() as f64) * 100.0
    );

    println!("\n=== Simulation Strategy ===\n");

    println!("For self-contained transactions (use only checkpoint data):");
    println!("  ✓ Can simulate immediately with Walrus alone");
    println!("  ✓ No additional data fetching needed");
    println!("  → Estimate: {:.1}% of PTBs might be self-contained",
        (available_count as f64 / required_objects.len() as f64) * 100.0
    );

    println!("\nFor transactions needing historical data:");
    println!("  1. Query Walrus PostgreSQL for object locations");
    println!("     - Input: (object_id, version)");
    println!("     - Output: checkpoint number containing that version");
    println!("  2. Fetch historical checkpoint from Walrus");
    println!("  3. Extract object data from checkpoint");
    println!("  4. Simulate transaction");

    println!("\nFor packages:");
    println!("  - Still need gRPC or build separate package archive");
    println!("  - Packages don't change often, so caching is effective");
    println!("  - Could pre-fetch all referenced packages");

    println!("\n=== Recommended Tools ===\n");

    println!("What Walrus provides:");
    println!("  ✓ REST API for checkpoint queries");
    println!("  ✓ PostgreSQL for metadata (checkpoint ranges, blob info)");
    println!("  ✓ Aggregator for fast byte-range fetching");

    println!("\nWhat we need to build:");
    println!("  1. Object-version index (PostgreSQL extension)");
    println!("     CREATE TABLE object_versions (");
    println!("       object_id TEXT,");
    println!("       version BIGINT,");
    println!("       checkpoint_number BIGINT,");
    println!("       PRIMARY KEY (object_id, version)");
    println!("     );");
    println!();
    println!("  2. Checkpoint cache (local or Redis)");
    println!("     - Cache decoded CheckpointData");
    println!("     - Indexed by checkpoint number");
    println!();
    println!("  3. Smart fetcher (in sui-transport)");
    println!("     - Check: Is object in latest checkpoint? Use Walrus");
    println!("     - Check: Is object in cache? Use cache");
    println!("     - Fallback: Query index → Fetch checkpoint → Extract");

    println!("\n=== Next Experiments ===\n");
    println!("1. Find a simple PTB that only uses checkpoint data");
    println!("   → Try to simulate it with Walrus data alone");
    println!("2. Build object-version index for recent checkpoints");
    println!("   → Measure index size and query performance");
    println!("3. Implement hybrid fetcher prototype");
    println!("   → Compare latency: Walrus vs gRPC vs GraphQL");

    Ok(())
}
