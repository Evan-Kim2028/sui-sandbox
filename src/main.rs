use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use sui_sdk::SuiClientBuilder;

use sui_move_interface_extractor::args::{Args, Command};
use sui_move_interface_extractor::benchmark::ptb_eval::run_ptb_eval;
use sui_move_interface_extractor::benchmark::runner::run_benchmark;
use sui_move_interface_extractor::benchmark::sandbox_exec::run_sandbox_exec;
use sui_move_interface_extractor::corpus::{collect_package_ids, run_corpus};
use sui_move_interface_extractor::runner::{run_batch, run_single, run_single_local_bytecode_dir};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Validate global arguments
    args.validate().map_err(|e| anyhow!(e))?;

    // Handle subcommands
    if let Some(ref command) = args.command {
        match command {
            Command::BenchmarkLocal(bench_args) => {
                bench_args.validate().map_err(|e| anyhow!(e))?;
                return run_benchmark(bench_args);
            }
            Command::TxReplay(replay_args) => {
                replay_args.validate().map_err(|e| anyhow!(e))?;
                return run_tx_replay(replay_args);
            }
            Command::PtbEval(eval_args) => {
                eval_args.validate().map_err(|e| anyhow!(e))?;
                return run_ptb_eval(eval_args);
            }
            Command::SandboxExec(exec_args) => {
                exec_args.validate().map_err(|e| anyhow!(e))?;
                return run_sandbox_exec(exec_args);
            }
        }
    }

    // Default mode: interface extraction
    if args.bytecode_package_dir.is_some() {
        return run_single_local_bytecode_dir(&args).await;
    }

    if args.bytecode_corpus_root.is_some() {
        run_corpus(&args, Arc::new(build_sui_client(&args).await?)).await?;
        return Ok(());
    }

    let package_ids = collect_package_ids(&args)?;
    if package_ids.is_empty() {
        return Err(anyhow!(
            "no ids provided (use --package-id, --package-ids-file, or --mvr-catalog)"
        ));
    }

    let client = Arc::new(build_sui_client(&args).await?);

    let is_batch = args.out_dir.is_some()
        || args.package_ids_file.is_some()
        || args.mvr_catalog.is_some()
        || package_ids.len() > 1;

    if is_batch {
        run_batch(&args, client, package_ids).await?;
        return Ok(());
    }

    let package_id = package_ids.first().expect("non-empty ids");
    run_single(&args, client, package_id).await
}

/// Build a SuiClient with timeout configuration from CLI args.
async fn build_sui_client(args: &Args) -> Result<sui_sdk::SuiClient> {
    SuiClientBuilder::default()
        .request_timeout(Duration::from_secs(args.rpc_timeout_secs))
        .build(&args.rpc_url)
        .await
        .map_err(|e| anyhow!("Failed to build Sui client: {}", e))
}

/// Run transaction replay mode.
///
/// Note: Download functionality has been removed in v0.6.0. Transactions must be
/// pre-cached in the cache directory. Use the DataFetcher API to download transactions.
fn run_tx_replay(args: &sui_move_interface_extractor::args::TxReplayArgs) -> Result<()> {
    use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
    use sui_move_interface_extractor::benchmark::tx_replay::{
        replay_parallel, CachedTransaction, TransactionCache,
    };

    // Download-only mode is no longer supported via CLI
    if args.download_only {
        return Err(anyhow!(
            "Download mode has been removed in v0.6.0. Use the DataFetcher API to download transactions."
        ));
    }

    // Handle cache operations
    let cache_dir = args.cache_dir.as_ref().ok_or_else(|| {
        anyhow!("Cache directory (--cache-dir) is required. Transactions must be pre-cached.")
    })?;

    let cache = TransactionCache::new(cache_dir)?;

    // Clear cache if requested
    if args.clear_cache {
        let cleared = cache.clear()?;
        eprintln!("Cleared {} cached transactions", cleared);
        return Ok(());
    }

    // Load transactions from cache
    eprintln!("Loading transactions from cache...");
    let digests = cache.list()?;

    let mut cached_txs: Vec<CachedTransaction> = Vec::new();
    for digest in &digests {
        match cache.load(digest) {
            Ok(cached) => {
                // Filter to framework-only if requested
                if args.framework_only && !cached.transaction.uses_only_framework() {
                    continue;
                }
                cached_txs.push(cached);
            }
            Err(e) => {
                if args.verbose {
                    eprintln!("Warning: Failed to load {}: {}", digest, e);
                }
            }
        }
    }

    eprintln!("Loaded {} transactions from cache", cached_txs.len());

    if cached_txs.is_empty() {
        return Err(anyhow!(
            "No transactions in cache. Pre-cache transactions using the DataFetcher API."
        ));
    }

    // Parallel replay mode
    if args.parallel {
        // Initialize resolver
        let resolver = LocalModuleResolver::with_sui_framework()?;
        eprintln!("Loaded {} framework modules", resolver.module_count());

        // Run parallel replay
        eprintln!(
            "Running parallel replay with {} threads...",
            args.threads.unwrap_or_else(rayon::current_num_threads)
        );

        let result = replay_parallel(&cached_txs, &resolver, args.threads)?;

        // Print results
        println!("\n========================================");
        println!("PARALLEL REPLAY RESULTS");
        println!("========================================");
        println!("Total transactions: {}", result.total);
        println!(
            "Successful: {} ({:.1}%)",
            result.successful,
            100.0 * result.successful as f64 / result.total as f64
        );
        println!(
            "Status match: {} ({:.1}%)",
            result.status_matched,
            100.0 * result.status_matched as f64 / result.total as f64
        );
        println!("Time: {} ms ({:.1} tx/s)", result.elapsed_ms, result.tps);

        return Ok(());
    }

    // Sequential replay from cache
    run_tx_replay_with_cached_transactions(args, cached_txs)
}

