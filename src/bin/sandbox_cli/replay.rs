//! Replay command - replay historical transactions locally

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::{Args, Subcommand, ValueEnum};
use move_binary_format::CompiledModule;
use serde::Serialize;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use self::hydration::{
    build_historical_state_provider, build_local_state_provider, build_replay_state,
    default_local_cache_dir, ReplayHydrationConfig,
};
use self::presentation::{build_replay_debug_json, enforce_strict, print_replay_result};
use super::network::resolve_graphql_endpoint;
use super::output::format_error;
use super::SandboxState;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use sui_prefetch::compute_dynamic_field_id;
use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};
use sui_sandbox_core::types::parse_type_tag;
use sui_sandbox_types::{PtbCommand, TransactionInput};
use sui_state_fetcher::{
    build_aliases as build_aliases_shared, checkpoint_to_replay_state,
    fetch_child_object as fetch_child_object_shared, find_tx_in_checkpoint, PackageData,
    VersionedObject,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{historical_endpoint_and_api_key_from_env, GrpcClient};
use sui_types::effects::TransactionEffectsAPI;

mod analysis;
mod batch;
mod deps;
mod dynamic_fields;
mod effects;
mod execute_state;
pub(crate) mod hydration;
mod mutate;
mod presentation;
mod support;
mod synthesis;
#[cfg(feature = "walrus")]
mod walrus_batch;
#[cfg(feature = "walrus")]
mod walrus_helpers;

use self::analysis::{build_analyze_replay_output, build_replay_diagnostics};
use self::deps::{
    fetch_dependency_closure, fetch_dependency_closure_walrus, fetch_package_via_walrus,
};
use self::dynamic_fields::{
    fetch_child_object_by_key, resolve_key_type_via_graphql, ChildFetchOptions, MissEntry,
};
use self::effects::{build_effects_summary, build_execution_path};
use self::execute_state::{execute_from_json, execute_replay_state};
use self::mutate::ReplayMutateCmd;
use self::support::{
    build_replay_object_maps, build_simulation_config, emit_linkage_debug_info,
    ensure_system_objects, hydrate_resolver_from_replay_state, maybe_patch_replay_objects,
};
use self::synthesis::synthesize_missing_inputs;
#[cfg(feature = "walrus")]
use self::walrus_helpers::{fetch_via_prev_tx, parse_checkpoint_spec};
use sui_sandbox_core::replay_reporting::ReplayDiagnostics;

#[derive(Args, Debug)]
pub struct ReplayCli {
    #[command(subcommand)]
    pub command: Option<ReplaySubcommand>,

    #[command(flatten)]
    pub replay: ReplayCmd,
}

#[derive(Subcommand, Debug)]
pub enum ReplaySubcommand {
    /// Mutate replay inputs/state and re-run with automatic hydration
    Mutate(ReplayMutateCmd),
}

impl ReplayCli {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        match &self.command {
            Some(ReplaySubcommand::Mutate(cmd)) => cmd.execute(state, json_output, verbose).await,
            None => self.replay.execute(state, json_output, verbose).await,
        }
    }
}

#[derive(Args, Debug)]
pub struct ReplayCmd {
    /// Transaction digest
    pub digest: Option<String>,

    /// Shared replay hydration controls (source/fallback/prefetch/system-object injection).
    #[command(flatten)]
    pub hydration: ReplayHydrationArgs,

    /// Runtime defaults profile (tunes fallback and transport behavior).
    #[arg(
        long,
        value_enum,
        default_value = "balanced",
        help_heading = "Hydration"
    )]
    pub profile: ReplayProfile,

    /// Disable fallback paths and force direct VM path execution only
    #[arg(long, default_value_t = false)]
    pub vm_only: bool,

    /// Fail with non-zero status if replay or comparison mismatches occur
    #[arg(long, default_value_t = false)]
    pub strict: bool,

    /// Compare local execution with on-chain effects
    #[arg(long)]
    pub compare: bool,

    /// Hydration-only mode (skip VM execution and return replay state summary)
    #[arg(long, alias = "hydrate-only", default_value_t = false)]
    pub analyze_only: bool,

    /// Show detailed execution trace
    #[arg(long, short)]
    pub verbose: bool,

    /// Fetch strategy for dynamic field children during replay
    #[arg(long, value_enum, default_value = "full")]
    pub fetch_strategy: FetchStrategy,

    /// Reconcile dynamic-field effects when on-chain lists omit them
    #[arg(long, default_value_t = true)]
    pub reconcile_dynamic_fields: bool,

    /// If replay fails due to missing input objects, synthesize placeholders and retry
    #[arg(long, default_value_t = false)]
    pub synthesize_missing: bool,

    /// Allow dynamic-field reads to synthesize placeholder values when data is missing
    #[arg(long, default_value_t = false)]
    pub self_heal_dynamic_fields: bool,

    /// Timeout in seconds for gRPC object fetches (default: 30)
    #[arg(long, default_value_t = 30)]
    pub grpc_timeout_secs: u64,

    /// Checkpoint(s) for Walrus-first replay (no gRPC/API key needed).
    /// Accepts: single (239615926), range (100..200), or list (100,105,110).
    #[arg(long)]
    pub checkpoint: Option<String>,

    /// Load replay state from a JSON file (custom data source, no network needed)
    #[arg(long)]
    pub state_json: Option<PathBuf>,

    /// Export fetched replay state as JSON before executing
    #[arg(long)]
    pub export_state: Option<PathBuf>,

    /// Replay the latest N checkpoints from Walrus (auto-discovers tip).
    /// Implies --source walrus and digest '*'.
    #[arg(long)]
    pub latest: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ReplayOutput {
    pub digest: String,
    pub local_success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<ReplayDiagnostics>,
    pub execution_path: ReplayExecutionPath,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ComparisonResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<ReplayEffectsSummary>,
    #[serde(skip)]
    pub effects_full: Option<sui_sandbox_core::ptb::TransactionEffects>,
    pub commands_executed: usize,
    /// When true, the batch summary was already printed; skip individual output.
    #[serde(skip)]
    pub batch_summary_printed: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct ReplayExecutionPath {
    pub requested_source: String,
    pub effective_source: String,
    pub vm_only: bool,
    pub allow_fallback: bool,
    pub auto_system_objects: bool,
    pub fallback_used: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fallback_reasons: Vec<String>,
    pub dynamic_field_prefetch: bool,
    pub prefetch_depth: usize,
    pub prefetch_limit: usize,
    pub dependency_fetch_mode: String,
    pub dependency_packages_fetched: usize,
    pub synthetic_inputs: usize,
}

#[derive(Debug, Serialize)]
pub struct ReplayEffectsSummary {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub gas_used: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub created: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub mutated: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub deleted: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub wrapped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unwrapped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub transferred: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub received: Vec<String>,
    pub events_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command_description: Option<String>,
    pub commands_succeeded: usize,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub return_values: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct ComparisonResult {
    pub status_match: bool,
    pub created_match: bool,
    pub mutated_match: bool,
    pub deleted_match: bool,
    pub on_chain_status: String,
    pub local_status: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub notes: Vec<String>,
}

/// Shared object cache for batch replay: id_hex → (type_str, bcs_bytes, version)
type SharedObjCache = Arc<parking_lot::Mutex<HashMap<String, (String, Vec<u8>, u64)>>>;
/// Shared package cache for batch replay: address → PackageData
type SharedPkgCache = Arc<parking_lot::Mutex<HashMap<AccountAddress, PackageData>>>;

#[cfg(feature = "walrus")]
pub(super) struct WalrusReplayData<'a> {
    pub preloaded_checkpoint: Option<&'a sui_types::full_checkpoint_content::CheckpointData>,
    pub shared_obj_cache: Option<SharedObjCache>,
    pub shared_pkg_cache: Option<SharedPkgCache>,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum FetchStrategy {
    Eager,
    Full,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ReplaySource {
    Grpc,
    Walrus,
    Hybrid,
    Local,
}

impl ReplaySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Grpc => "grpc",
            Self::Walrus => "walrus",
            Self::Hybrid => "hybrid",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ReplayProfile {
    Safe,
    Balanced,
    Fast,
}

impl ReplayProfile {
    fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Balanced => "balanced",
            Self::Fast => "fast",
        }
    }

    fn env_defaults(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Safe => &[
                ("SUI_CHECKPOINT_LOOKUP_GRAPHQL", "1"),
                ("SUI_PACKAGE_LOOKUP_GRAPHQL", "1"),
                ("SUI_OBJECT_FETCH_CONCURRENCY", "8"),
                ("SUI_PACKAGE_FETCH_CONCURRENCY", "4"),
                ("SUI_PACKAGE_FETCH_PARALLEL", "1"),
            ],
            Self::Balanced => &[],
            Self::Fast => &[
                ("SUI_CHECKPOINT_LOOKUP_GRAPHQL", "0"),
                ("SUI_PACKAGE_LOOKUP_GRAPHQL", "0"),
                ("SUI_OBJECT_FETCH_CONCURRENCY", "32"),
                ("SUI_PACKAGE_FETCH_CONCURRENCY", "16"),
                ("SUI_PACKAGE_FETCH_PARALLEL", "1"),
            ],
        }
    }
}

