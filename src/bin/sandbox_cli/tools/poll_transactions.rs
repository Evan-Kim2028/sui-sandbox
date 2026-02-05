//! Continuous transaction polling with local caching (GraphQL)

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sui_sandbox::graphql::{GraphQLClient, GraphQLTransaction};

#[derive(Debug, Parser)]
#[command(
    name = "poll-transactions",
    about = "Poll recent transactions via GraphQL"
)]
pub struct PollTransactionsCmd {
    /// How long to run (seconds)
    #[arg(long, default_value_t = 60, value_name = "SECS")]
    duration: u64,

    /// Output file path (JSONL)
    #[arg(long, default_value = "transactions_cache.jsonl", value_name = "FILE")]
    output: PathBuf,

    /// Polling interval in ms
    #[arg(long, default_value_t = 1000, value_name = "MS")]
    interval_ms: u64,

    /// Transactions per fetch
    #[arg(long, default_value_t = 20)]
    batch_size: usize,

    /// Only save PTB transactions (skip system txs)
    #[arg(long, default_value_t = false)]
    ptb_only: bool,

    /// Print detailed progress
    #[arg(long, short, default_value_t = false)]
    verbose: bool,
}

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

impl PollTransactionsCmd {
    pub fn execute(&self) -> Result<()> {
        println!("=== Transaction Polling ===");
        println!("Duration: {}s", self.duration);
        println!("Output: {}", self.output.display());
        println!("Interval: {}ms", self.interval_ms);
        println!("Batch size: {}", self.batch_size);
        println!("PTB only: {}", self.ptb_only);
        println!();

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

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output)
            .context("Failed to open output file")?;
        let mut writer = BufWriter::new(file);

        let start = Instant::now();
        let duration = Duration::from_secs(self.duration);
        let interval = Duration::from_millis(self.interval_ms);
        let mut last_request = Instant::now();

        println!("Starting polling loop...\n");

        while start.elapsed() < duration {
            let elapsed_since_last = last_request.elapsed();
            if elapsed_since_last < interval {
                std::thread::sleep(interval - elapsed_since_last);
            }
            last_request = Instant::now();

            stats.requests += 1;

            let result = if self.ptb_only {
                client.fetch_recent_ptb_transactions(self.batch_size)
            } else {
                client.fetch_recent_transactions_full(self.batch_size)
            };

            match result {
                Ok(txs) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;

                    let mut new_count = 0;

                    for tx in txs {
                        if seen_digests.contains(&tx.digest) {
                            stats.duplicates_skipped += 1;
                            continue;
                        }

                        if self.ptb_only && tx.sender.is_empty() {
                            stats.system_txs_skipped += 1;
                            continue;
                        }

                        seen_digests.insert(tx.digest.clone());

                        let cached = CachedTransaction {
                            fetched_at_ms: now_ms,
                            transaction: tx,
                        };

                        let line = serde_json::to_string(&cached)?;
                        writeln!(writer, "{}", line)?;

                        stats.total_fetched += 1;
                        new_count += 1;
                    }

                    writer.flush()?;

                    if self.verbose || new_count > 0 {
                        let elapsed = start.elapsed().as_secs();
                        let remaining = self.duration.saturating_sub(elapsed);
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
                        std::thread::sleep(Duration::from_secs(
                            2u64.pow(stats.rate_limits.min(5) as u32),
                        ));
                    } else {
                        stats.errors += 1;
                        if self.verbose {
                            eprintln!("[ERROR] {}", e);
                        }
                    }
                }
            }
        }

        writer.flush()?;

        println!("\n=== Summary ===");
        println!("Total fetched: {}", stats.total_fetched);
        println!("Duplicates skipped: {}", stats.duplicates_skipped);
        println!("System txs skipped: {}", stats.system_txs_skipped);
        println!("Errors: {}", stats.errors);
        println!("Rate limits: {}", stats.rate_limits);
        println!("Requests: {}", stats.requests);

        Ok(())
    }
}
