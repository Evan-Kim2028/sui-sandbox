use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use sui_sdk::SuiClientBuilder;

use sui_move_interface_extractor::args::{Args, Command};
use sui_move_interface_extractor::benchmark::runner::run_benchmark;
use sui_move_interface_extractor::corpus::{collect_package_ids, run_corpus};
use sui_move_interface_extractor::runner::{run_batch, run_single, run_single_local_bytecode_dir};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(Command::BenchmarkLocal(bench_args)) = &args.command {
        return run_benchmark(bench_args);
    }

    if args.corpus_local_bytes_check && args.bytecode_corpus_root.is_none() {
        return Err(anyhow!(
            "--corpus-local-bytes-check requires --bytecode-corpus-root"
        ));
    }

    if args.emit_submission_summary.is_some() && args.bytecode_corpus_root.is_none() {
        return Err(anyhow!(
            "--emit-submission-summary is only valid in corpus mode (requires --bytecode-corpus-root)"
        ));
    }

    if args.bytecode_package_dir.is_some() {
        if args.out_dir.is_some() || args.bytecode_corpus_root.is_some() {
            return Err(anyhow!(
                "--bytecode-package-dir is single-package mode; do not use with --out-dir/--bytecode-corpus-root"
            ));
        }
        return run_single_local_bytecode_dir(&args).await;
    }

    if args.bytecode_corpus_root.is_some() {
        run_corpus(
            &args,
            Arc::new(SuiClientBuilder::default().build(&args.rpc_url).await?),
        )
        .await?;
        return Ok(());
    }

    let package_ids = collect_package_ids(&args)?;
    if package_ids.is_empty() {
        return Err(anyhow!(
            "no ids provided (use --package-id, --package-ids-file, or --mvr-catalog)"
        ));
    }

    let client = Arc::new(SuiClientBuilder::default().build(&args.rpc_url).await?);

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
