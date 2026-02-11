use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Local;
use clap::{ArgAction, Args, ValueEnum};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command as TokioCommand;
use tokio::time::{Duration, Instant as TokioInstant};

use crate::sandbox_cli::SandboxState;

const KNOWN_MUTATORS: &[&str] = &[
    "baseline_vs_heal",
    "state_drop_required_object",
    "state_input_rewire",
    "state_object_version_skew",
    "state_shared_object_substitute",
    "state_pure_type_aware",
    "state_pure_signature_aware",
];

const KNOWN_ORACLES: &[&str] = &[
    "fail_to_heal",
    "forced_mutation_recovery",
    "timeout_resolution",
    "state_rehydration_success",
    "source_divergence",
];

const KNOWN_INVARIANTS: &[&str] = &[
    "commands_executed_gt_zero",
    "heal_not_timed_out",
    "baseline_failed_before_heal",
];

const KNOWN_SCORING: &[&str] = &["status-first", "recovery-priority", "balanced"];
const KNOWN_MINIMIZATION_MODES: &[&str] = &["state-diff", "operator-specific", "none"];

#[derive(Args, Debug, Clone)]
#[command(
    about = "Mutate replay inputs/state and re-run with automatic hydration",
    long_about = "Replay mutate executes replay mutation workflows over one or many targets.\n\n\
Supports single-target runs, fixture-driven deterministic runs, or target arrays from JSON.\n\
Provides native CLI orchestration for baseline/heal replay passes, state mutation, run scoring,\n\
and minimization with deterministic artifacts."
)]
pub struct ReplayMutateCmd {
    /// Single target digest
    #[arg(long, help_heading = "Targets")]
    pub digest: Option<String>,

    /// Checkpoint(s) for single-target mode. Accepts: single (239615926), range (100..200), list (100,105,110).
    #[arg(long, help_heading = "Targets")]
    pub checkpoint: Option<String>,

    /// Replay latest N checkpoints (live discovery mode)
    #[arg(long, help_heading = "Targets")]
    pub latest: Option<u64>,

    /// Load targets from JSON file (array, {targets:[...]}, {candidates:[...]}, or {discovered:[...]})
    #[arg(long, help_heading = "Targets")]
    pub targets_file: Option<PathBuf>,

    /// Deterministic fixture dataset JSON
    #[arg(long, help_heading = "Targets")]
    pub fixture: Option<PathBuf>,

    /// One-command guided deterministic demo mode
    #[arg(long, default_value_t = false, help_heading = "Mode")]
    pub demo: bool,

    /// No-op mode: parse inputs and emit planned targets without executing replay
    #[arg(long, default_value_t = false, help_heading = "Mode")]
    pub no_op: bool,

    /// Strategy config file (YAML/JSON)
    #[arg(long, help_heading = "Strategy")]
    pub strategy: Option<PathBuf>,

    /// Override mutator names (repeatable)
    #[arg(long = "mutator", value_name = "NAME", help_heading = "Strategy")]
    pub mutators: Vec<String>,

    /// Override oracle names (repeatable)
    #[arg(long = "oracle", value_name = "NAME", help_heading = "Strategy")]
    pub oracles: Vec<String>,

    /// Override invariant names (repeatable)
    #[arg(long = "invariant", value_name = "NAME", help_heading = "Strategy")]
    pub invariants: Vec<String>,

    /// Scoring strategy (status-first | recovery-priority | balanced)
    #[arg(long, help_heading = "Strategy")]
    pub scoring: Option<String>,

    /// Override minimization on/off
    #[arg(long, action = ArgAction::Set, help_heading = "Strategy")]
    pub minimize: Option<bool>,

    /// Disable minimization (equivalent to --minimize false)
    #[arg(long, default_value_t = false, help_heading = "Strategy")]
    pub no_minimize: bool,

    /// Minimization mode (state-diff | operator-specific | none)
    #[arg(long, help_heading = "Strategy")]
    pub minimization_mode: Option<String>,

    /// Max targets/transactions to test
    #[arg(long, default_value_t = 60, help_heading = "Execution")]
    pub max_transactions: usize,

    /// Per-replay timeout in seconds
    #[arg(long, default_value_t = 45, help_heading = "Execution")]
    pub replay_timeout: u64,

    /// Replay source adapter for baseline/heal runs
    #[arg(long, value_enum, default_value_t = ReplayMutateSource::Walrus, help_heading = "Execution")]
    pub replay_source: ReplayMutateSource,

    /// Max concurrent replay targets processed per batch
    #[arg(long, default_value_t = 1, help_heading = "Execution")]
    pub jobs: usize,

    /// Retry budget for transient replay execution failures/timeouts
    #[arg(long, default_value_t = 0, help_heading = "Execution")]
    pub retries: u32,

    /// If true, retries trigger only on timeout
    #[arg(long, action = ArgAction::Set, default_value_t = true, help_heading = "Execution")]
    pub retry_timeout_only: bool,

    /// Keep scanning targets/operators after the first fail->heal hit
    #[arg(long, default_value_t = false, help_heading = "Execution")]
    pub keep_going: bool,

    /// Optional secondary replay source for differential comparison
    #[arg(long, value_enum, help_heading = "Execution")]
    pub differential_source: Option<ReplayMutateSource>,

    /// Optional corpus input JSON (adds targets to this run)
    #[arg(long, help_heading = "Execution")]
    pub corpus_in: Option<PathBuf>,

    /// Optional corpus output JSON (writes/updates discovered interesting targets)
    #[arg(long, help_heading = "Execution")]
    pub corpus_out: Option<PathBuf>,

    /// Output root directory for mutate artifacts
    #[arg(
        long,
        default_value = "examples/out/replay_mutation_lab",
        help_heading = "Execution"
    )]
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub digest: String,
    pub checkpoint: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReplayMutateSource {
    Walrus,
    Grpc,
    Hybrid,
}

impl ReplayMutateSource {
    fn as_cli_value(self) -> &'static str {
        match self {
            Self::Walrus => "walrus",
            Self::Grpc => "grpc",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimizationConfig {
    pub enabled: bool,
    pub mode: String,
}

impl Default for MinimizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "state-diff".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub name: String,
    pub mutators: Vec<String>,
    pub oracles: Vec<String>,
    pub invariants: Vec<String>,
    pub scoring: String,
    pub minimization: MinimizationConfig,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            mutators: vec![
                "state_drop_required_object".to_string(),
                "state_input_rewire".to_string(),
                "state_object_version_skew".to_string(),
                "state_shared_object_substitute".to_string(),
                "state_pure_type_aware".to_string(),
                "state_pure_signature_aware".to_string(),
                "baseline_vs_heal".to_string(),
            ],
            oracles: vec![
                "fail_to_heal".to_string(),
                "forced_mutation_recovery".to_string(),
                "state_rehydration_success".to_string(),
                "source_divergence".to_string(),
            ],
            invariants: vec![
                "commands_executed_gt_zero".to_string(),
                "heal_not_timed_out".to_string(),
                "baseline_failed_before_heal".to_string(),
            ],
            scoring: "status-first".to_string(),
            minimization: MinimizationConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct StrategyFile {
    name: Option<String>,
    mutators: Option<Vec<String>>,
    oracles: Option<Vec<String>>,
    checks: Option<Vec<String>>,
    invariants: Option<Vec<String>>,
    scoring: Option<String>,
    minimization: Option<StrategyFileMinimization>,
}

#[derive(Debug, Clone, Deserialize)]
struct StrategyFileMinimization {
    enabled: Option<bool>,
    mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MutationPlan {
    pub mode: String,
    pub strategy: String,
    pub mutators: Vec<String>,
    pub rehydrate: String,
    pub timeout_secs: u64,
    pub replay_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub differential_source: Option<String>,
    pub jobs: usize,
    pub retries: u32,
    pub retry_timeout_only: bool,
    pub keep_going: bool,
    pub minimization_enabled: bool,
    pub minimization_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OraclePlan {
    pub checks: Vec<String>,
    pub invariants: Vec<String>,
    pub minimization: bool,
    pub scoring: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MinimizationResult {
    pub mode: String,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimizer: Option<String>,
    pub seed_objects: usize,
    pub broken_objects: usize,
    pub removed_objects: Vec<String>,
    pub added_objects: Vec<String>,
    pub changed_objects: Vec<String>,
    pub minimal_delta: Vec<String>,
    pub minimized_from: usize,
    pub minimized_to: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_state_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broken_state_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    pub target: Target,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub oracle_hits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub invariant_violations: Vec<String>,
    pub score: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimization: Option<MinimizationResult>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub fingerprint: String,
    pub summary: String,
    pub target: Target,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplayMutateReport {
    pub status: String,
    pub mode: String,
    pub out_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy_file: Option<String>,
    pub strategy: StrategyConfig,
    pub mutation_plan: MutationPlan,
    pub oracle_plan: OraclePlan,
    pub targets: Vec<Target>,
    pub run_records: Vec<RunRecord>,
    pub findings: Vec<Finding>,
    pub elapsed_ms: u64,
}

#[derive(Debug)]
struct NativeLabOutcome {
    run_dir: PathBuf,
    report: Value,
    elapsed_ms: u64,
}

#[derive(Debug, Clone)]
struct ReplayAttemptRaw {
    mode: &'static str,
    cmd: Vec<String>,
    exit_code: Option<i32>,
    elapsed_ms: u64,
    stdout: String,
    stderr: String,
    parsed: Option<Value>,
    parse_error: Option<String>,
    timed_out: bool,
}

#[derive(Debug)]
struct CandidateExecution {
    index: usize,
    target: Target,
    baseline: ReplayAttemptRaw,
    heal: ReplayAttemptRaw,
    differential_heal: Option<ReplayAttemptRaw>,
    attempt: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CorpusEntry {
    digest: String,
    checkpoint: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operator: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    findings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplayMutateCorpus {
    version: u32,
    generated_at: String,
    entries: Vec<CorpusEntry>,
}

trait ReplaySourceAdapter {
    fn source_name(&self) -> &'static str;
    fn replay_args(&self, target: &Target) -> Vec<String>;
    fn export_state_args(&self, digest: &str, checkpoint: u64, output_path: &Path) -> Vec<String>;
}

#[derive(Debug, Clone, Copy)]
struct DefaultReplaySourceAdapter {
    source: ReplayMutateSource,
}

impl ReplaySourceAdapter for DefaultReplaySourceAdapter {
    fn source_name(&self) -> &'static str {
        self.source.as_cli_value()
    }

    fn replay_args(&self, target: &Target) -> Vec<String> {
        let mut args = vec![
            "replay".to_string(),
            target.digest.clone(),
            "--source".to_string(),
            self.source_name().to_string(),
        ];
        if matches!(
            self.source,
            ReplayMutateSource::Walrus | ReplayMutateSource::Hybrid
        ) {
            args.extend(["--checkpoint".to_string(), target.checkpoint.to_string()]);
        }
        args.extend(["--compare".to_string(), "--json".to_string()]);
        args
    }

    fn export_state_args(&self, digest: &str, checkpoint: u64, output_path: &Path) -> Vec<String> {
        let mut args = vec![
            "replay".to_string(),
            digest.to_string(),
            "--source".to_string(),
            self.source_name().to_string(),
        ];
        if matches!(
            self.source,
            ReplayMutateSource::Walrus | ReplayMutateSource::Hybrid
        ) {
            args.extend(["--checkpoint".to_string(), checkpoint.to_string()]);
        }
        args.extend([
            "--compare".to_string(),
            "--export-state".to_string(),
            output_path.display().to_string(),
            "--json".to_string(),
        ]);
        args
    }
}

trait MutationOperator: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value>;
}

#[derive(Debug)]
struct StateDropRequiredObjectMutationOperator;

impl MutationOperator for StateDropRequiredObjectMutationOperator {
    fn name(&self) -> &'static str {
        "state_drop_required_object"
    }

    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value> {
        mutate_state_drop_required_object(seed_state_path, output_state_path)
    }
}

static STATE_DROP_REQUIRED_OBJECT_OPERATOR: StateDropRequiredObjectMutationOperator =
    StateDropRequiredObjectMutationOperator;

#[derive(Debug)]
struct StateInputRewireMutationOperator;

impl MutationOperator for StateInputRewireMutationOperator {
    fn name(&self) -> &'static str {
        "state_input_rewire"
    }

    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value> {
        mutate_state_input_rewire(seed_state_path, output_state_path)
    }
}

#[derive(Debug)]
struct StateObjectVersionSkewMutationOperator;

impl MutationOperator for StateObjectVersionSkewMutationOperator {
    fn name(&self) -> &'static str {
        "state_object_version_skew"
    }

    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value> {
        mutate_state_object_version_skew(seed_state_path, output_state_path)
    }
}

#[derive(Debug)]
struct StateSharedObjectSubstituteMutationOperator;

impl MutationOperator for StateSharedObjectSubstituteMutationOperator {
    fn name(&self) -> &'static str {
        "state_shared_object_substitute"
    }

    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value> {
        mutate_state_shared_object_substitute(seed_state_path, output_state_path)
    }
}

#[derive(Debug)]
struct StatePureTypeAwareMutationOperator;

impl MutationOperator for StatePureTypeAwareMutationOperator {
    fn name(&self) -> &'static str {
        "state_pure_type_aware"
    }

    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value> {
        mutate_state_pure_type_aware(seed_state_path, output_state_path)
    }
}

#[derive(Debug)]
struct StatePureSignatureAwareMutationOperator;

impl MutationOperator for StatePureSignatureAwareMutationOperator {
    fn name(&self) -> &'static str {
        "state_pure_signature_aware"
    }

