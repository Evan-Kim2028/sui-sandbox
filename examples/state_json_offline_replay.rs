//! Offline replay from synthetic state JSON (Rust example).
//!
//! This example demonstrates replaying a transaction entirely from local JSON
//! state without network hydration.
//!
//! Run:
//!   cargo run --example state_json_offline_replay
//!   cargo run --example state_json_offline_replay -- --state-json examples/data/state_json_synthetic_ptb_demo.json
//!   cargo run --example state_json_offline_replay -- --digest synthetic_make_move_vec_demo

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;
use std::path::PathBuf;

use sui_sandbox_core::replay_support::{
    build_replay_object_maps, build_simulation_config, hydrate_resolver_from_replay_state,
    maybe_patch_replay_objects,
};
use sui_sandbox_core::tx_replay::{
    replay_with_version_tracking_with_policy_with_effects, EffectsReconcilePolicy,
};
use sui_sandbox_core::vm::VMHarness;
use sui_state_fetcher::{build_address_aliases, parse_replay_states_file, ReplayState};

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
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}

fn run(args: Args) -> Result<()> {
    let state_path = args.state_json.unwrap_or_else(default_state_json_path);
    let requested_digest = args
        .digest
        .as_deref()
        .unwrap_or(DEFAULT_DIGEST)
        .trim()
        .to_string();

    let states = parse_replay_states_file(&state_path).with_context(|| {
        format!(
            "failed to parse replay states from {}",
            state_path.to_string_lossy()
        )
    })?;
    let replay_state = select_replay_state(states, &requested_digest)?;

    let mut linkage_upgrades: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    for package in replay_state.packages.values() {
        for (original, upgraded) in &package.linkage {
            if original != upgraded {
                linkage_upgrades.insert(*original, *upgraded);
            }
        }
    }

    let aliases = build_address_aliases(&replay_state);
    let resolver = hydrate_resolver_from_replay_state(&replay_state, &linkage_upgrades, &aliases)?;

    let package_versions: HashMap<AccountAddress, u64> = replay_state
        .packages
        .iter()
        .map(|(id, package)| (*id, package.version))
        .collect();

    let mut object_maps = build_replay_object_maps(&replay_state, &package_versions);
    maybe_patch_replay_objects(
        &resolver,
        &replay_state,
        &package_versions,
        &aliases,
        &mut object_maps,
        false,
    );

    let config = build_simulation_config(&replay_state);
    let mut harness = VMHarness::with_config(&resolver, false, config)
        .context("failed to create VM harness for replay")?;

    let execution = replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &object_maps.cached_objects,
        &aliases,
        Some(&object_maps.version_map),
        EffectsReconcilePolicy::DynamicFields,
    )?;

    let result = execution.result;

    println!("=== Offline Replay from Synthetic State JSON (Rust) ===");
    println!("Digest:     {}", replay_state.transaction.digest.0);
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
    println!();
    println!(
        "Tip: edit {} to try custom PTB commands/inputs and rerun.",
        state_path.display()
    );

    Ok(())
}

fn select_replay_state(states: Vec<ReplayState>, digest: &str) -> Result<ReplayState> {
    if states.is_empty() {
        return Err(anyhow!("replay state file did not contain any states"));
    }
    if states.len() == 1 {
        let state = states
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("replay state file did not contain any states"))?;
        if !digest.is_empty() && state.transaction.digest.0 != digest {
            return Err(anyhow!(
                "digest '{}' does not match replay state digest '{}'",
                digest,
                state.transaction.digest.0
            ));
        }
        return Ok(state);
    }

    let selected = states
        .into_iter()
        .find(|state| state.transaction.digest.0 == digest)
        .ok_or_else(|| anyhow!("replay state file does not contain digest '{}'", digest))?;
    Ok(selected)
}

fn default_state_json_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/state_json_synthetic_ptb_demo.json")
}
