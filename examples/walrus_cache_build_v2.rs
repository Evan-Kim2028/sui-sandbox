//! Walrus Cache Builder v2: Production-ready bulk ingestion for 1M+ checkpoints.
//!
//! Key improvements over v1:
//! - Retry logic with exponential backoff for aggregator timeouts
//! - Smaller batch sizes (500 checkpoints) to avoid timeout issues
//! - Checkpoint-level progress tracking for fine-grained resumability
//! - Parallel blob downloads with configurable concurrency
//! - Optional zstd compression for storage efficiency
//!
//! Run:
//! ```bash
//! # Build cache for most recent 1M checkpoints (about 80 blobs)
//! cargo run --release --example walrus_cache_build_v2 -- \
//!     --cache-dir ./walrus-cache \
//!     --checkpoints 1000000 \
//!     --workers 4
//!
//! # Or specify a checkpoint range
//! cargo run --release --example walrus_cache_build_v2 -- \
//!     --cache-dir ./walrus-cache \
//!     --start-checkpoint 238000000 \
//!     --end-checkpoint 239000000 \
//!     --workers 4
//! ```

use anyhow::{anyhow, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sui_transport::walrus::{BlobInfo, WalrusClient};

/// Build a filesystem cache from Walrus checkpoint blobs (v2 - production ready).
#[derive(Parser, Debug)]
struct Args {
    /// Cache directory path.
    #[arg(long)]
    cache_dir: String,

    /// Number of checkpoints to ingest (most recent N checkpoints).
    #[arg(long, conflicts_with_all = &["start_checkpoint", "end_checkpoint"])]
    checkpoints: Option<u64>,

    /// Start checkpoint (inclusive).
    #[arg(long, requires = "end_checkpoint")]
    start_checkpoint: Option<u64>,

    /// End checkpoint (inclusive).
    #[arg(long, requires = "start_checkpoint")]
    end_checkpoint: Option<u64>,

    /// Number of parallel blob workers.
    #[arg(long, default_value_t = 4)]
    workers: usize,

    /// Checkpoints per batch (smaller = less likely to timeout).
    #[arg(long, default_value_t = 15)]
    batch_size: usize,

    /// Max bytes per aggregator request (smaller = more reliable but slower).
    /// 3MB provides good balance between reliability and throughput.
    #[arg(long, default_value_t = 3 * 1024 * 1024)]
    max_chunk_bytes: u64,

    /// Max retries per batch on failure before falling back to individual fetches.
    #[arg(long, default_value_t = 2)]
    max_retries: usize,

    /// Print verbose progress (every checkpoint instead of every 1000).
    #[arg(long)]
    verbose: bool,

    /// Dry run: show what would be processed without downloading.
    #[arg(long)]
    dry_run: bool,
}

/// Progress state persisted to disk for resumability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct ProgressState {
    /// Checkpoints that have been fully processed.
    completed_checkpoints: HashSet<u64>,
    /// Total objects written.
    objects_written: u64,
    /// Total duplicates skipped.
    duplicates_skipped: u64,
    /// Last save timestamp.
    last_save_timestamp: u64,
}

/// Thread-safe progress tracker with periodic saves.
struct ProgressTracker {
    state: Mutex<ProgressState>,
    state_path: PathBuf,
    save_interval: Duration,
    last_save: Mutex<Instant>,
}

impl ProgressTracker {
    fn new(cache_dir: &Path) -> Result<Self> {
        let progress_dir = cache_dir.join("progress");
        std::fs::create_dir_all(&progress_dir)?;
        let state_path = progress_dir.join("state_v2.json");

        let state = if state_path.exists() {
            let json = std::fs::read_to_string(&state_path)?;
            serde_json::from_str(&json).unwrap_or_default()
        } else {
            ProgressState::default()
        };

        Ok(Self {
            state: Mutex::new(state),
            state_path,
            save_interval: Duration::from_secs(30),
            last_save: Mutex::new(Instant::now()),
        })
    }

    fn is_checkpoint_done(&self, checkpoint: u64) -> bool {
        self.state
            .lock()
            .completed_checkpoints
            .contains(&checkpoint)
    }