    fn apply(&self, seed_state_path: &Path, output_state_path: &Path) -> Result<Value> {
        mutate_state_pure_signature_aware(seed_state_path, output_state_path)
    }
}

static STATE_INPUT_REWIRE_OPERATOR: StateInputRewireMutationOperator =
    StateInputRewireMutationOperator;
static STATE_OBJECT_VERSION_SKEW_OPERATOR: StateObjectVersionSkewMutationOperator =
    StateObjectVersionSkewMutationOperator;
static STATE_SHARED_OBJECT_SUBSTITUTE_OPERATOR: StateSharedObjectSubstituteMutationOperator =
    StateSharedObjectSubstituteMutationOperator;
static STATE_PURE_TYPE_AWARE_OPERATOR: StatePureTypeAwareMutationOperator =
    StatePureTypeAwareMutationOperator;
static STATE_PURE_SIGNATURE_AWARE_OPERATOR: StatePureSignatureAwareMutationOperator =
    StatePureSignatureAwareMutationOperator;

fn mutation_operator_registry() -> Vec<&'static dyn MutationOperator> {
    vec![
        &STATE_DROP_REQUIRED_OBJECT_OPERATOR,
        &STATE_INPUT_REWIRE_OPERATOR,
        &STATE_OBJECT_VERSION_SKEW_OPERATOR,
        &STATE_SHARED_OBJECT_SUBSTITUTE_OPERATOR,
        &STATE_PURE_TYPE_AWARE_OPERATOR,
        &STATE_PURE_SIGNATURE_AWARE_OPERATOR,
    ]
}

fn select_mutation_operators(strategy: &StrategyConfig) -> Vec<&'static dyn MutationOperator> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let registry = mutation_operator_registry();
    for name in &strategy.mutators {
        if let Some(operator) = registry.iter().find(|op| op.name() == name.as_str()) {
            if seen.insert(operator.name()) {
                selected.push(*operator);
            }
        }
    }
    if selected.is_empty() {
        selected.push(&STATE_DROP_REQUIRED_OBJECT_OPERATOR);
    }
    selected
}

struct EvaluationView<'a> {
    source: &'a str,
    baseline_success: bool,
    heal_success: bool,
    differential_heal_success: Option<bool>,
    heal_error: Option<&'a str>,
    differential_heal_error: Option<&'a str>,
    baseline_timed_out: bool,
    heal_timed_out: bool,
    heal_commands_executed: u64,
}