#[derive(Debug, Default, Clone)]
struct ReplayAutoDefaults {
    profile_env_applied: Vec<&'static str>,
    auto_ptb_progress: bool,
    auto_error_context: bool,
}

#[derive(Args, Debug, Clone)]
pub struct ReplayHydrationArgs {
    /// Data source for replay hydration
    #[arg(long, value_enum, default_value = "hybrid", help_heading = "Hydration")]
    pub source: ReplaySource,

    /// Local cache directory used by --source local
    #[arg(long, help_heading = "Hydration")]
    pub cache_dir: Option<PathBuf>,

    /// Allow fallback to secondary sources when data is missing
    #[arg(
        long = "allow-fallback",
        alias = "fallback",
        default_value_t = true,
        action = clap::ArgAction::Set,
        help_heading = "Hydration"
    )]
    pub allow_fallback: bool,

    /// Prefetch depth for dynamic fields (default: 3)
    #[arg(long, default_value_t = 3, help_heading = "Dynamic Field")]
    pub prefetch_depth: usize,

    /// Prefetch limit for dynamic fields (default: 200)
    #[arg(long, default_value_t = 200, help_heading = "Dynamic Field")]
    pub prefetch_limit: usize,

    /// Disable dynamic field prefetch
    #[arg(long, default_value_t = false, help_heading = "Dynamic Field")]
    pub no_prefetch: bool,

    /// Auto-inject system objects (Clock/Random) when missing
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help_heading = "Hydration"
    )]
    pub auto_system_objects: bool,
}

impl ReplayCmd {
    fn digest_display(&self) -> &str {
        self.digest.as_deref().unwrap_or("*")
    }

    fn digest_required(&self) -> Result<&str> {
        self.digest.as_deref().ok_or_else(|| {
            anyhow!(
                "missing transaction digest: provide <DIGEST> (or use --checkpoint with '*' / digest list, --latest, or --state-json)"
            )
        })
    }

    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let debug_json = env_bool_opt("SUI_SANDBOX_DEBUG_JSON").unwrap_or(false);
        let allow_fallback = self.hydration.allow_fallback && !self.vm_only;

        if self.analyze_only && self.compare {
            return Err(anyhow!("--analyze-only cannot be combined with --compare"));
        }
        if (self.synthesize_missing || self.self_heal_dynamic_fields) && !cfg!(feature = "mm2") {
            return Err(anyhow!(
                "dynamic field synthesis requires the `mm2` feature"
            ));
        }
        if matches!(
            self.hydration.source,
            ReplaySource::Walrus | ReplaySource::Hybrid
        ) && !cfg!(feature = "walrus")
        {
            return Err(anyhow!("Walrus source requires the `walrus` feature"));
        }
        let effective_verbose = verbose || self.verbose;
        std::env::remove_var("SUI_WALRUS_AUTO_ENABLED_RUN");
        std::env::remove_var("SUI_WALRUS_EFFECTIVE_RUN");
        let auto_defaults = self.apply_auto_runtime_defaults(json_output, effective_verbose);
        let result = self
            .execute_inner(state, effective_verbose, json_output, auto_defaults)
            .await;