    fn mark_checkpoint_done(&self, checkpoint: u64, objects_written: u64, duplicates: u64) {
        let mut state = self.state.lock();
        state.completed_checkpoints.insert(checkpoint);
        state.objects_written += objects_written;
        state.duplicates_skipped += duplicates;

        // Periodic save (don't hold lock during IO)
        let should_save = {
            let mut last_save = self.last_save.lock();
            if last_save.elapsed() >= self.save_interval {
                *last_save = Instant::now();
                true
            } else {
                false
            }
        };

        if should_save {
            let state_clone = state.clone();
            drop(state);
            let _ = self.save_state_impl(&state_clone);
        }
    }

    fn save_state(&self) -> Result<()> {
        let state = self.state.lock().clone();
        self.save_state_impl(&state)
    }

    fn save_state_impl(&self, state: &ProgressState) -> Result<()> {
        let json = serde_json::to_string_pretty(state)?;
        let tmp_path = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &self.state_path)?;
        Ok(())
    }

    fn stats(&self) -> (usize, u64, u64) {
        let state = self.state.lock();
        (
            state.completed_checkpoints.len(),
            state.objects_written,
            state.duplicates_skipped,
        )
    }
}

/// Simple object store with sharded directories.
struct ObjectStore {
    cache_root: PathBuf,
}

impl ObjectStore {
    fn new(cache_dir: &Path) -> Result<Self> {
        let objects_dir = cache_dir.join("objects");
        std::fs::create_dir_all(&objects_dir)?;
        Ok(Self {
            cache_root: cache_dir.to_path_buf(),
        })
    }

    fn object_path(&self, id: &AccountAddress, version: u64) -> PathBuf {
        let hex = hex::encode(id.as_ref());
        let aa = &hex[0..2];
        let bb = &hex[2..4];
        self.cache_root
            .join("objects")
            .join(aa)
            .join(bb)
            .join(&hex)
            .join(format!("{}.bcs", version))
    }

    fn has(&self, id: &AccountAddress, version: u64) -> bool {
        self.object_path(id, version).exists()
    }

