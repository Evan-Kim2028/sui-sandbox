use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::sync::Arc;

use super::batch::{
    categorize_error, print_batch_summary, BatchFailure, BatchReplaySummary, BatchSuccess,
    DigestFilter,
};
use super::deps::apply_output_objects_to_cache;
use super::{
    ReplayCmd, ReplayHydrationArgs, ReplayOutput, SandboxState, SharedObjCache, SharedPkgCache,
    WalrusReplayData,
};
use sui_state_fetcher::package_data_from_move_package;

/// Efficient batch replay: fetches all checkpoints in one batched call,
/// pre-populates shared object/package caches, classifies transactions,
/// and prints a summary report.
#[cfg(feature = "walrus")]
pub(super) async fn execute_walrus_batch_v2(
    cmd: &ReplayCmd,
    state: &SandboxState,
    verbose: bool,
    checkpoints: &[u64],
    replay_progress: bool,
) -> Result<ReplayOutput> {
    use sui_sandbox::ptb_classifier::classify_checkpoint_tx;
    use sui_transport::walrus::WalrusClient;

    let digest_filter = DigestFilter::parse(&cmd.digest);

    if replay_progress || verbose {
        eprintln!(
            "[walrus-batch-v2] fetching {} checkpoint(s)...",
            checkpoints.len()
        );
    }
    let all_checkpoints = tokio::task::spawn_blocking({
        let cps = checkpoints.to_vec();
        move || {
            let walrus = WalrusClient::mainnet();
            walrus.get_checkpoints_batched(&cps, 10 * 1024 * 1024)
        }
    })
    .await
    .context("Walrus batch fetch task panicked")?
    .context("Failed to fetch checkpoints from Walrus")?;

    if replay_progress || verbose {
        let total_txs: usize = all_checkpoints
            .iter()
            .map(|(_, cp)| cp.transactions.len())
            .sum();
        eprintln!(
            "[walrus-batch-v2] fetched {} checkpoints, {} total transactions",
            all_checkpoints.len(),
            total_txs
        );
    }

    // Pre-populate shared caches from ALL checkpoints.
    // IMPORTANT: Only load input_objects (pre-execution state) for Move objects.
    // Output objects are applied incrementally during replay so that Tx_i sees
    // the post-execution state of Tx_0..Tx_{i-1} (intra-checkpoint progression).
    // Packages are loaded from both input and output (they're immutable).
    let walrus_obj_cache: SharedObjCache = Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let walrus_pkg_cache: SharedPkgCache = Arc::new(parking_lot::Mutex::new(HashMap::new()));
    {
        let mut obj_count = 0usize;
        let mut pkg_count = 0usize;
        for (_cp_num, cp_data) in &all_checkpoints {
            for tx in &cp_data.transactions {
                // Load Move objects only from input_objects (pre-execution state)
                for obj in &tx.input_objects {
                    match &obj.data {
                        sui_types::object::Data::Package(pkg) => {
                            let pkg_data = package_data_from_move_package(pkg);
                            walrus_pkg_cache
                                .lock()
                                .entry(pkg_data.address)
                                .or_insert_with(|| {
                                    pkg_count += 1;
                                    pkg_data
                                });
                        }
                        _ => {
                            let oid = format!("0x{}", hex::encode(obj.id().into_bytes()));
                            if let Some((ts, bcs, ver, _shared)) =
                                sui_transport::walrus::extract_object_bcs(obj)
                            {
                                walrus_obj_cache.lock().insert(oid, (ts, bcs, ver));
                                obj_count += 1;
                            }
                        }
                    }
                }
                // Load packages from output_objects too (packages are immutable)
                for obj in &tx.output_objects {
                    if let sui_types::object::Data::Package(pkg) = &obj.data {
                        let pkg_data = package_data_from_move_package(pkg);
                        walrus_pkg_cache
                            .lock()
                            .entry(pkg_data.address)
                            .or_insert_with(|| {
                                pkg_count += 1;
                                pkg_data
                            });
                    }
                }
            }
        }
        if replay_progress || verbose {
            eprintln!(
                "[walrus-batch-v2] pre-populated {} objects, {} packages from checkpoint data",
                obj_count, pkg_count
            );
        }
    }

    // Classify and replay each transaction
    let mut summary = BatchReplaySummary {
        total_checkpoints: all_checkpoints.len(),
        total_transactions: 0,
        total_ptbs: 0,
        replayed: 0,
        succeeded: 0,
        failed: 0,
        skipped_non_ptb: 0,
        by_tag: HashMap::new(),
        failures: Vec::new(),
        by_package: HashMap::new(),
        by_error_category: HashMap::new(),
        successes: Vec::new(),
    };

    let mut last_output: Option<ReplayOutput> = None;

    for (cp_num, cp_data) in &all_checkpoints {
        // Intra-checkpoint state progression: track which transactions'
        // output_objects have been applied to the cache. Before replaying
        // Tx_i, we apply outputs from Tx_0..Tx_{i-1} so Tx_i sees
        // post-execution state of earlier transactions in the same checkpoint.
        let mut intra_cp_applied = 0usize;

        for (tx_idx, tx) in cp_data.transactions.iter().enumerate() {
            summary.total_transactions += 1;
            let tx_digest = tx.transaction.digest().to_string();

            // Apply output_objects from all preceding transactions that
            // haven't been applied yet (even for skipped/filtered txs,
            // since subsequent transactions depend on that state).
            for preceding in intra_cp_applied..tx_idx {
                apply_output_objects_to_cache(
                    &walrus_obj_cache,
                    &walrus_pkg_cache,
                    &cp_data.transactions[preceding],
                );
            }
            intra_cp_applied = tx_idx;

            if !digest_filter.matches(&tx_digest) {
                continue;
            }

            let classification = match classify_checkpoint_tx(tx, *cp_num) {
                Some(c) => c,
                None => {
                    summary.skipped_non_ptb += 1;
                    continue;
                }
            };
            summary.total_ptbs += 1;

            if replay_progress {
                let tags_str = classification.tags.join(",");
                eprintln!(
                    "[walrus-batch-v2] replaying {} (cp {}) [{}]",
                    tx_digest, cp_num, tags_str
                );
            }

            let single = ReplayCmd {
                digest: tx_digest.clone(),
                hydration: ReplayHydrationArgs {
                    source: cmd.hydration.source,
                    allow_fallback: cmd.hydration.allow_fallback,
                    prefetch_depth: cmd.hydration.prefetch_depth,
                    prefetch_limit: cmd.hydration.prefetch_limit,
                    no_prefetch: cmd.hydration.no_prefetch,
                    auto_system_objects: cmd.hydration.auto_system_objects,
                },
                vm_only: cmd.vm_only,
                strict: false, // don't fail-fast in batch mode
                compare: cmd.compare,
                verbose: cmd.verbose,
                fetch_strategy: cmd.fetch_strategy,
                reconcile_dynamic_fields: cmd.reconcile_dynamic_fields,
                synthesize_missing: cmd.synthesize_missing,
                self_heal_dynamic_fields: cmd.self_heal_dynamic_fields,
                #[cfg(feature = "igloo")]
                igloo: cmd.igloo.clone(),
                grpc_timeout_secs: cmd.grpc_timeout_secs,
                checkpoint: Some(cp_num.to_string()),
                state_json: None,
                export_state: None,
                latest: None,
            };

            let output = single
                .execute_walrus_with_data(
                    state,
                    verbose,
                    *cp_num,
                    false,
                    WalrusReplayData {
                        preloaded_checkpoint: Some(cp_data),
                        shared_obj_cache: Some(Arc::clone(&walrus_obj_cache)),
                        shared_pkg_cache: Some(Arc::clone(&walrus_pkg_cache)),
                    },
                )
                .await;

            summary.replayed += 1;
            let success = match &output {
                Ok(o) => o.local_success,
                Err(_) => false,
            };

            for tag in &classification.tags {
                let entry = summary
                    .by_tag
                    .entry(tag.clone())
                    .or_insert((0usize, 0usize, 0usize));
                entry.0 += 1;
                if success {
                    entry.1 += 1;
                } else {
                    entry.2 += 1;
                }
            }

            let tx_packages = &classification.non_system_packages;
            for pkg in tx_packages {
                let entry = summary.by_package.entry(pkg.clone()).or_insert((0, 0, 0));
                entry.0 += 1;
                if success {
                    entry.1 += 1;
                } else {
                    entry.2 += 1;
                }
            }

            if success {
                summary.succeeded += 1;
                if replay_progress {
                    eprintln!("[walrus-batch-v2] {} -> success", tx_digest);
                }
                summary.successes.push(BatchSuccess {
                    digest: tx_digest.clone(),
                    checkpoint: *cp_num,
                    tags: classification.tags.clone(),
                    packages: tx_packages.clone(),
                });
            } else {
                summary.failed += 1;
                let error_msg = match &output {
                    Ok(o) => o
                        .local_error
                        .clone()
                        .unwrap_or_else(|| "unknown error".to_string()),
                    Err(e) => e.to_string(),
                };
                let category = categorize_error(&error_msg);
                *summary
                    .by_error_category
                    .entry(category.clone())
                    .or_insert(0) += 1;
                if replay_progress {
                    eprintln!("[walrus-batch-v2] {} -> FAILED: {}", tx_digest, error_msg);
                }
                summary.failures.push(BatchFailure {
                    digest: tx_digest.clone(),
                    checkpoint: *cp_num,
                    error: error_msg,
                    error_category: category,
                    tags: classification.tags.clone(),
                    packages: tx_packages.clone(),
                });
            }

            apply_output_objects_to_cache(&walrus_obj_cache, &walrus_pkg_cache, tx);
            intra_cp_applied = tx_idx + 1;

            if let Ok(o) = output {
                last_output = Some(o);
            }
        }
    }

    print_batch_summary(&summary);

    let mut out = last_output
        .ok_or_else(|| anyhow!("No PTB transactions found in the specified checkpoints"))?;
    out.batch_summary_printed = true;
    Ok(out)
}
