//! Batch PTB Replay Pipeline - Static Checkpoint Range
//!
//! This example fetches transactions from a fixed range of checkpoints and replays them
//! locally to validate the replay infrastructure. By using a static checkpoint range,
//! results are reproducible and we can aim for 100% parity.
//!
//! ## Required Setup
//!
//! Configure gRPC endpoint in your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on your provider
//! ```
//!
//! ## Usage
//!
//! ```bash
//! # First run: fetch and cache data
//! cargo run --example batch_ptb --release -- --fetch
//!
//! # Subsequent runs: use cache for fast iteration
//! cargo run --example batch_ptb --release
//!
//! # Compare ground-truth vs legacy prefetch strategies
//! cargo run --example batch_ptb --release -- --compare
//!
//! # Use legacy GraphQL-first prefetch strategy (comparison/regression only)
//! cargo run --example batch_ptb --release -- --legacy
//! ```

mod cache;
mod pipeline;

use anyhow::Result;
use std::time::Instant;

use pipeline::{BatchPipeline, BatchStats, PrefetchStrategy};

// =============================================================================
// STATIC CHECKPOINT RANGE
// =============================================================================
// These checkpoints are hardcoded for reproducible testing.
// To update: run with --discover flag to find current checkpoint range.

/// Starting checkpoint (inclusive)
/// Note: Check x-sui-lowest-available-checkpoint header for available range
const START_CHECKPOINT: u64 = 237_600_000;

/// Number of checkpoints to process (237600000 - 237600004)
const NUM_CHECKPOINTS: u64 = 5;