impl<'a> EvaluationView<'a> {
    fn from_record(record: &'a RunRecord) -> Self {
        let chosen = record.chosen.as_ref();
        Self {
            source: chosen
                .and_then(|c| c.get("source"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            baseline_success: chosen
                .and_then(|c| c.pointer("/baseline/local_success"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            heal_success: chosen
                .and_then(|c| c.pointer("/heal/local_success"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            differential_heal_success: chosen
                .and_then(|c| c.pointer("/differential/heal/local_success"))
                .and_then(Value::as_bool),
            heal_error: chosen
                .and_then(|c| c.pointer("/heal/local_error"))
                .and_then(Value::as_str),
            differential_heal_error: chosen
                .and_then(|c| c.pointer("/differential/heal/local_error"))
                .and_then(Value::as_str),
            baseline_timed_out: chosen
                .and_then(|c| c.pointer("/baseline/timed_out"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            heal_timed_out: chosen
                .and_then(|c| c.pointer("/heal/timed_out"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            heal_commands_executed: chosen
                .and_then(|c| c.pointer("/heal/commands_executed"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }
    }
}

trait OracleRule: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(&self, view: &EvaluationView<'_>) -> bool;
}

trait InvariantRule: Send + Sync {
    fn name(&self) -> &'static str;
    fn violated(&self, view: &EvaluationView<'_>) -> bool;
}

#[derive(Debug)]
struct FailToHealOracle;
#[derive(Debug)]
struct ForcedMutationRecoveryOracle;
#[derive(Debug)]
struct TimeoutResolutionOracle;
#[derive(Debug)]
struct StateRehydrationSuccessOracle;
#[derive(Debug)]
struct SourceDivergenceOracle;

impl OracleRule for FailToHealOracle {
    fn name(&self) -> &'static str {
        "fail_to_heal"
    }
    fn evaluate(&self, view: &EvaluationView<'_>) -> bool {
        !view.baseline_success && view.heal_success
    }
}

impl OracleRule for ForcedMutationRecoveryOracle {
    fn name(&self) -> &'static str {
        "forced_mutation_recovery"
    }
    fn evaluate(&self, view: &EvaluationView<'_>) -> bool {
        view.source == "forced_mutation"
    }
}

impl OracleRule for TimeoutResolutionOracle {
    fn name(&self) -> &'static str {
        "timeout_resolution"
    }
    fn evaluate(&self, view: &EvaluationView<'_>) -> bool {
        view.baseline_timed_out && !view.heal_timed_out
    }
}

impl OracleRule for StateRehydrationSuccessOracle {
    fn name(&self) -> &'static str {
        "state_rehydration_success"
    }
    fn evaluate(&self, view: &EvaluationView<'_>) -> bool {
        !view.baseline_success && view.heal_success
    }
}

impl OracleRule for SourceDivergenceOracle {
    fn name(&self) -> &'static str {
        "source_divergence"
    }
    fn evaluate(&self, view: &EvaluationView<'_>) -> bool {
        match view.differential_heal_success {
            Some(diff_success) => {
                if diff_success != view.heal_success {
                    return true;
                }
                view.heal_error.unwrap_or_default()
                    != view.differential_heal_error.unwrap_or_default()
            }
            None => false,
        }
    }
}

#[derive(Debug)]
struct CommandsExecutedGtZeroInvariant;
#[derive(Debug)]
struct HealNotTimedOutInvariant;
#[derive(Debug)]
struct BaselineFailedBeforeHealInvariant;

impl InvariantRule for CommandsExecutedGtZeroInvariant {
    fn name(&self) -> &'static str {
        "commands_executed_gt_zero"
    }
    fn violated(&self, view: &EvaluationView<'_>) -> bool {
        view.heal_success && view.heal_commands_executed == 0
    }
}

impl InvariantRule for HealNotTimedOutInvariant {
    fn name(&self) -> &'static str {
        "heal_not_timed_out"
    }
    fn violated(&self, view: &EvaluationView<'_>) -> bool {
        view.heal_success && view.heal_timed_out
    }
}

impl InvariantRule for BaselineFailedBeforeHealInvariant {
    fn name(&self) -> &'static str {
        "baseline_failed_before_heal"
    }
    fn violated(&self, view: &EvaluationView<'_>) -> bool {
        view.heal_success && view.baseline_success
    }
}

static FAIL_TO_HEAL_ORACLE: FailToHealOracle = FailToHealOracle;
static FORCED_MUTATION_RECOVERY_ORACLE: ForcedMutationRecoveryOracle = ForcedMutationRecoveryOracle;
static TIMEOUT_RESOLUTION_ORACLE: TimeoutResolutionOracle = TimeoutResolutionOracle;
static STATE_REHYDRATION_SUCCESS_ORACLE: StateRehydrationSuccessOracle =
    StateRehydrationSuccessOracle;
static SOURCE_DIVERGENCE_ORACLE: SourceDivergenceOracle = SourceDivergenceOracle;

static COMMANDS_EXECUTED_GT_ZERO_INVARIANT: CommandsExecutedGtZeroInvariant =
    CommandsExecutedGtZeroInvariant;
static HEAL_NOT_TIMED_OUT_INVARIANT: HealNotTimedOutInvariant = HealNotTimedOutInvariant;
static BASELINE_FAILED_BEFORE_HEAL_INVARIANT: BaselineFailedBeforeHealInvariant =
    BaselineFailedBeforeHealInvariant;

fn oracle_rule_registry() -> Vec<&'static dyn OracleRule> {
    vec![
        &FAIL_TO_HEAL_ORACLE,
        &FORCED_MUTATION_RECOVERY_ORACLE,
        &TIMEOUT_RESOLUTION_ORACLE,
        &STATE_REHYDRATION_SUCCESS_ORACLE,
        &SOURCE_DIVERGENCE_ORACLE,
    ]
}

fn invariant_rule_registry() -> Vec<&'static dyn InvariantRule> {
    vec![
        &COMMANDS_EXECUTED_GT_ZERO_INVARIANT,
        &HEAL_NOT_TIMED_OUT_INVARIANT,
        &BASELINE_FAILED_BEFORE_HEAL_INVARIANT,
    ]
}

impl ReplayMutateCmd {
    pub async fn execute(
        &self,
        _state: &mut SandboxState,
        json_output: bool,
        _verbose: bool,
    ) -> Result<()> {
        self.validate_inputs()?;
        let strategy = self.resolve_strategy()?;

        let started = Instant::now();
        let (mut targets, discover_meta, mode) = self.resolve_targets_and_meta().await?;
        if let Some(path) = &self.corpus_in {
            let mut corpus_targets = load_corpus_targets(path)?;
            targets.append(&mut corpus_targets);
            targets = dedup_targets(targets);
        }
        if targets.len() > self.max_transactions {
            targets.truncate(self.max_transactions);
        }

        let mutation_plan = MutationPlan {
            mode: mode.clone(),
            strategy: strategy.name.clone(),
            mutators: strategy.mutators.clone(),
            rehydrate: "full + synthesize_missing + self_heal_dynamic_fields".to_string(),
            timeout_secs: self.replay_timeout,
            replay_source: self.replay_source.as_cli_value().to_string(),
            differential_source: self
                .differential_source
                .map(|s| s.as_cli_value().to_string()),
            jobs: self.jobs,
            retries: self.retries,
            retry_timeout_only: self.retry_timeout_only,
            keep_going: self.keep_going,
            minimization_enabled: strategy.minimization.enabled,
            minimization_mode: strategy.minimization.mode.clone(),
        };
        let oracle_plan = OraclePlan {
            checks: strategy.oracles.clone(),
            invariants: strategy.invariants.clone(),
            minimization: strategy.minimization.enabled,
            scoring: strategy.scoring.clone(),
        };

        let mut run_records = Vec::new();
        let mut findings = Vec::new();

        if !self.no_op {
            if self.targets_file.is_some() {
                for (idx, target) in targets.iter().enumerate() {
                    let target_out = self.out_dir.join(format!(
                        "target_{:03}_{}",
                        idx + 1,
                        short_digest(&target.digest)
                    ));
                    let outcome = self
                        .execute_native_lab(
                            vec![target.clone()],
                            serde_json::json!({
                                "source": "targets-file",
                                "discovered": [{"digest": target.digest, "checkpoint": target.checkpoint}],
                            }),
                            &target_out,
                            &strategy,
                        )
                        .await?;
                    if let Some(mut record) = self.record_from_report(
                        target,
                        &outcome.report,
                        Some(&outcome.run_dir),
                        outcome.elapsed_ms,
                    ) {
                        if let Err(err) =
                            self.decorate_record(&mut record, &strategy, Some(&outcome.run_dir))
                        {
                            eprintln!(
                                "[replay-mutate] warning: failed to decorate run record: {err}"
                            );
                        }
                        findings.extend(self.findings_from_record(&record));
                        run_records.push(record);
                    }
                }
            } else {
                let outcome = self
                    .execute_native_lab(
                        targets.clone(),
                        discover_meta.clone(),
                        &self.out_dir,
                        &strategy,
                    )
                    .await?;
                let target = targets.first().cloned().unwrap_or(Target {
                    digest: "*".to_string(),
                    checkpoint: 0,
                    source: Some("live".to_string()),
                    label: None,
                });
                if let Some(mut record) = self.record_from_report(
                    &target,
                    &outcome.report,
                    Some(&outcome.run_dir),
                    outcome.elapsed_ms,
                ) {
                    if let Err(err) =
                        self.decorate_record(&mut record, &strategy, Some(&outcome.run_dir))
                    {
                        eprintln!("[replay-mutate] warning: failed to decorate run record: {err}");
                    }
                    findings.extend(self.findings_from_record(&record));
                    run_records.push(record);
                }
            }
        }

        let report = ReplayMutateReport {
            status: if self.no_op {
                "no_op".to_string()
            } else if run_records.is_empty() {
                "completed_no_records".to_string()
            } else {
                "completed".to_string()
            },
            mode,
            out_dir: self.out_dir.display().to_string(),
            strategy_file: self.strategy.as_ref().map(|p| p.display().to_string()),
            strategy,
            mutation_plan,
            oracle_plan,
            targets,
            run_records,
            findings,
            elapsed_ms: started.elapsed().as_millis() as u64,
        };

        fs::create_dir_all(&self.out_dir).with_context(|| {
            format!(
                "failed to create replay mutate output dir: {}",
                self.out_dir.display()
            )
        })?;
        let report_path = self.out_dir.join("replay_mutate_report.json");
        fs::write(&report_path, serde_json::to_string_pretty(&report)?).with_context(|| {
            format!(
                "failed to write replay mutate report: {}",
                report_path.display()
            )
        })?;

        if let Some(corpus_path) = &self.corpus_out {
            let corpus = build_corpus_from_report(&report);
            write_replay_mutate_corpus(corpus_path, &corpus)?;
        }

        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            let mut text = String::new();
            let _ = writeln!(&mut text, "Replay mutate complete");
            let _ = writeln!(&mut text, "  mode: {}", report.mode);
            let _ = writeln!(&mut text, "  strategy: {}", report.mutation_plan.strategy);
            let _ = writeln!(&mut text, "  targets: {}", report.targets.len());
            let _ = writeln!(&mut text, "  run records: {}", report.run_records.len());
            let _ = writeln!(&mut text, "  findings: {}", report.findings.len());
            let _ = writeln!(&mut text, "  report: {}", report_path.display());
            print!("{text}");
        }

        Ok(())
    }

    fn validate_inputs(&self) -> Result<()> {
        if self.max_transactions == 0 {
            return Err(anyhow!("--max-transactions must be >= 1"));
        }
        if self.replay_timeout < 5 {
            return Err(anyhow!("--replay-timeout must be >= 5"));
        }
        if self.jobs == 0 {
            return Err(anyhow!("--jobs must be >= 1"));
        }
        if self.no_minimize && matches!(self.minimize, Some(true)) {
            return Err(anyhow!(
                "--no-minimize cannot be combined with --minimize true"
            ));
        }
        if self.latest.is_some() && self.replay_source != ReplayMutateSource::Walrus {
            return Err(anyhow!(
                "--latest discovery currently requires --replay-source walrus"
            ));
        }
        if let Some(diff) = self.differential_source {
            if diff == self.replay_source {
                return Err(anyhow!(
                    "--differential-source must differ from --replay-source"
                ));
            }
        }
        if let Some(path) = &self.corpus_in {
            if !path.exists() {
                return Err(anyhow!("corpus file not found: {}", path.display()));
            }
        }

        if let Some(path) = &self.strategy {
            if !path.exists() {
                return Err(anyhow!("strategy file not found: {}", path.display()));
            }
        }

        if self.demo {
            if self.digest.is_some()
                || self.checkpoint.is_some()
                || self.latest.is_some()
                || self.targets_file.is_some()
            {
                return Err(anyhow!(
                    "--demo cannot be combined with --digest/--checkpoint/--latest/--targets-file"
                ));
            }
            return Ok(());
        }

        if self.targets_file.is_some()
            && (self.digest.is_some() || self.checkpoint.is_some() || self.latest.is_some())
        {
            return Err(anyhow!(
                "--targets-file cannot be combined with --digest/--checkpoint/--latest"
            ));
        }

        if self.fixture.is_some()
            && (self.digest.is_some() || self.checkpoint.is_some() || self.latest.is_some())
        {
            return Err(anyhow!(
                "--fixture cannot be combined with --digest/--checkpoint/--latest"
            ));
        }

        if self.digest.is_some() && self.checkpoint.is_none() {
            return Err(anyhow!(
                "--checkpoint is required when --digest is provided"
            ));
        }
        if self.checkpoint.is_some() && self.digest.is_none() {
            return Err(anyhow!(
                "--digest is required when --checkpoint is provided"
            ));
        }

        Ok(())
    }

    async fn resolve_targets_and_meta(&self) -> Result<(Vec<Target>, Value, String)> {
        if self.demo {
            let fixture = self
                .fixture
                .clone()
                .unwrap_or_else(|| PathBuf::from("examples/data/replay_mutation_fixture_v1.json"));
            let targets = load_targets_from_json(&fixture)?;
            return Ok((
                targets.clone(),
                serde_json::json!({
                    "source": "demo_fixture",
                    "fixture_path": fixture.display().to_string(),
                    "discovered": discovered_entries(&targets),
                }),
                "guided-demo".to_string(),
            ));
        }
        if let Some(path) = &self.targets_file {
            let targets = load_targets_from_json(path)?;
            return Ok((
                targets.clone(),
                serde_json::json!({
                    "source": "targets-file",
                    "targets_file": path.display().to_string(),
                    "discovered": discovered_entries(&targets),
                }),
                "targets-file".to_string(),
            ));
        }
        if let Some(path) = &self.fixture {
            let targets = load_targets_from_json(path)?;
            return Ok((
                targets.clone(),
                serde_json::json!({
                    "source": "fixture",
                    "fixture_path": path.display().to_string(),
                    "discovered": discovered_entries(&targets),
                }),
                "fixture".to_string(),
            ));
        }
        if let (Some(digest), Some(checkpoint_spec)) = (&self.digest, &self.checkpoint) {
            let checkpoints = parse_checkpoint_spec(checkpoint_spec)?;
            let targets: Vec<Target> = checkpoints
                .into_iter()
                .map(|cp| Target {
                    digest: digest.clone(),
                    checkpoint: cp,
                    source: Some("explicit".to_string()),
                    label: None,
                })
                .collect();
            return Ok((
                targets.clone(),
                serde_json::json!({
                    "source": "pinned",
                    "pinned": true,
                    "discovered": discovered_entries(&targets),
                }),
                "single".to_string(),
            ));
        }
        if let Some(latest_window) = self.latest {
            let (targets, latest_checkpoint, start_checkpoint) = self
                .discover_latest_targets(latest_window, self.max_transactions)
                .await?;
            return Ok((
                targets.clone(),
                serde_json::json!({
                    "source": "walrus_latest_window",
                    "latest_checkpoint": latest_checkpoint,
                    "start_checkpoint": start_checkpoint,
                    "discovered": discovered_entries(&targets),
                }),
                "live".to_string(),
            ));
        }
        Err(anyhow!(
            "missing target input: use one of --digest+--checkpoint, --targets-file, --fixture, --latest, or --demo"
        ))
    }

    #[cfg(feature = "walrus")]
    async fn discover_latest_targets(
        &self,
        latest_window: u64,
        max_tx: usize,
    ) -> Result<(Vec<Target>, u64, u64)> {
        use sui_transport::walrus::WalrusClient;

        let latest_checkpoint =
            tokio::task::spawn_blocking(|| WalrusClient::mainnet().get_latest_checkpoint())
                .await
                .context("Walrus latest checkpoint task panicked")?
                .context("failed to fetch latest Walrus checkpoint")?;
        let start_checkpoint = latest_checkpoint.saturating_sub(latest_window.saturating_sub(1));
        let mut targets = Vec::new();
        for cp in start_checkpoint..=latest_checkpoint {
            if targets.len() >= max_tx {
                break;
            }
            let checkpoint_data = tokio::task::spawn_blocking(move || {
                let walrus = WalrusClient::mainnet();
                walrus.get_checkpoint(cp)
            })
            .await
            .context("Walrus checkpoint fetch task panicked")?
            .with_context(|| format!("failed to fetch Walrus checkpoint {cp}"))?;
            for tx in checkpoint_data.transactions {
                if targets.len() >= max_tx {
                    break;
                }
                targets.push(Target {
                    digest: tx.transaction.digest().to_string(),
                    checkpoint: cp,
                    source: Some("walrus_latest_window".to_string()),
                    label: None,
                });
            }
        }
        if targets.is_empty() {
            return Err(anyhow!(
                "no candidate transactions discovered from latest {} checkpoint(s)",
                latest_window
            ));
        }
        Ok((targets, latest_checkpoint, start_checkpoint))
    }

    #[cfg(not(feature = "walrus"))]
    async fn discover_latest_targets(
        &self,
        _latest_window: u64,
        _max_tx: usize,
    ) -> Result<(Vec<Target>, u64, u64)> {
        Err(anyhow!(
            "latest target discovery requires walrus feature support"
        ))
    }

    fn resolve_strategy(&self) -> Result<StrategyConfig> {
        let mut strategy = StrategyConfig::default();

        if let Some(path) = &self.strategy {
            let file = load_strategy_file(path)?;
            if let Some(name) = file.name {
                strategy.name = name;
            }
            if let Some(mutators) = file.mutators {
                strategy.mutators = mutators;
            }
            if let Some(oracles) = file.oracles.or(file.checks) {
                strategy.oracles = oracles;
            }
            if let Some(invariants) = file.invariants {
                strategy.invariants = invariants;
            }
            if let Some(scoring) = file.scoring {
                strategy.scoring = scoring;
            }
            if let Some(min) = file.minimization {
                if let Some(enabled) = min.enabled {
                    strategy.minimization.enabled = enabled;
                }
                if let Some(mode) = min.mode {
                    strategy.minimization.mode = mode;
                }
            }
            if strategy.name == "default" {
                strategy.name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("default")
                    .to_string();
            }
        }

        if !self.mutators.is_empty() {
            strategy.mutators = self.mutators.clone();
        }
        if !self.oracles.is_empty() {
            strategy.oracles = self.oracles.clone();
        }
        if !self.invariants.is_empty() {
            strategy.invariants = self.invariants.clone();
        }
        if let Some(scoring) = &self.scoring {
            strategy.scoring = scoring.clone();
        }
        if let Some(mode) = &self.minimization_mode {
            strategy.minimization.mode = mode.clone();
        }
        if self.no_minimize {
            strategy.minimization.enabled = false;
        }
        if let Some(minimize) = self.minimize {
            strategy.minimization.enabled = minimize;
        }

        strategy.mutators = dedup_preserve(strategy.mutators);
        strategy.oracles = dedup_preserve(strategy.oracles);
        strategy.invariants = dedup_preserve(strategy.invariants);

        validate_strategy_config(&strategy)?;
        Ok(strategy)
    }

    async fn execute_native_lab(
        &self,
        targets: Vec<Target>,
        discover_meta: Value,
        out_root: &Path,
        strategy: &StrategyConfig,
    ) -> Result<NativeLabOutcome> {
        let started = Instant::now();
        let run_dir = self.create_run_dir(out_root)?;

        let candidate_pool_path = run_dir.join("candidate_pool.json");
        write_json_file(&candidate_pool_path, &discover_meta)?;

        let mut attempts = Vec::new();
        let mut chosen: Option<Value> = None;

        if targets.is_empty() {
            let report = serde_json::json!({
                "status": "no_candidates",
                "message": "No candidate transactions discovered from target inputs.",
            });
            write_json_file(&run_dir.join("attempts.json"), &attempts)?;
            write_json_file(&run_dir.join("report.json"), &report)?;
            self.write_run_readme(&run_dir, &report, &attempts)?;
            return Ok(NativeLabOutcome {
                run_dir,
                report,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }

        let mut cursor = 0usize;
        while cursor < targets.len() {
            let end = std::cmp::min(cursor + self.jobs, targets.len());
            let batch: Vec<(usize, Target)> = targets[cursor..end]
                .iter()
                .cloned()
                .enumerate()
                .map(|(offset, target)| (cursor + offset + 1, target))
                .collect();

            let batch_results = stream::iter(batch.into_iter().map(|(index, target)| async move {
                self.run_candidate_execution(index, target).await
            }))
            .buffer_unordered(self.jobs)
            .collect::<Vec<Result<CandidateExecution>>>()
            .await;

            let mut executions = Vec::new();
            for result in batch_results {
                executions.push(result?);
            }
            executions.sort_by_key(|exec| exec.index);

            for execution in executions {
                self.write_attempt_artifacts(&run_dir, &execution)?;
                attempts.push(execution.attempt.clone());
                if self.attempt_is_fail_then_heal(&execution.attempt) {
                    if chosen.is_none() {
                        chosen = Some(execution.attempt.clone());
                    }
                    if !self.keep_going {
                        break;
                    }
                }
            }
            cursor = end;
            if chosen.is_some() && !self.keep_going {
                break;
            }
        }

        write_json_file(&run_dir.join("attempts.json"), &attempts)?;

        let mut report = serde_json::json!({
            "status": if chosen.is_some() {
                "found_fail_then_heal"
            } else {
                "no_fail_then_heal_found"
            },
            "candidate_source": discover_meta.get("source").cloned().unwrap_or(Value::String("unknown".to_string())),
            "tested": attempts.len(),
            "chosen": chosen.clone(),
        });

        if let Some(chosen_case) = chosen.as_ref() {
            if let (Some(digest), Some(checkpoint)) = (
                chosen_case.get("digest").and_then(Value::as_str),
                chosen_case.get("checkpoint").and_then(Value::as_u64),
            ) {
                let export_path = run_dir.join("winning_state.json");
                let export = self
                    .run_replay_export_state(digest, checkpoint, &export_path)
                    .await?;
                if let Some(obj) = report.as_object_mut() {
                    obj.insert(
                        "winning_state_export".to_string(),
                        serde_json::json!({
                            "path": export_path.display().to_string(),
                            "exit_code": export.exit_code,
                        }),
                    );
                }
            }
        }

        if chosen.is_none() && !attempts.is_empty() {
            self.run_forced_mutation_demo(&run_dir, &attempts, &mut report, strategy)
                .await?;
        }

        write_json_file(&run_dir.join("report.json"), &report)?;
        self.write_run_readme(&run_dir, &report, &attempts)?;

        Ok(NativeLabOutcome {
            run_dir,
            report,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    fn create_run_dir(&self, out_root: &Path) -> Result<PathBuf> {
        let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
        let run_dir = out_root.join(format!("run_{}", stamp));
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("failed to create run dir: {}", run_dir.display()))?;
        Ok(run_dir)
    }

    fn source_adapter(&self) -> DefaultReplaySourceAdapter {
        DefaultReplaySourceAdapter {
            source: self.replay_source,
        }
    }

    fn adapter_for_source(&self, source: ReplayMutateSource) -> DefaultReplaySourceAdapter {
        DefaultReplaySourceAdapter { source }
    }

    fn sandbox_bin(&self) -> Result<PathBuf> {
        if let Ok(path) = std::env::var("SUI_SANDBOX_BIN") {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            return Ok(exe);
        }
        Err(anyhow!(
            "unable to resolve sui-sandbox binary path for replay mutate orchestration"
        ))
    }

    async fn run_replay_mode(
        &self,
        target: &Target,
        mode: &'static str,
    ) -> Result<ReplayAttemptRaw> {
        self.run_replay_mode_with_adapter(target, mode, self.source_adapter())
            .await
    }

    async fn run_replay_mode_with_adapter(
        &self,
        target: &Target,
        mode: &'static str,
        adapter: DefaultReplaySourceAdapter,
    ) -> Result<ReplayAttemptRaw> {
        let mut args = adapter.replay_args(target);
        match mode {
            "baseline" => {
                args.extend([
                    "--fetch-strategy".to_string(),
                    "eager".to_string(),
                    "--no-prefetch".to_string(),
                    "--allow-fallback".to_string(),
                    "false".to_string(),
                ]);
            }
            "heal" => {
                args.extend([
                    "--fetch-strategy".to_string(),
                    "full".to_string(),
                    "--allow-fallback".to_string(),
                    "true".to_string(),
                    "--synthesize-missing".to_string(),
                    "--self-heal-dynamic-fields".to_string(),
                ]);
            }
            _ => return Err(anyhow!("unsupported replay mode: {mode}")),
        }
        self.run_command_json(mode, args).await
    }

    async fn run_replay_mode_with_retry(
        &self,
        target: &Target,
        mode: &'static str,
    ) -> Result<ReplayAttemptRaw> {
        let mut run = self.run_replay_mode(target, mode).await?;
        if self.retries == 0 {
            return Ok(run);
        }

        let mut attempt = 0u32;
        while attempt < self.retries && self.should_retry_attempt(&run) {
            attempt += 1;
            run = self.run_replay_mode(target, mode).await?;
        }
        Ok(run)
    }

    async fn run_replay_mode_with_retry_source(
        &self,
        target: &Target,
        mode: &'static str,
        source: ReplayMutateSource,
    ) -> Result<ReplayAttemptRaw> {
        let adapter = self.adapter_for_source(source);
        let mut run = self
            .run_replay_mode_with_adapter(target, mode, adapter)
            .await?;
        if self.retries == 0 {
            return Ok(run);
        }
        let mut attempt = 0u32;
        while attempt < self.retries && self.should_retry_attempt(&run) {
            attempt += 1;
            run = self
                .run_replay_mode_with_adapter(target, mode, adapter)
                .await?;
        }
        Ok(run)
    }

    fn should_retry_attempt(&self, run: &ReplayAttemptRaw) -> bool {
        if self.retry_timeout_only {
            return run.timed_out;
        }
        run.timed_out || run.parse_error.is_some() || run.exit_code.is_none()
    }

    async fn run_candidate_execution(
        &self,
        index: usize,
        target: Target,
    ) -> Result<CandidateExecution> {
        let baseline = self.run_replay_mode_with_retry(&target, "baseline").await?;
        let heal = self.run_replay_mode_with_retry(&target, "heal").await?;
        let differential_heal = if let Some(source) = self.differential_source {
            Some(
                self.run_replay_mode_with_retry_source(&target, "heal", source)
                    .await?,
            )
        } else {
            None
        };
        let baseline_summary = summarize_attempt(&baseline);
        let heal_summary = summarize_attempt(&heal);
        let differential_summary = differential_heal.as_ref().map(summarize_attempt);
        let attempt = serde_json::json!({
            "index": index,
            "digest": target.digest,
            "checkpoint": target.checkpoint,
            "baseline": baseline_summary,
            "heal": heal_summary,
            "differential": differential_summary.as_ref().map(|summary| {
                serde_json::json!({
                    "source": self.differential_source.map(|s| s.as_cli_value()).unwrap_or(""),
                    "heal": summary,
                })
            }),
        });
        Ok(CandidateExecution {
            index,
            target,
            baseline,
            heal,
            differential_heal,
            attempt,
        })
    }

    fn write_attempt_artifacts(
        &self,
        run_dir: &Path,
        execution: &CandidateExecution,
    ) -> Result<()> {
        let pair_dir = run_dir.join(format!(
            "attempt_{:03}_{}",
            execution.index, execution.target.digest
        ));
        fs::create_dir_all(&pair_dir).with_context(|| {
            format!(
                "failed to create attempt artifact dir: {}",
                pair_dir.display()
            )
        })?;
        fs::write(
            pair_dir.join("baseline_stdout.json"),
            execution.baseline.stdout.as_bytes(),
        )
        .with_context(|| {
            format!(
                "failed to write baseline stdout artifact in {}",
                pair_dir.display()
            )
        })?;
        fs::write(
            pair_dir.join("baseline_stderr.log"),
            execution.baseline.stderr.as_bytes(),
        )
        .with_context(|| {
            format!(
                "failed to write baseline stderr artifact in {}",
                pair_dir.display()
            )
        })?;
        fs::write(
            pair_dir.join("heal_stdout.json"),
            execution.heal.stdout.as_bytes(),
        )
        .with_context(|| {
            format!(
                "failed to write heal stdout artifact in {}",
                pair_dir.display()
            )
        })?;
        fs::write(
            pair_dir.join("heal_stderr.log"),
            execution.heal.stderr.as_bytes(),
        )
        .with_context(|| {
            format!(
                "failed to write heal stderr artifact in {}",
                pair_dir.display()
            )
        })?;
        if let Some(diff) = &execution.differential_heal {
            fs::write(
                pair_dir.join("differential_heal_stdout.json"),
                diff.stdout.as_bytes(),
            )
            .with_context(|| {
                format!(
                    "failed to write differential heal stdout artifact in {}",
                    pair_dir.display()
                )
            })?;
            fs::write(
                pair_dir.join("differential_heal_stderr.log"),
                diff.stderr.as_bytes(),
            )
            .with_context(|| {
                format!(
                    "failed to write differential heal stderr artifact in {}",
                    pair_dir.display()
                )
            })?;
        }
        Ok(())
    }

    fn attempt_is_fail_then_heal(&self, attempt: &Value) -> bool {
        let baseline_ok = attempt
            .pointer("/baseline/local_success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let heal_ok = attempt
            .pointer("/heal/local_success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_ptb = attempt
            .pointer("/baseline/commands_executed")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
            || attempt
                .pointer("/heal/commands_executed")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0;
        has_ptb && !baseline_ok && heal_ok
    }

    async fn run_replay_with_state_json(
        &self,
        digest: &str,
        state_json_path: &Path,
    ) -> Result<ReplayAttemptRaw> {
        let args = vec![
            "replay".to_string(),
            digest.to_string(),
            "--state-json".to_string(),
            state_json_path.display().to_string(),
            "--compare".to_string(),
            "--json".to_string(),
        ];
        self.run_command_json("state_json_broken", args).await
    }

    async fn run_replay_export_state(
        &self,
        digest: &str,
        checkpoint: u64,
        output_path: &Path,
    ) -> Result<ReplayAttemptRaw> {
        let args = self
            .source_adapter()
            .export_state_args(digest, checkpoint, output_path);
        self.run_command_json("export_state", args).await
    }

    async fn run_command_json(
        &self,
        mode: &'static str,
        args: Vec<String>,
    ) -> Result<ReplayAttemptRaw> {
        let bin = self.sandbox_bin()?;
        let started = TokioInstant::now();

        let mut command = TokioCommand::new(&bin);
        command
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {} {}", bin.display(), args.join(" ")))?;

        let mut stdout_reader = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture replay stdout"))?;
        let mut stderr_reader = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("failed to capture replay stderr"))?;
        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stdout_reader.read_to_end(&mut buf).await;
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr_reader.read_to_end(&mut buf).await;
            buf
        });

        let deadline = Duration::from_secs(self.replay_timeout);
        let mut timed_out = false;
        let exit_code = match tokio::time::timeout(deadline, child.wait()).await {
            Ok(status_res) => status_res
                .with_context(|| format!("failed while waiting for replay mode '{mode}'"))?
                .code(),
            Err(_) => {
                timed_out = true;
                let _ = child.kill().await;
                let _ = child.wait().await;
                None
            }
        };

        let stdout_bytes = stdout_task.await.unwrap_or_default();
        let stderr_bytes = stderr_task.await.unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
        let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let (parsed, parse_error) = if stdout.is_empty() {
            (None, None)
        } else {
            match serde_json::from_str::<Value>(&stdout) {
                Ok(value) => (Some(value), None),
                Err(err) => (None, Some(err.to_string())),
            }
        };

        Ok(ReplayAttemptRaw {
            mode,
            cmd: std::iter::once(bin.display().to_string())
                .chain(args.iter().cloned())
                .collect(),
            exit_code,
            elapsed_ms,
            stdout,
            stderr,
            parsed,
            parse_error,
            timed_out,
        })
    }

    async fn run_forced_mutation_demo(
        &self,
        run_dir: &Path,
        attempts: &[Value],
        report: &mut Value,
        strategy: &StrategyConfig,
    ) -> Result<()> {
        let mut seed_pool = attempts
            .iter()
            .filter(|attempt| {
                attempt
                    .pointer("/heal/local_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && attempt
                        .pointer("/heal/commands_executed")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0
            })
            .cloned()
            .collect::<Vec<_>>();

        if seed_pool.is_empty() {
            seed_pool = attempts.to_vec();
        }

        let operators = select_mutation_operators(strategy);
        let mut forced_seed_entries: Vec<Value> = Vec::new();
        let mut chosen_seed: Option<Value> = None;
        let mut successful_mutations: Vec<Value> = Vec::new();

        'seed_loop: for seed in seed_pool {
            let Some(seed_digest) = seed.get("digest").and_then(Value::as_str) else {
                continue;
            };
            let Some(seed_checkpoint) = seed.get("checkpoint").and_then(Value::as_u64) else {
                continue;
            };
            let seed_slug = format!("{}_{}", seed_digest, seed_checkpoint);
            let seed_state = run_dir.join(format!("seed_state_{}.json", seed_slug));

            let seed_export = self
                .run_replay_export_state(seed_digest, seed_checkpoint, &seed_state)
                .await?;
            let seed_entry_base = serde_json::json!({
                "seed_digest": seed_digest,
                "seed_checkpoint": seed_checkpoint,
                "seed_export": {
                    "path": seed_state.display().to_string(),
                    "exit_code": seed_export.exit_code,
                },
            });

            if seed_export.exit_code != Some(0) || !seed_state.exists() {
                forced_seed_entries.push(seed_entry_base);
                continue;
            }

            for operator in &operators {
                let mut seed_entry = seed_entry_base.clone();
                let broken_state = run_dir.join(format!(
                    "broken_state_{}_{}.json",
                    seed_slug,
                    operator.name()
                ));
                let mut mutation = operator.apply(&seed_state, &broken_state)?;
                if let Some(obj) = mutation.as_object_mut() {
                    obj.insert(
                        "operator".to_string(),
                        Value::String(operator.name().to_string()),
                    );
                }
                if let Some(obj) = seed_entry.as_object_mut() {
                    obj.insert("mutation".to_string(), mutation.clone());
                    obj.insert(
                        "mutation_operator".to_string(),
                        Value::String(operator.name().to_string()),
                    );
                }
                if !mutation
                    .get("mutated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    forced_seed_entries.push(seed_entry);
                    continue;
                }

                let broken_run = self
                    .run_replay_with_state_json(seed_digest, &broken_state)
                    .await?;
                let heal_run = self
                    .run_replay_mode_with_retry(
                        &Target {
                            digest: seed_digest.to_string(),
                            checkpoint: seed_checkpoint,
                            source: Some("forced_mutation".to_string()),
                            label: None,
                        },
                        "heal",
                    )
                    .await?;

                let broken_summary = summarize_attempt(&broken_run);
                let heal_summary = summarize_attempt(&heal_run);

                fs::write(
                    run_dir.join(format!(
                        "forced_broken_stdout_{}_{}.json",
                        seed_slug,
                        operator.name()
                    )),
                    broken_run.stdout.as_bytes(),
                )?;
                fs::write(
                    run_dir.join(format!(
                        "forced_broken_stderr_{}_{}.log",
                        seed_slug,
                        operator.name()
                    )),
                    broken_run.stderr.as_bytes(),
                )?;
                fs::write(
                    run_dir.join(format!(
                        "forced_heal_stdout_{}_{}.json",
                        seed_slug,
                        operator.name()
                    )),
                    heal_run.stdout.as_bytes(),
                )?;
                fs::write(
                    run_dir.join(format!(
                        "forced_heal_stderr_{}_{}.log",
                        seed_slug,
                        operator.name()
                    )),
                    heal_run.stderr.as_bytes(),
                )?;

                if let Some(obj) = seed_entry.as_object_mut() {
                    obj.insert("broken_attempt".to_string(), broken_summary.clone());
                    obj.insert("heal_attempt".to_string(), heal_summary.clone());
                }

                let broken_ok = broken_summary
                    .get("local_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let heal_ok = heal_summary
                    .get("local_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                forced_seed_entries.push(seed_entry.clone());
                if !broken_ok && heal_ok {
                    successful_mutations.push(seed_entry.clone());
                    if chosen_seed.is_none() {
                        chosen_seed = Some(seed_entry.clone());
                    }
                    if let Some(obj) = report.as_object_mut() {
                        obj.insert(
                            "status".to_string(),
                            Value::String("found_fail_then_heal_forced_mutation".to_string()),
                        );
                        obj.insert(
                            "chosen".to_string(),
                            serde_json::json!({
                                "source": "forced_mutation",
                                "digest": seed_digest,
                                "checkpoint": seed_checkpoint,
                                "mutation": mutation,
                                "baseline": broken_summary,
                                "heal": heal_summary,
                            }),
                        );
                    }
                    if !self.keep_going {
                        break 'seed_loop;
                    }
                }
            }
        }

        let forced_demo = serde_json::json!({
            "seed_candidates": forced_seed_entries,
            "chosen_seed": chosen_seed,
            "successful_mutations": successful_mutations,
        });
        if let Some(obj) = report.as_object_mut() {
            obj.insert("forced_mutation_demo".to_string(), forced_demo);
        }
        Ok(())
    }

    fn write_run_readme(&self, run_dir: &Path, report: &Value, attempts: &[Value]) -> Result<()> {
        let status = report
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let candidate_source = report
            .get("candidate_source")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let chosen = report.get("chosen").and_then(Value::as_object);
        let has_differential = attempts
            .iter()
            .any(|attempt| attempt.get("differential").is_some());

        let mut lines = Vec::new();
        lines.push("# Replay Mutation Lab\n".to_string());
        lines.push(format!("Status: **{}**\n", status));
        lines.push(format!("Candidate source: `{}`\n", candidate_source));
        lines.push(format!("Transactions tested: `{}`\n", attempts.len()));

        if let Some(chosen) = chosen {
            lines.push("## Winning Case\n".to_string());
            if let Some(source) = chosen.get("source").and_then(Value::as_str) {
                lines.push(format!("- Source: `{}`", source));
            }
            if let Some(digest) = chosen.get("digest").and_then(Value::as_str) {
                lines.push(format!("- Digest: `{}`", digest));
            }
            if let Some(checkpoint) = chosen.get("checkpoint").and_then(Value::as_u64) {
                lines.push(format!("- Checkpoint: `{}`", checkpoint));
            }
            if let Some(base) = chosen.get("baseline") {
                lines.push(format!(
                    "- Baseline success: `{}`",
                    base.get("local_success").unwrap_or(&Value::Null)
                ));
                lines.push(format!(
                    "- Baseline error: `{}`",
                    base.get("local_error")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                ));
            }
            if let Some(heal) = chosen.get("heal") {
                lines.push(format!(
                    "- Heal success: `{}`",
                    heal.get("local_success").unwrap_or(&Value::Null)
                ));
                lines.push(format!(
                    "- Heal synthetic inputs: `{}`",
                    heal.get("synthetic_inputs").unwrap_or(&Value::Null)
                ));
            }
        } else {
            lines.push("## Result\n".to_string());
            lines
                .push("No strict fail -> heal success pair found in this scan window.".to_string());
            lines.push(
                "See `attempts.json` for nearest cases (fail/fail or success/success).".to_string(),
            );
        }

        lines.push("\n## Files\n".to_string());
        lines.push("- `candidate_pool.json`".to_string());
        lines.push("- `attempts.json`".to_string());
        lines.push("- `report.json`".to_string());
        lines.push("- `attempt_*/baseline_stdout.json` + `heal_stdout.json`".to_string());
        if has_differential {
            lines.push("- `attempt_*/differential_heal_stdout.json`".to_string());
        }
        if chosen.is_some() {
            lines.push("- `winning_state.json`".to_string());
        }
        if report.get("forced_mutation_demo").is_some() {
            lines.push("- `seed_state_*.json` / `broken_state_*.json`".to_string());
            lines.push("- `forced_broken_stdout_*.json` + `forced_heal_stdout_*.json`".to_string());
        }

        fs::write(run_dir.join("README.md"), lines.join("\n") + "\n").with_context(|| {
            format!(
                "failed to write replay mutate run README in {}",
                run_dir.display()
            )
        })?;
        Ok(())
    }

    fn record_from_report(
        &self,
        target: &Target,
        report: &Value,
        run_dir: Option<&Path>,
        elapsed_ms: u64,
    ) -> Option<RunRecord> {
        let status = report.get("status")?.as_str()?.to_string();
        let chosen = report.get("chosen").cloned();
        let mut resolved_target = target.clone();
        if let Some(chosen_obj) = chosen.as_ref().and_then(Value::as_object) {
            if let Some(digest) = chosen_obj.get("digest").and_then(Value::as_str) {
                resolved_target.digest = digest.to_string();
            }
            if let Some(checkpoint) = chosen_obj.get("checkpoint").and_then(Value::as_u64) {
                resolved_target.checkpoint = checkpoint;
            }
        }
        Some(RunRecord {
            target: resolved_target,
            status,
            run_dir: run_dir.map(|p| p.display().to_string()),
            report_path: run_dir.map(|p| p.join("report.json").display().to_string()),
            chosen,
            oracle_hits: Vec::new(),
            invariant_violations: Vec::new(),
            score: 0,
            minimization: None,
            elapsed_ms,
        })
    }

    fn decorate_record(
        &self,
        record: &mut RunRecord,
        strategy: &StrategyConfig,
        run_dir: Option<&Path>,
    ) -> Result<()> {
        let (oracle_hits, invariant_violations) = evaluate_record(record, strategy);
        record.score = score_record(
            record,
            &oracle_hits,
            &invariant_violations,
            &strategy.scoring,
        );
        record.oracle_hits = oracle_hits;
        record.invariant_violations = invariant_violations;
        if strategy.minimization.enabled {
            record.minimization = minimize_record(record, run_dir, &strategy.minimization)?;
        }
        Ok(())
    }

    fn findings_from_record(&self, record: &RunRecord) -> Vec<Finding> {
        let mut out = Vec::new();

        if record_has_fail_then_heal(record) {
            let fingerprint = format!(
                "{}|{}|{}",
                record.target.digest, record.target.checkpoint, record.status
            );
            out.push(Finding {
                fingerprint,
                summary: "Found fail->heal mutation recovery path".to_string(),
                target: record.target.clone(),
                severity: "high".to_string(),
            });
        }

        for oracle in &record.oracle_hits {
            if oracle == "forced_mutation_recovery" {
                out.push(Finding {
                    fingerprint: format!(
                        "{}|{}|oracle:{}",
                        record.target.digest, record.target.checkpoint, oracle
                    ),
                    summary: "Forced mutation path recovered via heal replay".to_string(),
                    target: record.target.clone(),
                    severity: "high".to_string(),
                });
            } else if oracle == "source_divergence" {
                out.push(Finding {
                    fingerprint: format!(
                        "{}|{}|oracle:{}",
                        record.target.digest, record.target.checkpoint, oracle
                    ),
                    summary:
                        "Differential replay source divergence detected between primary and secondary heal passes"
                            .to_string(),
                    target: record.target.clone(),
                    severity: "medium".to_string(),
                });
            }
        }

        for inv in &record.invariant_violations {
            out.push(Finding {
                fingerprint: format!(
                    "{}|{}|invariant:{}",
                    record.target.digest, record.target.checkpoint, inv
                ),
                summary: format!("Invariant violated: {inv}"),
                target: record.target.clone(),
                severity: "medium".to_string(),
            });
        }

        out
    }
}

fn discovered_entries(targets: &[Target]) -> Vec<Value> {
    targets
        .iter()
        .map(|target| {
            serde_json::json!({
                "digest": target.digest,
                "checkpoint": target.checkpoint,
            })
        })
        .collect()
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(value)?;
    fs::write(path, data)
        .with_context(|| format!("failed to write JSON artifact: {}", path.display()))?;
    Ok(())
}

fn summarize_attempt(run: &ReplayAttemptRaw) -> Value {
    let out = run.parsed.as_ref().and_then(Value::as_object);
    let local_success = out.and_then(|o| o.get("local_success")).cloned();
    let local_error = out.and_then(|o| o.get("local_error")).cloned();
    let execution_path = out
        .and_then(|o| o.get("execution_path"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let synthetic_inputs = execution_path
        .get("synthetic_inputs")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let commands_executed = out.and_then(|o| o.get("commands_executed")).cloned();

    serde_json::json!({
        "mode": run.mode,
        "cmd": run.cmd,
        "exit_code": run.exit_code,
        "elapsed_ms": run.elapsed_ms,
        "local_success": local_success.unwrap_or(Value::Null),
        "local_error": local_error.unwrap_or(Value::Null),
        "synthetic_inputs": synthetic_inputs,
        "execution_path": execution_path,
        "timed_out": run.timed_out,
        "commands_executed": commands_executed.unwrap_or(Value::Null),
        "parse_error": run.parse_error,
    })
}

fn mutate_state_drop_required_object(src_path: &Path, out_path: &Path) -> Result<Value> {
    let text = fs::read_to_string(src_path)
        .with_context(|| format!("failed to read seed state: {}", src_path.display()))?;
    let mut state: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON state file: {}", src_path.display()))?;

    let inputs = state
        .pointer("/transaction/inputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let Some(objects) = state.get_mut("objects").and_then(Value::as_object_mut) else {
        return Ok(serde_json::json!({
            "removed_object_id": Value::Null,
            "mutated": false,
        }));
    };
    if objects.is_empty() {
        return Ok(serde_json::json!({
            "removed_object_id": Value::Null,
            "mutated": false,
        }));
    }

    let system_ids = [
        "0000000000000000000000000000000000000000000000000000000000000006",
        "0000000000000000000000000000000000000000000000000000000000000008",
    ];

    let mut candidate_key: Option<String> = None;
    for input in &inputs {
        let Some(obj) = input.as_object() else {
            continue;
        };
        let input_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
        if !matches!(
            input_type,
            "ImmOrOwnedObject" | "SharedObject" | "Receiving"
        ) {
            continue;
        }
        let Some(raw_oid) = obj.get("object_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(norm_input) = normalize_object_id64(raw_oid) else {
            continue;
        };
        if system_ids.contains(&norm_input.as_str()) {
            continue;
        }
        if let Some(found_key) = objects
            .keys()
            .find(|key| normalize_object_id64(key).as_deref() == Some(norm_input.as_str()))
            .cloned()
        {
            candidate_key = Some(found_key);
            break;
        }
    }

    if candidate_key.is_none() {
        candidate_key = objects
            .keys()
            .find(|key| {
                normalize_object_id64(key)
                    .map(|n| !system_ids.contains(&n.as_str()))
                    .unwrap_or(false)
            })
            .cloned();
    }

    let Some(candidate_key) = candidate_key else {
        return Ok(serde_json::json!({
            "removed_object_id": Value::Null,
            "mutated": false,
        }));
    };

    let removed = objects.remove(&candidate_key);
    let removed_type_tag = removed
        .as_ref()
        .and_then(|v| v.get("type_tag"))
        .cloned()
        .unwrap_or(Value::Null);

    fs::write(out_path, serde_json::to_string_pretty(&state)?)
        .with_context(|| format!("failed to write broken state: {}", out_path.display()))?;

    Ok(serde_json::json!({
        "removed_object_id": candidate_key,
        "removed_type_tag": removed_type_tag,
        "mutated": true,
    }))
}

fn mutate_state_input_rewire(src_path: &Path, out_path: &Path) -> Result<Value> {
    let mut state = load_state_json(src_path)?;
    let Some(inputs) = state
        .pointer_mut("/transaction/inputs")
        .and_then(Value::as_array_mut)
    else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "missing transaction.inputs",
        }));
    };
    if inputs.len() < 2 {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "insufficient inputs",
        }));
    }

    let mut candidate_indexes = Vec::new();
    for (idx, input) in inputs.iter().enumerate() {
        let input_type = input
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(
            input_type,
            "SharedObject" | "ImmOrOwnedObject" | "Receiving"
        ) {
            candidate_indexes.push(idx);
        }
    }
    let (i, j) = if candidate_indexes.len() >= 2 {
        (candidate_indexes[0], candidate_indexes[1])
    } else {
        (0, 1)
    };

    let before_i = inputs.get(i).cloned().unwrap_or(Value::Null);
    let before_j = inputs.get(j).cloned().unwrap_or(Value::Null);
    inputs.swap(i, j);

    write_state_json(out_path, &state)?;
    Ok(serde_json::json!({
        "mutated": true,
        "rewired_indices": [i, j],
        "before": {
            "i": before_i,
            "j": before_j,
        },
    }))
}

fn mutate_state_object_version_skew(src_path: &Path, out_path: &Path) -> Result<Value> {
    let mut state = load_state_json(src_path)?;
    let input_candidates = state
        .pointer("/transaction/inputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let Some(objects) = state.get_mut("objects").and_then(Value::as_object_mut) else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "missing objects map",
        }));
    };

    let mut target_key: Option<String> = None;
    for input in &input_candidates {
        let raw_id = input.get("object_id").and_then(Value::as_str);
        let Some(raw_id) = raw_id else {
            continue;
        };
        if let Some(found) = find_object_key(objects, raw_id) {
            if !is_system_object_id(raw_id) {
                target_key = Some(found);
                break;
            }
        }
    }
    if target_key.is_none() {
        target_key = objects.keys().find(|k| !is_system_object_id(k)).cloned();
    }
    let Some(target_key) = target_key else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "no suitable object",
        }));
    };

    let Some(obj) = objects.get_mut(&target_key) else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "object disappeared during mutation",
        }));
    };
    let Some(obj_map) = obj.as_object_mut() else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "object is not map-like",
        }));
    };
    let old_version = obj_map.get("version").cloned().unwrap_or(Value::Null);
    let old_num = parse_version_u64(obj_map.get("version"))?;
    let new_num = old_num.saturating_add(1);
    obj_map.insert(
        "version".to_string(),
        Value::Number(serde_json::Number::from(new_num)),
    );

    write_state_json(out_path, &state)?;
    Ok(serde_json::json!({
        "mutated": true,
        "object_id": target_key,
        "old_version": old_version,
        "new_version": new_num,
    }))
}

