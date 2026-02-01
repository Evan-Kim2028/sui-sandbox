//! Walrus Cache Builder v3: High-performance with blob index parsing.
//!
//! Key optimizations over v2:
//! - Parses blob footer/index for direct checkpoint offsets (skips per-checkpoint metadata API)
//! - Uses HTTP Range headers for efficient byte-range fetches
//! - High concurrency with thread pool (configurable workers)
//! - Processes checkpoints in parallel within each blob
//!
//! Based on patterns from DeepBook's Walrus storage implementation.
//!
//! Run:
//! ```bash
//! cargo run --release --example walrus_cache_build_v3 -- \
//!     --cache-dir ./walrus-cache \
//!     --checkpoints 100000 \
//!     --workers 16
//! ```

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sui_transport::blob::Blob;
use sui_types::full_checkpoint_content::CheckpointData;

/// Build a filesystem cache from Walrus checkpoint blobs (v3 - high performance).
#[derive(Parser, Debug)]
struct Args {
    /// Cache directory path.
    #[arg(long)]
    cache_dir: String,

    /// Number of checkpoints to ingest (most recent N).
    #[arg(long, conflicts_with_all = &["start_checkpoint", "end_checkpoint"])]
    checkpoints: Option<u64>,

    /// Start checkpoint (inclusive).
    #[arg(long, requires = "end_checkpoint")]
    start_checkpoint: Option<u64>,

    /// End checkpoint (inclusive).
    #[arg(long, requires = "start_checkpoint")]
    end_checkpoint: Option<u64>,

    /// Number of parallel workers.
    #[arg(long, default_value_t = 16)]
    workers: usize,

    /// Dry run: show what would be processed.
    #[arg(long)]
    dry_run: bool,
}

/// Walrus blob metadata
#[derive(Debug, Clone, Deserialize)]
struct BlobMetadata {
    blob_id: String,
    start_checkpoint: u64,
    end_checkpoint: u64,
    total_size: u64,
}

#[derive(Debug, Deserialize)]
struct BlobsResponse {
    blobs: Vec<BlobMetadata>,
}

/// Checkpoint location in blob (from index)
#[derive(Debug, Clone)]
struct CheckpointLocation {
    checkpoint: u64,
    blob_id: String,
    offset: u64,
    length: u64,
}

/// Progress state
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct ProgressState {
    completed_checkpoints: HashSet<u64>,
    objects_written: u64,
    duplicates_skipped: u64,
}

/// Object store
struct ObjectStore {
    cache_root: PathBuf,
}

impl ObjectStore {
    fn new(cache_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(cache_dir.join("objects"))?;
        Ok(Self {
            cache_root: cache_dir.to_path_buf(),
        })
    }

    fn object_path(&self, id: &AccountAddress, version: u64) -> PathBuf {
        let hex = hex::encode(id.as_ref());
        self.cache_root
            .join("objects")
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(&hex)
            .join(format!("{}.bcs", version))
    }

    fn has(&self, id: &AccountAddress, version: u64) -> bool {
        self.object_path(id, version).exists()
    }