fn main() -> Result<()> {
    // Load environment from .env file
    dotenv::dotenv().ok();

    let args: Vec<String> = std::env::args().collect();
    let fetch_mode = args.iter().any(|a| a == "--fetch" || a == "-f");
    let quiet_mode = args.iter().any(|a| a == "--quiet" || a == "-q");
    let compare_mode = args.iter().any(|a| a == "--compare" || a == "-c");
    let legacy_mode = args.iter().any(|a| a == "--legacy" || a == "-l");
    let mm2_mode = args.iter().any(|a| a == "--mm2" || a == "-m");

    // Determine prefetch strategy
    let strategy = if legacy_mode {
        PrefetchStrategy::LegacyGraphQL
    } else if mm2_mode {
        PrefetchStrategy::MM2Predictive
    } else {
        PrefetchStrategy::GroundTruth
    };

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║              Batch PTB Replay - Static Checkpoint Range              ║");
    println!("║                                                                      ║");
    println!(
        "║  Fetches transactions from checkpoints {} - {}     ║",
        START_CHECKPOINT,
        START_CHECKPOINT + NUM_CHECKPOINTS - 1
    );
    println!("║  Goal: 100% local/on-chain parity                                    ║");
    if compare_mode {
        println!("║  Mode: COMPARISON (ground-truth vs legacy prefetch)                 ║");
    } else if fetch_mode {
        println!("║  Mode: FETCH (will cache data to disk)                              ║");
    } else {
        println!("║  Mode: REPLAY (using cached data if available)                      ║");
    }
    match strategy {
        PrefetchStrategy::GroundTruth => {
            println!("║  Prefetch: GROUND-TRUTH-FIRST (recommended)                        ║");
        }
        PrefetchStrategy::MM2Predictive => {
            println!("║  Prefetch: MM2 PREDICTIVE (ground-truth + bytecode analysis)       ║");
        }
        PrefetchStrategy::LegacyGraphQL => {
            println!("║  Prefetch: LEGACY GRAPHQL-FIRST                                    ║");
        }
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let total_start = Instant::now();

    // Create and run the pipeline
    let rt = tokio::runtime::Runtime::new()?;

    if compare_mode {
        // Run comparison mode
        let comparison = rt.block_on(async {
            BatchPipeline::run_comparison(START_CHECKPOINT, NUM_CHECKPOINTS).await
        })?;

        comparison.print_summary();

        // Exit with error code if ground-truth is worse than legacy
        if comparison.ground_truth_stats.match_rate() < comparison.legacy_stats.match_rate() {
            eprintln!("WARNING: Ground-truth strategy performed worse than legacy!");
            std::process::exit(1);
        }

        return Ok(());
    }

    // Run with specified strategy
    let stats = rt.block_on(async {
        BatchPipeline::run_with_strategy(
            START_CHECKPOINT,
            NUM_CHECKPOINTS,
            fetch_mode,
            quiet_mode,
            strategy,
        )
        .await
    })?;

    let total_elapsed = total_start.elapsed();

    // Print summary
    print_summary(&stats, total_elapsed);

    // Exit with error code if not 100% match
    if stats.match_rate() < 1.0 && stats.transactions_processed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn print_summary(stats: &BatchStats, total_elapsed: std::time::Duration) {
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         BATCH REPLAY SUMMARY                         ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");

    println!(
        "║  Checkpoints processed:   {:>6}                                     ║",
        stats.checkpoints_processed
    );
    println!(
        "║  Transactions fetched:    {:>6}                                     ║",
        stats.transactions_fetched
    );
    println!(
        "║  Transactions processed:  {:>6}                                     ║",
        stats.transactions_processed
    );
    println!(
        "║  Successful replays:      {:>6}                                     ║",
        stats.successful_replays
    );
    println!(
        "║  Failed replays:          {:>6}                                     ║",
        stats.failed_replays
    );
    println!(
        "║  Skipped (fetch errors):  {:>6}                                     ║",
        stats.skipped_fetch_errors
    );

    let match_pct = stats.match_rate() * 100.0;
    println!(
        "║  Match rate (local=onchain): {:>5.1}%                                  ║",
        match_pct
    );

    println!("╠══════════════════════════════════════════════════════════════════════╣");

    // Framework vs Complex breakdown
    let fw_pct = if stats.framework_total > 0 {
        (stats.framework_matches as f64 / stats.framework_total as f64) * 100.0
    } else {
        0.0
    };
    let cx_pct = if stats.complex_total > 0 {
        (stats.complex_matches as f64 / stats.complex_total as f64) * 100.0
    } else {
        0.0
    };
    println!(
        "║  Framework-only PTBs: {:>3}/{:<3} ({:>5.1}% match)                       ║",
        stats.framework_matches, stats.framework_total, fw_pct
    );
    println!(
        "║  Complex PTBs:        {:>3}/{:<3} ({:>5.1}% match)                       ║",
        stats.complex_matches, stats.complex_total, cx_pct
    );

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║  Total objects fetched:   {:>6}                                     ║",
        stats.total_objects_fetched
    );
    println!(
        "║  Total packages fetched:  {:>6}                                     ║",
        stats.total_packages_fetched
    );
    println!(
        "║  Dynamic fields resolved: {:>6}                                     ║",
        stats.dynamic_fields_resolved
    );

    // MM2 analysis stats (if any)
    if stats.mm2_predictions_made > 0 {
        println!("╠══════════════════════════════════════════════════════════════════════╣");
        println!(
            "║  MM2 packages analyzed:   {:>6}                                     ║",
            stats.mm2_packages_analyzed
        );
        println!(
            "║  MM2 packages fetched:    {:>6}                                     ║",
            stats.mm2_packages_fetched_for_analysis
        );
        println!(
            "║  MM2 predictions made:    {:>6}                                     ║",
            stats.mm2_predictions_made
        );
        println!(
            "║  MM2 predictions matched: {:>6}                                     ║",
            stats.mm2_predictions_matched
        );
        let mm2_precision = if stats.mm2_predictions_made > 0 {
            (stats.mm2_predictions_matched as f64 / stats.mm2_predictions_made as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "║  MM2 prediction accuracy: {:>5.1}%                                    ║",
            mm2_precision
        );
    }

    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║  Data fetch time:     {:>8.2}s                                     ║",
        stats.data_fetch_time.as_secs_f64()
    );
    println!(
        "║  Execution time:      {:>8.2}s                                     ║",
        stats.execution_time.as_secs_f64()
    );
    println!(
        "║  Total time:          {:>8.2}s                                     ║",
        total_elapsed.as_secs_f64()
    );

    if stats.transactions_processed > 0 {
        let avg_exec =
            stats.execution_time.as_millis() as f64 / stats.transactions_processed as f64;
        println!(
            "║  Avg execution/tx:    {:>8.2}ms                                    ║",
            avg_exec
        );
    }

    println!("╠══════════════════════════════════════════════════════════════════════╣");

    if stats.match_rate() >= 1.0 {
        println!("║  ✓ PERFECT: 100% of replays match on-chain results                 ║");
    } else if stats.match_rate() >= 0.9 {
        println!("║  ○ EXCELLENT: >90% of replays match on-chain results               ║");
    } else if stats.match_rate() >= 0.7 {
        println!("║  ○ GOOD: >70% of replays match on-chain results                    ║");
    } else {
        println!("║  ✗ NEEDS WORK: <70% of replays match on-chain results              ║");
    }

    println!("╚══════════════════════════════════════════════════════════════════════╝");

    // Print failure breakdown if any
    if !stats.failure_reasons.is_empty() {
        println!("\nFailure Breakdown:");
        let mut sorted_reasons: Vec<_> = stats.failure_reasons.iter().collect();
        sorted_reasons.sort_by(|a, b| b.1.cmp(a.1));

        for (reason, count) in sorted_reasons.iter().take(10) {
            // Show first 200 chars for better debugging
            let truncated = if reason.len() > 200 {
                format!("{}...", &reason[..197])
            } else {
                reason.to_string()
            };
            println!("  {:>4}x  {}", count, truncated);
        }
    }

    // Print mismatch details if any
    if !stats.mismatches.is_empty() {
        println!("\nMismatched Transactions (local != on-chain):");
        for (digest, local, onchain, error, is_framework) in stats.mismatches.iter().take(20) {
            let local_str = if *local { "SUCCESS" } else { "FAILURE" };
            let onchain_str = if *onchain { "SUCCESS" } else { "FAILURE" };
            let category = if *is_framework {
                "framework"
            } else {
                "complex"
            };
            println!(
                "  {} [{}] local={} onchain={}",
                &digest[..16],
                category,
                local_str,
                onchain_str
            );
            if let Some(err) = error {
                // Show first 500 chars for better debugging
                let truncated = if err.len() > 500 { &err[..497] } else { err };
                println!("    Error: {}", truncated);
            }
        }
    }
}