fn mutate_state_shared_object_substitute(src_path: &Path, out_path: &Path) -> Result<Value> {
    let mut state = load_state_json(src_path)?;
    let objects_map = state
        .get("objects")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let object_keys = state
        .get("objects")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let Some(inputs_snapshot) = state
        .pointer("/transaction/inputs")
        .and_then(Value::as_array)
    else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "missing transaction.inputs",
        }));
    };

    let mut target_input_index: Option<usize> = None;
    let mut original_object_id = String::new();
    for (idx, input) in inputs_snapshot.iter().enumerate() {
        let input_type = input
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if input_type != "SharedObject" {
            continue;
        }
        let Some(obj_id) = input.get("object_id").and_then(Value::as_str) else {
            continue;
        };
        if is_system_object_id(obj_id) {
            continue;
        }
        target_input_index = Some(idx);
        original_object_id = obj_id.to_string();
        break;
    }

    let Some(target_input_index) = target_input_index else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "no shared input candidate",
        }));
    };

    let replacement = object_keys
        .iter()
        .find(|candidate| {
            !is_system_object_id(candidate)
                && normalize_object_id64(candidate).as_deref()
                    != normalize_object_id64(&original_object_id).as_deref()
        })
        .cloned();
    let Some(replacement_key) = replacement else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "no replacement object found",
        }));
    };

    let maybe_version = objects_map
        .get(&replacement_key)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("version"))
        .cloned();

    let Some(inputs) = state
        .pointer_mut("/transaction/inputs")
        .and_then(Value::as_array_mut)
    else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "missing transaction.inputs",
        }));
    };

    let Some(input_obj) = inputs
        .get_mut(target_input_index)
        .and_then(Value::as_object_mut)
    else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "target input not object",
        }));
    };
    input_obj.insert(
        "object_id".to_string(),
        Value::String(to_hex_prefixed(&replacement_key)),
    );
    if let Some(version_value) = maybe_version {
        input_obj.insert("initial_shared_version".to_string(), version_value);
    }

    write_state_json(out_path, &state)?;
    Ok(serde_json::json!({
        "mutated": true,
        "input_index": target_input_index,
        "original_object_id": original_object_id,
        "replacement_object_id": replacement_key,
    }))
}