    fn put(&self, id: &AccountAddress, version: u64, bcs: &[u8]) -> Result<()> {
        let path = self.object_path(id, version);
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            // Ignore race condition where another thread creates the dir
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("tmp");
        // Retry once on failure (race condition)
        if std::fs::write(&tmp, bcs).is_err() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&tmp, bcs)?;
        }
        // Ignore rename errors if file already exists (race)
        let _ = std::fs::rename(&tmp, &path);
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   Walrus Cache Builder v3 (High-Performance Blob Index)      ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Cache directory: {}", args.cache_dir);
    println!("Workers: {}", args.workers);
    println!();

    let http = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .build();

    let archival_url = "https://walrus-sui-archival.mainnet.walrus.space";
    let aggregator_url = "https://aggregator.walrus-mainnet.walrus.space";

    // Fetch blob metadata
    println!("Fetching blob metadata...");
    let blobs_response: BlobsResponse = http
        .get(&format!("{}/v1/app_blobs", archival_url))
        .call()
        .context("failed to fetch blobs")?
        .into_json()
        .context("failed to parse blobs")?;

    let latest = blobs_response
        .blobs
        .iter()
        .map(|b| b.end_checkpoint)
        .max()
        .unwrap_or(0);

    // Determine range
    let (start_cp, end_cp) = if let Some(count) = args.checkpoints {
        let start = latest.saturating_sub(count - 1);
        println!("Target: {} checkpoints ({}..{})", count, start, latest);
        (start, latest)
    } else if let (Some(s), Some(e)) = (args.start_checkpoint, args.end_checkpoint) {
        println!("Target: checkpoint range {}..{}", s, e);
        (s, e)
    } else {
        return Err(anyhow!(
            "Must specify --checkpoints or --start/end-checkpoint"
        ));
    };

    // Find relevant blobs
    let relevant_blobs: Vec<BlobMetadata> = blobs_response
        .blobs
        .into_iter()
        .filter(|b| b.end_checkpoint >= start_cp && b.start_checkpoint <= end_cp)
        .collect();

    println!("Found {} blobs covering the range", relevant_blobs.len());

    if args.dry_run {
        println!("\n[Dry run] Would process:");
        for blob in &relevant_blobs {
            println!(
                "  Blob {}: checkpoints {}..{} ({:.1} MB)",
                &blob.blob_id[..16],
                blob.start_checkpoint,
                blob.end_checkpoint,
                blob.total_size as f64 / 1024.0 / 1024.0
            );
        }
        return Ok(());
    }

    // Load progress
    let cache_dir = Path::new(&args.cache_dir);
    let progress_path = cache_dir.join("progress").join("state_v3.json");
    std::fs::create_dir_all(progress_path.parent().unwrap())?;
    let progress: Arc<Mutex<ProgressState>> = Arc::new(Mutex::new(if progress_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&progress_path)?)?
    } else {
        ProgressState::default()
    }));

    let object_store = Arc::new(ObjectStore::new(cache_dir)?);

    // Build work queue: parse blob indices to get checkpoint locations
    println!("\nParsing blob indices...");
    let mut all_locations: Vec<CheckpointLocation> = Vec::new();

    for blob in &relevant_blobs {
        let blob_url = format!("{}/v1/blobs/{}", aggregator_url, blob.blob_id);

        // Fetch footer (last 24 bytes) using suffix range
        let footer = match fetch_suffix(&http, &blob_url, 24) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "  Blob {}: SKIPPED (footer fetch failed: {})",
                    &blob.blob_id[..16],
                    e
                );
                continue;
            }
        };
        if footer.len() != 24 {
            eprintln!(
                "Blob {} has invalid footer (got {} bytes)",
                &blob.blob_id[..16],
                footer.len()
            );
            continue;
        }

        let mut cur = Cursor::new(&footer);
        let magic = read_u32_le(&mut cur)?;
        if magic != 0x574c4244 {
            eprintln!(
                "Blob {} has invalid magic: {:x}",
                &blob.blob_id[..16],
                magic
            );
            continue;
        }
        let _version = read_u32_le(&mut cur)?;
        let index_offset = read_u64_le(&mut cur)?;
        let entry_count = read_u32_le(&mut cur)?;

        // Fetch index (from offset to end)
        let index_bytes = match fetch_from_offset(&http, &blob_url, index_offset) {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "  Blob {}: SKIPPED (index fetch failed: {})",
                    &blob.blob_id[..16],
                    e
                );
                continue;
            }
        };
        let mut cur = Cursor::new(&index_bytes);

        let mut count = 0;
        for _ in 0..entry_count {
            let name_len = read_u32_le(&mut cur)? as usize;
            let mut name = vec![0u8; name_len];
            cur.read_exact(&mut name)?;
            let checkpoint: u64 = String::from_utf8_lossy(&name).parse().unwrap_or(0);
            let offset = read_u64_le(&mut cur)?;
            let length = read_u64_le(&mut cur)?;
            let _crc = read_u32_le(&mut cur)?;

            if checkpoint >= start_cp && checkpoint <= end_cp {
                // Skip already completed
                if !progress.lock().completed_checkpoints.contains(&checkpoint) {
                    all_locations.push(CheckpointLocation {
                        checkpoint,
                        blob_id: blob.blob_id.clone(),
                        offset,
                        length,
                    });
                    count += 1;
                }
            }
        }
        println!(
            "  Blob {}: {} checkpoints to fetch",
            &blob.blob_id[..16],
            count
        );
    }

    println!("\nTotal checkpoints to fetch: {}", all_locations.len());

    if all_locations.is_empty() {
        println!("All checkpoints already done!");
        return Ok(());
    }

    // Process with thread pool
    let work_queue = Arc::new(Mutex::new(all_locations));
    let total_processed = Arc::new(AtomicU64::new(0));
    let total_to_process = work_queue.lock().len() as u64;
    let start_time = Instant::now();

    let mut handles = Vec::new();
    for worker_id in 0..args.workers {
        let work_queue = Arc::clone(&work_queue);
        let progress = Arc::clone(&progress);
        let object_store = Arc::clone(&object_store);
        let total_processed = Arc::clone(&total_processed);
        let aggregator_url = aggregator_url.to_string();

        let handle = std::thread::spawn(move || -> Result<()> {
            let http = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(30))
                .build();

            loop {
                // Get next work item
                let location = {
                    let mut queue = work_queue.lock();
                    queue.pop()
                };

                let Some(loc) = location else { break };

                // Fetch checkpoint data
                let blob_url = format!("{}/v1/blobs/{}", aggregator_url, loc.blob_id);
                let bytes = match fetch_range_with_retry(&http, &blob_url, loc.offset, loc.length) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!(
                            "[Worker {}] Failed to fetch checkpoint {}: {}",
                            worker_id, loc.checkpoint, e
                        );
                        continue;
                    }
                };

                // Deserialize
                let data: CheckpointData = match Blob::from_bytes(&bytes) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!(
                            "[Worker {}] Failed to deserialize checkpoint {}: {}",
                            worker_id, loc.checkpoint, e
                        );
                        continue;
                    }
                };

                // Extract objects
                let mut objects_written = 0u64;
                let mut duplicates = 0u64;

                for tx in &data.transactions {
                    for obj in tx.input_objects.iter().chain(tx.output_objects.iter()) {
                        if let Some(move_data) = obj.data.try_as_move() {
                            let id = AccountAddress::new(obj.id().into_bytes());
                            let version = obj.version().value();
                            if object_store.has(&id, version) {
                                duplicates += 1;
                            } else {
                                if let Err(e) = object_store.put(&id, version, move_data.contents())
                                {
                                    eprintln!("Failed to write object: {}", e);
                                }
                                objects_written += 1;
                            }
                        }
                    }
                }

                // Update progress
                {
                    let mut p = progress.lock();
                    p.completed_checkpoints.insert(loc.checkpoint);
                    p.objects_written += objects_written;
                    p.duplicates_skipped += duplicates;
                }

                let done = total_processed.fetch_add(1, Ordering::Relaxed) + 1;
                if done.is_multiple_of(500) {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let rate = done as f64 / elapsed;
                    let remaining = total_to_process - done;
                    let eta = remaining as f64 / rate;
                    println!(
                        "[Progress] {}/{} ({:.1}%) - {:.1} cp/sec - ETA: {:.0}s",
                        done,
                        total_to_process,
                        done as f64 / total_to_process as f64 * 100.0,
                        rate,
                        eta
                    );
                }
            }
            Ok(())
        });
        handles.push(handle);
    }

    // Wait for workers
    for handle in handles {
        handle.join().map_err(|_| anyhow!("Worker panicked"))??;
    }

    // Save progress
    let progress_state = progress.lock().clone();
    std::fs::write(
        &progress_path,
        serde_json::to_string_pretty(&progress_state)?,
    )?;

    let elapsed = start_time.elapsed().as_secs_f64();
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Summary");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "Checkpoints processed: {}",
        progress_state.completed_checkpoints.len()
    );
    println!("Objects written:       {}", progress_state.objects_written);
    println!(
        "Duplicates skipped:    {}",
        progress_state.duplicates_skipped
    );
    println!("Elapsed:               {:.1}s", elapsed);
    println!(
        "Checkpoints/sec:       {:.1}",
        total_to_process as f64 / elapsed
    );

    let disk_usage = calculate_disk_usage(cache_dir)?;
    println!("Disk usage:            {:.2} GB", disk_usage as f64 / 1e9);

    Ok(())
}

