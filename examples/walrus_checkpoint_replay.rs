//! Walrus Checkpoint PTB Replay (Single Entry Point)
//!
//! This example replays PTB transactions from a Walrus checkpoint range by:
//! - Fetching checkpoint transaction data from Walrus (free, public)
//! - Loading required Move packages via gRPC archive (cacheable)
//! - Executing PTBs locally in the sui-sandbox Move VM harness
//!
//! It is designed as the *single* entry point for Walrus integration demos.
//!
//! Run:
//! ```bash
//! # Default range (small, reproducible)
//! cargo run --release --example walrus_checkpoint_replay
//!
//! # Custom range
//! cargo run --release --example walrus_checkpoint_replay -- --start 238627315 --end 238627325
//! ```

mod walrus_checkpoint;

use anyhow::{anyhow, Result};
use clap::Parser;
use dotenv::dotenv;
use std::sync::Arc;
use std::time::Instant;

use sui_sandbox_core::simulation::SimulationEnvironment;
use sui_state_fetcher::HistoricalStateProvider;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;
use sui_transport::walrus::WalrusClient;

use sui_historical_cache::{FsObjectStore, FsPackageStore};
use walrus_checkpoint::replay_engine::{ReasonCode, ReplayEngine, ReplayStats};

/// Replay PTBs from a Walrus checkpoint range.
#[derive(Parser, Debug)]
struct Args {
    /// Inclusive start checkpoint number.
    #[arg(long)]
    start: Option<u64>,

    /// Exclusive end checkpoint number.
    #[arg(long)]
    end: Option<u64>,

    /// Stop after executing N PTBs total (across the whole range).
    #[arg(long)]
    max_ptbs: Option<usize>,

    /// Max retry attempts per PTB (default: 4 includes MM2 attempt).
    #[arg(long, default_value_t = 4)]
    max_attempts: usize,

    /// Print per-transaction failures.
    #[arg(long)]
    verbose_failures: bool,

    /// Load framework packages (0x1/0x2/0x3) from GraphQL instead of bundled bytecode.
    #[arg(long, default_value_t = false)]
    framework_from_graphql: bool,

    /// Fetch checkpoints in batched blob byte-ranges (fewer downloads, faster).
    #[arg(long, default_value_t = true)]
    batch_by_blob: bool,

    /// Max bytes per merged blob download (only used with --batch-by-blob).
    #[arg(long, default_value_t = 128 * 1024 * 1024)]
    max_blob_chunk_bytes: u64,

    /// Optional cache directory for L2 object/package lookups.
    #[arg(long)]
    cache_dir: Option<String>,
}

/// Default range chosen to match the existing benchmark docs.
const DEFAULT_START: u64 = 238_627_315;
const DEFAULT_END: u64 = 238_627_325; // exclusive