fn mutate_state_pure_type_aware(src_path: &Path, out_path: &Path) -> Result<Value> {
    let mut state = load_state_json(src_path)?;
    let Some(inputs) = state
        .pointer_mut("/transaction/inputs")
        .and_then(Value::as_array_mut)
    else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "missing transaction.inputs",
        }));
    };

    let Some((idx, input_obj)) = inputs.iter_mut().enumerate().find_map(|(idx, item)| {
        let obj = item.as_object_mut()?;
        if obj.get("type").and_then(Value::as_str) == Some("Pure") {
            Some((idx, obj))
        } else {
            None
        }
    }) else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "no pure input",
        }));
    };

    let old_bytes = input_obj.get("bytes").cloned().unwrap_or(Value::Null);
    input_obj.insert("bytes".to_string(), Value::String("AA==".to_string()));
    write_state_json(out_path, &state)?;
    Ok(serde_json::json!({
        "mutated": true,
        "input_index": idx,
        "old_bytes": old_bytes,
        "new_bytes": "AA==",
    }))
}

fn mutate_state_pure_signature_aware(src_path: &Path, out_path: &Path) -> Result<Value> {
    let mut state = load_state_json(src_path)?;
    let Some(inputs) = state
        .pointer_mut("/transaction/inputs")
        .and_then(Value::as_array_mut)
    else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "missing transaction.inputs",
        }));
    };

    let Some((idx, input_obj)) = inputs.iter_mut().enumerate().find_map(|(idx, item)| {
        let obj = item.as_object_mut()?;
        if obj.get("type").and_then(Value::as_str) == Some("Pure") {
            Some((idx, obj))
        } else {
            None
        }
    }) else {
        return Ok(serde_json::json!({
            "mutated": false,
            "reason": "no pure input",
        }));
    };

    let old = input_obj
        .get("bytes")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut bytes = BASE64_STANDARD.decode(old.as_bytes()).unwrap_or_default();
    if bytes.is_empty() {
        bytes.push(0x01);
    } else {
        bytes[0] ^= 0x80;
    }
    let new_b64 = BASE64_STANDARD.encode(bytes);
    input_obj.insert("bytes".to_string(), Value::String(new_b64.clone()));

    write_state_json(out_path, &state)?;
    Ok(serde_json::json!({
        "mutated": true,
        "input_index": idx,
        "old_bytes": old,
        "new_bytes": new_b64,
    }))
}

