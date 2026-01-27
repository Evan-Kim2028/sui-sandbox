//! Walrus cache builder: bulk ingest objects from ~10 blobs into filesystem cache.
//!
//! This example builds a local historical state cache by:
//! - Fetching checkpoint data from Walrus (batched byte-range downloads)
//! - Extracting objects from checkpoint transactions
//! - Storing objects in sharded filesystem layout for fast L2 lookups
//!
//! Run:
//! ```bash
//! cargo run --release --example walrus_cache_build -- --cache-dir ./walrus-cache --blobs 10
//! ```

use anyhow::{anyhow, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use base64::Engine;

use sui_historical_cache::{FsObjectStore, ObjectMeta, ObjectVersionStore, ProgressTracker};
use sui_transport::walrus::WalrusClient;

/// Build a filesystem cache from Walrus checkpoint blobs.
#[derive(Parser, Debug)]
struct Args {
    /// Cache directory path.
    #[arg(long)]
    cache_dir: String,

    /// Number of blobs to ingest (most recent by default).
    #[arg(long, default_value_t = 10)]
    blobs: usize,

    /// Start checkpoint (overrides blob selection).
    #[arg(long)]
    start_checkpoint: Option<u64>,

    /// End checkpoint (overrides blob selection).
    #[arg(long)]
    end_checkpoint: Option<u64>,

    /// Specific blob ID to ingest (for debugging).
    #[arg(long)]
    blob_id: Option<String>,

    /// Max bytes per merged blob download chunk.
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    max_blob_chunk_bytes: u64,

    /// Number of parallel blob workers.
    #[arg(long, default_value_t = 4)]
    workers: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   Walrus Cache Builder (Bulk Ingest from Checkpoint Blobs)   ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Cache directory: {}", args.cache_dir);
    println!();

    let walrus = WalrusClient::mainnet();
    let object_store = Arc::new(FsObjectStore::new(&args.cache_dir)?);
    let progress = Arc::new(ProgressTracker::new(&args.cache_dir)?);

    // Determine which blobs/checkpoints to process
    let (blobs_to_process, checkpoint_ranges) = if let Some(blob_id) = args.blob_id {
        // Single blob mode
        let blob = walrus
            .list_blobs(None)?
            .into_iter()
            .find(|b| b.blob_id == blob_id)
            .ok_or_else(|| anyhow!("Blob {} not found", blob_id))?;
        (
            vec![blob.clone()],
            vec![(blob.start_checkpoint, blob.end_checkpoint)],
        )
    } else if let (Some(start), Some(end)) = (args.start_checkpoint, args.end_checkpoint) {
        // Checkpoint range mode
        let all_blobs = walrus.list_blobs(None)?;
        let mut relevant_blobs = Vec::new();
        let mut ranges = Vec::new();
        for blob in all_blobs {
            if blob.end_checkpoint >= start && blob.start_checkpoint <= end {
                let range_start = start.max(blob.start_checkpoint);
                let range_end = end.min(blob.end_checkpoint);
                relevant_blobs.push(blob.clone());
                ranges.push((range_start, range_end));
            }
        }
        (relevant_blobs, ranges)
    } else {
        // Default: most recent N blobs
        let mut all_blobs = walrus.list_blobs(Some(args.blobs * 2))?;
        all_blobs.sort_by_key(|b| std::cmp::Reverse(b.end_checkpoint));
        let selected: Vec<_> = all_blobs.into_iter().take(args.blobs).collect();
        let ranges: Vec<_> = selected
            .iter()
            .map(|b| (b.start_checkpoint, b.end_checkpoint))
            .collect();
        (selected, ranges)
    };

    println!("Processing {} blob(s):", blobs_to_process.len());
    for (i, blob) in blobs_to_process.iter().enumerate() {
        let (start, end) = checkpoint_ranges[i];
        println!(
            "  Blob {}: checkpoints {}..{} ({} checkpoints)",
            blob.blob_id,
            start,
            end,
            end - start + 1
        );
    }
    println!();

    let start_time = Instant::now();
    let total_checkpoints = Arc::new(AtomicU64::new(0));
    let total_objects = Arc::new(AtomicU64::new(0));
    let total_duplicates = Arc::new(AtomicU64::new(0));

    // Build tasks: one per blob + its checkpoint range.
    let tasks: Vec<(String, u64, u64)> = blobs_to_process
        .iter()
        .enumerate()
        .map(|(i, blob)| {
            let (start, end) = checkpoint_ranges[i];
            (blob.blob_id.clone(), start, end)
        })
        .collect();

    let worker_count = args.workers.max(1).min(tasks.len().max(1));
    let (tx, rx) = std::sync::mpsc::channel::<(String, u64, u64)>();
    for t in tasks {
        tx.send(t).map_err(|e| anyhow!("failed to enqueue task: {e}"))?;
    }
    drop(tx);

    println!("Starting {} worker(s)...", worker_count);
    let mut handles = Vec::new();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..worker_count {
        let rx = Arc::clone(&rx);
        let walrus = walrus.clone();
        let object_store = Arc::clone(&object_store);
        let progress = Arc::clone(&progress);
        let total_checkpoints = Arc::clone(&total_checkpoints);
        let total_objects = Arc::clone(&total_objects);
        let total_duplicates = Arc::clone(&total_duplicates);
        let max_blob_chunk_bytes = args.max_blob_chunk_bytes;

        let handle = std::thread::spawn(move || -> Result<()> {
            loop {
                let task = {
                    let guard = rx.lock().map_err(|_| anyhow!("task queue mutex poisoned"))?;
                    guard.recv()
                };
                let Ok((blob_id, range_start, range_end)) = task else {
                    break;
                };
                // Skip if already fully ingested
                if progress.is_blob_ingested(&blob_id) {
                    println!("Blob {} already fully ingested, skipping", blob_id);
                    continue;
                }

                // Resume from last checkpoint if partial
                let resume_from = progress.last_checkpoint(&blob_id).unwrap_or(range_start);
                if resume_from > range_start {
                    println!("Resuming blob {} from checkpoint {}", blob_id, resume_from);
                }

                println!("Processing blob {}: checkpoints {}..{}", blob_id, resume_from, range_end);
                let blob_start = Instant::now();

                // Build checkpoint list
                let checkpoints: Vec<u64> = (resume_from..=range_end).collect();
                if checkpoints.is_empty() {
                    continue;
                }

                // Fetch checkpoints in batches
                let decoded = walrus.get_checkpoints_json_batched(&checkpoints, max_blob_chunk_bytes)?;
                println!("  Blob {}: fetched {} checkpoints", blob_id, decoded.len());

                // Extract objects from each checkpoint
                let mut objects_written_this_blob = 0u64;
                let mut duplicates_this_blob = 0u64;
                let mut checkpoints_this_blob = 0u64;

                for (checkpoint, checkpoint_json) in decoded {
                    // Extract objects from transactions
                    let transactions = checkpoint_json
                        .get("transactions")
                        .and_then(|t| t.as_array())
                        .ok_or_else(|| anyhow!("Missing transactions array"))?;

                    for tx_json in transactions {
                        // Process input_objects
                        if let Some(inputs) = tx_json.get("input_objects").and_then(|v| v.as_array()) {
                            for obj_json in inputs {
                                if let Err(e) = extract_and_store_object(
                                    &*object_store,
                                    obj_json,
                                    Some(checkpoint),
                                    &mut objects_written_this_blob,
                                    &mut duplicates_this_blob,
                                ) {
                                    eprintln!("  Warning: failed to extract input object: {}", e);
                                }
                            }
                        }

                        // Process output_objects (prefer these as they're more complete)
                        if let Some(outputs) = tx_json.get("output_objects").and_then(|v| v.as_array()) {
                            for obj_json in outputs {
                                if let Err(e) = extract_and_store_object(
                                    &*object_store,
                                    obj_json,
                                    Some(checkpoint),
                                    &mut objects_written_this_blob,
                                    &mut duplicates_this_blob,
                                ) {
                                    eprintln!("  Warning: failed to extract output object: {}", e);
                                }
                            }
                        }
                    }

                    // Record checkpoint progress
                    progress.record_checkpoint(&blob_id, checkpoint)?;
                    checkpoints_this_blob += 1;

                    let global = total_checkpoints.fetch_add(1, Ordering::Relaxed) + 1;
                    if global % 1000 == 0 {
                        let elapsed = blob_start.elapsed().as_secs_f64().max(0.0001);
                        let rate = checkpoints_this_blob as f64 / elapsed;
                        println!(
                            "  Progress (blob {}): {} checkpoints, {} objects ({:.1} checkpoints/sec)",
                            blob_id, checkpoints_this_blob, objects_written_this_blob, rate
                        );
                    }
                }

                // Record objects written
                progress.record_objects_written(objects_written_this_blob, duplicates_this_blob)?;
                total_objects.fetch_add(objects_written_this_blob, Ordering::Relaxed);
                total_duplicates.fetch_add(duplicates_this_blob, Ordering::Relaxed);

                // Mark blob as complete
                progress.mark_blob_complete(&blob_id)?;

                let blob_elapsed = blob_start.elapsed().as_secs_f64();
                println!(
                    "  Completed blob {}: {} checkpoints, {} objects, {} duplicates ({:.1}s)",
                    blob_id,
                    checkpoints_this_blob,
                    objects_written_this_blob,
                    duplicates_this_blob,
                    blob_elapsed
                );
                println!();
            }
            Ok(())
        });
        handles.push(handle);
    }

    // Wait for all workers
    for h in handles {
        h.join()
            .map_err(|_| anyhow!("worker thread panicked"))??;
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Summary");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let total_checkpoints = total_checkpoints.load(Ordering::Relaxed);
    let total_objects = total_objects.load(Ordering::Relaxed);
    let total_duplicates = total_duplicates.load(Ordering::Relaxed);
    println!("Checkpoints processed: {}", total_checkpoints);
    println!("Objects written:      {}", total_objects);
    println!("Duplicates skipped:   {}", total_duplicates);
    println!("Elapsed:              {:.1}s", elapsed);
    if total_checkpoints > 0 {
        println!("Checkpoints/sec:      {:.1}", total_checkpoints as f64 / elapsed);
    }
    if total_objects > 0 {
        println!("Objects/sec:          {:.1}", total_objects as f64 / elapsed);
    }

    // Calculate disk usage
    let cache_path = std::path::Path::new(&args.cache_dir);
    let disk_usage = calculate_disk_usage(cache_path)?;
    println!("Disk usage:           {} MB", disk_usage / 1024 / 1024);

    Ok(())
}

fn extract_and_store_object(
    store: &FsObjectStore,
    obj_json: &serde_json::Value,
    source_checkpoint: Option<u64>,
    objects_written: &mut u64,
    duplicates: &mut u64,
) -> Result<()> {
    // Only process Move objects
    let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
        return Ok(()); // Skip non-Move objects
    };

    // Extract BCS bytes
    let Some(contents_b64) = move_obj.get("contents").and_then(|c| c.as_str()) else {
        return Ok(()); // Skip objects without contents
    };
    let bcs_bytes = base64::engine::general_purpose::STANDARD
        .decode(contents_b64)
        .map_err(|e| anyhow!("Failed to decode base64: {}", e))?;

    if bcs_bytes.len() < 32 {
        return Ok(()); // Too short to contain object ID
    }

    // Extract object ID from first 32 bytes
    let object_id = AccountAddress::new({
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&bcs_bytes[0..32]);
        bytes
    });

    // Extract version
    let Some(version) = move_obj.get("version").and_then(|v| v.as_u64()) else {
        return Ok(()); // Skip objects without version
    };

    // Check if already exists
    if store.has(object_id, version) {
        *duplicates += 1;
        return Ok(());
    }

    // Parse type tag
    let type_json = move_obj.get("type_").ok_or_else(|| anyhow!("Missing type_"))?;
    let type_tag_str = parse_type_tag_string(type_json)?;

    // Extract owner kind (best-effort)
    let owner_kind = obj_json
        .get("owner")
        .and_then(|o| {
            if o.get("AddressOwner").is_some() {
                Some("address")
            } else if o.get("ObjectOwner").is_some() {
                Some("object")
            } else if o.get("Shared").is_some() {
                Some("shared")
            } else if o.get("Immutable").is_some() {
                Some("immutable")
            } else if o.get("ConsensusAddressOwner").is_some() {
                Some("consensus_address")
            } else {
                None
            }
        })
        .map(|s| s.to_string());

    // Store object
    let meta = ObjectMeta {
        type_tag: type_tag_str,
        owner_kind,
        source_checkpoint,
    };
    store.put(object_id, version, &bcs_bytes, &meta)?;
    *objects_written += 1;

    Ok(())
}