/// Run transaction replay with cached transactions (includes object data).
fn run_tx_replay_with_cached_transactions(
    args: &sui_move_interface_extractor::args::TxReplayArgs,
    cached_txs: Vec<sui_move_interface_extractor::benchmark::tx_replay::CachedTransaction>,
) -> Result<()> {
    use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
    use sui_move_interface_extractor::benchmark::vm::VMHarness;

    eprintln!("Processing {} transactions...\n", cached_txs.len());

    // Initialize module resolver for replay
    let mut resolver = LocalModuleResolver::with_sui_framework()?;
    eprintln!("Loaded {} framework modules", resolver.module_count());

    // Track replay statistics
    let mut total_replayed = 0;
    let mut total_success = 0;
    let mut total_match = 0;

    // Process transactions
    for cached in &cached_txs {
        let tx = &cached.transaction;

        // Print summary
        println!("{}", tx.summary());

        if args.verbose {
            println!("  Commands: {}", tx.commands.len());
            println!("  Inputs: {}", tx.inputs.len());

            // Show packages used
            let third_party = tx.third_party_packages();
            if third_party.is_empty() {
                println!("  Packages: framework only");
            } else {
                println!("  Packages: framework + {} third-party", third_party.len());
                for pkg in &third_party {
                    println!("    - {}", pkg);
                }
            }

            if let Some(effects) = &tx.effects {
                println!("  On-chain effects:");
                println!("    Created: {}", effects.created.len());
                println!("    Mutated: {}", effects.mutated.len());
                println!("    Deleted: {}", effects.deleted.len());
                println!("    Gas: {} computation", effects.gas_used.computation_cost);
            }
        }

        // Fetch object data from cache
        if args.fetch_objects || args.replay {
            eprintln!("  Fetching input objects...");
            if args.verbose {
                println!("  Fetched {} input objects", cached.objects.len());
            }
        }

        // Execute full replay if requested
        if args.replay {
            // Load cached packages
            if !cached.packages.is_empty() {
                for pkg_id in cached.packages.keys() {
                    if let Some(modules) = cached.get_package_modules(pkg_id) {
                        let non_empty: Vec<(String, Vec<u8>)> =
                            modules.into_iter().filter(|(_, b)| !b.is_empty()).collect();
                        if !non_empty.is_empty() {
                            if args.verbose {
                                eprintln!(
                                    "  Loading package {} ({} modules)...",
                                    pkg_id,
                                    non_empty.len()
                                );
                            }
                            match resolver.add_package_modules(non_empty) {
                                Ok((count, _)) if args.verbose => {
                                    eprintln!("    Loaded {} modules", count)
                                }
                                Err(e) if args.verbose => {
                                    eprintln!("    Warning: Failed to load: {}", e)
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            // Create harness and execute replay with cached objects
            eprintln!("  Executing local replay...");
            total_replayed += 1;

            match VMHarness::new(&resolver, false) {
                Ok(mut harness) => match tx.replay_with_objects(&mut harness, &cached.objects) {
                    Ok(result) => {
                        if result.local_success {
                            total_success += 1;
                            println!("  LOCAL RESULT: SUCCESS");
                        } else {
                            println!("  LOCAL RESULT: FAILURE");
                            if let Some(err) = &result.local_error {
                                println!("    Error: {}", err);
                            }
                        }

                        if let Some(cmp) = &result.comparison {
                            println!("  COMPARISON:");
                            println!("    Match score: {:.0}%", cmp.match_score * 100.0);
                            println!(
                                "    Status match: {}",
                                if cmp.status_match { "YES" } else { "NO" }
                            );
                            println!(
                                "    Created match: {}",
                                if cmp.created_count_match { "YES" } else { "NO" }
                            );
                            println!(
                                "    Mutated match: {}",
                                if cmp.mutated_count_match { "YES" } else { "NO" }
                            );
                            println!(
                                "    Deleted match: {}",
                                if cmp.deleted_count_match { "YES" } else { "NO" }
                            );

                            if cmp.status_match {
                                total_match += 1;
                            }

                            for note in &cmp.notes {
                                println!("    Note: {}", note);
                            }
                        }
                    }
                    Err(e) => {
                        println!("  REPLAY ERROR: {}", e);
                    }
                },
                Err(e) => {
                    eprintln!("  Failed to create VM harness: {}", e);
                }
            }
        }

        println!();
    }

    // Print summary statistics
    if args.replay && total_replayed > 0 {
        println!("========================================");
        println!("REPLAY SUMMARY");
        println!("========================================");
        println!("Total transactions: {}", cached_txs.len());
        println!("Replayed: {}", total_replayed);
        println!(
            "Successful: {} ({:.1}%)",
            total_success,
            100.0 * total_success as f64 / total_replayed as f64
        );
        println!(
            "Status match: {} ({:.1}%)",
            total_match,
            100.0 * total_match as f64 / total_replayed as f64
        );
    }

    Ok(())
}