fn load_state_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read state JSON: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("invalid state JSON: {}", path.display()))
}

fn write_state_json(path: &Path, state: &Value) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(state)?)
        .with_context(|| format!("failed to write state JSON: {}", path.display()))
}

fn is_system_object_id(raw: &str) -> bool {
    let norm = normalize_object_id64(raw).unwrap_or_default();
    norm == "0000000000000000000000000000000000000000000000000000000000000006"
        || norm == "0000000000000000000000000000000000000000000000000000000000000008"
}

fn find_object_key(objects: &Map<String, Value>, raw_id: &str) -> Option<String> {
    let norm = normalize_object_id64(raw_id)?;
    objects
        .keys()
        .find(|k| normalize_object_id64(k).as_deref() == Some(norm.as_str()))
        .cloned()
}

fn parse_version_u64(value: Option<&Value>) -> Result<u64> {
    let Some(value) = value else {
        return Ok(0);
    };
    if let Some(v) = value.as_u64() {
        return Ok(v);
    }
    if let Some(s) = value.as_str() {
        return s
            .parse::<u64>()
            .with_context(|| format!("invalid version string: {s}"));
    }
    bail!("unsupported version representation: {value:?}")
}

fn to_hex_prefixed(raw: &str) -> String {
    if raw.starts_with("0x") || raw.starts_with("0X") {
        raw.to_ascii_lowercase()
    } else {
        format!("0x{}", raw.to_ascii_lowercase())
    }
}

fn normalize_object_id64(raw: &str) -> Option<String> {
    let s = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
    if s.is_empty() {
        return None;
    }
    Some(format!("{:0>64}", s.to_ascii_lowercase()))
}

fn load_strategy_file(path: &Path) -> Result<StrategyFile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read strategy file: {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext == "json" {
        serde_json::from_str(&text)
            .with_context(|| format!("invalid JSON strategy file: {}", path.display()))
    } else {
        serde_yaml::from_str(&text)
            .with_context(|| format!("invalid YAML strategy file: {}", path.display()))
    }
}

fn validate_strategy_config(strategy: &StrategyConfig) -> Result<()> {
    if strategy.mutators.is_empty() {
        return Err(anyhow!("strategy requires at least one mutator"));
    }
    if strategy.oracles.is_empty() {
        return Err(anyhow!("strategy requires at least one oracle"));
    }

    let unknown_mutators: Vec<String> = strategy
        .mutators
        .iter()
        .filter(|m| !KNOWN_MUTATORS.contains(&m.as_str()))
        .cloned()
        .collect();
    if !unknown_mutators.is_empty() {
        return Err(anyhow!(
            "unknown mutator(s): {} (known: {})",
            unknown_mutators.join(", "),
            KNOWN_MUTATORS.join(", ")
        ));
    }

    let unknown_oracles: Vec<String> = strategy
        .oracles
        .iter()
        .filter(|o| !KNOWN_ORACLES.contains(&o.as_str()))
        .cloned()
        .collect();
    if !unknown_oracles.is_empty() {
        return Err(anyhow!(
            "unknown oracle(s): {} (known: {})",
            unknown_oracles.join(", "),
            KNOWN_ORACLES.join(", ")
        ));
    }

    let unknown_invariants: Vec<String> = strategy
        .invariants
        .iter()
        .filter(|i| !KNOWN_INVARIANTS.contains(&i.as_str()))
        .cloned()
        .collect();
    if !unknown_invariants.is_empty() {
        return Err(anyhow!(
            "unknown invariant(s): {} (known: {})",
            unknown_invariants.join(", "),
            KNOWN_INVARIANTS.join(", ")
        ));
    }

    if !KNOWN_SCORING.contains(&strategy.scoring.as_str()) {
        return Err(anyhow!(
            "unknown scoring '{}': expected one of {}",
            strategy.scoring,
            KNOWN_SCORING.join(", ")
        ));
    }

    if !KNOWN_MINIMIZATION_MODES.contains(&strategy.minimization.mode.as_str()) {
        return Err(anyhow!(
            "unknown minimization mode '{}': expected one of {}",
            strategy.minimization.mode,
            KNOWN_MINIMIZATION_MODES.join(", ")
        ));
    }

    Ok(())
}

fn evaluate_record(record: &RunRecord, strategy: &StrategyConfig) -> (Vec<String>, Vec<String>) {
    let mut oracle_hits = Vec::new();
    let mut invariant_violations = Vec::new();
    let view = EvaluationView::from_record(record);
    let oracle_rules = oracle_rule_registry();
    let invariant_rules = invariant_rule_registry();

    for oracle in &strategy.oracles {
        if let Some(rule) = oracle_rules.iter().find(|r| r.name() == oracle.as_str()) {
            if rule.evaluate(&view) {
                oracle_hits.push(oracle.clone());
            }
        }
    }

    for invariant in &strategy.invariants {
        if let Some(rule) = invariant_rules
            .iter()
            .find(|r| r.name() == invariant.as_str())
        {
            if rule.violated(&view) {
                invariant_violations.push(invariant.clone());
            }
        }
    }

    (oracle_hits, invariant_violations)
}