        match result {
            Ok(output) => {
                // In batch mode the summary was already printed; skip individual output.
                if output.batch_summary_printed {
                    return Ok(());
                }
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    print_replay_result(&output, self.compare, verbose || self.verbose);
                }
                if self.strict {
                    enforce_strict(&output)?;
                }
                if output.local_success {
                    Ok(())
                } else {
                    if debug_json {
                        eprintln!(
                            "{}",
                            build_replay_debug_json(
                                "replay_execution_failed",
                                output.local_error.as_deref().unwrap_or("Replay failed"),
                                Some(&output),
                                allow_fallback,
                            )?
                        );
                    }
                    Err(anyhow!(output
                        .local_error
                        .unwrap_or_else(|| "Replay failed".to_string())))
                }
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                if debug_json {
                    eprintln!(
                        "{}",
                        build_replay_debug_json(
                            "replay_fetch_failed",
                            &e.to_string(),
                            None,
                            allow_fallback
                        )?
                    );
                }
                Err(e)
            }
        }
    }

    fn apply_auto_runtime_defaults(&self, json_output: bool, verbose: bool) -> ReplayAutoDefaults {
        let mut defaults = ReplayAutoDefaults::default();
        for (key, value) in self.profile.env_defaults() {
            if std::env::var(key).is_err() {
                std::env::set_var(key, value);
                defaults.profile_env_applied.push(*key);
            }
        }

        let auto_progress = auto_progress_enabled(json_output);
        if env_bool_opt("SUI_PTB_PROGRESS").is_none() && auto_progress {
            std::env::set_var("SUI_PTB_PROGRESS", "1");
            defaults.auto_ptb_progress = true;
        }
        if env_bool_opt("SUI_DEBUG_ERROR_CONTEXT").is_none() && (self.strict || verbose) {
            std::env::set_var("SUI_DEBUG_ERROR_CONTEXT", "1");
            defaults.auto_error_context = true;
        }
        defaults
    }

    fn print_effective_runtime_config(
        &self,
        json_output: bool,
        allow_fallback: bool,
        strict_df_checkpoint: bool,
        auto_defaults: &ReplayAutoDefaults,
    ) {
        if json_output {
            return;
        }

        let walrus_effective = env_bool_opt("SUI_WALRUS_EFFECTIVE_RUN").unwrap_or_else(|| {
            matches!(self.hydration.source, ReplaySource::Walrus)
                || matches!(self.hydration.source, ReplaySource::Hybrid)
                    && (self.latest.is_some() || self.checkpoint.is_some())
        });
        let walrus_auto = env_bool_opt("SUI_WALRUS_AUTO_ENABLED_RUN").unwrap_or(false);
        let replay_progress =
            env_bool_opt("SUI_REPLAY_PROGRESS").unwrap_or(auto_progress_enabled(json_output));
        let ptb_progress = env_bool_opt("SUI_PTB_PROGRESS").unwrap_or(false);
        let gql_checkpoint = env_bool_opt("SUI_CHECKPOINT_LOOKUP_GRAPHQL").unwrap_or(true);
        let gql_package = env_bool_opt("SUI_PACKAGE_LOOKUP_GRAPHQL").unwrap_or(true);
        let gql_cb = env_bool_opt("SUI_GRAPHQL_CIRCUIT_BREAKER").unwrap_or(true);

        let object_concurrency =
            std::env::var("SUI_OBJECT_FETCH_CONCURRENCY").unwrap_or_else(|_| "16".to_string());
        let package_concurrency =
            std::env::var("SUI_PACKAGE_FETCH_CONCURRENCY").unwrap_or_else(|_| "8".to_string());
        let package_parallel = env_bool_opt("SUI_PACKAGE_FETCH_PARALLEL").unwrap_or(true);

        let mut auto_notes: Vec<String> = Vec::new();
        if !auto_defaults.profile_env_applied.is_empty() {
            auto_notes.push(format!(
                "profile({})={}",
                self.profile.as_str(),
                auto_defaults.profile_env_applied.join("|")
            ));
        }
        if auto_defaults.auto_ptb_progress {
            auto_notes.push("ptb_progress=tty".to_string());
        }
        if auto_defaults.auto_error_context {
            auto_notes.push("error_context=verbose_or_strict".to_string());
        }
        if walrus_auto {
            auto_notes.push("walrus=source_default".to_string());
        }
        if auto_notes.is_empty() {
            auto_notes.push("none".to_string());
        }

        eprintln!(
            "[replay_config] profile={} source={} fallback={} walrus={} strict_df={} replay_progress={} ptb_progress={} gql_checkpoint={} gql_package={} gql_circuit={} obj_conc={} pkg_conc={} pkg_parallel={} auto={}",
            self.profile.as_str(),
            self.hydration.source.as_str(),
            allow_fallback,
            if walrus_effective {
                if walrus_auto { "on(auto)" } else { "on" }
            } else {
                "off"
            },
            strict_df_checkpoint,
            replay_progress,
            ptb_progress,
            gql_checkpoint,
            gql_package,
            gql_cb,
            object_concurrency,
            package_concurrency,
            package_parallel,
            auto_notes.join(","),
        );
    }

    async fn execute_inner(
        &self,
        state: &SandboxState,
        verbose: bool,
        json_output: bool,
        auto_defaults: ReplayAutoDefaults,
    ) -> Result<ReplayOutput> {
        let allow_fallback = self.hydration.allow_fallback && !self.vm_only;
        let replay_progress =
            env_bool_opt("SUI_REPLAY_PROGRESS").unwrap_or(auto_progress_enabled(json_output));
        let strict_df_checkpoint =
            env_bool_opt("SUI_DF_STRICT_CHECKPOINT").unwrap_or(self.strict || self.compare);
        if strict_df_checkpoint {
            std::env::set_var("SUI_DF_STRICT_CHECKPOINT", "1");
        }

        if verbose {
            eprintln!("Fetching transaction {}...", self.digest_display());
        }

        // JSON state path: --state-json provided, load from file (no network)
        if let Some(json_path) = &self.state_json {
            if replay_progress || verbose {
                self.print_effective_runtime_config(
                    json_output,
                    allow_fallback,
                    strict_df_checkpoint,
                    &auto_defaults,
                );
            }
            return execute_from_json(self, state, verbose, json_path, replay_progress).await;
        }

        if matches!(self.hydration.source, ReplaySource::Local)
            && (self.checkpoint.is_some() || self.latest.is_some())
        {
            return Err(anyhow!(
                "--source local does not support --checkpoint/--latest"
            ));
        }

        // --latest N: auto-discover tip checkpoint and replay the latest N checkpoints
        #[cfg(feature = "walrus")]
        if let Some(count) = self.latest {
            if self.analyze_only {
                return Err(anyhow!(
                    "--analyze-only supports single-digest replay; do not combine with --latest"
                ));
            }
            use sui_transport::walrus::WalrusClient;
            if count == 0 {
                return Err(anyhow!("--latest must be at least 1"));
            }
            if count > 100 {
                return Err(anyhow!("--latest max is 100 (got {})", count));
            }
            let tip =
                tokio::task::spawn_blocking(|| WalrusClient::mainnet().get_latest_checkpoint())
                    .await
                    .context("Walrus tip fetch task panicked")?
                    .context("Failed to get latest checkpoint from Walrus")?;
            let start = tip.saturating_sub(count - 1);
            let checkpoints: Vec<u64> = (start..=tip).collect();
            if replay_progress || verbose {
                eprintln!(
                    "[walrus] latest {} checkpoints: {}..{} (tip={})",
                    checkpoints.len(),
                    start,
                    tip,
                    tip
                );
                self.print_effective_runtime_config(
                    json_output,
                    allow_fallback,
                    strict_df_checkpoint,
                    &auto_defaults,
                );
            }
            return self
                .execute_walrus_batch_v2(state, verbose, &checkpoints, replay_progress)
                .await;
        }

        // Walrus-first path: --checkpoint provided, skip gRPC entirely
        #[cfg(feature = "walrus")]
        if let Some(ref checkpoint_str) = self.checkpoint {
            let checkpoints = parse_checkpoint_spec(checkpoint_str)?;
            let digest_filter = self.digest_display();
            if self.analyze_only
                && (checkpoints.len() != 1 || digest_filter == "*" || digest_filter.contains(','))
            {
                return Err(anyhow!(
                    "--analyze-only supports a single digest with a single checkpoint"
                ));
            }
            if replay_progress || verbose {
                self.print_effective_runtime_config(
                    json_output,
                    allow_fallback,
                    strict_df_checkpoint,
                    &auto_defaults,
                );
            }
            if checkpoints.len() == 1 && digest_filter != "*" && !digest_filter.contains(',') {
                return self
                    .execute_walrus_first(state, verbose, checkpoints[0], replay_progress)
                    .await;
            } else {
                return self
                    .execute_walrus_batch_v2(state, verbose, &checkpoints, replay_progress)
                    .await;
            }
        }

        if matches!(self.hydration.source, ReplaySource::Local) {
            let cache_dir = self
                .hydration
                .cache_dir
                .clone()
                .unwrap_or_else(default_local_cache_dir);
            let provider = build_local_state_provider(Some(&cache_dir))?;
            let digest = self.digest_required()?;
            let replay_state = build_replay_state(
                provider.as_ref(),
                digest,
                ReplayHydrationConfig {
                    prefetch_dynamic_fields: false,
                    prefetch_depth: self.hydration.prefetch_depth,
                    prefetch_limit: self.hydration.prefetch_limit,
                    auto_system_objects: self.hydration.auto_system_objects,
                },
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to load digest '{}' from local cache {}",
                    digest,
                    cache_dir.display()
                )
            })?;
            if verbose {
                eprintln!(
                    "[local] loaded state for {} from {}",
                    digest,
                    cache_dir.display()
                );
            }
            if replay_progress || verbose {
                self.print_effective_runtime_config(
                    json_output,
                    allow_fallback,
                    strict_df_checkpoint,
                    &auto_defaults,
                );
            }
            return execute_replay_state(
                self,
                state,
                &replay_state,
                "local",
                "local_cache",
                allow_fallback,
                verbose,
            );
        }

        let provider =
            build_historical_state_provider(state, self.hydration.source, allow_fallback, verbose)
                .await?;
        if replay_progress || verbose {
            self.print_effective_runtime_config(
                json_output,
                allow_fallback,
                strict_df_checkpoint,
                &auto_defaults,
            );
        }

        let enable_dynamic_fields =
            !self.hydration.no_prefetch && self.fetch_strategy == FetchStrategy::Full;
        let digest = self.digest_required()?;
        let replay_state = build_replay_state(
            provider.as_ref(),
            digest,
            ReplayHydrationConfig {
                prefetch_dynamic_fields: enable_dynamic_fields,
                prefetch_depth: self.hydration.prefetch_depth,
                prefetch_limit: self.hydration.prefetch_limit,
                auto_system_objects: self.hydration.auto_system_objects,
            },
        )
        .await?;
        if replay_progress {
            eprintln!("[replay] state built");
        }

        if self.analyze_only {
            return Ok(build_analyze_replay_output(
                self,
                &replay_state,
                self.hydration.source.as_str(),
                self.hydration.source.as_str(),
                allow_fallback,
                enable_dynamic_fields,
                self.hydration.prefetch_depth,
                self.hydration.prefetch_limit,
            ));
        }

        if verbose {
            eprintln!(
                "  Sender: {}",
                replay_state.transaction.sender.to_hex_literal()
            );
            eprintln!("  Commands: {}", replay_state.transaction.commands.len());
            eprintln!("  Inputs: {}", replay_state.transaction.inputs.len());
        }

        let pkg_aliases = build_aliases_shared(
            &replay_state.packages,
            Some(provider.as_ref()),
            replay_state.checkpoint,
        );
        if replay_progress {
            eprintln!("[replay] aliases built");
        }

        let mut resolver = hydrate_resolver_from_replay_state(
            state,
            &replay_state,
            &pkg_aliases.linkage_upgrades,
            &pkg_aliases.aliases,
        );
        if replay_progress {
            eprintln!("[replay] resolver hydrated");
        }

        if verbose {
            eprintln!("[deps] resolving dependency closure (GraphQL)");
        }
        let fetched_deps = fetch_dependency_closure(
            &mut resolver,
            provider.graphql(),
            replay_state.checkpoint,
            verbose,
        )
        .unwrap_or(0);
        if verbose {
            eprintln!(
                "[deps] dependency closure complete (fetched {})",
                fetched_deps
            );
        }
        if replay_progress {
            eprintln!("[replay] dependency closure done");
        }
        let dependency_fetch_mode = "graphql_dependency_closure".to_string();
        if verbose && fetched_deps > 0 {
            eprintln!(
                "[deps] fetched {} missing dependency packages",
                fetched_deps
            );
        }
        emit_linkage_debug_info(&resolver, &pkg_aliases.aliases);

        if verbose {
            eprintln!("Executing locally...");
        }
        if replay_progress {
            eprintln!("[replay] executing locally");
        }

        let mut maps = build_replay_object_maps(&replay_state, &pkg_aliases.versions);
        let debug_patcher = std::env::var("SUI_DEBUG_PATCHER")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        maybe_patch_replay_objects(
            &resolver,
            &replay_state,
            &pkg_aliases.versions,
            &pkg_aliases.aliases,
            &mut maps,
            replay_progress,
            verbose || debug_patcher,
        );
        let versions_str = maps.versions_str.clone();
        let mut cached_objects = maps.cached_objects;
        let mut version_map = maps.version_map;

        let synth_modules = if self.self_heal_dynamic_fields {
            let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
            if modules.is_empty() {
                if verbose {
                    eprintln!("[self_heal] no modules loaded; dynamic-field synthesis disabled");
                }
                None
            } else {
                Some(Arc::new(modules))
            }
        } else {
            None
        };

        let reconcile_policy = if self.reconcile_dynamic_fields {
            EffectsReconcilePolicy::DynamicFields
        } else {
            EffectsReconcilePolicy::Strict
        };
        let make_harness =
            |version_map: &HashMap<String, u64>| -> Result<sui_sandbox_core::vm::VMHarness> {
                let config = build_simulation_config(&replay_state);
                let mut harness =
                    sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
                harness.set_address_aliases_with_versions(
                    pkg_aliases.aliases.clone(),
                    versions_str.clone(),
                );

                let max_version = version_map.values().copied().max().unwrap_or(0);
                if enable_dynamic_fields {
                    let provider_clone = Arc::clone(&provider);
                    let provider_clone_for_key = Arc::clone(&provider);
                    let checkpoint = replay_state.checkpoint;
                    let strict_checkpoint = strict_df_checkpoint && checkpoint.is_some();
                    let synth_modules_for_fetcher = synth_modules.clone();
                    let self_heal_dynamic_fields = self.self_heal_dynamic_fields;
                    let fetcher = move |_parent: AccountAddress, child_id: AccountAddress| {
                        fetch_child_object_shared(
                            &provider_clone,
                            child_id,
                            checkpoint,
                            max_version,
                        )
                    };
                    harness.set_versioned_child_fetcher(Box::new(fetcher));

                    let alias_map = pkg_aliases.aliases.clone();
                    let alias_map_for_fetcher = alias_map.clone();
                    let child_id_aliases: Arc<
                        parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>,
                    > = Arc::new(parking_lot::Mutex::new(HashMap::new()));
                    let child_id_aliases_for_fetcher = child_id_aliases.clone();
                    let debug_df = matches!(
                        std::env::var("SUI_DEBUG_DF_FETCH")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref(),
                        Some("1") | Some("true") | Some("yes") | Some("on")
                    );
                    let debug_df_full = matches!(
                        std::env::var("SUI_DEBUG_DF_FETCH_FULL")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref(),
                        Some("1") | Some("true") | Some("yes") | Some("on")
                    );
                    let miss_cache: Arc<parking_lot::Mutex<HashMap<String, MissEntry>>> =
                        Arc::new(parking_lot::Mutex::new(HashMap::new()));
                    let log_self_heal = matches!(
                        std::env::var("SUI_SELF_HEAL_LOG")
                            .ok()
                            .as_deref()
                            .map(|v| v.to_ascii_lowercase())
                            .as_deref(),
                        Some("1") | Some("true") | Some("yes") | Some("on")
                    ) || verbose;
                    let key_fetcher =
                        move |parent: AccountAddress,
                              child_id: AccountAddress,
                              key_type: &TypeTag,
                              key_bytes: &[u8]| {
                            let options = ChildFetchOptions {
                                provider: &provider_clone_for_key,
                                checkpoint,
                                max_version,
                                strict_checkpoint,
                                aliases: &alias_map_for_fetcher,
                                child_id_aliases: &child_id_aliases_for_fetcher,
                                miss_cache: Some(&miss_cache),
                                debug_df,
                                debug_df_full,
                                self_heal_dynamic_fields,
                                synth_modules: synth_modules_for_fetcher.clone(),
                                log_self_heal,
                            };
                            fetch_child_object_by_key(
                                &options, parent, child_id, key_type, key_bytes,
                            )
                        };
                    harness.set_key_based_child_fetcher(Box::new(key_fetcher));
                    harness.set_child_id_aliases(child_id_aliases.clone());

                    let resolver_cache: Arc<Mutex<HashMap<String, TypeTag>>> =
                        Arc::new(Mutex::new(HashMap::new()));
                    let provider_clone_for_resolver = Arc::clone(&provider);
                    let child_id_aliases_for_resolver = child_id_aliases.clone();
                    let alias_map_for_resolver = alias_map;
                    let resolver_checkpoint = replay_state.checkpoint;
                    let resolver_strict_checkpoint = strict_checkpoint;
                    let key_type_resolver =
                        move |parent: AccountAddress, key_bytes: &[u8]| -> Option<TypeTag> {
                            resolve_key_type_via_graphql(
                                provider_clone_for_resolver.graphql(),
                                parent,
                                key_bytes,
                                resolver_checkpoint,
                                resolver_strict_checkpoint,
                                &alias_map_for_resolver,
                                &child_id_aliases_for_resolver,
                                &resolver_cache,
                            )
                        };
                    harness.set_key_type_resolver(Box::new(key_type_resolver));
                }

                Ok(harness)
            };

        let replay_once = |cached: &HashMap<String, String>,
                           versions: &HashMap<String, u64>|
         -> Result<sui_sandbox_core::tx_replay::ReplayExecution> {
            let mut harness = make_harness(versions)?;
            sui_sandbox_core::tx_replay::replay_with_version_tracking_with_policy_with_effects(
                &replay_state.transaction,
                &mut harness,
                cached,
                &pkg_aliases.aliases,
                Some(&versions_str),
                reconcile_policy,
            )
        };

        let mut replay_result = replay_once(&cached_objects, &version_map);
        if replay_progress {
            eprintln!("[replay] first execution attempt done");
        }
        let mut synthetic_logs: Vec<String> = Vec::new();
        let mut fallback_used = false;
        let mut fallback_reasons: Vec<String> = Vec::new();

        if self.synthesize_missing
            && replay_result
                .as_ref()
                .map(|r| !r.result.local_success)
                .unwrap_or(true)
        {
            let missing =
                tx_replay::find_missing_input_objects(&replay_state.transaction, &cached_objects);
            if !missing.is_empty() {
                eprintln!(
                    "[replay_fallback] missing_input_objects={} (attempting synthesis)",
                    missing.len()
                );
                match synthesize_missing_inputs(
                    &missing,
                    &mut cached_objects,
                    &mut version_map,
                    &resolver,
                    &pkg_aliases.aliases,
                    &provider,
                    verbose,
                ) {
                    Ok(logs) => {
                        synthetic_logs = logs;
                        if !synthetic_logs.is_empty() {
                            eprintln!(
                                "[replay_fallback] synthesized_inputs={}",
                                synthetic_logs.len()
                            );
                            fallback_used = true;
                            fallback_reasons.push(
                                "synthesized_missing_inputs_after_initial_failure".to_string(),
                            );
                            replay_result = replay_once(&cached_objects, &version_map);
                        }
                    }
                    Err(e) => {
                        if verbose {
                            eprintln!("[replay_fallback] synthesis_error={}", e);
                        }
                    }
                }
            }
        }

        let execution_path = build_execution_path(
            self,
            allow_fallback,
            enable_dynamic_fields,
            dependency_fetch_mode,
            fetched_deps,
            fallback_used,
            fallback_reasons,
            synthetic_logs.len(),
        );

        match replay_result {
            Ok(execution) => {
                let result = execution.result;
                let effects_summary = build_effects_summary(&execution.effects);
                let comparison = if self.compare {
                    result.comparison.map(|c| {
                        let mut notes = c.notes.clone();
                        if !synthetic_logs.is_empty() {
                            notes.push(format!("synthetic_inputs={}", synthetic_logs.len()));
                        }
                        ComparisonResult {
                            status_match: c.status_match,
                            created_match: c.created_count_match,
                            mutated_match: c.mutated_count_match,
                            deleted_match: c.deleted_count_match,
                            on_chain_status: if c.status_match && result.local_success {
                                "success".to_string()
                            } else if c.status_match && !result.local_success {
                                "failed".to_string()
                            } else {
                                "unknown".to_string()
                            },
                            local_status: if result.local_success {
                                "success".to_string()
                            } else {
                                "failed".to_string()
                            },
                            notes,
                        }
                    })
                } else {
                    None
                };

                if !synthetic_logs.is_empty() && verbose {
                    for line in &synthetic_logs {
                        eprintln!("[replay_fallback] {}", line);
                    }
                }
                let diagnostics = if result.local_success {
                    None
                } else {
                    build_replay_diagnostics(
                        &replay_state,
                        &cached_objects,
                        &resolver,
                        allow_fallback,
                    )
                };

                Ok(ReplayOutput {
                    digest: self.digest_display().to_string(),
                    local_success: result.local_success,
                    local_error: result.local_error,
                    diagnostics,
                    execution_path,
                    comparison,
                    analysis: None,
                    effects: Some(effects_summary),
                    effects_full: Some(execution.effects),
                    commands_executed: result.commands_executed,
                    batch_summary_printed: false,
                })
            }
            Err(e) => {
                let diagnostics = build_replay_diagnostics(
                    &replay_state,
                    &cached_objects,
                    &resolver,
                    allow_fallback,
                );
                Ok(ReplayOutput {
                    digest: self.digest_display().to_string(),
                    local_success: false,
                    local_error: Some(e.to_string()),
                    diagnostics,
                    execution_path,
                    comparison: None,
                    analysis: None,
                    effects: None,
                    effects_full: None,
                    commands_executed: 0,
                    batch_summary_printed: false,
                })
            }
        }
    }

    /// Walrus-first replay: fetch checkpoint data from Walrus, convert to ReplayState,
    /// and execute locally. No gRPC, GraphQL, or API keys needed.
    #[cfg(feature = "walrus")]
    async fn execute_walrus_first(
        &self,
        state: &SandboxState,
        verbose: bool,
        checkpoint_num: u64,
        replay_progress: bool,
    ) -> Result<ReplayOutput> {
        self.execute_walrus_with_data(
            state,
            verbose,
            checkpoint_num,
            replay_progress,
            WalrusReplayData {
                preloaded_checkpoint: None,
                shared_obj_cache: None,
                shared_pkg_cache: None,
            },
        )
        .await
    }

    /// Core Walrus replay logic. Accepts optional pre-loaded checkpoint data and shared caches
    /// to avoid re-fetching in batch mode.
    #[cfg(feature = "walrus")]
    async fn execute_walrus_with_data(
        &self,
        state: &SandboxState,
        verbose: bool,
        checkpoint_num: u64,
        replay_progress: bool,
        data: WalrusReplayData<'_>,
    ) -> Result<ReplayOutput> {
        use sui_transport::walrus::WalrusClient;
        let allow_fallback = self.hydration.allow_fallback && !self.vm_only;
        let digest = self.digest_required()?;

        // Fetch checkpoint if not pre-loaded
        let owned_checkpoint;
        let checkpoint_data = if let Some(pre) = data.preloaded_checkpoint {
            pre
        } else {
            if verbose {
                eprintln!(
                    "[walrus] fetching checkpoint {} for digest {}",
                    checkpoint_num, digest
                );
            }
            owned_checkpoint = tokio::task::spawn_blocking(move || {
                let walrus = WalrusClient::mainnet();
                walrus.get_checkpoint(checkpoint_num)
            })
            .await
            .context("Walrus fetch task panicked")?
            .context("Failed to fetch checkpoint from Walrus")?;
            if replay_progress {
                eprintln!(
                    "[walrus] checkpoint fetched ({} transactions)",
                    owned_checkpoint.transactions.len()
                );
            }
            &owned_checkpoint
        };
        let shared_obj_cache = data.shared_obj_cache.clone();
        let shared_pkg_cache = data.shared_pkg_cache.clone();

        let mut replay_state = checkpoint_to_replay_state(checkpoint_data, digest)
            .context("Failed to convert checkpoint to replay state")?;

        // Build a map of shared object versions from effects' input_consensus_objects.
        // Read-only shared objects are NOT included in checkpoint input_objects,
        // so we need their exact versions from effects to fetch them correctly.
        let shared_obj_versions: HashMap<AccountAddress, u64> = {
            let mut map = HashMap::new();
            if let Some(tx_idx) = find_tx_in_checkpoint(checkpoint_data, digest) {
                let effects = &checkpoint_data.transactions[tx_idx].effects;
                for ico in effects.input_consensus_objects() {
                    let (obj_id, seq) = ico.id_and_version();
                    map.insert(AccountAddress::from(obj_id), seq.value());
                }
            }
            map
        };

        if replay_progress {
            eprintln!("[walrus] replay state built");
        }

        if self.analyze_only {
            return Ok(build_analyze_replay_output(
                self,
                &replay_state,
                "walrus",
                "walrus_checkpoint",
                allow_fallback,
                false,
                0,
                0,
            ));
        }

        if verbose {
            eprintln!(
                "  Sender: {}",
                replay_state.transaction.sender.to_hex_literal()
            );
            eprintln!("  Commands: {}", replay_state.transaction.commands.len());
            eprintln!("  Inputs: {}", replay_state.transaction.inputs.len());
            eprintln!(
                "  Packages from checkpoint: {}",
                replay_state.packages.len()
            );
            for (addr, pkg) in &replay_state.packages {
                eprintln!(
                    "    pkg {} v{} original={:?} linkage_entries={}",
                    addr.to_hex_literal(),
                    pkg.version,
                    pkg.original_id.map(|a| a.to_hex_literal()),
                    pkg.linkage.len()
                );
            }
        }

        // Build initial aliases from checkpoint packages
        let mut pkg_aliases =
            build_aliases_shared(&replay_state.packages, None, replay_state.checkpoint);
        if replay_progress {
            eprintln!("[walrus] aliases built");
        }

        let mut resolver = hydrate_resolver_from_replay_state(
            state,
            &replay_state,
            &pkg_aliases.linkage_upgrades,
            &pkg_aliases.aliases,
        );
        if replay_progress {
            eprintln!("[walrus] resolver hydrated");
        }

        // Fetch missing packages via Walrus (previousTransaction → checkpoint)
        // Falls back to GraphQL module fetch for system packages
        let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
        let graphql_client = GraphQLClient::new(&graphql_endpoint);

        // Package cache: shared across direct fetch and dependency closure
        // Use shared cache if provided (batch mode), otherwise create from checkpoint
        let walrus_pkg_cache: Arc<parking_lot::Mutex<HashMap<AccountAddress, PackageData>>> =
            if let Some(cache) = shared_pkg_cache {
                // Merge checkpoint packages into shared cache
                {
                    let mut c = cache.lock();
                    for (addr, pkg) in &replay_state.packages {
                        c.entry(*addr).or_insert_with(|| pkg.clone());
                    }
                }
                cache
            } else {
                let mut cache = HashMap::new();
                for (addr, pkg) in &replay_state.packages {
                    cache.insert(*addr, pkg.clone());
                }
                Arc::new(parking_lot::Mutex::new(cache))
            };

        // Extract required package IDs from PTB commands
        let mut direct_fetched = 0usize;
        {
            let mut required_pkgs: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for cmd in &replay_state.transaction.commands {
                match cmd {
                    PtbCommand::MoveCall {
                        package,
                        type_arguments,
                        ..
                    } => {
                        required_pkgs.insert(package.clone());
                        for ty in type_arguments {
                            for pkg_id in
                                sui_sandbox_core::utilities::extract_package_ids_from_type(ty)
                            {
                                required_pkgs.insert(pkg_id);
                            }
                        }
                    }
                    PtbCommand::Publish { dependencies, .. } => {
                        for dep in dependencies {
                            required_pkgs.insert(dep.clone());
                        }
                    }
                    PtbCommand::Upgrade { package, .. } => {
                        required_pkgs.insert(package.clone());
                    }
                    _ => {}
                }
            }

            // Fetch any that aren't already in the resolver
            for pkg_hex in &required_pkgs {
                if let Ok(addr) = AccountAddress::from_hex_literal(pkg_hex) {
                    if !replay_state.packages.contains_key(&addr) && !resolver.has_package(&addr) {
                        if verbose {
                            eprintln!("[walrus] fetching package {} via Walrus", pkg_hex);
                        }
                        // Try Walrus-backed fetch (previousTransaction → checkpoint)
                        if let Some(pkg_data) = fetch_package_via_walrus(
                            &graphql_client,
                            &walrus_pkg_cache,
                            pkg_hex,
                            verbose,
                        ) {
                            if verbose {
                                eprintln!(
                                    "[walrus] got pkg {} v{} original={:?} linkage_entries={}",
                                    pkg_data.address.to_hex_literal(),
                                    pkg_data.version,
                                    pkg_data.original_id.map(|a| a.to_hex_literal()),
                                    pkg_data.linkage.len()
                                );
                                for (orig, upgraded) in &pkg_data.linkage {
                                    eprintln!(
                                        "  linkage: {} → {}",
                                        orig.to_hex_literal(),
                                        upgraded.to_hex_literal()
                                    );
                                }
                            }
                            let _ = resolver.add_package_modules_at(
                                pkg_data.modules.clone(),
                                Some(pkg_data.address),
                            );
                            // Register per-package linkage + global linkage + aliases
                            resolver.add_package_linkage(
                                pkg_data.address,
                                pkg_data.runtime_id(),
                                &pkg_data.linkage,
                            );
                            for (original, upgraded) in &pkg_data.linkage {
                                resolver.add_linkage_upgrade(*original, *upgraded);
                            }
                            if let Some(orig_id) = pkg_data.original_id {
                                if orig_id != pkg_data.address {
                                    resolver.add_address_alias(pkg_data.address, orig_id);
                                }
                            }
                            replay_state.packages.insert(pkg_data.address, pkg_data);
                            direct_fetched += 1;
                        } else {
                            // Fallback: GraphQL module fetch (no linkage, but works for
                            // system packages 0x1/0x2/0x3 which don't need linkage)
                            if verbose {
                                eprintln!(
                                    "[walrus] Walrus fallback failed for {}, trying GraphQL",
                                    pkg_hex
                                );
                            }
                            match graphql_client.fetch_package(pkg_hex) {
                                Ok(gql_pkg) => {
                                    let modules = sui_transport::decode_graphql_modules(
                                        pkg_hex,
                                        &gql_pkg.modules,
                                    )?;
                                    if !modules.is_empty() {
                                        let _ =
                                            resolver.add_package_modules_at(modules, Some(addr));
                                        direct_fetched += 1;
                                    }
                                }
                                Err(e) => {
                                    if verbose {
                                        eprintln!(
                                            "[walrus] failed to fetch package {}: {}",
                                            pkg_hex, e
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if direct_fetched > 0 && verbose {
                eprintln!("[walrus] fetched {} direct packages", direct_fetched);
            }
        }

        // Resolve transitive dependency closure (Walrus-backed)
        let fetched_deps = fetch_dependency_closure_walrus(
            &mut resolver,
            &graphql_client,
            &walrus_pkg_cache,
            &mut replay_state,
            verbose,
        )?;
        if fetched_deps > 0 && verbose {
            eprintln!(
                "[walrus] fetched {} transitive dependency packages",
                fetched_deps
            );
        }
        if replay_progress {
            eprintln!("[walrus] dependency closure done");
        }
        let dependency_packages_fetched = direct_fetched + fetched_deps;

        // Rebuild aliases now that all packages are loaded
        pkg_aliases = build_aliases_shared(&replay_state.packages, None, replay_state.checkpoint);
        // Re-register new aliases in the resolver
        for (original, upgraded) in &pkg_aliases.linkage_upgrades {
            resolver.add_linkage_upgrade(*original, *upgraded);
        }
        for (storage, runtime) in &pkg_aliases.aliases {
            resolver.add_address_alias(*storage, *runtime);
        }
        emit_linkage_debug_info(&resolver, &pkg_aliases.aliases);

        // Inject system objects (Clock, Random)
        if self.hydration.auto_system_objects {
            ensure_system_objects(
                &mut replay_state.objects,
                &HashMap::new(),
                replay_state.transaction.timestamp_ms,
                replay_state.checkpoint,
            );
        }

        // Fetch missing input objects via GraphQL
        {
            let mut missing_ids: Vec<(AccountAddress, Option<u64>)> = Vec::new();
            for input in &replay_state.transaction.inputs {
                let (id_str, version) = match input {
                    TransactionInput::Object {
                        object_id, version, ..
                    } => (Some(object_id.clone()), Some(*version)),
                    TransactionInput::SharedObject { object_id, .. } => {
                        // Look up the exact version from effects' input_consensus_objects
                        let version = AccountAddress::from_hex_literal(object_id)
                            .ok()
                            .and_then(|addr| shared_obj_versions.get(&addr).copied());
                        (Some(object_id.clone()), version)
                    }
                    TransactionInput::ImmutableObject {
                        object_id, version, ..
                    } => (Some(object_id.clone()), Some(*version)),
                    TransactionInput::Receiving {
                        object_id, version, ..
                    } => (Some(object_id.clone()), Some(*version)),
                    TransactionInput::Pure { .. } => (None, None),
                };
                if let Some(id_str) = id_str {
                    if let Ok(addr) = AccountAddress::from_hex_literal(&id_str) {
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            replay_state.objects.entry(addr)
                        {
                            // Check shared object cache first (populated from
                            // other checkpoints' data) before falling back to GraphQL
                            let addr_hex = addr.to_hex_literal();
                            let found_in_cache = if let Some(ref cache) = shared_obj_cache {
                                if let Some((ts, bcs, ver)) = cache.lock().get(&addr_hex).cloned() {
                                    entry.insert(VersionedObject {
                                        id: addr,
                                        version: ver,
                                        digest: None,
                                        type_tag: Some(ts),
                                        bcs_bytes: bcs,
                                        is_shared: shared_obj_versions.contains_key(&addr),
                                        is_immutable: false,
                                    });
                                    true
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                            if !found_in_cache {
                                missing_ids.push((addr, version));
                            }
                        }
                    }
                }
            }
            if !missing_ids.is_empty() {
                if verbose {
                    eprintln!(
                        "[walrus] fetching {} missing input objects via GraphQL",
                        missing_ids.len()
                    );
                }
                for (addr, version) in &missing_ids {
                    let addr_hex = addr.to_hex_literal();
                    let obj_result = match version {
                        Some(v) => graphql_client
                            .fetch_object_at_version(&addr_hex, *v)
                            .or_else(|_| graphql_client.fetch_object(&addr_hex)),
                        _ => graphql_client.fetch_object(&addr_hex),
                    };
                    match obj_result {
                        Ok(gql_obj) => {
                            if let Some(bcs_b64) = &gql_obj.bcs_base64 {
                                if let Ok(bcs_bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(bcs_b64)
                                {
                                    let (is_shared, is_immutable) = match &gql_obj.owner {
                                        sui_transport::graphql::ObjectOwner::Shared { .. } => {
                                            (true, false)
                                        }
                                        sui_transport::graphql::ObjectOwner::Immutable => {
                                            (false, true)
                                        }
                                        _ => (false, false),
                                    };
                                    replay_state.objects.insert(
                                        *addr,
                                        VersionedObject {
                                            id: *addr,
                                            version: gql_obj.version,
                                            digest: gql_obj.digest.clone(),
                                            type_tag: gql_obj.type_string.clone(),
                                            bcs_bytes,
                                            is_shared,
                                            is_immutable,
                                        },
                                    );
                                    if verbose {
                                        eprintln!(
                                            "[walrus] fetched object {} v{}",
                                            addr_hex, gql_obj.version
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if verbose {
                                eprintln!("[walrus] failed to fetch object {}: {}", addr_hex, e);
                            }
                        }
                    }
                }
            }
        }

        // Export state if requested (before execution, after all data gathered)
        if let Some(export_path) = &self.export_state {
            let json = serde_json::to_string_pretty(&replay_state)
                .context("Failed to serialize replay state")?;
            std::fs::write(export_path, json)
                .with_context(|| format!("Failed to write state to {}", export_path.display()))?;
            if verbose {
                eprintln!("[export] wrote replay state to {}", export_path.display());
            }
        }

        if verbose {
            eprintln!("Executing locally...");
        }

        let mut maps = build_replay_object_maps(&replay_state, &pkg_aliases.versions);
        maybe_patch_replay_objects(
            &resolver,
            &replay_state,
            &pkg_aliases.versions,
            &pkg_aliases.aliases,
            &mut maps,
            false,
            verbose,
        );
        let versions_str = maps.versions_str.clone();
        let cached_objects = maps.cached_objects;

        let reconcile_policy = if self.reconcile_dynamic_fields {
            EffectsReconcilePolicy::DynamicFields
        } else {
            EffectsReconcilePolicy::Strict
        };

        // Build VM harness and execute
        let config = build_simulation_config(&replay_state);
        let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
        harness
            .set_address_aliases_with_versions(pkg_aliases.aliases.clone(), versions_str.clone());

        // Set up Walrus+GraphQL child fetcher for dynamic fields
        {
            let gql = Arc::new(graphql_client);
            let checkpoint = replay_state.checkpoint;

            // Pre-populate child cache from all objects in the checkpoint data.
            // Use shared cache if provided (batch mode), otherwise create a fresh one.
            let walrus_obj_cache: SharedObjCache = shared_obj_cache
                .clone()
                .unwrap_or_else(|| Arc::new(parking_lot::Mutex::new(HashMap::new())));
            if shared_obj_cache.is_none() {
                // Only pre-populate from checkpoint if we didn't get a shared cache
                // (shared cache is already pre-populated by batch v2)
                let mut prepop_count = 0usize;
                for tx in &checkpoint_data.transactions {
                    for obj in tx.input_objects.iter().chain(tx.output_objects.iter()) {
                        let oid = format!("0x{}", hex::encode(obj.id().into_bytes()));
                        if let Some((ts, bcs, ver, _shared)) =
                            sui_transport::walrus::extract_object_bcs(obj)
                        {
                            walrus_obj_cache.lock().insert(oid, (ts, bcs, ver));
                            prepop_count += 1;
                        }
                    }
                }
                if verbose {
                    eprintln!(
                        "[walrus-df] pre-populated cache with {} objects from checkpoint",
                        prepop_count
                    );
                }
            }

            // Fetch unchanged_loaded_runtime_objects via gRPC — these are the
            // dynamic field children that were loaded during execution.
            // For each, use previous_transaction → checkpoint → Walrus to get BCS.
            {
                // Try archive endpoint first (has full history), fall back to live fullnode
                let (grpc_endpoint, api_key) = historical_endpoint_and_api_key_from_env();
                match GrpcClient::with_api_key(&grpc_endpoint, api_key).await {
                    Ok(grpc) => {
                        match grpc.get_transaction(digest).await {
                            Ok(Some(tx)) => {
                                let runtime_objs = &tx.unchanged_loaded_runtime_objects;
                                if !runtime_objs.is_empty() {
                                    if verbose {
                                        eprintln!(
                                            "[walrus-df] gRPC: {} unchanged_loaded_runtime_objects",
                                            runtime_objs.len()
                                        );
                                    }
                                    // For each runtime object, try to fetch via previous_transaction
                                    for (obj_id, version) in runtime_objs {
                                        if walrus_obj_cache.lock().contains_key(obj_id) {
                                            continue;
                                        }
                                        // Try gRPC get_object_at_version to get previous_transaction
                                        match grpc
                                            .get_object_at_version(obj_id, Some(*version))
                                            .await
                                        {
                                            Ok(Some(grpc_obj)) => {
                                                // If we got BCS directly from gRPC, use it
                                                if let (Some(type_str), Some(bcs_bytes)) =
                                                    (&grpc_obj.type_string, &grpc_obj.bcs)
                                                {
                                                    if !bcs_bytes.is_empty() {
                                                        walrus_obj_cache.lock().insert(
                                                            obj_id.clone(),
                                                            (
                                                                type_str.clone(),
                                                                bcs_bytes.clone(),
                                                                *version,
                                                            ),
                                                        );
                                                        if verbose {
                                                            eprintln!(
                                                                "[walrus-df] gRPC: fetched {} v{} directly",
                                                                obj_id, version
                                                            );
                                                        }
                                                        continue;
                                                    }
                                                }
                                                // Fallback: use previous_transaction to find via Walrus
                                                if let Some(prev_tx) =
                                                    &grpc_obj.previous_transaction
                                                {
                                                    if let Some(found) = fetch_via_prev_tx(
                                                        &gql,
                                                        &walrus_obj_cache,
                                                        obj_id,
                                                        prev_tx,
                                                    ) {
                                                        if verbose {
                                                            eprintln!(
                                                                "[walrus-df] Walrus: fetched {} via prevTx {}",
                                                                obj_id, prev_tx
                                                            );
                                                        }
                                                        let _ = found; // already cached by fetch_via_prev_tx
                                                    }
                                                }
                                            }
                                            Ok(None) => {
                                                // Object not found at version (pruned) — try latest
                                                if let Ok(gql_obj) = gql.fetch_object(obj_id) {
                                                    if let Some(prev_tx) =
                                                        &gql_obj.previous_transaction
                                                    {
                                                        let _ = fetch_via_prev_tx(
                                                            &gql,
                                                            &walrus_obj_cache,
                                                            obj_id,
                                                            prev_tx,
                                                        );
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                if verbose {
                                                    eprintln!(
                                                        "[walrus-df] gRPC: failed to fetch {} v{}: {}",
                                                        obj_id, version, e
                                                    );
                                                }
                                                // Fallback: try GraphQL + Walrus
                                                if let Ok(gql_obj) = gql.fetch_object(obj_id) {
                                                    if let Some(prev_tx) =
                                                        &gql_obj.previous_transaction
                                                    {
                                                        let _ = fetch_via_prev_tx(
                                                            &gql,
                                                            &walrus_obj_cache,
                                                            obj_id,
                                                            prev_tx,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    let cached = walrus_obj_cache.lock().len();
                                    if verbose {
                                        eprintln!(
                                            "[walrus-df] cache now has {} objects after runtime obj fetch",
                                            cached
                                        );
                                    }
                                }
                            }
                            Ok(None) => {
                                if verbose {
                                    eprintln!("[walrus-df] gRPC: transaction not found");
                                }
                            }
                            Err(e) => {
                                if verbose {
                                    eprintln!("[walrus-df] gRPC: failed to get transaction: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if verbose {
                            eprintln!("[walrus-df] gRPC: failed to connect: {}", e);
                        }
                    }
                }
            }

            // Build parent→children index from checkpoint data for DF resolution.
            // For each dynamic_field::Field object, record (parent, actual_id, type_str, key_bcs_size).
            // This allows the child fetcher to find objects by parent when computed hash differs.
            struct DfChild {
                actual_id: String,
                key_type_str: String,
            }
            let df_children_by_parent: Arc<HashMap<String, Vec<DfChild>>> = {
                let mut map: HashMap<String, Vec<DfChild>> = HashMap::new();
                let tx_index = checkpoint_data
                    .transactions
                    .iter()
                    .position(|tx| tx.transaction.digest().to_string() == digest);
                if let Some(idx) = tx_index {
                    let target_tx = &checkpoint_data.transactions[idx];
                    for obj in &target_tx.input_objects {
                        if let sui_types::object::Data::Move(move_obj) = &obj.data {
                            let type_str = move_obj.type_().to_string();
                            if type_str.contains("::dynamic_field::Field<")
                                || type_str.contains("::dynamic_object_field::Wrapper<")
                            {
                                let parent_addr = match obj.owner {
                                    sui_types::object::Owner::ObjectOwner(addr) => {
                                        Some(AccountAddress::from(addr).to_hex_literal())
                                    }
                                    _ => None,
                                };
                                if let Some(parent_hex) = parent_addr {
                                    // Extract K from Field<K, V> or Wrapper<K>
                                    let key_type_str = if let Some(rest) = type_str
                                        .find("::dynamic_field::Field<")
                                        .map(|i| &type_str[i + "::dynamic_field::Field<".len()..])
                                    {
                                        rest.strip_suffix('>').and_then(split_first_type_param)
                                    } else if let Some(rest) =
                                        type_str.find("::dynamic_object_field::Wrapper<").map(|i| {
                                            &type_str
                                                [i + "::dynamic_object_field::Wrapper<".len()..]
                                        })
                                    {
                                        rest.strip_suffix('>').map(|s| s.to_string())
                                    } else {
                                        None
                                    };
                                    if let Some(kt) = key_type_str {
                                        let actual_id =
                                            format!("0x{}", hex::encode(obj.id().into_bytes()));
                                        map.entry(parent_hex).or_default().push(DfChild {
                                            actual_id,
                                            key_type_str: kt,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                if verbose && !map.is_empty() {
                    let total: usize = map.values().map(|v| v.len()).sum();
                    eprintln!(
                        "[walrus-df] indexed {} DF children across {} parents from checkpoint",
                        total,
                        map.len()
                    );
                }
                Arc::new(map)
            };

            // Helper: fetch object by ID, trying targeted strategies:
            // 1. Cache (pre-populated from checkpoint data)
            // 2. GraphQL latest (works for objects that still exist)
            // 3. Targeted Walrus lookup via previousTransaction → checkpoint
            let fetch_child_obj = {
                let gql = Arc::clone(&gql);
                let cache = Arc::clone(&walrus_obj_cache);
                move |id_hex: &str| -> Option<(String, Vec<u8>, u64)> {
                    // Strategy 1: Check pre-populated cache
                    if let Some(cached) = cache.lock().get(id_hex) {
                        return Some(cached.clone());
                    }

                    // Strategy 2: GraphQL latest — get object + previousTransactionBlock
                    if let Ok(obj) = gql.fetch_object(id_hex) {
                        if let (Some(type_str), Some(bcs_b64)) =
                            (obj.type_string.as_ref(), obj.bcs_base64.as_ref())
                        {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(bcs_b64)
                            {
                                let result = (type_str.clone(), bytes, obj.version);
                                cache.lock().insert(id_hex.to_string(), result.clone());
                                return Some(result);
                            }
                        }

                        // Object exists but BCS not available at latest — use
                        // previousTransactionBlock to find it in Walrus
                        if let Some(prev_tx) = &obj.previous_transaction {
                            if let Some(found) = fetch_via_prev_tx(&gql, &cache, id_hex, prev_tx) {
                                return Some(found);
                            }
                        }
                    }

                    None
                }
            };
            let fetch_child_obj = Arc::new(fetch_child_obj);

            // Versioned child fetcher (ID-based)
            let fetcher_fn = Arc::clone(&fetch_child_obj);
            let fetcher = move |_parent: AccountAddress,
                                child_id: AccountAddress|
                  -> Option<(TypeTag, Vec<u8>, u64)> {
                let id_hex = child_id.to_hex_literal();
                let (type_str, bytes, version) = fetcher_fn(&id_hex)?;
                let tag = parse_type_tag(&type_str).ok()?;
                Some((tag, bytes, version))
            };
            harness.set_versioned_child_fetcher(Box::new(fetcher));

            // Key-based child fetcher — uses parent→children index from checkpoint
            // to find DF children when the computed hash doesn't match the on-chain ID.
            let key_fetcher_fn = Arc::clone(&fetch_child_obj);
            let key_fetcher_cache = Arc::clone(&walrus_obj_cache);
            let key_fetcher_df_index = Arc::clone(&df_children_by_parent);
            let key_fetcher = move |parent: AccountAddress,
                                    child_id: AccountAddress,
                                    key_type: &TypeTag,
                                    key_bytes: &[u8]|
                  -> Option<(TypeTag, Vec<u8>)> {
                let id_hex = child_id.to_hex_literal();
                // Try direct ID lookup first
                if let Some((type_str, bytes, _version)) = key_fetcher_fn(&id_hex) {
                    let tag = parse_type_tag(&type_str).ok()?;
                    return Some((tag, bytes));
                }

                // Direct lookup failed — search parent's DF children from checkpoint.
                let parent_hex = parent.to_hex_literal();
                if let Some(children) = key_fetcher_df_index.get(&parent_hex) {
                    // Try each child of this parent
                    if let Ok(type_bcs) = bcs::to_bytes(key_type) {
                        for child in children {
                            // Compute hash for this child's key type
                            if let Ok(child_key_tag) = parse_type_tag(&child.key_type_str) {
                                if let Ok(child_type_bcs) = bcs::to_bytes(&child_key_tag) {
                                    if let Some(computed) = compute_dynamic_field_id(
                                        &parent_hex,
                                        key_bytes,
                                        &child_type_bcs,
                                    ) {
                                        if computed == id_hex {
                                            // This child matches! Fetch by actual ID.
                                            if let Some((ts, bytes, _ver)) =
                                                key_fetcher_fn(&child.actual_id)
                                            {
                                                // Cache under computed ID too
                                                key_fetcher_cache.lock().insert(
                                                    id_hex.clone(),
                                                    (ts.clone(), bytes.clone(), _ver),
                                                );
                                                let tag = parse_type_tag(&ts).ok()?;
                                                return Some((tag, bytes));
                                            }
                                        }
                                    }
                                    // Also try with the VM's key_type (may differ due to aliases)
                                    if type_bcs != child_type_bcs {
                                        if let Some(computed) = compute_dynamic_field_id(
                                            &parent_hex,
                                            key_bytes,
                                            &type_bcs,
                                        ) {
                                            if computed == id_hex {
                                                if let Some((ts, bytes, _ver)) =
                                                    key_fetcher_fn(&child.actual_id)
                                                {
                                                    key_fetcher_cache.lock().insert(
                                                        id_hex.clone(),
                                                        (ts.clone(), bytes.clone(), _ver),
                                                    );
                                                    let tag = parse_type_tag(&ts).ok()?;
                                                    return Some((tag, bytes));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // If we have children but couldn't match by hash, try by key_type string match
                    let key_type_str = format!("{}", key_type);
                    for child in children {
                        if child.key_type_str == key_type_str
                            || child.key_type_str.ends_with(&key_type_str)
                        {
                            if let Some((ts, bytes, _ver)) = key_fetcher_fn(&child.actual_id) {
                                key_fetcher_cache
                                    .lock()
                                    .insert(id_hex.clone(), (ts.clone(), bytes.clone(), _ver));
                                let tag = parse_type_tag(&ts).ok()?;
                                return Some((tag, bytes));
                            }
                        }
                    }
                }

                None
            };
            harness.set_key_based_child_fetcher(Box::new(key_fetcher));

            // Child ID aliases for dynamic field hash overrides
            let child_id_aliases: Arc<parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>> =
                Arc::new(parking_lot::Mutex::new(HashMap::new()));
            harness.set_child_id_aliases(child_id_aliases.clone());

            // Key type resolver — resolves dynamic field key types via GraphQL
            let gql_for_resolver = Arc::clone(&gql);
            let alias_map_for_resolver = pkg_aliases.aliases.clone();
            let child_id_aliases_for_resolver = child_id_aliases.clone();
            let resolver_cache: Arc<Mutex<HashMap<String, TypeTag>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let key_type_resolver =
                move |parent: AccountAddress, key_bytes: &[u8]| -> Option<TypeTag> {
                    resolve_key_type_via_graphql(
                        &gql_for_resolver,
                        parent,
                        key_bytes,
                        checkpoint,
                        false, // Walrus path always allows fallback to latest
                        &alias_map_for_resolver,
                        &child_id_aliases_for_resolver,
                        &resolver_cache,
                    )
                };
            harness.set_key_type_resolver(Box::new(key_type_resolver));
        }

        let replay_result =
            sui_sandbox_core::tx_replay::replay_with_version_tracking_with_policy_with_effects(
                &replay_state.transaction,
                &mut harness,
                &cached_objects,
                &pkg_aliases.aliases,
                Some(&versions_str),
                reconcile_policy,
            );

        match replay_result {
            Ok(execution) => {
                let result = execution.result;
                let effects_summary = build_effects_summary(&execution.effects);
                let comparison = if self.compare {
                    result.comparison.map(|c| ComparisonResult {
                        status_match: c.status_match,
                        created_match: c.created_count_match,
                        mutated_match: c.mutated_count_match,
                        deleted_match: c.deleted_count_match,
                        on_chain_status: if c.status_match && result.local_success {
                            "success".to_string()
                        } else if c.status_match && !result.local_success {
                            "failed".to_string()
                        } else {
                            "unknown".to_string()
                        },
                        local_status: if result.local_success {
                            "success".to_string()
                        } else {
                            "failed".to_string()
                        },
                        notes: c.notes.clone(),
                    })
                } else {
                    None
                };
                let diagnostics = if result.local_success {
                    None
                } else {
                    build_replay_diagnostics(
                        &replay_state,
                        &cached_objects,
                        &resolver,
                        allow_fallback,
                    )
                };

                Ok(ReplayOutput {
                    digest: digest.to_string(),
                    local_success: result.local_success,
                    local_error: result.local_error,
                    diagnostics,
                    execution_path: ReplayExecutionPath {
                        requested_source: self
                            .hydration
                            .source
                            .to_possible_value()
                            .map_or_else(|| "hybrid".to_string(), |v| v.get_name().to_string()),
                        effective_source: "walrus_checkpoint".to_string(),
                        vm_only: self.vm_only,
                        allow_fallback,
                        auto_system_objects: self.hydration.auto_system_objects,
                        fallback_used: false,
                        fallback_reasons: Vec::new(),
                        dynamic_field_prefetch: false,
                        prefetch_depth: 0,
                        prefetch_limit: 0,
                        dependency_fetch_mode: "walrus_checkpoint".to_string(),
                        dependency_packages_fetched,
                        synthetic_inputs: 0,
                    },
                    comparison,
                    analysis: None,
                    effects: Some(effects_summary),
                    effects_full: Some(execution.effects),
                    commands_executed: result.commands_executed,
                    batch_summary_printed: false,
                })
            }
            Err(e) => {
                let diagnostics = build_replay_diagnostics(
                    &replay_state,
                    &cached_objects,
                    &resolver,
                    allow_fallback,
                );
                Ok(ReplayOutput {
                    digest: digest.to_string(),
                    local_success: false,
                    local_error: Some(e.to_string()),
                    diagnostics,
                    execution_path: ReplayExecutionPath {
                        requested_source: self
                            .hydration
                            .source
                            .to_possible_value()
                            .map_or_else(|| "hybrid".to_string(), |v| v.get_name().to_string()),
                        effective_source: "walrus_checkpoint".to_string(),
                        vm_only: self.vm_only,
                        allow_fallback,
                        auto_system_objects: self.hydration.auto_system_objects,
                        fallback_used: false,
                        fallback_reasons: Vec::new(),
                        dynamic_field_prefetch: false,
                        prefetch_depth: 0,
                        prefetch_limit: 0,
                        dependency_fetch_mode: "walrus_checkpoint".to_string(),
                        dependency_packages_fetched,
                        synthetic_inputs: 0,
                    },
                    comparison: None,
                    analysis: None,
                    effects: None,
                    effects_full: None,
                    commands_executed: 0,
                    batch_summary_printed: false,
                })
            }
        }
    }

    /// Efficient batch replay: fetches all checkpoints in one batched call,
    /// pre-populates shared object/package caches, classifies transactions,
    /// and prints a summary report.
    #[cfg(feature = "walrus")]
    async fn execute_walrus_batch_v2(
        &self,
        state: &SandboxState,
        verbose: bool,
        checkpoints: &[u64],
        replay_progress: bool,
    ) -> Result<ReplayOutput> {
        walrus_batch::execute_walrus_batch_v2(self, state, verbose, checkpoints, replay_progress)
            .await
    }
}

/// Split the first type parameter from a comma-separated type list,
/// respecting angle bracket nesting.
/// e.g. "u64, SomeStruct<A, B>" -> Some("u64")
/// e.g. "SomeStruct<A, B>" -> Some("SomeStruct<A, B>")
fn split_first_type_param(s: &str) -> Option<String> {
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                return Some(s[..i].trim().to_string());
            }
            _ => {}
        }
    }
    Some(s.trim().to_string())
}

fn env_bool_opt(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

fn auto_progress_enabled(json_output: bool) -> bool {
    !json_output && std::io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::FromArgMatches;

    fn parse_replay_cmd(args: &[&str]) -> ReplayCmd {
        let cmd = <ReplayCmd as clap::Args>::augment_args(clap::Command::new("replay"));
        let matches = cmd.try_get_matches_from(args).expect("parse");
        ReplayCmd::from_arg_matches(&matches).expect("from arg matches")
    }

    #[test]
    fn test_replay_output_serialization() {
        let output = ReplayOutput {
            digest: "test123".to_string(),
            local_success: true,
            local_error: None,
            diagnostics: None,
            execution_path: ReplayExecutionPath::default(),
            comparison: Some(ComparisonResult {
                status_match: true,
                created_match: true,
                mutated_match: true,
                deleted_match: true,
                on_chain_status: "success".to_string(),
                local_status: "success".to_string(),
                notes: Vec::new(),
            }),
            analysis: None,
            effects: None,
            effects_full: None,
            commands_executed: 3,
            batch_summary_printed: false,
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"local_success\":true"));
        assert!(json.contains("\"status_match\":true"));
    }

    #[test]
    fn test_replay_cmd_explicit_bool_flags_parse() {
        let defaults = parse_replay_cmd(&["replay", "dummy-digest"]);
        assert!(defaults.hydration.allow_fallback);
        assert!(defaults.hydration.auto_system_objects);
        assert_eq!(defaults.profile, ReplayProfile::Balanced);

        let disabled = parse_replay_cmd(&[
            "replay",
            "dummy-digest",
            "--allow-fallback",
            "false",
            "--auto-system-objects",
            "false",
        ]);
        assert!(!disabled.hydration.allow_fallback);
        assert!(!disabled.hydration.auto_system_objects);

        let enabled = parse_replay_cmd(&[
            "replay",
            "dummy-digest",
            "--allow-fallback",
            "true",
            "--auto-system-objects",
            "true",
        ]);
        assert!(enabled.hydration.allow_fallback);
        assert!(enabled.hydration.auto_system_objects);

        let fast = parse_replay_cmd(&["replay", "dummy-digest", "--profile", "fast"]);
        assert_eq!(fast.profile, ReplayProfile::Fast);
    }
}
