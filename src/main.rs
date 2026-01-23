use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use sui_sdk::SuiClientBuilder;

use sui_move_interface_extractor::args::Args;
use sui_move_interface_extractor::corpus::{collect_package_ids, run_corpus};
use sui_move_interface_extractor::runner::{run_batch, run_single, run_single_local_bytecode_dir};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Validate global arguments
    args.validate().map_err(|e| anyhow!(e))?;

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