fn record_has_fail_then_heal(record: &RunRecord) -> bool {
    let baseline_ok = record
        .chosen
        .as_ref()
        .and_then(|c| c.pointer("/baseline/local_success"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let heal_ok = record
        .chosen
        .as_ref()
        .and_then(|c| c.pointer("/heal/local_success"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    !baseline_ok && heal_ok
}

fn score_record(
    record: &RunRecord,
    oracle_hits: &[String],
    invariant_violations: &[String],
    scoring: &str,
) -> i64 {
    let mut score = if record_has_fail_then_heal(record) {
        100
    } else if record
        .chosen
        .as_ref()
        .and_then(|c| c.pointer("/baseline/local_success"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        60
    } else {
        20
    };

    score += (oracle_hits.len() as i64) * 15;
    score -= (invariant_violations.len() as i64) * 25;

    let source = record
        .chosen
        .as_ref()
        .and_then(|c| c.get("source"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let heal_commands = record
        .chosen
        .as_ref()
        .and_then(|c| c.pointer("/heal/commands_executed"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as i64;

    match scoring {
        "recovery-priority" => {
            if source == "forced_mutation" {
                score += 30;
            }
            score += heal_commands.min(20);
        }
        "balanced" => {
            score += (heal_commands / 2).min(10);
        }
        _ => {}
    }

    score
}

fn minimize_record(
    record: &RunRecord,
    run_dir: Option<&Path>,
    config: &MinimizationConfig,
) -> Result<Option<MinimizationResult>> {
    if !config.enabled || config.mode == "none" {
        return Ok(None);
    }
    if config.mode != "state-diff" && config.mode != "operator-specific" {
        return Ok(None);
    }

    let Some(run_dir) = run_dir else {
        return Ok(None);
    };

    let mut seed_states = Vec::new();
    let mut broken_states = Vec::new();

    for entry in fs::read_dir(run_dir).with_context(|| {
        format!(
            "failed to read run dir for minimization: {}",
            run_dir.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with("seed_state_") && name.ends_with(".json") {
            seed_states.push(path);
        } else if name.starts_with("broken_state_") && name.ends_with(".json") {
            broken_states.push(path);
        }
    }

    if seed_states.is_empty() || broken_states.is_empty() {
        return Ok(None);
    }

    seed_states.sort();
    broken_states.sort();

    let seed_path = seed_states[0].clone();
    let broken_path = broken_states[0].clone();

    let seed_doc: Value =
        serde_json::from_str(&fs::read_to_string(&seed_path).with_context(|| {
            format!(
                "failed to read seed state for minimization: {}",
                seed_path.display()
            )
        })?)
        .with_context(|| format!("invalid seed state JSON: {}", seed_path.display()))?;

    let broken_doc: Value =
        serde_json::from_str(&fs::read_to_string(&broken_path).with_context(|| {
            format!(
                "failed to read broken state for minimization: {}",
                broken_path.display()
            )
        })?)
        .with_context(|| format!("invalid broken state JSON: {}", broken_path.display()))?;

    let seed_objects = seed_doc
        .get("objects")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    let broken_objects = broken_doc
        .get("objects")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);

    let mut removed = Vec::new();
    let mut added = Vec::new();
    let mut changed = Vec::new();

    for key in seed_objects.keys() {
        if !broken_objects.contains_key(key) {
            removed.push(key.clone());
        }
    }
    for key in broken_objects.keys() {
        if !seed_objects.contains_key(key) {
            added.push(key.clone());
        }
    }
    for (key, seed_val) in &seed_objects {
        if let Some(broken_val) = broken_objects.get(key) {
            if seed_val != broken_val {
                changed.push(key.clone());
            }
        }
    }

    let seed_inputs = seed_doc
        .pointer("/transaction/inputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let broken_inputs = broken_doc
        .pointer("/transaction/inputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let max_inputs = std::cmp::max(seed_inputs.len(), broken_inputs.len());
    for idx in 0..max_inputs {
        let a = seed_inputs.get(idx);
        let b = broken_inputs.get(idx);
        if a != b {
            changed.push(format!("tx_input[{idx}]"));
        }
    }

    removed.sort();
    added.sort();
    changed.sort();

    let (minimal_delta, operator, minimizer) =
        choose_minimal_delta(record, &removed, &added, &changed, &config.mode);

    let total_delta = removed.len() + added.len() + changed.len();
    let result = MinimizationResult {
        mode: config.mode.clone(),
        verified: minimal_delta.len() <= 1 || total_delta <= 1,
        operator,
        minimizer,
        seed_objects: seed_objects.len(),
        broken_objects: broken_objects.len(),
        removed_objects: removed,
        added_objects: added,
        changed_objects: changed,
        minimal_delta: minimal_delta.clone(),
        minimized_from: total_delta,
        minimized_to: minimal_delta.len(),
        seed_state_path: Some(seed_path.display().to_string()),
        broken_state_path: Some(broken_path.display().to_string()),
        artifact_path: Some(run_dir.join("minimization.json").display().to_string()),
    };

    let artifact_path = run_dir.join("minimization.json");
    fs::write(&artifact_path, serde_json::to_string_pretty(&result)?).with_context(|| {
        format!(
            "failed to write minimization artifact: {}",
            artifact_path.display()
        )
    })?;

    Ok(Some(result))
}

fn normalize_object_id(id: &str) -> String {
    if id.starts_with("0x") {
        id.to_ascii_lowercase()
    } else {
        format!("0x{}", id.to_ascii_lowercase())
    }
}

fn find_object_delta_entry(delta: &[String], object_id: &str) -> Option<String> {
    let normalized = normalize_object_id(object_id);
    delta.iter().find_map(|entry| {
        if entry.starts_with("tx_input[") {
            return None;
        }
        let entry_norm = normalize_object_id(entry);
        if entry_norm == normalized {
            Some(entry.clone())
        } else {
            None
        }
    })
}

fn generic_minimal_delta(
    record: &RunRecord,
    removed: &[String],
    added: &[String],
    changed: &[String],
) -> Vec<String> {
    let preferred_removed = record
        .chosen
        .as_ref()
        .and_then(|c| c.pointer("/mutation/removed_object_id"))
        .and_then(Value::as_str)
        .map(normalize_object_id);

    let mut minimal_delta = Vec::new();
    if let Some(preferred) = preferred_removed {
        if removed
            .iter()
            .any(|id| normalize_object_id(id) == preferred)
        {
            minimal_delta.push(preferred);
        }
    }
    if minimal_delta.is_empty() {
        if let Some(first) = removed.first() {
            minimal_delta.push(first.clone());
        } else if let Some(first) = changed.first() {
            minimal_delta.push(first.clone());
        } else if let Some(first) = added.first() {
            minimal_delta.push(first.clone());
        }
    }
    minimal_delta
}

fn operator_specific_delta(
    operator: &str,
    record: &RunRecord,
    removed: &[String],
    added: &[String],
    changed: &[String],
) -> Vec<String> {
    let Some(mutation) = record
        .chosen
        .as_ref()
        .and_then(|c| c.get("mutation"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };

    match operator {
        "state_drop_required_object" => mutation
            .get("removed_object_id")
            .and_then(Value::as_str)
            .and_then(|id| find_object_delta_entry(removed, id))
            .into_iter()
            .collect(),
        "state_input_rewire" => mutation
            .get("rewired_indices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|idx| format!("tx_input[{idx}]"))
            .filter(|tag| changed.contains(tag))
            .collect(),
        "state_object_version_skew" => mutation
            .get("object_id")
            .and_then(Value::as_str)
            .and_then(|id| find_object_delta_entry(changed, id))
            .into_iter()
            .collect(),
        "state_shared_object_substitute" => {
            let mut out = Vec::new();
            if let Some(idx) = mutation.get("input_index").and_then(Value::as_u64) {
                let tag = format!("tx_input[{idx}]");
                if changed.contains(&tag) {
                    out.push(tag);
                }
            }
            if let Some(obj) = mutation
                .get("replacement_object_id")
                .and_then(Value::as_str)
                .and_then(|id| {
                    find_object_delta_entry(changed, id)
                        .or_else(|| find_object_delta_entry(added, id))
                })
            {
                out.push(obj);
            }
            dedup_preserve(out)
        }
        "state_pure_type_aware" | "state_pure_signature_aware" => mutation
            .get("input_index")
            .and_then(Value::as_u64)
            .map(|idx| format!("tx_input[{idx}]"))
            .filter(|tag| changed.contains(tag))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn choose_minimal_delta(
    record: &RunRecord,
    removed: &[String],
    added: &[String],
    changed: &[String],
    mode: &str,
) -> (Vec<String>, Option<String>, Option<String>) {
    let operator = record
        .chosen
        .as_ref()
        .and_then(|c| c.pointer("/mutation/operator"))
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    if mode == "operator-specific" {
        if let Some(operator_name) = operator.as_deref() {
            let selected = operator_specific_delta(operator_name, record, removed, added, changed);
            if !selected.is_empty() {
                return (
                    selected,
                    Some(operator_name.to_string()),
                    Some(format!("operator:{operator_name}")),
                );
            }
        }
    }

    let generic = generic_minimal_delta(record, removed, added, changed);
    (generic, operator, Some("generic:state-diff".to_string()))
}

fn dedup_preserve(values: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

fn short_digest(digest: &str) -> String {
    digest.chars().take(10).collect::<String>()
}

fn parse_checkpoint_spec(spec: &str) -> Result<Vec<u64>> {
    let s = spec.trim();
    if s.is_empty() {
        return Err(anyhow!("checkpoint spec cannot be empty"));
    }

    if let Some((a, b)) = s.split_once("..") {
        let start: u64 = a.trim().parse().context("invalid checkpoint range start")?;
        let end: u64 = b.trim().parse().context("invalid checkpoint range end")?;
        if start > end {
            return Err(anyhow!(
                "checkpoint range start must be <= end ({}..{})",
                start,
                end
            ));
        }
        return Ok((start..=end).collect());
    }

    if s.contains(',') {
        let mut out = Vec::new();
        for part in s.split(',') {
            let cp: u64 = part
                .trim()
                .parse()
                .with_context(|| format!("invalid checkpoint in list: {}", part.trim()))?;
            out.push(cp);
        }
        if out.is_empty() {
            return Err(anyhow!("checkpoint list cannot be empty"));
        }
        return Ok(out);
    }

    let one: u64 = s.parse().context("invalid checkpoint")?;
    Ok(vec![one])
}

fn load_targets_from_json(path: &Path) -> Result<Vec<Target>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read targets file: {}", path.display()))?;
    let root: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON in targets file: {}", path.display()))?;

    let items = match &root {
        Value::Array(a) => a,
        Value::Object(obj) => obj
            .get("targets")
            .or_else(|| obj.get("candidates"))
            .or_else(|| obj.get("discovered"))
            .ok_or_else(|| {
                anyhow!(
                    "targets file must be an array or object containing one of: targets, candidates, discovered"
                )
            })?
            .as_array()
            .ok_or_else(|| anyhow!("targets/candidates/discovered must be an array"))?,
        _ => {
            return Err(anyhow!(
                "targets file must be a JSON array or object with targets/candidates/discovered"
            ));
        }
    };

    let mut out = Vec::new();
    for item in items {
        let Value::Object(obj) = item else {
            continue;
        };
        let digest = obj
            .get("digest")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if digest.is_empty() {
            continue;
        }
        let checkpoint = obj.get("checkpoint").and_then(Value::as_u64).or_else(|| {
            obj.get("checkpoint")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<u64>().ok())
        });
        let Some(checkpoint) = checkpoint else {
            continue;
        };
        let label = obj
            .get("label")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let source = obj
            .get("source")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        out.push(Target {
            digest,
            checkpoint,
            source,
            label,
        });
    }

    if out.is_empty() {
        return Err(anyhow!(
            "no valid targets found in {} (need digest + checkpoint)",
            path.display()
        ));
    }

    Ok(out)
}

fn dedup_targets(targets: Vec<Target>) -> Vec<Target> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for target in targets {
        let key = format!("{}:{}", target.digest, target.checkpoint);
        if seen.insert(key) {
            out.push(target);
        }
    }
    out
}

fn load_corpus_targets(path: &Path) -> Result<Vec<Target>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read corpus file: {}", path.display()))?;
    let parsed = serde_json::from_str::<ReplayMutateCorpus>(&text);
    if let Ok(corpus) = parsed {
        let mut out = Vec::new();
        for entry in corpus.entries {
            if entry.digest.is_empty() {
                continue;
            }
            out.push(Target {
                digest: entry.digest,
                checkpoint: entry.checkpoint,
                source: Some("corpus".to_string()),
                label: None,
            });
        }
        return Ok(out);
    }

    load_targets_from_json(path)
}

fn build_corpus_from_report(report: &ReplayMutateReport) -> ReplayMutateCorpus {
    let mut finding_index: HashMap<String, Vec<String>> = HashMap::new();
    for finding in &report.findings {
        let key = format!("{}:{}", finding.target.digest, finding.target.checkpoint);
        finding_index
            .entry(key)
            .or_default()
            .push(finding.fingerprint.clone());
    }

    let mut entries = Vec::new();
    for record in &report.run_records {
        let key = format!("{}:{}", record.target.digest, record.target.checkpoint);
        let findings = finding_index.remove(&key).unwrap_or_default();
        let operator = record
            .chosen
            .as_ref()
            .and_then(|c| c.pointer("/mutation/operator"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        entries.push(CorpusEntry {
            digest: record.target.digest.clone(),
            checkpoint: record.target.checkpoint,
            status: Some(record.status.clone()),
            score: Some(record.score),
            operator,
            findings,
        });
    }

    ReplayMutateCorpus {
        version: 1,
        generated_at: Local::now().to_rfc3339(),
        entries,
    }
}

fn merge_corpus_entries(
    new_entries: Vec<CorpusEntry>,
    old_entries: Vec<CorpusEntry>,
) -> Vec<CorpusEntry> {
    let mut merged: HashMap<String, CorpusEntry> = HashMap::new();
    for entry in old_entries.into_iter().chain(new_entries.into_iter()) {
        let key = format!(
            "{}:{}:{}",
            entry.digest,
            entry.checkpoint,
            entry.operator.clone().unwrap_or_default()
        );
        match merged.get(&key) {
            Some(existing) => {
                let existing_score = existing.score.unwrap_or(i64::MIN);
                let current_score = entry.score.unwrap_or(i64::MIN);
                if current_score >= existing_score {
                    merged.insert(key, entry);
                }
            }
            None => {
                merged.insert(key, entry);
            }
        }
    }
    let mut out: Vec<CorpusEntry> = merged.into_values().collect();
    out.sort_by(|a, b| {
        a.digest
            .cmp(&b.digest)
            .then(a.checkpoint.cmp(&b.checkpoint))
            .then(a.operator.cmp(&b.operator))
    });
    out
}

fn write_replay_mutate_corpus(path: &Path, corpus: &ReplayMutateCorpus) -> Result<()> {
    let mut merged = corpus.clone();
    if path.exists() {
        if let Ok(old_text) = fs::read_to_string(path) {
            if let Ok(old) = serde_json::from_str::<ReplayMutateCorpus>(&old_text) {
                merged.entries = merge_corpus_entries(merged.entries, old.entries);
            }
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create corpus dir: {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(&merged)?)
        .with_context(|| format!("failed to write corpus file: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn default_strategy_is_valid() {
        let strategy = StrategyConfig::default();
        validate_strategy_config(&strategy).expect("default strategy should validate");
    }

    #[test]
    fn dedup_preserves_order() {
        let input = vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "c".to_string(),
            "b".to_string(),
        ];
        let deduped = dedup_preserve(input);
        assert_eq!(deduped, vec!["a", "b", "c"]);
    }

    #[test]
    fn summarize_attempt_reads_execution_fields() {
        let run = ReplayAttemptRaw {
            mode: "heal",
            cmd: vec!["sui-sandbox".to_string(), "replay".to_string()],
            exit_code: Some(0),
            elapsed_ms: 123,
            stdout: "{}".to_string(),
            stderr: String::new(),
            parsed: Some(serde_json::json!({
                "local_success": true,
                "local_error": null,
                "commands_executed": 7,
                "execution_path": {
                    "synthetic_inputs": 3
                }
            })),
            parse_error: None,
            timed_out: false,
        };

        let summary = summarize_attempt(&run);
        assert_eq!(summary["local_success"].as_bool(), Some(true));
        assert_eq!(summary["commands_executed"].as_u64(), Some(7));
        assert_eq!(summary["synthetic_inputs"].as_u64(), Some(3));
    }

    #[test]
    fn mutate_state_drop_required_object_removes_input_object() {
        let tmp = TempDir::new().expect("tmpdir");
        let src = tmp.path().join("seed.json");
        let dst = tmp.path().join("broken.json");
        let target_object = "0x1111111111111111111111111111111111111111111111111111111111111111";
        let other_object = "0x2222222222222222222222222222222222222222222222222222222222222222";

        let state = serde_json::json!({
            "transaction": {
                "inputs": [
                    {"type": "ImmOrOwnedObject", "object_id": target_object}
                ]
            },
            "objects": {
                target_object: {"type_tag": "0x2::foo::Bar"},
                other_object: {"type_tag": "0x2::foo::Baz"}
            }
        });
        fs::write(&src, serde_json::to_string_pretty(&state).unwrap()).expect("write seed");

        let mutation = mutate_state_drop_required_object(&src, &dst).expect("mutate");
        assert_eq!(mutation["mutated"].as_bool(), Some(true));

        let broken: Value =
            serde_json::from_str(&fs::read_to_string(&dst).expect("read broken")).expect("json");
        let objects = broken["objects"].as_object().expect("objects map");
        assert!(!objects.contains_key(target_object));
        assert!(objects.contains_key(other_object));
    }

    #[test]
    fn source_adapter_walrus_includes_checkpoint() {
        let adapter = DefaultReplaySourceAdapter {
            source: ReplayMutateSource::Walrus,
        };
        let target = Target {
            digest: "abc".to_string(),
            checkpoint: 42,
            source: None,
            label: None,
        };
        let args = adapter.replay_args(&target);
        assert!(args.contains(&"--checkpoint".to_string()));
        assert!(args.contains(&"42".to_string()));
    }

    #[test]
    fn source_adapter_grpc_omits_checkpoint() {
        let adapter = DefaultReplaySourceAdapter {
            source: ReplayMutateSource::Grpc,
        };
        let target = Target {
            digest: "abc".to_string(),
            checkpoint: 42,
            source: None,
            label: None,
        };
        let args = adapter.replay_args(&target);
        assert!(!args.contains(&"--checkpoint".to_string()));
    }

    #[test]
    fn select_mutation_operators_uses_strategy_filter() {
        let strategy = StrategyConfig {
            name: "s".to_string(),
            mutators: vec!["state_drop_required_object".to_string()],
            ..StrategyConfig::default()
        };
        let ops = select_mutation_operators(&strategy);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name(), "state_drop_required_object");
    }

    #[test]
    fn evaluate_record_uses_plugin_rules() {
        let strategy = StrategyConfig::default();
        let record = RunRecord {
            target: Target {
                digest: "d".to_string(),
                checkpoint: 1,
                source: None,
                label: None,
            },
            status: "found_fail_then_heal_forced_mutation".to_string(),
            run_dir: None,
            report_path: None,
            chosen: Some(serde_json::json!({
                "source": "forced_mutation",
                "baseline": {"local_success": false, "timed_out": false},
                "heal": {"local_success": true, "timed_out": false, "commands_executed": 1}
            })),
            oracle_hits: vec![],
            invariant_violations: vec![],
            score: 0,
            minimization: None,
            elapsed_ms: 0,
        };

        let (oracles, invariants) = evaluate_record(&record, &strategy);
        assert!(oracles.contains(&"fail_to_heal".to_string()));
        assert!(oracles.contains(&"forced_mutation_recovery".to_string()));
        assert!(oracles.contains(&"state_rehydration_success".to_string()));
        assert!(invariants.is_empty());
    }

    #[test]
    fn mutation_operator_registry_contains_multiple_concrete_operators() {
        let ops = mutation_operator_registry();
        let names: Vec<&str> = ops.iter().map(|op| op.name()).collect();
        assert!(names.contains(&"state_drop_required_object"));
        assert!(names.contains(&"state_input_rewire"));
        assert!(names.contains(&"state_object_version_skew"));
        assert!(names.contains(&"state_shared_object_substitute"));
        assert!(names.contains(&"state_pure_type_aware"));
        assert!(names.contains(&"state_pure_signature_aware"));
    }

    #[test]
    fn mutate_state_input_rewire_swaps_inputs() {
        let tmp = TempDir::new().expect("tmpdir");
        let src = tmp.path().join("seed.json");
        let dst = tmp.path().join("rewired.json");
        let state = serde_json::json!({
            "transaction": {
                "inputs": [
                    {"type":"SharedObject","object_id":"0x111"},
                    {"type":"SharedObject","object_id":"0x222"},
                    {"type":"Pure","bytes":"AA=="}
                ]
            },
            "objects": {
                "111": {"version": 1},
                "222": {"version": 2}
            }
        });
        fs::write(&src, serde_json::to_string_pretty(&state).unwrap()).expect("write seed");

        let mutation = mutate_state_input_rewire(&src, &dst).expect("rewire");
        assert_eq!(mutation["mutated"].as_bool(), Some(true));
        let rewritten: Value =
            serde_json::from_str(&fs::read_to_string(&dst).expect("read dst")).expect("json");
        let first_id = rewritten["transaction"]["inputs"][0]["object_id"]
            .as_str()
            .unwrap_or_default();
        let second_id = rewritten["transaction"]["inputs"][1]["object_id"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(first_id, "0x222");
        assert_eq!(second_id, "0x111");
    }

    #[test]
    fn mutate_state_object_version_skew_changes_version() {
        let tmp = TempDir::new().expect("tmpdir");
        let src = tmp.path().join("seed.json");
        let dst = tmp.path().join("skewed.json");
        let state = serde_json::json!({
            "transaction": {
                "inputs": [
                    {"type":"SharedObject","object_id":"0x111"}
                ]
            },
            "objects": {
                "111": {"version": 7}
            }
        });
        fs::write(&src, serde_json::to_string_pretty(&state).unwrap()).expect("write seed");

        let mutation = mutate_state_object_version_skew(&src, &dst).expect("skew");
        assert_eq!(mutation["mutated"].as_bool(), Some(true));
        let rewritten: Value =
            serde_json::from_str(&fs::read_to_string(&dst).expect("read dst")).expect("json");
        assert_eq!(rewritten["objects"]["111"]["version"].as_u64(), Some(8));
    }

    #[test]
    fn evaluate_record_source_divergence_oracle_hits() {
        let mut strategy = StrategyConfig::default();
        strategy.oracles = vec!["source_divergence".to_string()];
        let record = RunRecord {
            target: Target {
                digest: "d".to_string(),
                checkpoint: 1,
                source: None,
                label: None,
            },
            status: "completed".to_string(),
            run_dir: None,
            report_path: None,
            chosen: Some(serde_json::json!({
                "source": "forced_mutation",
                "baseline": {"local_success": false, "timed_out": false},
                "heal": {"local_success": true, "timed_out": false, "commands_executed": 1, "local_error": null},
                "differential": {"heal": {"local_success": false, "local_error": "boom"}}
            })),
            oracle_hits: vec![],
            invariant_violations: vec![],
            score: 0,
            minimization: None,
            elapsed_ms: 0,
        };
        let (oracles, _) = evaluate_record(&record, &strategy);
        assert_eq!(oracles, vec!["source_divergence".to_string()]);
    }

    #[test]
    fn load_corpus_targets_reads_entries() {
        let tmp = TempDir::new().expect("tmpdir");
        let corpus_path = tmp.path().join("corpus.json");
        let corpus = serde_json::json!({
            "version": 1,
            "generated_at": "2026-02-10T00:00:00Z",
            "entries": [
                {"digest": "abc", "checkpoint": 7, "status": "ok"},
                {"digest": "def", "checkpoint": 8}
            ]
        });
        fs::write(&corpus_path, serde_json::to_string_pretty(&corpus).unwrap()).unwrap();
        let targets = load_corpus_targets(&corpus_path).expect("load corpus targets");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].digest, "abc");
        assert_eq!(targets[0].checkpoint, 7);
        assert_eq!(targets[0].source.as_deref(), Some("corpus"));
    }

    #[test]
    fn validate_strategy_accepts_operator_specific_minimization_mode() {
        let mut strategy = StrategyConfig::default();
        strategy.minimization.mode = "operator-specific".to_string();
        validate_strategy_config(&strategy).expect("operator-specific mode should validate");
    }

    #[test]
    fn choose_minimal_delta_prefers_operator_specific_rewire_indices() {
        let record = RunRecord {
            target: Target {
                digest: "d".to_string(),
                checkpoint: 1,
                source: None,
                label: None,
            },
            status: "found_fail_then_heal_forced_mutation".to_string(),
            run_dir: None,
            report_path: None,
            chosen: Some(serde_json::json!({
                "mutation": {
                    "operator": "state_input_rewire",
                    "rewired_indices": [3, 1]
                }
            })),
            oracle_hits: vec![],
            invariant_violations: vec![],
            score: 0,
            minimization: None,
            elapsed_ms: 0,
        };

        let removed: Vec<String> = vec![];
        let added: Vec<String> = vec![];
        let changed = vec![
            "tx_input[1]".to_string(),
            "tx_input[3]".to_string(),
            "0xabc".to_string(),
        ];
        let (minimal, operator, minimizer) =
            choose_minimal_delta(&record, &removed, &added, &changed, "operator-specific");
        assert_eq!(
            minimal,
            vec!["tx_input[3]".to_string(), "tx_input[1]".to_string()]
        );
        assert_eq!(operator.as_deref(), Some("state_input_rewire"));
        assert_eq!(minimizer.as_deref(), Some("operator:state_input_rewire"));
    }

    #[test]
    fn minimize_record_operator_specific_rewire_end_to_end() {
        let tmp = TempDir::new().expect("tmpdir");
        let seed_path = tmp.path().join("seed_state_case.json");
        let broken_path = tmp.path().join("broken_state_case.json");

        let seed = serde_json::json!({
            "transaction": {
                "inputs": [
                    {"type": "Pure", "bytes": "AA=="},
                    {"type": "Pure", "bytes": "AQ=="}
                ]
            },
            "objects": {
                "0x111": {"version": 1}
            }
        });
        let broken = serde_json::json!({
            "transaction": {
                "inputs": [
                    {"type": "Pure", "bytes": "AQ=="},
                    {"type": "Pure", "bytes": "AA=="}
                ]
            },
            "objects": {
                "0x111": {"version": 1}
            }
        });
        fs::write(&seed_path, serde_json::to_string_pretty(&seed).unwrap()).expect("seed write");
        fs::write(&broken_path, serde_json::to_string_pretty(&broken).unwrap())
            .expect("broken write");

        let record = RunRecord {
            target: Target {
                digest: "d".to_string(),
                checkpoint: 1,
                source: None,
                label: None,
            },
            status: "found_fail_then_heal_forced_mutation".to_string(),
            run_dir: Some(tmp.path().display().to_string()),
            report_path: None,
            chosen: Some(serde_json::json!({
                "mutation": {
                    "operator": "state_input_rewire",
                    "rewired_indices": [1, 0]
                }
            })),
            oracle_hits: vec![],
            invariant_violations: vec![],
            score: 0,
            minimization: None,
            elapsed_ms: 0,
        };
        let config = MinimizationConfig {
            enabled: true,
            mode: "operator-specific".to_string(),
        };
        let minimized = minimize_record(&record, Some(tmp.path()), &config)
            .expect("minimize")
            .expect("minimization result");

        assert_eq!(
            minimized.minimal_delta,
            vec!["tx_input[1]".to_string(), "tx_input[0]".to_string()]
        );
        assert_eq!(minimized.operator.as_deref(), Some("state_input_rewire"));
        assert_eq!(
            minimized.minimizer.as_deref(),
            Some("operator:state_input_rewire")
        );
        assert_eq!(minimized.mode, "operator-specific");
        assert_eq!(minimized.minimized_to, 2);
        assert!(tmp.path().join("minimization.json").exists());
    }

    #[test]
    fn record_fail_then_heal_detection_ignores_negative_status_labels() {
        let record = RunRecord {
            target: Target {
                digest: "d".to_string(),
                checkpoint: 1,
                source: None,
                label: None,
            },
            status: "no_fail_then_heal_found".to_string(),
            run_dir: None,
            report_path: None,
            chosen: Some(serde_json::json!({
                "baseline": {"local_success": false},
                "heal": {"local_success": false}
            })),
            oracle_hits: vec![],
            invariant_violations: vec![],
            score: 0,
            minimization: None,
            elapsed_ms: 0,
        };

        assert!(!record_has_fail_then_heal(&record));
        let (oracles, _invariants) = evaluate_record(
            &record,
            &StrategyConfig {
                name: "s".to_string(),
                mutators: vec!["state_drop_required_object".to_string()],
                oracles: vec!["fail_to_heal".to_string()],
                invariants: vec![],
                scoring: "status-first".to_string(),
                minimization: MinimizationConfig::default(),
            },
        );
        assert!(oracles.is_empty());
        let score = score_record(&record, &[], &[], "status-first");
        assert_eq!(score, 60);
    }
}