/// Fetch the last N bytes using suffix range (bytes=-N)
fn fetch_suffix(http: &ureq::Agent, url: &str, suffix_len: u64) -> Result<Vec<u8>> {
    let mut attempts = 0;
    loop {
        let result = http
            .get(url)
            .set("Range", &format!("bytes=-{}", suffix_len))
            .call();

        match result {
            Ok(response) => {
                let mut bytes = Vec::with_capacity(suffix_len as usize);
                response.into_reader().read_to_end(&mut bytes)?;
                return Ok(bytes);
            }
            Err(e) if attempts < 3 => {
                attempts += 1;
                eprintln!("  Retry {} fetching suffix: {}", attempts, e);
                std::thread::sleep(Duration::from_millis(500 * attempts));
            }
            Err(e) => return Err(anyhow!("HTTP request failed after retries: {}", e)),
        }
    }
}

/// Fetch from offset to end of file (bytes=N-)
fn fetch_from_offset(http: &ureq::Agent, url: &str, offset: u64) -> Result<Vec<u8>> {
    let mut attempts = 0;
    loop {
        let result = http
            .get(url)
            .set("Range", &format!("bytes={}-", offset))
            .call();

        match result {
            Ok(response) => {
                let mut bytes = Vec::new();
                response.into_reader().read_to_end(&mut bytes)?;
                return Ok(bytes);
            }
            Err(e) if attempts < 3 => {
                attempts += 1;
                eprintln!("  Retry {} fetching from offset: {}", attempts, e);
                std::thread::sleep(Duration::from_millis(500 * attempts));
            }
            Err(e) => return Err(anyhow!("HTTP request failed after retries: {}", e)),
        }
    }
}