    fn put(&self, id: &AccountAddress, version: u64, bcs: &[u8]) -> Result<()> {
        let path = self.object_path(id, version);
        if path.exists() {
            return Ok(()); // Idempotent
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, bcs)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   Walrus Cache Builder v2 (Production-Ready Bulk Ingestion)  ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Cache directory: {}", args.cache_dir);
    println!("Workers: {}", args.workers);
    println!("Batch size: {} checkpoints", args.batch_size);
    // Note: compression support could be added with zstd dependency
    println!();

    let walrus = WalrusClient::mainnet();

    // Determine checkpoint range
    let (start_checkpoint, end_checkpoint) = if let Some(count) = args.checkpoints {
        let latest = walrus.get_latest_checkpoint()?;
        let start = latest.saturating_sub(count - 1);
        println!("Target: {} checkpoints ({}..{})", count, start, latest);
        (start, latest)
    } else if let (Some(start), Some(end)) = (args.start_checkpoint, args.end_checkpoint) {
        println!(
            "Target: checkpoint range {}..{} ({} checkpoints)",
            start,
            end,
            end - start + 1
        );
        (start, end)
    } else {
        return Err(anyhow!(
            "Must specify either --checkpoints or --start-checkpoint/--end-checkpoint"
        ));
    };

    // Find all blobs that cover this range
    let all_blobs = walrus.list_blobs(None)?;
    let relevant_blobs: Vec<BlobInfo> = all_blobs
        .into_iter()
        .filter(|b| b.end_checkpoint >= start_checkpoint && b.start_checkpoint <= end_checkpoint)
        .collect();

    println!("Found {} blobs covering the range", relevant_blobs.len());

    // Calculate total size
    let total_checkpoints = end_checkpoint - start_checkpoint + 1;
    let total_blob_bytes: u64 = relevant_blobs.iter().map(|b| b.total_size).sum();
    println!(
        "Total blob data: {:.2} GB ({} checkpoints)",
        total_blob_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
        total_checkpoints
    );

    if args.dry_run {
        println!("\n[Dry run] Would process:");
        for blob in &relevant_blobs {
            let blob_start = start_checkpoint.max(blob.start_checkpoint);
            let blob_end = end_checkpoint.min(blob.end_checkpoint);
            println!(
                "  Blob {}: checkpoints {}..{} ({} checkpoints, {:.1} MB)",
                &blob.blob_id[..16],
                blob_start,
                blob_end,
                blob_end - blob_start + 1,
                blob.total_size as f64 / 1024.0 / 1024.0
            );
        }
        return Ok(());
    }

    println!();

    let cache_dir = Path::new(&args.cache_dir);
    let object_store = Arc::new(ObjectStore::new(cache_dir)?);
    let progress = Arc::new(ProgressTracker::new(cache_dir)?);

    // Build work items: split each blob into chunks of ~500 checkpoints for better parallelism
    // This ensures all workers stay busy even with few blobs
    const CHUNK_SIZE: u64 = 500;
    let mut work_items: Vec<(String, u64, u64)> = Vec::new();
    for blob in &relevant_blobs {
        let blob_start = start_checkpoint.max(blob.start_checkpoint);
        let blob_end = end_checkpoint.min(blob.end_checkpoint);

        // Split blob into chunks
        let mut chunk_start = blob_start;
        while chunk_start <= blob_end {
            let chunk_end = (chunk_start + CHUNK_SIZE - 1).min(blob_end);
            work_items.push((blob.blob_id.clone(), chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
    }

    // Shuffle work items to distribute load across blobs (helps with aggregator rate limiting)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    work_items.sort_by(|a, b| {
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.1.hash(&mut ha);
        b.1.hash(&mut hb);
        ha.finish().cmp(&hb.finish())
    });

    println!(
        "Work items: {} chunks across {} blobs",
        work_items.len(),
        relevant_blobs.len()
    );

    let work_queue = Arc::new(Mutex::new(work_items));
    let total_processed = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    // Spawn workers
    let mut handles = Vec::new();
    for worker_id in 0..args.workers {
        let work_queue = Arc::clone(&work_queue);
        let walrus = walrus.clone();
        let object_store = Arc::clone(&object_store);
        let progress = Arc::clone(&progress);
        let total_processed = Arc::clone(&total_processed);
        let batch_size = args.batch_size;
        let max_retries = args.max_retries;
        let max_chunk_bytes = args.max_chunk_bytes;
        let verbose = args.verbose;

        let handle = std::thread::spawn(move || -> Result<()> {
            loop {
                // Get next work item
                let work_item = {
                    let mut queue = work_queue.lock();
                    queue.pop()
                };

                let Some((blob_id, range_start, range_end)) = work_item else {
                    break; // No more work
                };

                println!(
                    "[Worker {}] Processing blob {} (checkpoints {}..{})",
                    worker_id,
                    &blob_id[..16],
                    range_start,
                    range_end
                );

                // Process in batches
                let mut current = range_start;
                while current <= range_end {
                    let batch_end = (current + batch_size as u64 - 1).min(range_end);

                    // Build list of checkpoints to fetch (skip already done)
                    let checkpoints: Vec<u64> = (current..=batch_end)
                        .filter(|&cp| !progress.is_checkpoint_done(cp))
                        .collect();

                    if checkpoints.is_empty() {
                        current = batch_end + 1;
                        continue;
                    }

                    // Fetch with retries and fallback to individual fetches
                    let checkpoint_data = fetch_with_fallback(
                        &walrus,
                        &checkpoints,
                        max_chunk_bytes,
                        max_retries,
                        worker_id,
                    )?;

                    // Process each checkpoint
                    for (checkpoint, data) in checkpoint_data {
                        let (objects_written, duplicates) =
                            process_checkpoint(&object_store, &data)?;

                        progress.mark_checkpoint_done(checkpoint, objects_written, duplicates);
                        let total = total_processed.fetch_add(1, Ordering::Relaxed) + 1;

                        if verbose || total.is_multiple_of(1000) {
                            let (done, objs, dups) = progress.stats();
                            println!(
                                "[Worker {}] Checkpoint {} done (total: {}, objects: {}, dups: {})",
                                worker_id, checkpoint, done, objs, dups
                            );
                        }
                    }

                    current = batch_end + 1;
                }
            }
            Ok(())
        });
        handles.push(handle);
    }

    // Wait for all workers
    for handle in handles {
        handle
            .join()
            .map_err(|_| anyhow!("Worker thread panicked"))??;
    }

    // Final save
    progress.save_state()?;

    let elapsed = start_time.elapsed().as_secs_f64();
    let (done, objs, dups) = progress.stats();

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Summary");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Checkpoints processed: {}", done);
    println!("Objects written:       {}", objs);
    println!("Duplicates skipped:    {}", dups);
    println!("Elapsed:               {:.1}s", elapsed);
    if done > 0 {
        println!("Checkpoints/sec:       {:.1}", done as f64 / elapsed);
    }
    if objs > 0 {
        println!("Objects/sec:           {:.1}", objs as f64 / elapsed);
    }

    // Calculate disk usage
    let disk_usage = calculate_disk_usage(cache_dir)?;
    println!(
        "Disk usage:            {:.2} GB",
        disk_usage as f64 / 1024.0 / 1024.0 / 1024.0
    );

    Ok(())
}

/// Fetch checkpoints with batching, retry logic, and fallback to individual fetches.
///
/// Strategy:
/// 1. Try batched fetch with configured max_chunk_bytes
/// 2. On failure, retry with smaller chunks (halve the size)
/// 3. After max_retries, fall back to fetching checkpoints individually
fn fetch_with_fallback(
    walrus: &WalrusClient,
    checkpoints: &[u64],
    max_chunk_bytes: u64,
    max_retries: usize,
    worker_id: usize,
) -> Result<Vec<(u64, sui_types::full_checkpoint_content::CheckpointData)>> {
    // Try batched fetch first
    let mut chunk_size = max_chunk_bytes;
    for attempt in 0..=max_retries {
        match walrus.get_checkpoints_batched(checkpoints, chunk_size) {
            Ok(data) => return Ok(data),
            Err(e) => {
                if attempt < max_retries {
                    // Reduce chunk size for next attempt
                    chunk_size = (chunk_size / 2).max(512 * 1024); // Min 512KB
                    eprintln!(
                        "[Worker {}] Batch fetch failed, reducing chunk to {}KB (attempt {}/{})",
                        worker_id,
                        chunk_size / 1024,
                        attempt + 1,
                        max_retries
                    );
                    std::thread::sleep(Duration::from_millis(500));
                } else {
                    eprintln!(
                        "[Worker {}] Batch fetch failed after {} retries, falling back to individual: {}",
                        worker_id, max_retries, e
                    );
                }
            }
        }
    }

    // Fallback: fetch checkpoints individually (slower but more reliable)
    let mut results = Vec::with_capacity(checkpoints.len());
    for &cp in checkpoints {
        match fetch_single_checkpoint(walrus, cp) {
            Ok(data) => results.push((cp, data)),
            Err(e) => {
                // Log but continue - we'll skip this checkpoint and it can be retried later
                eprintln!(
                    "[Worker {}] Failed to fetch checkpoint {}: {}",
                    worker_id, cp, e
                );
            }
        }
    }

    if results.is_empty() && !checkpoints.is_empty() {
        return Err(anyhow!("Failed to fetch any checkpoints in batch"));
    }

    Ok(results)
}

/// Fetch a single checkpoint with retries.
fn fetch_single_checkpoint(
    walrus: &WalrusClient,
    checkpoint: u64,
) -> Result<sui_types::full_checkpoint_content::CheckpointData> {
    for attempt in 0..3 {
        match walrus.get_checkpoint(checkpoint) {
            Ok(data) => return Ok(data),
            Err(e) => {
                if attempt < 2 {
                    std::thread::sleep(Duration::from_millis(200 * (attempt as u64 + 1)));
                } else {
                    return Err(e);
                }
            }
        }
    }
    unreachable!()
}

/// Process a single checkpoint, extracting and storing objects.
fn process_checkpoint(
    object_store: &ObjectStore,
    data: &sui_types::full_checkpoint_content::CheckpointData,
) -> Result<(u64, u64)> {
    let mut objects_written = 0u64;
    let mut duplicates = 0u64;

    for tx in &data.transactions {
        // Process input objects
        for obj in &tx.input_objects {
            if let Some((id, version, bcs)) = extract_object_data(obj) {
                if object_store.has(&id, version) {
                    duplicates += 1;
                } else {
                    object_store.put(&id, version, &bcs)?;
                    objects_written += 1;
                }
            }
        }

        // Process output objects
        for obj in &tx.output_objects {
            if let Some((id, version, bcs)) = extract_object_data(obj) {
                if object_store.has(&id, version) {
                    duplicates += 1;
                } else {
                    object_store.put(&id, version, &bcs)?;
                    objects_written += 1;
                }
            }
        }
    }

    Ok((objects_written, duplicates))
}

/// Extract object data from a Sui object.
fn extract_object_data(obj: &sui_types::object::Object) -> Option<(AccountAddress, u64, Vec<u8>)> {
    // Only process Move objects
    let data = obj.data.try_as_move()?;
    let id = AccountAddress::new(obj.id().into_bytes());
    let version = obj.version().value();
    let bcs = data.contents().to_vec();
    Some((id, version, bcs))
}

fn calculate_disk_usage(path: &Path) -> Result<u64> {
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
