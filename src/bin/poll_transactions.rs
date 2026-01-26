//! Continuous transaction polling with local caching (GraphQL)
//!
//! Polls for new transactions and saves them to a local JSONL cache.
//! Provides complete transaction data including full effects.
//!
//! TRADEOFFS vs gRPC streaming (stream_transactions):
//!   ✅ Complete effects - includes created/mutated/deleted arrays
//!   ✅ Reliable connections - stateless HTTP requests
//!   ✅ Good for replay verification
//!   ❌ Polling gaps - may miss transactions between polls
//!   ❌ Lower throughput (~10-20 tx/sec due to rate limits)
//!   ❌ Higher latency (polling interval dependent)
//!
//! Use this for: replay verification, effects analysis, historical queries
//! Use stream_transactions for: real-time monitoring, high-volume collection
//!
//! Usage:
//!   cargo run --bin poll_transactions -- --duration 600 --output txs_cache.jsonl
//!
//! Options:
//!   --duration <secs>    How long to run (default: 60)
//!   --output <file>      Output file path (default: transactions_cache.jsonl)
//!   --interval <ms>      Polling interval in ms (default: 1000)
//!   --batch-size <n>     Transactions per fetch (default: 20)
//!   --ptb-only           Only save PTB transactions (skip system txs)
//!   --verbose            Print detailed progress

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sui_sandbox::graphql::{GraphQLClient, GraphQLTransaction};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedTransaction {
    /// When this was fetched (Unix timestamp ms)
    fetched_at_ms: u64,
    /// The full transaction data
    #[serde(flatten)]
    transaction: GraphQLTransaction,
}

#[derive(Debug)]
struct Stats {
    total_fetched: usize,
    duplicates_skipped: usize,
    system_txs_skipped: usize,
    errors: usize,
    rate_limits: usize,
    requests: usize,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Parse arguments
    let mut duration_secs = 60u64;
    let mut output_path = PathBuf::from("transactions_cache.jsonl");
    let mut interval_ms = 1000u64;
    let mut batch_size = 20usize;
    let mut ptb_only = false;
    let mut verbose = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--duration" => {
                i += 1;
                duration_secs = args.get(i).map(|s| s.parse().unwrap_or(60)).unwrap_or(60);
            }
            "--output" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    output_path = PathBuf::from(p);
                }
            }
            "--interval" => {
                i += 1;
                interval_ms = args
                    .get(i)
                    .map(|s| s.parse().unwrap_or(1000))
                    .unwrap_or(1000);
            }
            "--batch-size" => {
                i += 1;
                batch_size = args.get(i).map(|s| s.parse().unwrap_or(20)).unwrap_or(20);
            }
            "--ptb-only" => {
                ptb_only = true;
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    println!("=== Transaction Polling ===");
    println!("Duration: {}s", duration_secs);
    println!("Output: {}", output_path.display());
    println!("Interval: {}ms", interval_ms);
    println!("Batch size: {}", batch_size);
    println!("PTB only: {}", ptb_only);
    println!();

    // Initialize
    let client = GraphQLClient::mainnet();
    let mut seen_digests: HashSet<String> = HashSet::new();
    let mut stats = Stats {
        total_fetched: 0,
        duplicates_skipped: 0,
        system_txs_skipped: 0,
        errors: 0,
        rate_limits: 0,
        requests: 0,
    };

    // Open output file
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&output_path)
        .context("Failed to open output file")?;
    let mut writer = BufWriter::new(file);

    let start = Instant::now();
    let duration = Duration::from_secs(duration_secs);
    let interval = Duration::from_millis(interval_ms);

    let mut last_request = Instant::now();

    println!("Starting polling loop...");
    println!();

    while start.elapsed() < duration {
        // Rate limit ourselves
        let elapsed_since_last = last_request.elapsed();
        if elapsed_since_last < interval {
            std::thread::sleep(interval - elapsed_since_last);
        }
        last_request = Instant::now();

        stats.requests += 1;

        // Fetch transactions
        let result = if ptb_only {
            client.fetch_recent_ptb_transactions(batch_size)
        } else {
            client.fetch_recent_transactions_full(batch_size)
        };

        match result {
            Ok(txs) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;

                let mut new_count = 0;

                for tx in txs {
                    // Skip duplicates
                    if seen_digests.contains(&tx.digest) {
                        stats.duplicates_skipped += 1;
                        continue;
                    }

                    // Skip system transactions if ptb_only
                    if ptb_only && tx.sender.is_empty() {
                        stats.system_txs_skipped += 1;
                        continue;
                    }

                    seen_digests.insert(tx.digest.clone());

                    // Write to cache
                    let cached = CachedTransaction {
                        fetched_at_ms: now_ms,
                        transaction: tx,
                    };

                    let line = serde_json::to_string(&cached)?;
                    writeln!(writer, "{}", line)?;

                    stats.total_fetched += 1;
                    new_count += 1;
                }

                // Flush periodically
                writer.flush()?;

                if verbose || new_count > 0 {
                    let elapsed = start.elapsed().as_secs();
                    let remaining = duration_secs.saturating_sub(elapsed);
                    println!(
                        "[{:3}s remaining] +{} new txs (total: {}, dups: {}, seen: {})",
                        remaining,
                        new_count,
                        stats.total_fetched,
                        stats.duplicates_skipped,
                        seen_digests.len()
                    );
                }
            }
            Err(e) => {
                let err_str = format!("{:?}", e);
                if err_str.contains("429") || err_str.to_lowercase().contains("rate") {
                    stats.rate_limits += 1;
                    eprintln!(
                        "[RATE LIMITED] Backing off... (count: {})",
                        stats.rate_limits
                    );
                    // Exponential backoff on rate limit
                    std::thread::sleep(Duration::from_secs(
                        2u64.pow(stats.rate_limits.min(5) as u32),
                    ));
                } else {
                    stats.errors += 1;
                    if verbose {
                        eprintln!("[ERROR] {}", e);
                    }
                }
            }
        }
    }

    // Final flush
    writer.flush()?;

    // Print summary
    println!();
    println!("=== Summary ===");
    println!("Duration: {:.1}s", start.elapsed().as_secs_f64());
    println!("Requests made: {}", stats.requests);
    println!("Transactions saved: {}", stats.total_fetched);
    println!("Duplicates skipped: {}", stats.duplicates_skipped);
    println!("System txs skipped: {}", stats.system_txs_skipped);
    println!("Unique digests seen: {}", seen_digests.len());
    println!("Errors: {}", stats.errors);
    println!("Rate limits hit: {}", stats.rate_limits);
    println!();

    let rate = stats.total_fetched as f64 / start.elapsed().as_secs_f64();
    println!("Effective rate: {:.2} txs/sec", rate);
    println!("Output: {}", output_path.display());

    // File size
    if let Ok(meta) = std::fs::metadata(&output_path) {
        let size_kb = meta.len() as f64 / 1024.0;
        println!("File size: {:.1} KB", size_kb);
    }

    Ok(())
}