fn fetch_range(http: &ureq::Agent, url: &str, start: u64, length: u64) -> Result<Vec<u8>> {
    let end = start + length - 1;
    let response = http
        .get(url)
        .set("Range", &format!("bytes={}-{}", start, end))
        .call()
        .context("HTTP request failed")?;

    let mut bytes = Vec::with_capacity(length as usize);
    response.into_reader().read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn fetch_range_with_retry(
    http: &ureq::Agent,
    url: &str,
    start: u64,
    length: u64,
) -> Result<Vec<u8>> {
    let mut attempts = 0;
    loop {
        match fetch_range(http, url, start, length) {
            Ok(b) => return Ok(b),
            Err(e) if attempts < 3 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(500 * attempts));
            }
            Err(e) => return Err(e),
        }
    }
}

fn read_u32_le<T: AsRef<[u8]>>(cur: &mut Cursor<T>) -> Result<u32> {
    let mut buf = [0u8; 4];
    cur.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64_le<T: AsRef<[u8]>>(cur: &mut Cursor<T>) -> Result<u64> {
    let mut buf = [0u8; 8];
    cur.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn calculate_disk_usage(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                total += calculate_disk_usage(&path)?;
            } else if let Ok(m) = path.metadata() {
                total += m.len();
            }
        }
    }
    Ok(total)
}