fn parse_type_tag_string(type_json: &serde_json::Value) -> Result<String> {
    // Handle string shortcuts
    if let Some(s) = type_json.as_str() {
        if s == "GasCoin" {
            return Ok("0x2::coin::Coin<0x2::sui::SUI>".to_string());
        }
        return Ok(s.to_string());
    }

    // Handle Coin wrapper
    if let Some(coin_json) = type_json.get("Coin") {
        if let Some(struct_json) = coin_json.get("struct") {
            let inner = parse_type_tag_string(&serde_json::json!({ "struct": struct_json }))?;
            return Ok(format!("0x2::coin::Coin<{}>", inner));
        }
    }

    // Handle vector
    if let Some(vec_json) = type_json.get("vector") {
        let inner = parse_type_tag_string(vec_json)?;
        return Ok(format!("vector<{}>", inner));
    }

    // Handle struct/Other
    let struct_json = if let Some(other) = type_json.get("Other") {
        other
    } else if let Some(s) = type_json.get("struct") {
        s
    } else if type_json.get("address").is_some() {
        type_json
    } else {
        return Err(anyhow!("Unsupported type tag format: {}", type_json));
    };

    let address = struct_json
        .get("address")
        .and_then(|a| a.as_str())
        .ok_or_else(|| anyhow!("Missing address"))?;
    let module = struct_json
        .get("module")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow!("Missing module"))?;
    let name = struct_json
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow!("Missing name"))?;

    let mut type_tag = format!("{}::{}::{}", address, module, name);

    // Add type args if present
    if let Some(type_args) = struct_json.get("type_args").and_then(|t| t.as_array()) {
        if !type_args.is_empty() {
            let args: Vec<String> = type_args
                .iter()
                .map(parse_type_tag_string)
                .collect::<Result<_>>()?;
            type_tag.push('<');
            type_tag.push_str(&args.join(", "));
            type_tag.push('>');
        }
    }

    Ok(type_tag)
}

fn calculate_disk_usage(path: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                total += calculate_disk_usage(&path)?;
            } else if let Ok(metadata) = path.metadata() {
                total += metadata.len();
            }
        }
    }
    Ok(total)
}