fn build_state_fetcher(
    rt: &tokio::runtime::Runtime,
) -> Option<(Arc<HistoricalStateProvider>, String)> {
    let archive_endpoint = std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT")
        .unwrap_or_else(|_| "https://archive.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let graphql = GraphQLClient::mainnet();
    let archive_client =
        rt.block_on(async { GrpcClient::with_api_key(&archive_endpoint, api_key.clone()).await });
    if let Ok(grpc) = archive_client {
        return Some((
            Arc::new(HistoricalStateProvider::with_clients(grpc, graphql)),
            archive_endpoint,
        ));
    }

    if let Ok(env_endpoint) = std::env::var("SUI_GRPC_ENDPOINT") {
        let graphql = GraphQLClient::mainnet();
        if let Ok(grpc) =
            rt.block_on(async { GrpcClient::with_api_key(&env_endpoint, api_key).await })
        {
            return Some((
                Arc::new(HistoricalStateProvider::with_clients(grpc, graphql)),
                env_endpoint,
            ));
        }
    }

    None
}

fn main() -> Result<()> {
    dotenv().ok();
    let args = Args::parse();

    let start = args.start.unwrap_or(DEFAULT_START);
    let end = args.end.unwrap_or(DEFAULT_END);
    if start >= end {
        return Err(anyhow!("--start must be < --end"));
    }

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   Walrus Checkpoint PTB Replay (Walrus + gRPC Archive)        ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!(
        "Checkpoint range: {}..{} ({} checkpoints)",
        start,
        end,
        end - start
    );
    if let Some(n) = args.max_ptbs {
        println!("Max PTBs: {}", n);
    }
    println!("Max attempts per PTB: {}", args.max_attempts);
    println!();

    let walrus = WalrusClient::mainnet();
    let graphql = GraphQLClient::mainnet();

    // Create a dedicated runtime for async gRPC fetches.
    let rt = tokio::runtime::Runtime::new()?;
    let archive_endpoint = std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT")
        .unwrap_or_else(|_| "https://archive.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
    println!("Connecting to gRPC archive endpoint...");
    let grpc = rt.block_on(async { GrpcClient::with_api_key(&archive_endpoint, api_key).await })?;
    println!("✓ Connected to: {}", grpc.endpoint());
    println!();

    let grpc = Arc::new(grpc);
    let state_fetcher = match build_state_fetcher(&rt) {
        Some((fetcher, endpoint)) => {
            println!("✓ Historical state fetcher enabled: {}", endpoint);
            println!();
            Some(fetcher)
        }
        None => {
            println!("! Historical state fetcher disabled (archive unavailable)");
            println!();
            None
        }
    };

    // Initialize disk cache if provided
    let object_store = args.cache_dir.as_ref().and_then(|dir| {
        FsObjectStore::new(dir)
            .map_err(|e| {
                eprintln!("Warning: Failed to initialize object cache: {}", e);
            })
            .ok()
    });
    let package_store = args.cache_dir.as_ref().and_then(|dir| {
        FsPackageStore::new(dir)
            .map_err(|e| {
                eprintln!("Warning: Failed to initialize package cache: {}", e);
            })
            .ok()
    });

    if let Some(cache_dir) = &args.cache_dir {
        println!("✓ Disk cache enabled: {}", cache_dir);
        println!();
    }

    let mut engine = ReplayEngine::new(
        &walrus,
        Arc::clone(&grpc),
        &graphql,
        &rt,
        state_fetcher,
        object_store,
        package_store,
    );

    // Keep one environment alive and reuse the resolver; reset state per transaction.
    let mut env = SimulationEnvironment::new()?;
    if args.framework_from_graphql {
        let loaded = env
            .resolver_mut()
            .load_sui_framework_from_graphql(&graphql)?;
        println!(
            "✓ Loaded {} framework modules from GraphQL (0x1/0x2/0x3)",
            loaded
        );
    }

    let mut stats = ReplayStats::default();

    let start_time = Instant::now();

    let mut checkpoints_data: Vec<(u64, Vec<serde_json::Value>)> = Vec::new();
    let mut ptb_target = args.max_ptbs.unwrap_or(usize::MAX);

    let checkpoints: Vec<u64> = (start..end).collect();
    if args.batch_by_blob {
        let checkpoint_fetch_start = Instant::now();
        let decoded =
            walrus.get_checkpoints_json_batched(&checkpoints, args.max_blob_chunk_bytes)?;
        let checkpoint_fetch_s = checkpoint_fetch_start.elapsed().as_secs_f64();
        println!(
            "Fetched {} checkpoints via batched blob download (total {:.2}s, avg {:.2}ms/checkpoint)",
            decoded.len(),
            checkpoint_fetch_s,
            (checkpoint_fetch_s * 1000.0) / (decoded.len().max(1) as f64)
        );
        for (checkpoint, checkpoint_json) in decoded {
            if ptb_target == 0 {
                break;
            }
            let transactions = checkpoint_json
                .get("transactions")
                .and_then(|t| t.as_array())
                .ok_or_else(|| anyhow!("Walrus checkpoint missing transactions array"))?;
            let mut ptbs: Vec<serde_json::Value> = Vec::new();
            for tx_json in transactions {
                let is_ptb = tx_json
                    .pointer(
                        "/transaction/data/0/intent_message/value/V1/kind/ProgrammableTransaction",
                    )
                    .is_some();
                if !is_ptb {
                    continue;
                }
                if ptb_target == 0 {
                    break;
                }
                ptbs.push(tx_json.clone());
                ptb_target = ptb_target.saturating_sub(1);
            }
            println!("Checkpoint {}: {} PTBs", checkpoint, ptbs.len());
            checkpoints_data.push((checkpoint, ptbs));
        }
    } else {
        for checkpoint in start..end {
            if ptb_target == 0 {
                break;
            }

            let checkpoint_fetch_start = Instant::now();
            // Prefer BCS -> local JSON; still single-checkpoint path.
            let checkpoint_json = walrus.get_checkpoint_json(checkpoint)?;
            let checkpoint_fetch_s = checkpoint_fetch_start.elapsed().as_secs_f64();

            let transactions = checkpoint_json
                .get("transactions")
                .and_then(|t| t.as_array())
                .ok_or_else(|| anyhow!("Walrus checkpoint missing transactions array"))?;

            let mut ptbs: Vec<serde_json::Value> = Vec::new();
            for tx_json in transactions {
                let is_ptb = tx_json
                    .pointer(
                        "/transaction/data/0/intent_message/value/V1/kind/ProgrammableTransaction",
                    )
                    .is_some();
                if !is_ptb {
                    continue;
                }
                if ptb_target == 0 {
                    break;
                }
                ptbs.push(tx_json.clone());
                ptb_target = ptb_target.saturating_sub(1);
            }

            println!(
                "Checkpoint {}: {} PTBs (fetch: {:.2}s)",
                checkpoint,
                ptbs.len(),
                checkpoint_fetch_s
            );

            checkpoints_data.push((checkpoint, ptbs));
        }
    }

    let all_tx_refs: Vec<&serde_json::Value> = checkpoints_data
        .iter()
        .flat_map(|(_, txs)| txs.iter())
        .collect();

    let batch_prefetch = engine.pre_scan_batch(&all_tx_refs, None, args.verbose_failures);
    if batch_prefetch.prefetched_objects > 0 {
        println!(
            "Prefetch (range): {} txs, {} objects (gRPC)",
            batch_prefetch.txs_prefetched, batch_prefetch.prefetched_objects
        );
        println!();
    }

    for (checkpoint, transactions) in checkpoints_data {
        if transactions.is_empty() {
            continue;
        }
        println!(
            "Replay checkpoint {}: {} PTBs",
            checkpoint,
            transactions.len()
        );

        for tx_json in transactions {
            stats.ptbs_seen += 1;

            // Ingest Walrus input/output objects into the run-wide cache.
            if let Err(e) = engine.ingest_tx_objects(&tx_json) {
                if args.verbose_failures {
                    eprintln!("  cache ingest warning: {e:#}");
                }
            }

            let digest = tx_json
                .pointer("/effects/V2/transaction_digest")
                .and_then(|v| v.as_str());
            let prefetch_versions = digest.and_then(|d| batch_prefetch.tx_versions.get(d));

            let outcome = engine.replay_one_ptb_best_effort_with_prefetch(
                &mut env,
                checkpoint,
                &tx_json,
                args.max_attempts,
                args.verbose_failures,
                prefetch_versions,
            );

            // Skip unsupported/parse errors.
            if matches!(
                outcome.final_reason,
                ReasonCode::ParseError | ReasonCode::UnsupportedCommand
            ) {
                stats.skipped += 1;
                stats.record(outcome.final_reason);
                if args.verbose_failures {
                    eprintln!(
                        "  PTB skipped (checkpoint {}): {} {:?}",
                        outcome.checkpoint, outcome.digest, outcome.final_reason
                    );
                    if let Some(attempt) = outcome.attempts.first() {
                        for note in &attempt.notes {
                            eprintln!("    {}", note);
                        }
                    }
                }
                continue;
            }

            stats.ptbs_executed += 1;
            stats.record(outcome.final_reason);
            if outcome.final_parity {
                stats.strict_matches += 1;
                stats.strict_match_digests.push(outcome.digest.clone());
                if let Some(summary) = engine.summarize_ptb_commands(&tx_json) {
                    stats
                        .strict_match_summaries
                        .push((outcome.digest.clone(), summary));
                }
            } else {
                stats.non_parity += 1;
                if args.verbose_failures {
                    eprintln!(
                        "  PTB mismatch (checkpoint {}): {} {:?}",
                        outcome.checkpoint, outcome.digest, outcome.final_reason
                    );
                    for attempt in &outcome.attempts {
                        eprintln!(
                            "    - {:?} success={} parity={} reason={:?} ({:.2}ms)",
                            attempt.kind,
                            attempt.success,
                            attempt.parity,
                            attempt.reason,
                            attempt.duration.as_secs_f64() * 1000.0
                        );
                        for note in &attempt.notes {
                            eprintln!("      {}", note);
                        }
                    }
                }
            }
        }
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Summary");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("PTBs seen:      {}", stats.ptbs_seen);
    println!("PTBs executed:  {}", stats.ptbs_executed);
    println!("PTBs strict match: {}", stats.strict_matches);
    println!("PTBs non-parity: {}", stats.non_parity);
    println!("PTBs skipped:   {}", stats.skipped);
    if stats.ptbs_executed > 0 {
        let pct = (stats.strict_matches as f64 / stats.ptbs_executed as f64) * 100.0;
        println!("Strict parity rate: {:.2}%", pct);
    }
    if !stats.strict_match_digests.is_empty() {
        println!(
            "Strict match digests ({}):",
            stats.strict_match_digests.len()
        );
        for digest in &stats.strict_match_digests {
            println!("  - {}", digest);
        }
    }
    if !stats.strict_match_summaries.is_empty() {
        println!("Strict match summaries:");
        for (digest, summary) in &stats.strict_match_summaries {
            println!("  - {}: {}", digest, summary);
        }
    }
    if !stats.reason_counts.is_empty() {
        println!("Outcome reasons:");
        for (reason, count) in &stats.reason_counts {
            if *count == 0 {
                continue;
            }
            println!("  - {:?}: {}", reason, count);
        }
    }
    println!("Elapsed: {:.2}s", elapsed);

    // Print cache metrics if disk cache was enabled
    if args.cache_dir.is_some() {
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Cache Metrics");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        let metrics_snapshot = engine.metrics.snapshot();
        println!("{}", metrics_snapshot.format_report());
    }

    Ok(())
}
