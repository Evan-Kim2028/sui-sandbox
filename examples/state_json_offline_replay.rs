//! Offline replay from synthetic state JSON (Rust example).
//!
//! Run:
//!   cargo run --example state_json_offline_replay
//!   cargo run --example state_json_offline_replay -- --state-json examples/data/state_json_synthetic_ptb_demo.json
//!   cargo run --example state_json_offline_replay -- --digest synthetic_make_move_vec_demo

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use sui_sandbox_core::replay_support::replay_state_json_offline;

const DEFAULT_DIGEST: &str = "synthetic_make_move_vec_demo";

#[derive(Parser, Debug)]
#[command(about = "Offline replay from a replay-state JSON fixture")]
struct Args {
    /// Replay state JSON file (single-state or multi-state)
    #[arg(long = "state-json")]
    state_json: Option<PathBuf>,

    /// Transaction digest selector (required when state file contains multiple states)
    #[arg(long)]
    digest: Option<String>,

    /// Verbose replay diagnostics
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let state_path = args.state_json.unwrap_or_else(default_state_json_path);
    let digest = args.digest.as_deref().or(Some(DEFAULT_DIGEST));

    let out = replay_state_json_offline(&state_path, digest, args.verbose)?;
    let result = out.execution.result;

    println!("=== Offline Replay from Synthetic State JSON (Rust) ===");
    println!("Digest:     {}", out.replay_state.transaction.digest.0);
    println!("State JSON: {}", state_path.display());
    println!();
    println!("Result Summary");
    println!("  digest:             {}", result.digest.0);
    println!("  local_success:      {}", result.local_success);
    println!("  commands_executed:  {}", result.commands_executed);
    println!("  commands_failed:    {}", result.commands_failed);
    println!("  gas_used:           {}", result.gas_used);
    if let Some(error) = result.local_error.as_deref() {
        println!("  local_error:        {}", error);
    }
    println!("  effective_source:   state_json");
    println!("  fallback_used:      false");

    Ok(())
}

fn default_state_json_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/state_json_synthetic_ptb_demo.json")
}