fn print_usage() {
    println!("GraphQL Transaction Polling Tool");
    println!();
    println!("Fetches transactions from Sui mainnet via GraphQL polling.");
    println!();
    println!("TRADEOFFS vs stream_transactions (gRPC):");
    println!("  ✓ Complete effects - includes created/mutated/deleted arrays");
    println!("  ✓ Reliable connections - stateless HTTP requests");
    println!("  ✓ Better for replay verification");
    println!("  ✗ Polling gaps - may miss transactions between polls");
    println!("  ✗ Lower throughput (~10-20 tx/sec due to rate limits)");
    println!("  ✗ Higher latency (depends on polling interval)");
    println!();
    println!("USE THIS FOR: replay verification, effects analysis");
    println!("USE stream_transactions FOR: real-time monitoring, high-volume collection");
    println!();
    println!("Usage:");
    println!("  poll_transactions [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --duration <secs>    How long to run (default: 60)");
    println!("  --output <file>      Output file (default: transactions_cache.jsonl)");
    println!("  --interval <ms>      Polling interval in ms (default: 1000)");
    println!("  --batch-size <n>     Transactions per fetch (default: 20)");
    println!("  --ptb-only           Only save PTB transactions (skip system txs)");
    println!("  --verbose, -v        Print detailed progress");
    println!("  --help, -h           Show this help");
    println!();
    println!("Examples:");
    println!("  # Run for 10 minutes with 2-second polling");
    println!("  poll_transactions --duration 600 --interval 2000 --output my_cache.jsonl");
    println!();
    println!("  # Quick test for 30 seconds");
    println!("  poll_transactions --duration 30 --verbose");
    println!();
    println!("Rate Limit Notes:");
    println!("  - Sui GraphQL has rate limits (~100 req/min estimated)");
    println!("  - Use --interval 1000+ to stay safe");
    println!("  - Tool automatically backs off on 429 errors");
}
