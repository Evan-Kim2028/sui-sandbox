//! Reusable checkpoint-source PTB universe engine.
//!
//! End-to-end flow:
//! 1) Pull checkpoints from a source (`walrus` or `grpc-stream`)
//! 2) Build package/function universe from observed PTBs
//! 3) Fetch top packages + dependency closure from GraphQL
//! 4) Generate mock PTBs from observed MoveCall signatures
//! 5) Execute them locally in SimulationEnvironment
//! 6) Write JSON artifacts for inspection

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::file_format::{
    CompiledModule, DatatypeHandleIndex, SignatureToken, Visibility,
};
use move_core_types::account_address::AccountAddress;
use move_core_types::u256::U256;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ptb::PTBBuilder;
use crate::simulation::SimulationEnvironment;
use sui_resolver::is_framework_address;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcCheckpoint, GrpcClient, GrpcCommand, GrpcInput, GrpcTransaction};
use sui_transport::walrus::WalrusClient;
use sui_types::full_checkpoint_content::{CheckpointData, CheckpointTransaction};
use sui_types::transaction::{
    CallArg, Command as SuiCommand, ObjectArg, TransactionDataAPI, TransactionKind,
};

pub const DEFAULT_LATEST: u64 = 10;
pub const DEFAULT_TOP_PACKAGES: usize = 8;
pub const DEFAULT_MAX_PTBS: usize = 20;
pub const DEFAULT_STREAM_TIMEOUT_SECS: u64 = 120;
const MAX_DEP_ROUNDS: usize = 8;
const BATCH_CHUNK_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Args {
    pub source: CheckpointSource,
    pub latest: u64,
    pub top_packages: usize,
    pub max_ptbs: usize,
    pub out_dir: PathBuf,
    pub grpc_endpoint: Option<String>,
    pub stream_timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointSource {
    Walrus,
    GrpcStream,
}

impl CheckpointSource {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "walrus" => Ok(Self::Walrus),
            "grpc-stream" | "grpc_stream" => Ok(Self::GrpcStream),
            other => Err(anyhow!(
                "invalid --source value '{other}' (expected: walrus, grpc-stream)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Walrus => "walrus",
            Self::GrpcStream => "grpc-stream",
        }
    }
}

#[derive(Debug)]
enum LoadedCheckpoints {
    Walrus(Vec<(u64, CheckpointData)>),
    GrpcStream(Vec<(u64, GrpcCheckpoint)>),
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct FunctionKey {
    package: String,
    module: String,
    function: String,
}

#[derive(Debug, Clone)]
struct ObservedFunction {
    key: FunctionKey,
    observed_calls: usize,
}

#[derive(Debug)]
struct UniverseStats {
    start_checkpoint: u64,
    end_checkpoint: u64,
    transactions_total: usize,
    ptb_transactions: usize,
    ptb_app_transactions: usize,
    tag_counts: BTreeMap<String, usize>,
    package_counts: BTreeMap<String, usize>,
    function_counts: BTreeMap<FunctionKey, usize>,
}

#[derive(Debug, Serialize)]
struct UniverseSummary {
    start_checkpoint: u64,
    end_checkpoint: u64,
    checkpoints_loaded: usize,
    transactions_total: usize,
    ptb_transactions: usize,
    ptb_app_transactions: usize,
    top_tags: Vec<CountRow>,
    top_packages: Vec<CountRow>,
    top_functions: Vec<FunctionCountRow>,
}

#[derive(Debug, Serialize)]
struct CountRow {
    key: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct FunctionCountRow {
    package: String,
    module: String,
    function: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct PackageFetchRecord {
    address: String,
    source: String,
    fetch_mode: String,
    version: Option<u64>,
    module_count: usize,
    deployed: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum MockArgPlan {
    Pure {
        move_type: String,
        value: String,
        bcs_bytes: Vec<u8>,
    },
    Clock {
        move_type: String,
    },
    Random {
        move_type: String,
    },
    TxContext {
        move_type: String,
    },
}

#[derive(Debug, Clone)]
struct FunctionPlan {
    source: String,
    package_addr: AccountAddress,
    package: String,
    module: String,
    function: String,
    observed_calls: usize,
    visibility: String,
    is_entry: bool,
    type_param_count: usize,
    args: Vec<MockArgPlan>,
    skip_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct FunctionCandidateRecord {
    source: String,
    package: String,
    module: String,
    function: String,
    observed_calls: usize,
    visibility: String,
    is_entry: bool,
    type_param_count: usize,
    accepted: bool,
    skip_reason: Option<String>,
    args: Vec<ArgSpec>,
}

#[derive(Debug, Serialize)]
struct ArgSpec {
    kind: String,
    move_type: String,
    mock_value: Option<String>,
    bcs_base64: Option<String>,
}

#[derive(Debug, Serialize)]
struct PtbSpecFile {
    source: String,
    package: String,
    module: String,
    function: String,
    observed_calls: usize,
    args: Vec<ArgSpec>,
}

#[derive(Debug, Serialize)]
struct PtbExecutionRecord {
    source: String,
    package: String,
    module: String,
    function: String,
    observed_calls: usize,
    spec_file: String,
    success: bool,
    commands_succeeded: usize,
    failed_command_index: Option<usize>,
    failed_command_description: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PtbClassification {
    digest: String,
    checkpoint: Option<u64>,
    tags: Vec<String>,
    is_framework_only: bool,
    is_trivial_framework: bool,
    non_system_packages: Vec<String>,
    system_packages: Vec<String>,
    has_publish: bool,
    has_upgrade: bool,
    has_shared_inputs: bool,
    has_receiving_inputs: bool,
    command_kinds: Vec<String>,
}

/// Parse CLI args from `std::env::args()` and run the PTB universe flow.
pub fn run_cli() -> Result<()> {
    let args = parse_args()?;
    run(args)
}

/// Run the PTB universe flow with explicit config.
pub fn run_with_args(args: Args) -> Result<()> {
    run(args)
}

fn run(args: Args) -> Result<()> {
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create output dir {}", args.out_dir.display()))?;

    println!("=== Checkpoint PTB Universe Example ===");
    println!("source: {}", args.source.as_str());
    println!("latest checkpoints: {}", args.latest);
    println!("top packages: {}", args.top_packages);
    println!("max PTBs to execute: {}", args.max_ptbs);
    if args.source == CheckpointSource::GrpcStream {
        println!("stream timeout (secs): {}", args.stream_timeout_secs);
        if let Some(endpoint) = args.grpc_endpoint.as_deref() {
            println!("grpc endpoint: {}", endpoint);
        } else {
            println!("grpc endpoint: <default from SUI_GRPC_ENDPOINT or archive.mainnet.sui.io>");
        }
    }
    println!("output dir: {}", args.out_dir.display());

    let loaded = load_checkpoints(&args)?;
    let (universe, checkpoints_loaded, latest_cp) = match &loaded {
        LoadedCheckpoints::Walrus(checkpoints) => {
            let start = checkpoints.first().map(|(cp, _)| *cp).unwrap_or(0);
            let end = checkpoints.last().map(|(cp, _)| *cp).unwrap_or(0);
            (
                analyze_universe_walrus(checkpoints, start, end),
                checkpoints.len(),
                end,
            )
        }
        LoadedCheckpoints::GrpcStream(checkpoints) => {
            let start = checkpoints.first().map(|(cp, _)| *cp).unwrap_or(0);
            let end = checkpoints.last().map(|(cp, _)| *cp).unwrap_or(0);
            (
                analyze_universe_grpc(checkpoints, start, end),
                checkpoints.len(),
                end,
            )
        }
    };
    let summary = universe_summary(&universe, checkpoints_loaded);
    write_json(args.out_dir.join("universe_summary.json"), &summary)?;

    let top_packages = top_package_addrs(&universe.package_counts, args.top_packages);

    println!(
        "downloading {} top package(s) + dependency closure...",
        top_packages.len()
    );

    let graphql = GraphQLClient::mainnet();
    let mut env = SimulationEnvironment::new().context("init simulation environment")?;
    let sender = AccountAddress::from_hex_literal("0xa11ce").unwrap_or(AccountAddress::ONE);
    env.set_sender(sender);

    let mut fetch_records = Vec::new();
    let mut deployed = BTreeSet::new();

    for package in &top_packages {
        fetch_and_deploy_package(
            &mut env,
            &graphql,
            *package,
            latest_cp,
            "top_package",
            &mut deployed,
            &mut fetch_records,
        );
    }

    fetch_dependency_closure(
        &mut env,
        &graphql,
        latest_cp,
        &mut deployed,
        &mut fetch_records,
    )?;

    write_json(args.out_dir.join("package_downloads.json"), &fetch_records)?;

    let top_package_hex: BTreeSet<String> = top_packages
        .iter()
        .map(canonical_address)
        .collect::<BTreeSet<_>>();

    let observed = select_observed_functions(&universe.function_counts, &top_package_hex);
    let planning_limit = args.max_ptbs.saturating_mul(5).max(args.max_ptbs);

    println!(
        "planning mock PTBs from observed universe (candidates: up to {})...",
        planning_limit
    );

    let mut candidates = Vec::new();
    let mut plans_to_execute = Vec::new();
    let mut seen_functions = BTreeSet::new();

    for observed_fn in observed.into_iter().take(planning_limit) {
        let plan = plan_function(&mut env, observed_fn);
        seen_functions.insert(FunctionKey {
            package: plan.package.clone(),
            module: plan.module.clone(),
            function: plan.function.clone(),
        });
        let candidate = plan_to_candidate_record(&plan);
        if plan.skip_reason.is_none() {
            plans_to_execute.push(plan.clone());
        }
        candidates.push(candidate);
    }

    if plans_to_execute.len() < args.max_ptbs {
        let needed = args.max_ptbs - plans_to_execute.len();
        let fallback_limit = needed.saturating_mul(4).max(needed);
        println!(
            "observed universe produced {} executable PTB(s), scanning loaded package functions for {} more...",
            plans_to_execute.len(),
            needed
        );

        let fallback_plans = discover_callable_fallback_plans(
            &mut env,
            &top_packages,
            &seen_functions,
            fallback_limit,
        );

        for plan in fallback_plans {
            seen_functions.insert(FunctionKey {
                package: plan.package.clone(),
                module: plan.module.clone(),
                function: plan.function.clone(),
            });
            candidates.push(plan_to_candidate_record(&plan));
            plans_to_execute.push(plan);
            if plans_to_execute.len() >= args.max_ptbs {
                break;
            }
        }
    }

    write_json(args.out_dir.join("function_candidates.json"), &candidates)?;

    let baseline = env.create_checkpoint();
    let specs_dir = args.out_dir.join("ptb_specs");
    std::fs::create_dir_all(&specs_dir)
        .with_context(|| format!("create specs dir {}", specs_dir.display()))?;

    let mut executions = Vec::new();
    for (idx, plan) in plans_to_execute.into_iter().take(args.max_ptbs).enumerate() {
        env.restore_checkpoint(baseline.clone());

        let spec = spec_from_plan(&plan);
        let spec_file_name = format!(
            "{:03}_{}_{}_{}_{}.json",
            idx,
            sanitize_for_filename(&plan.source),
            short_package(&plan.package),
            sanitize_for_filename(&plan.module),
            sanitize_for_filename(&plan.function),
        );
        let spec_path = specs_dir.join(&spec_file_name);
        write_json(&spec_path, &spec)?;

        let execution = execute_plan(&mut env, &plan, format!("ptb_specs/{spec_file_name}"))?;
        executions.push(execution);
    }

    write_json(args.out_dir.join("ptb_execution_results.json"), &executions)?;
    write_output_readme(&args, &summary, &fetch_records, &candidates, &executions)?;

    let success = executions.iter().filter(|r| r.success).count();
    let failed = executions.len().saturating_sub(success);

    println!("\n=== Completed ===");
    println!("Checkpoints analyzed: {}", checkpoints_loaded);
    println!("Top packages targeted: {}", top_packages.len());
    println!("Candidate functions planned: {}", candidates.len());
    println!("PTBs executed: {}", executions.len());
    println!("PTB success: {}", success);
    println!("PTB failed: {}", failed);
    println!("Artifacts: {}", args.out_dir.display());

    Ok(())
}

fn load_checkpoints(args: &Args) -> Result<LoadedCheckpoints> {
    match args.source {
        CheckpointSource::Walrus => load_walrus_checkpoints(args.latest),
        CheckpointSource::GrpcStream => load_grpc_stream_checkpoints(
            args.latest,
            args.grpc_endpoint.as_deref(),
            args.stream_timeout_secs,
        ),
    }
}

fn load_walrus_checkpoints(latest: u64) -> Result<LoadedCheckpoints> {
    let walrus = WalrusClient::mainnet();
    let latest_cp = walrus
        .get_latest_checkpoint()
        .context("failed to get latest Walrus checkpoint")?;
    let start_cp = latest_cp.saturating_sub(latest.saturating_sub(1));
    let checkpoints: Vec<u64> = (start_cp..=latest_cp).collect();

    println!(
        "fetching checkpoints {}..{} from Walrus ({} total)...",
        start_cp,
        latest_cp,
        checkpoints.len()
    );

    let mut loaded = walrus
        .get_checkpoints_batched(&checkpoints, BATCH_CHUNK_BYTES)
        .context("failed to fetch checkpoints from Walrus")?;
    loaded.sort_by_key(|(cp, _)| *cp);
    Ok(LoadedCheckpoints::Walrus(loaded))
}

fn load_grpc_stream_checkpoints(
    latest: u64,
    endpoint: Option<&str>,
    stream_timeout_secs: u64,
) -> Result<LoadedCheckpoints> {
    let endpoint_owned = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let runtime = tokio::runtime::Runtime::new().context("create tokio runtime for gRPC stream")?;
    let checkpoints = runtime.block_on(async move {
        let client = match endpoint_owned.as_deref() {
            Some(ep) => {
                println!("connecting to gRPC stream endpoint: {ep}");
                GrpcClient::new(ep).await?
            }
            None => {
                println!("connecting to gRPC stream endpoint via default resolver");
                GrpcClient::mainnet().await?
            }
        };

        let resolved_endpoint = client.endpoint().to_string();
        println!("subscribing to checkpoint stream at {resolved_endpoint}...");
        let mut stream = client.subscribe_checkpoints().await.map_err(|err| {
            anyhow!(
                "failed to subscribe checkpoints via gRPC endpoint {}: {} (if this endpoint does not support subscriptions, try --grpc-endpoint https://fullnode.mainnet.sui.io:443)",
                resolved_endpoint,
                err
            )
        })?;

        let timeout = Duration::from_secs(stream_timeout_secs.max(1));
        let started = Instant::now();
        let target = latest as usize;
        let mut seen = BTreeSet::new();
        let mut out = Vec::with_capacity(target);

        while out.len() < target {
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return Err(anyhow!(
                    "timed out after {}s while waiting for {} streamed checkpoints (collected {})",
                    timeout.as_secs(),
                    target,
                    out.len()
                ));
            }
            let wait = timeout.saturating_sub(elapsed);
            let next = tokio::time::timeout(wait, stream.next())
                .await
                .map_err(|_| {
                    anyhow!(
                        "timed out after {}s while waiting for checkpoint stream data",
                        timeout.as_secs()
                    )
                })?;
            match next {
                Some(Ok(checkpoint)) => {
                    if seen.insert(checkpoint.sequence_number) {
                        out.push((checkpoint.sequence_number, checkpoint));
                    }
                }
                Some(Err(err)) => {
                    return Err(anyhow!("gRPC checkpoint stream error: {}", err));
                }
                None => {
                    return Err(anyhow!(
                        "gRPC checkpoint stream ended before collecting {} checkpoints",
                        target
                    ));
                }
            }
        }

        out.sort_by_key(|(cp, _)| *cp);
        Ok::<Vec<(u64, GrpcCheckpoint)>, anyhow::Error>(out)
    })?;

    Ok(LoadedCheckpoints::GrpcStream(checkpoints))
}

fn parse_args() -> Result<Args> {
    let mut source = CheckpointSource::Walrus;
    let mut latest = DEFAULT_LATEST;
    let mut top_packages = DEFAULT_TOP_PACKAGES;
    let mut max_ptbs = DEFAULT_MAX_PTBS;
    let mut out_dir = PathBuf::from("examples/out/walrus_ptb_universe");
    let mut grpc_endpoint = None;
    let mut stream_timeout_secs = DEFAULT_STREAM_TIMEOUT_SECS;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--source" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--source requires a value"))?;
                source = CheckpointSource::parse(&v)?;
            }
            "--latest" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--latest requires a value"))?;
                latest = v
                    .parse::<u64>()
                    .with_context(|| format!("invalid --latest value: {v}"))?;
            }
            "--top-packages" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--top-packages requires a value"))?;
                top_packages = v
                    .parse::<usize>()
                    .with_context(|| format!("invalid --top-packages value: {v}"))?;
            }
            "--max-ptbs" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--max-ptbs requires a value"))?;
                max_ptbs = v
                    .parse::<usize>()
                    .with_context(|| format!("invalid --max-ptbs value: {v}"))?;
            }
            "--out-dir" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--out-dir requires a value"))?;
                out_dir = PathBuf::from(v);
            }
            "--grpc-endpoint" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--grpc-endpoint requires a value"))?;
                grpc_endpoint = Some(v);
            }
            "--stream-timeout-secs" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("--stream-timeout-secs requires a value"))?;
                stream_timeout_secs = v
                    .parse::<u64>()
                    .with_context(|| format!("invalid --stream-timeout-secs value: {v}"))?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(anyhow!("unknown argument '{other}'. Use --help for usage."));
            }
        }
    }

    if latest == 0 {
        return Err(anyhow!("--latest must be > 0"));
    }
    if top_packages == 0 {
        return Err(anyhow!("--top-packages must be > 0"));
    }
    if max_ptbs == 0 {
        return Err(anyhow!("--max-ptbs must be > 0"));
    }
    if stream_timeout_secs == 0 {
        return Err(anyhow!("--stream-timeout-secs must be > 0"));
    }

    Ok(Args {
        source,
        latest,
        top_packages,
        max_ptbs,
        out_dir,
        grpc_endpoint,
        stream_timeout_secs,
    })
}

fn print_usage() {
    println!(
        "Checkpoint-source PTB universe example\n\n\
Usage:\n  cargo run --example walrus_ptb_universe -- [OPTIONS]\n\n\
Options:\n  --source SRC         Checkpoint source: walrus | grpc-stream (default: walrus)\n  --latest N           Number of checkpoints to analyze/collect (default: {DEFAULT_LATEST})\n  --top-packages N     Number of top packages to fetch (default: {DEFAULT_TOP_PACKAGES})\n  --max-ptbs N         Max generated PTBs to execute (default: {DEFAULT_MAX_PTBS})\n  --out-dir PATH       Output directory (default: examples/out/walrus_ptb_universe)\n  --grpc-endpoint URL  gRPC endpoint for --source grpc-stream (default: env/default resolver)\n  --stream-timeout-secs N  Max seconds to wait for streaming checkpoints (default: {DEFAULT_STREAM_TIMEOUT_SECS})\n  --help               Show this help\n"
    );
}

fn analyze_universe_walrus(
    checkpoints: &[(u64, CheckpointData)],
    start_checkpoint: u64,
    end_checkpoint: u64,
) -> UniverseStats {
    let mut stats = UniverseStats {
        start_checkpoint,
        end_checkpoint,
        transactions_total: 0,
        ptb_transactions: 0,
        ptb_app_transactions: 0,
        tag_counts: BTreeMap::new(),
        package_counts: BTreeMap::new(),
        function_counts: BTreeMap::new(),
    };

    for (checkpoint_num, checkpoint_data) in checkpoints {
        for tx in &checkpoint_data.transactions {
            stats.transactions_total += 1;

            if let Some(classification) = classify_walrus_checkpoint_tx(tx, *checkpoint_num) {
                stats.ptb_transactions += 1;
                if !classification.is_framework_only {
                    stats.ptb_app_transactions += 1;
                }
                for tag in classification.tags {
                    *stats.tag_counts.entry(tag).or_insert(0) += 1;
                }
                for pkg in classification.non_system_packages {
                    if let Some(norm) = normalize_package_opt(&pkg) {
                        *stats.package_counts.entry(norm).or_insert(0) += 1;
                    }
                }
            }

            let tx_data = tx.transaction.data().intent_message().value.clone();
            let ptb = match tx_data.kind() {
                TransactionKind::ProgrammableTransaction(ptb) => ptb,
                _ => continue,
            };

            for command in &ptb.commands {
                let SuiCommand::MoveCall(call) = command else {
                    continue;
                };

                let package = normalize_package(&call.package.to_hex_uncompressed());
                if is_system_package_hex(&package) {
                    continue;
                }

                let key = FunctionKey {
                    package,
                    module: call.module.to_string(),
                    function: call.function.to_string(),
                };
                *stats.function_counts.entry(key).or_insert(0) += 1;
            }
        }
    }

    stats
}

fn analyze_universe_grpc(
    checkpoints: &[(u64, GrpcCheckpoint)],
    start_checkpoint: u64,
    end_checkpoint: u64,
) -> UniverseStats {
    let mut stats = UniverseStats {
        start_checkpoint,
        end_checkpoint,
        transactions_total: 0,
        ptb_transactions: 0,
        ptb_app_transactions: 0,
        tag_counts: BTreeMap::new(),
        package_counts: BTreeMap::new(),
        function_counts: BTreeMap::new(),
    };

    for (checkpoint_num, checkpoint_data) in checkpoints {
        for tx in &checkpoint_data.transactions {
            stats.transactions_total += 1;

            if let Some(classification) = classify_grpc_checkpoint_tx(tx, *checkpoint_num) {
                stats.ptb_transactions += 1;
                if !classification.is_framework_only {
                    stats.ptb_app_transactions += 1;
                }
                for tag in classification.tags {
                    *stats.tag_counts.entry(tag).or_insert(0) += 1;
                }
                for pkg in classification.non_system_packages {
                    *stats.package_counts.entry(pkg).or_insert(0) += 1;
                }
            }

            if !tx.is_ptb() {
                continue;
            }

            for command in &tx.commands {
                let GrpcCommand::MoveCall {
                    package,
                    module,
                    function,
                    ..
                } = command
                else {
                    continue;
                };
                if package.trim().is_empty() {
                    continue;
                }

                let package = normalize_package(package);
                if is_system_package_hex(&package) {
                    continue;
                }

                let key = FunctionKey {
                    package,
                    module: module.clone(),
                    function: function.clone(),
                };
                *stats.function_counts.entry(key).or_insert(0) += 1;
            }
        }
    }

    stats
}

fn classify_grpc_checkpoint_tx(
    tx: &GrpcTransaction,
    checkpoint_num: u64,
) -> Option<PtbClassification> {
    if !tx.is_ptb() {
        return None;
    }

    let mut system_packages: BTreeSet<String> = BTreeSet::new();
    let mut non_system_packages: BTreeSet<String> = BTreeSet::new();
    let mut command_kinds: BTreeSet<String> = BTreeSet::new();
    let mut has_publish = false;
    let mut has_upgrade = false;

    for command in &tx.commands {
        match command {
            GrpcCommand::MoveCall { package, .. } => {
                command_kinds.insert("MoveCall".to_string());
                if package.trim().is_empty() {
                    continue;
                }
                let norm = normalize_package(package);
                if is_system_package_hex(&norm) {
                    system_packages.insert(norm);
                } else {
                    non_system_packages.insert(norm);
                }
            }
            GrpcCommand::SplitCoins { .. } => {
                command_kinds.insert("SplitCoins".to_string());
            }
            GrpcCommand::MergeCoins { .. } => {
                command_kinds.insert("MergeCoins".to_string());
            }
            GrpcCommand::TransferObjects { .. } => {
                command_kinds.insert("TransferObjects".to_string());
            }
            GrpcCommand::MakeMoveVec { .. } => {
                command_kinds.insert("MakeMoveVec".to_string());
            }
            GrpcCommand::Publish { .. } => {
                command_kinds.insert("Publish".to_string());
                has_publish = true;
            }
            GrpcCommand::Upgrade { .. } => {
                command_kinds.insert("Upgrade".to_string());
                has_upgrade = true;
            }
        }
    }

    let mut has_shared_inputs = false;
    let mut has_receiving_inputs = false;
    for input in &tx.inputs {
        match input {
            GrpcInput::SharedObject { .. } => has_shared_inputs = true,
            GrpcInput::Receiving { .. } => has_receiving_inputs = true,
            _ => {}
        }
    }

    let is_framework_only = non_system_packages.is_empty();
    let simple_cmds_only = command_kinds.iter().all(|k| {
        matches!(
            k.as_str(),
            "MoveCall" | "SplitCoins" | "MergeCoins" | "TransferObjects" | "MakeMoveVec"
        )
    });
    let is_trivial_framework =
        is_framework_only && simple_cmds_only && !has_publish && !has_upgrade && !has_shared_inputs;

    let mut tags = Vec::new();
    if is_framework_only {
        tags.push("framework_only".to_string());
    } else {
        tags.push("app_call".to_string());
    }
    if has_publish {
        tags.push("publish".to_string());
    }
    if has_upgrade {
        tags.push("upgrade".to_string());
    }
    if has_shared_inputs {
        tags.push("shared".to_string());
    }
    if has_receiving_inputs {
        tags.push("receiving".to_string());
    }
    if non_system_packages.len() > 1 {
        tags.push("cross_package".to_string());
    }
    if simple_cmds_only {
        tags.push("simple_cmds_only".to_string());
    }
    if is_trivial_framework {
        tags.push("trivial_framework".to_string());
    }

    Some(PtbClassification {
        digest: tx.digest.clone(),
        checkpoint: Some(checkpoint_num),
        tags,
        is_framework_only,
        is_trivial_framework,
        non_system_packages: non_system_packages.into_iter().collect(),
        system_packages: system_packages.into_iter().collect(),
        has_publish,
        has_upgrade,
        has_shared_inputs,
        has_receiving_inputs,
        command_kinds: command_kinds.into_iter().collect(),
    })
}

fn classify_walrus_checkpoint_tx(
    tx: &CheckpointTransaction,
    checkpoint_num: u64,
) -> Option<PtbClassification> {
    let tx_data = tx.transaction.data().intent_message().value.clone();
    let digest = tx.transaction.digest().to_string();

    let ptb = match tx_data.kind() {
        TransactionKind::ProgrammableTransaction(ptb) => ptb,
        _ => return None,
    };

    let mut system_packages: BTreeSet<String> = BTreeSet::new();
    let mut non_system_packages: BTreeSet<String> = BTreeSet::new();
    let mut command_kinds: BTreeSet<String> = BTreeSet::new();
    let mut has_publish = false;
    let mut has_upgrade = false;

    for command in &ptb.commands {
        match command {
            SuiCommand::MoveCall(call) => {
                command_kinds.insert("MoveCall".to_string());
                let package = normalize_package(&call.package.to_hex_uncompressed());
                if is_framework_address(&package) {
                    system_packages.insert(package);
                } else {
                    non_system_packages.insert(package);
                }
            }
            SuiCommand::SplitCoins(..) => {
                command_kinds.insert("SplitCoins".to_string());
            }
            SuiCommand::MergeCoins(..) => {
                command_kinds.insert("MergeCoins".to_string());
            }
            SuiCommand::TransferObjects(..) => {
                command_kinds.insert("TransferObjects".to_string());
            }
            SuiCommand::MakeMoveVec(..) => {
                command_kinds.insert("MakeMoveVec".to_string());
            }
            SuiCommand::Publish(..) => {
                command_kinds.insert("Publish".to_string());
                has_publish = true;
            }
            SuiCommand::Upgrade(..) => {
                command_kinds.insert("Upgrade".to_string());
                has_upgrade = true;
            }
        }
    }

    let mut has_shared_inputs = false;
    let mut has_receiving_inputs = false;
    for input in &ptb.inputs {
        match input {
            CallArg::Object(ObjectArg::SharedObject { .. }) => has_shared_inputs = true,
            CallArg::Object(ObjectArg::Receiving(..)) => has_receiving_inputs = true,
            _ => {}
        }
    }

    let is_framework_only = non_system_packages.is_empty();
    let simple_cmds_only = command_kinds.iter().all(|k| {
        matches!(
            k.as_str(),
            "MoveCall" | "SplitCoins" | "MergeCoins" | "TransferObjects" | "MakeMoveVec"
        )
    });
    let is_trivial_framework =
        is_framework_only && simple_cmds_only && !has_publish && !has_upgrade && !has_shared_inputs;

    let mut tags = Vec::new();
    if is_framework_only {
        tags.push("framework_only".to_string());
    } else {
        tags.push("app_call".to_string());
    }
    if has_publish {
        tags.push("publish".to_string());
    }
    if has_upgrade {
        tags.push("upgrade".to_string());
    }
    if has_shared_inputs {
        tags.push("shared".to_string());
    }
    if has_receiving_inputs {
        tags.push("receiving".to_string());
    }
    if non_system_packages.len() > 1 {
        tags.push("cross_package".to_string());
    }
    if simple_cmds_only {
        tags.push("simple_cmds_only".to_string());
    }
    if is_trivial_framework {
        tags.push("trivial_framework".to_string());
    }

    Some(PtbClassification {
        digest,
        checkpoint: Some(checkpoint_num),
        tags,
        is_framework_only,
        is_trivial_framework,
        non_system_packages: non_system_packages.into_iter().collect(),
        system_packages: system_packages.into_iter().collect(),
        has_publish,
        has_upgrade,
        has_shared_inputs,
        has_receiving_inputs,
        command_kinds: command_kinds.into_iter().collect(),
    })
}

fn universe_summary(stats: &UniverseStats, checkpoints_loaded: usize) -> UniverseSummary {
    let top_tags = top_count_rows(&stats.tag_counts, 20);
    let top_packages = top_count_rows(&stats.package_counts, 25);

    let mut funcs: Vec<(FunctionKey, usize)> = stats
        .function_counts
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    funcs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let top_functions = funcs
        .into_iter()
        .take(30)
        .map(|(key, count)| FunctionCountRow {
            package: key.package,
            module: key.module,
            function: key.function,
            count,
        })
        .collect();

    UniverseSummary {
        start_checkpoint: stats.start_checkpoint,
        end_checkpoint: stats.end_checkpoint,
        checkpoints_loaded,
        transactions_total: stats.transactions_total,
        ptb_transactions: stats.ptb_transactions,
        ptb_app_transactions: stats.ptb_app_transactions,
        top_tags,
        top_packages,
        top_functions,
    }
}

fn top_count_rows(map: &BTreeMap<String, usize>, limit: usize) -> Vec<CountRow> {
    let mut rows: Vec<CountRow> = map
        .iter()
        .map(|(k, v)| CountRow {
            key: k.clone(),
            count: *v,
        })
        .collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    rows.truncate(limit);
    rows
}

fn top_package_addrs(counts: &BTreeMap<String, usize>, limit: usize) -> Vec<AccountAddress> {
    let mut rows: Vec<(String, usize)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    rows.into_iter()
        .filter_map(|(pkg, _)| AccountAddress::from_hex_literal(&pkg).ok())
        .filter(|addr| !is_system_package_addr(addr))
        .take(limit)
        .collect()
}

fn fetch_and_deploy_package(
    env: &mut SimulationEnvironment,
    graphql: &GraphQLClient,
    address: AccountAddress,
    checkpoint: u64,
    source: &str,
    deployed: &mut BTreeSet<AccountAddress>,
    records: &mut Vec<PackageFetchRecord>,
) {
    if deployed.contains(&address) {
        return;
    }

    let addr_hex = canonical_address(&address);

    let (package, fetch_mode) = match graphql.fetch_package_at_checkpoint(&addr_hex, checkpoint) {
        Ok(pkg) => (pkg, "checkpoint".to_string()),
        Err(_) => match graphql.fetch_package(&addr_hex) {
            Ok(pkg) => (pkg, "latest".to_string()),
            Err(err) => {
                records.push(PackageFetchRecord {
                    address: addr_hex,
                    source: source.to_string(),
                    fetch_mode: "failed".to_string(),
                    version: None,
                    module_count: 0,
                    deployed: false,
                    error: Some(err.to_string()),
                });
                return;
            }
        },
    };

    let version = Some(package.version);
    let modules = match sui_transport::decode_graphql_modules(&addr_hex, &package.modules) {
        Ok(m) if m.is_empty() => {
            records.push(PackageFetchRecord {
                address: addr_hex,
                source: source.to_string(),
                fetch_mode,
                version,
                module_count: 0,
                deployed: false,
                error: Some("package has zero decodable modules".to_string()),
            });
            return;
        }
        Ok(m) => m,
        Err(err) => {
            records.push(PackageFetchRecord {
                address: addr_hex,
                source: source.to_string(),
                fetch_mode,
                version,
                module_count: 0,
                deployed: false,
                error: Some(err.to_string()),
            });
            return;
        }
    };

    match env.deploy_package_at_address(&addr_hex, modules.clone()) {
        Ok(_) => {
            deployed.insert(address);
            records.push(PackageFetchRecord {
                address: addr_hex,
                source: source.to_string(),
                fetch_mode,
                version,
                module_count: modules.len(),
                deployed: true,
                error: None,
            });
        }
        Err(err) => {
            records.push(PackageFetchRecord {
                address: addr_hex,
                source: source.to_string(),
                fetch_mode,
                version,
                module_count: modules.len(),
                deployed: false,
                error: Some(err.to_string()),
            });
        }
    }
}

fn fetch_dependency_closure(
    env: &mut SimulationEnvironment,
    graphql: &GraphQLClient,
    checkpoint: u64,
    deployed: &mut BTreeSet<AccountAddress>,
    records: &mut Vec<PackageFetchRecord>,
) -> Result<()> {
    let mut seen = BTreeSet::new();

    for _ in 0..MAX_DEP_ROUNDS {
        let pending: Vec<AccountAddress> = {
            let missing = env.resolver_mut().get_missing_dependencies();
            missing
                .into_iter()
                .filter(|addr| !seen.contains(addr))
                .collect()
        };

        if pending.is_empty() {
            break;
        }

        for addr in pending {
            seen.insert(addr);
            if is_system_package_addr(&addr) {
                continue;
            }
            fetch_and_deploy_package(
                env,
                graphql,
                addr,
                checkpoint,
                "dependency",
                deployed,
                records,
            );
        }
    }

    Ok(())
}

fn select_observed_functions(
    function_counts: &BTreeMap<FunctionKey, usize>,
    allowed_packages: &BTreeSet<String>,
) -> Vec<ObservedFunction> {
    let mut rows: Vec<ObservedFunction> = function_counts
        .iter()
        .filter(|(key, _)| allowed_packages.contains(&key.package))
        .map(|(key, count)| ObservedFunction {
            key: key.clone(),
            observed_calls: *count,
        })
        .collect();

    rows.sort_by(|a, b| {
        b.observed_calls
            .cmp(&a.observed_calls)
            .then_with(|| a.key.cmp(&b.key))
    });

    rows
}

fn discover_callable_fallback_plans(
    env: &mut SimulationEnvironment,
    packages: &[AccountAddress],
    seen: &BTreeSet<FunctionKey>,
    limit: usize,
) -> Vec<FunctionPlan> {
    let mut out = Vec::new();
    let mut seen_local = seen.clone();

    for package in packages {
        let package_hex = canonical_address(package);
        let module_names = {
            let resolver = env.resolver_mut();
            resolver.get_package_modules(package)
        };

        for module_name in module_names {
            let function_names = {
                let resolver = env.resolver_mut();
                let Some(module) = resolver.get_module_by_addr_name(package, &module_name) else {
                    continue;
                };

                module
                    .function_defs
                    .iter()
                    .map(|def| {
                        let handle = &module.function_handles[def.function.0 as usize];
                        module.identifier_at(handle.name).to_string()
                    })
                    .collect::<Vec<String>>()
            };

            for function_name in function_names {
                let key = FunctionKey {
                    package: package_hex.clone(),
                    module: module_name.clone(),
                    function: function_name.clone(),
                };
                if !seen_local.insert(key.clone()) {
                    continue;
                }

                let observed = ObservedFunction {
                    key,
                    observed_calls: 0,
                };
                let mut plan = plan_function(env, observed);
                plan.source = "package_scan".to_string();
                if plan.skip_reason.is_none() {
                    out.push(plan);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
    }

    out
}

fn plan_function(env: &mut SimulationEnvironment, observed: ObservedFunction) -> FunctionPlan {
    let package_addr = match AccountAddress::from_hex_literal(&observed.key.package) {
        Ok(addr) => addr,
        Err(err) => {
            return FunctionPlan {
                source: "observed_universe".to_string(),
                package_addr: AccountAddress::ZERO,
                package: observed.key.package,
                module: observed.key.module,
                function: observed.key.function,
                observed_calls: observed.observed_calls,
                visibility: "unknown".to_string(),
                is_entry: false,
                type_param_count: 0,
                args: vec![],
                skip_reason: Some(format!("invalid package address: {err}")),
            }
        }
    };

    let mut plan = FunctionPlan {
        source: "observed_universe".to_string(),
        package_addr,
        package: observed.key.package,
        module: observed.key.module,
        function: observed.key.function,
        observed_calls: observed.observed_calls,
        visibility: "unknown".to_string(),
        is_entry: false,
        type_param_count: 0,
        args: vec![],
        skip_reason: None,
    };

    let (visibility, is_entry, type_param_count, params) = {
        let resolver = env.resolver_mut();
        let Some(module) = resolver.get_module_by_addr_name(&package_addr, &plan.module) else {
            return FunctionPlan {
                source: "observed_universe".to_string(),
                package_addr,
                package: plan.package,
                module: plan.module,
                function: plan.function,
                observed_calls: plan.observed_calls,
                visibility: "unknown".to_string(),
                is_entry: false,
                type_param_count: 0,
                args: vec![],
                skip_reason: Some("module not loaded in resolver".to_string()),
            };
        };

        let mut found = None;
        for def in &module.function_defs {
            let handle = &module.function_handles[def.function.0 as usize];
            let fn_name = module.identifier_at(handle.name).to_string();
            if fn_name == plan.function {
                let visibility = visibility_to_string(def.visibility);
                let is_entry = def.is_entry;
                let type_param_count = handle.type_parameters.len();
                let params = module.signatures[handle.parameters.0 as usize].0.clone();
                found = Some((visibility, is_entry, type_param_count, params));
                break;
            }
        }

        match found {
            Some((visibility, is_entry, type_param_count, params)) => {
                (visibility, is_entry, type_param_count, params)
            }
            None => ("unknown".to_string(), false, 0usize, Vec::new()),
        }
    };

    if visibility == "unknown" {
        plan.skip_reason = Some("function not found in module".to_string());
        return plan;
    }

    plan.visibility = visibility.clone();
    plan.is_entry = is_entry;
    plan.type_param_count = type_param_count;

    if type_param_count > 0 {
        plan.skip_reason = Some(format!(
            "generic function with {} type parameter(s)",
            type_param_count
        ));
        return plan;
    }

    if visibility != "public" && !is_entry {
        plan.skip_reason = Some(format!(
            "function visibility '{visibility}' is not PTB-callable"
        ));
        return plan;
    }

    let module = {
        let resolver = env.resolver_mut();
        resolver
            .get_module_by_addr_name(&package_addr, &plan.module)
            .cloned()
    };

    let Some(module) = module else {
        plan.skip_reason = Some("module disappeared during planning".to_string());
        return plan;
    };

    let mut args = Vec::new();
    for token in &params {
        match plan_token(&module, token) {
            Ok(arg) => args.push(arg),
            Err(reason) => {
                plan.skip_reason = Some(reason);
                return plan;
            }
        }
    }

    plan.args = args;
    plan
}

fn plan_to_candidate_record(plan: &FunctionPlan) -> FunctionCandidateRecord {
    let args = plan.args.iter().map(arg_to_spec).collect::<Vec<ArgSpec>>();

    FunctionCandidateRecord {
        source: plan.source.clone(),
        package: plan.package.clone(),
        module: plan.module.clone(),
        function: plan.function.clone(),
        observed_calls: plan.observed_calls,
        visibility: plan.visibility.clone(),
        is_entry: plan.is_entry,
        type_param_count: plan.type_param_count,
        accepted: plan.skip_reason.is_none(),
        skip_reason: plan.skip_reason.clone(),
        args,
    }
}

fn spec_from_plan(plan: &FunctionPlan) -> PtbSpecFile {
    PtbSpecFile {
        source: plan.source.clone(),
        package: plan.package.clone(),
        module: plan.module.clone(),
        function: plan.function.clone(),
        observed_calls: plan.observed_calls,
        args: plan.args.iter().map(arg_to_spec).collect(),
    }
}

fn execute_plan(
    env: &mut SimulationEnvironment,
    plan: &FunctionPlan,
    spec_file: String,
) -> Result<PtbExecutionRecord> {
    let mut builder = PTBBuilder::new();
    let mut call_args = Vec::new();

    for arg in &plan.args {
        match arg {
            MockArgPlan::Pure { bcs_bytes, .. } => {
                let input = builder.pure_bytes(bcs_bytes.clone());
                call_args.push(input);
            }
            MockArgPlan::Clock { .. } => {
                let clock = env.get_clock_object().context("get clock object")?;
                let input = builder.add_object_input(clock);
                call_args.push(input);
            }
            MockArgPlan::Random { .. } => {
                let random = env.get_random_object().context("get random object")?;
                let input = builder.add_object_input(random);
                call_args.push(input);
            }
            MockArgPlan::TxContext { .. } => {
                // TxContext is implicit and should not be passed as PTB input.
            }
        }
    }

    builder
        .move_call(
            plan.package_addr,
            &plan.module,
            &plan.function,
            vec![],
            call_args,
        )
        .with_context(|| {
            format!(
                "build move_call {}::{}::{}",
                plan.package, plan.module, plan.function
            )
        })?;

    let (inputs, commands) = builder.into_parts();
    let result = env.execute_ptb(inputs, commands);

    Ok(PtbExecutionRecord {
        source: plan.source.clone(),
        package: plan.package.clone(),
        module: plan.module.clone(),
        function: plan.function.clone(),
        observed_calls: plan.observed_calls,
        spec_file,
        success: result.success,
        commands_succeeded: result.commands_succeeded,
        failed_command_index: result.failed_command_index,
        failed_command_description: result.failed_command_description,
        error: result
            .raw_error
            .clone()
            .or_else(|| result.error.map(|e| e.to_string())),
    })
}

fn plan_token(module: &CompiledModule, token: &SignatureToken) -> Result<MockArgPlan, String> {
    let move_type = token_to_string(module, token);

    match token {
        SignatureToken::Bool => Ok(MockArgPlan::Pure {
            move_type,
            value: "false".to_string(),
            bcs_bytes: bcs::to_bytes(&false).map_err(|e| e.to_string())?,
        }),
        SignatureToken::U8 => Ok(MockArgPlan::Pure {
            move_type,
            value: "7u8".to_string(),
            bcs_bytes: bcs::to_bytes(&7u8).map_err(|e| e.to_string())?,
        }),
        SignatureToken::U16 => Ok(MockArgPlan::Pure {
            move_type,
            value: "42u16".to_string(),
            bcs_bytes: bcs::to_bytes(&42u16).map_err(|e| e.to_string())?,
        }),
        SignatureToken::U32 => Ok(MockArgPlan::Pure {
            move_type,
            value: "42u32".to_string(),
            bcs_bytes: bcs::to_bytes(&42u32).map_err(|e| e.to_string())?,
        }),
        SignatureToken::U64 => Ok(MockArgPlan::Pure {
            move_type,
            value: "42u64".to_string(),
            bcs_bytes: bcs::to_bytes(&42u64).map_err(|e| e.to_string())?,
        }),
        SignatureToken::U128 => Ok(MockArgPlan::Pure {
            move_type,
            value: "42u128".to_string(),
            bcs_bytes: bcs::to_bytes(&42u128).map_err(|e| e.to_string())?,
        }),
        SignatureToken::U256 => {
            let value = U256::from(42u64);
            Ok(MockArgPlan::Pure {
                move_type,
                value: "42u256".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::Address => {
            let addr = AccountAddress::from_hex_literal("0xa11ce").unwrap_or(AccountAddress::ONE);
            Ok(MockArgPlan::Pure {
                move_type,
                value: canonical_address(&addr).to_string(),
                bcs_bytes: bcs::to_bytes(&addr).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::Vector(inner) => plan_vector_token(module, &move_type, inner),
        SignatureToken::Reference(inner) | SignatureToken::MutableReference(inner) => {
            if is_special_struct(module, inner, "tx_context", "TxContext") {
                Ok(MockArgPlan::TxContext { move_type })
            } else if is_special_struct(module, inner, "clock", "Clock") {
                Ok(MockArgPlan::Clock { move_type })
            } else if is_special_struct(module, inner, "random", "Random") {
                Ok(MockArgPlan::Random { move_type })
            } else {
                Err(format!(
                    "unsupported reference argument '{}' (only TxContext/Clock/Random refs are auto-mocked)",
                    move_type
                ))
            }
        }
        SignatureToken::Datatype(_) | SignatureToken::DatatypeInstantiation(_) => Err(format!(
            "unsupported struct argument '{}': object/value construction is domain-specific",
            move_type
        )),
        SignatureToken::TypeParameter(idx) => {
            Err(format!("unsupported generic type parameter T{idx}"))
        }
        SignatureToken::Signer => Err("unsupported signer argument".to_string()),
    }
}

fn plan_vector_token(
    module: &CompiledModule,
    move_type: &str,
    inner: &SignatureToken,
) -> Result<MockArgPlan, String> {
    match inner {
        SignatureToken::U8 => {
            let value = b"walrus-mock".to_vec();
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "b\"walrus-mock\"".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::Bool => {
            let value = vec![true, false];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[true,false]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::U16 => {
            let value = vec![1u16, 2u16];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[1,2]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::U32 => {
            let value = vec![1u32, 2u32];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[1,2]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::U64 => {
            let value = vec![1u64, 2u64];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[1,2]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::U128 => {
            let value = vec![1u128, 2u128];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[1,2]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::U256 => {
            let value = vec![U256::from(1u64), U256::from(2u64)];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[1u256,2u256]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        SignatureToken::Address => {
            let a0 = AccountAddress::from_hex_literal("0xa11ce").unwrap_or(AccountAddress::ONE);
            let a1 = AccountAddress::from_hex_literal("0xb0b").unwrap_or(AccountAddress::TWO);
            let value = vec![a0, a1];
            Ok(MockArgPlan::Pure {
                move_type: move_type.to_string(),
                value: "[0xa11ce,0xb0b]".to_string(),
                bcs_bytes: bcs::to_bytes(&value).map_err(|e| e.to_string())?,
            })
        }
        _ => Err(format!(
            "unsupported vector element type '{}' for '{}'",
            token_to_string(module, inner),
            move_type
        )),
    }
}

fn is_special_struct(
    module: &CompiledModule,
    token: &SignatureToken,
    expect_module: &str,
    expect_name: &str,
) -> bool {
    match token {
        SignatureToken::Datatype(idx) => {
            let Some((addr, module_name, struct_name)) = struct_identity(module, *idx) else {
                return false;
            };
            addr == AccountAddress::from_hex_literal("0x2").unwrap_or(AccountAddress::TWO)
                && module_name == expect_module
                && struct_name == expect_name
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, _) = &**inst;
            let Some((addr, module_name, struct_name)) = struct_identity(module, *idx) else {
                return false;
            };
            addr == AccountAddress::from_hex_literal("0x2").unwrap_or(AccountAddress::TWO)
                && module_name == expect_module
                && struct_name == expect_name
        }
        _ => false,
    }
}

fn struct_identity(
    module: &CompiledModule,
    idx: DatatypeHandleIndex,
) -> Option<(AccountAddress, String, String)> {
    let handle = module.datatype_handles.get(idx.0 as usize)?;
    let module_handle = module.module_handles.get(handle.module.0 as usize)?;
    let addr = *module.address_identifier_at(module_handle.address);
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(handle.name).to_string();
    Some((addr, module_name, struct_name))
}

fn token_to_string(module: &CompiledModule, token: &SignatureToken) -> String {
    match token {
        SignatureToken::Bool => "bool".to_string(),
        SignatureToken::U8 => "u8".to_string(),
        SignatureToken::U16 => "u16".to_string(),
        SignatureToken::U32 => "u32".to_string(),
        SignatureToken::U64 => "u64".to_string(),
        SignatureToken::U128 => "u128".to_string(),
        SignatureToken::U256 => "u256".to_string(),
        SignatureToken::Address => "address".to_string(),
        SignatureToken::Signer => "signer".to_string(),
        SignatureToken::Vector(inner) => format!("vector<{}>", token_to_string(module, inner)),
        SignatureToken::Reference(inner) => format!("&{}", token_to_string(module, inner)),
        SignatureToken::MutableReference(inner) => {
            format!("&mut {}", token_to_string(module, inner))
        }
        SignatureToken::Datatype(idx) => {
            if let Some((addr, module_name, struct_name)) = struct_identity(module, *idx) {
                format!(
                    "{}::{}::{}",
                    canonical_address(&addr),
                    module_name,
                    struct_name
                )
            } else {
                "<unknown struct>".to_string()
            }
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, type_args) = &**inst;
            let base = if let Some((addr, module_name, struct_name)) = struct_identity(module, *idx)
            {
                format!(
                    "{}::{}::{}",
                    canonical_address(&addr),
                    module_name,
                    struct_name
                )
            } else {
                "<unknown struct>".to_string()
            };
            let rendered = type_args
                .iter()
                .map(|t| token_to_string(module, t))
                .collect::<Vec<String>>()
                .join(", ");
            format!("{}<{}>", base, rendered)
        }
        SignatureToken::TypeParameter(idx) => format!("T{idx}"),
    }
}

fn visibility_to_string(v: Visibility) -> String {
    match v {
        Visibility::Private => "private".to_string(),
        Visibility::Public => "public".to_string(),
        Visibility::Friend => "friend".to_string(),
    }
}

fn arg_to_spec(arg: &MockArgPlan) -> ArgSpec {
    match arg {
        MockArgPlan::Pure {
            move_type,
            value,
            bcs_bytes,
        } => ArgSpec {
            kind: "pure".to_string(),
            move_type: move_type.clone(),
            mock_value: Some(value.clone()),
            bcs_base64: Some(base64::engine::general_purpose::STANDARD.encode(bcs_bytes)),
        },
        MockArgPlan::Clock { move_type } => ArgSpec {
            kind: "shared_clock".to_string(),
            move_type: move_type.clone(),
            mock_value: Some("env.get_clock_object()".to_string()),
            bcs_base64: None,
        },
        MockArgPlan::Random { move_type } => ArgSpec {
            kind: "shared_random".to_string(),
            move_type: move_type.clone(),
            mock_value: Some("env.get_random_object()".to_string()),
            bcs_base64: None,
        },
        MockArgPlan::TxContext { move_type } => ArgSpec {
            kind: "implicit_tx_context".to_string(),
            move_type: move_type.clone(),
            mock_value: Some("injected by VM".to_string()),
            bcs_base64: None,
        },
    }
}

fn canonical_address(addr: &AccountAddress) -> String {
    format!("0x{}", hex::encode(addr.as_ref()))
}

fn normalize_package(raw: &str) -> String {
    let trimmed = raw.trim();
    let no_prefix = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let no_leading = no_prefix.trim_start_matches('0');
    let normalized = if no_leading.is_empty() {
        "0"
    } else {
        no_leading
    };
    format!("0x{:0>64}", normalized.to_ascii_lowercase())
}

fn normalize_package_opt(raw: &str) -> Option<String> {
    AccountAddress::from_hex_literal(raw)
        .ok()
        .map(|addr| canonical_address(&addr))
}

fn is_system_package_addr(addr: &AccountAddress) -> bool {
    let Some(norm) = normalize_package_opt(&canonical_address(addr)) else {
        return false;
    };
    is_system_package_hex(&norm)
}

fn is_system_package_hex(package: &str) -> bool {
    let stripped = package
        .strip_prefix("0x")
        .unwrap_or(package)
        .trim_start_matches('0');
    matches!(stripped, "1" | "2" | "3")
}

fn short_package(package: &str) -> String {
    package
        .strip_prefix("0x")
        .unwrap_or(package)
        .chars()
        .take(10)
        .collect()
}

fn sanitize_for_filename(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn write_json<T: Serialize, P: AsRef<Path>>(path: P, value: &T) -> Result<()> {
    let path_ref = path.as_ref();
    if let Some(parent) = path_ref.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(value)?;
    std::fs::write(path_ref, body).with_context(|| format!("write {}", path_ref.display()))?;
    Ok(())
}

fn write_output_readme(
    args: &Args,
    summary: &UniverseSummary,
    packages: &[PackageFetchRecord],
    candidates: &[FunctionCandidateRecord],
    executions: &[PtbExecutionRecord],
) -> Result<()> {
    let package_ok = packages.iter().filter(|p| p.deployed).count();
    let candidate_ok = candidates.iter().filter(|c| c.accepted).count();
    let exec_ok = executions.iter().filter(|e| e.success).count();

    let source_line = match args.source {
        CheckpointSource::Walrus => "source=walrus".to_string(),
        CheckpointSource::GrpcStream => {
            let endpoint = args
                .grpc_endpoint
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("<default>");
            format!("source=grpc-stream endpoint={endpoint}")
        }
    };

    let content = format!(
        "# Checkpoint PTB Universe Output\n\n\
Generated by `cargo run --example walrus_ptb_universe -- --source {source} --latest {latest} --top-packages {top_packages} --max-ptbs {max_ptbs}`\n\
Runtime: `{source_line}`\n\n\
## Window\n\
- Checkpoints: `{start}..{end}` ({count} checkpoints)\n\
- Transactions scanned: `{tx_total}`\n\
- PTB transactions: `{ptb_total}`\n\
- App PTB transactions: `{app_ptb_total}`\n\n\
## Outcome\n\
- Packages deployed: `{package_ok}` / `{package_total}`\n\
- Callable candidates: `{candidate_ok}` / `{candidate_total}`\n\
- Executed PTBs: `{exec_total}`\n\
- Successful PTBs: `{exec_ok}`\n\n\
## Files\n\
- `universe_summary.json`\n\
- `package_downloads.json`\n\
- `function_candidates.json`\n\
- `ptb_execution_results.json`\n\
- `ptb_specs/*.json`\n",
        source = args.source.as_str(),
        source_line = source_line,
        latest = args.latest,
        top_packages = args.top_packages,
        max_ptbs = args.max_ptbs,
        start = summary.start_checkpoint,
        end = summary.end_checkpoint,
        count = summary.checkpoints_loaded,
        tx_total = summary.transactions_total,
        ptb_total = summary.ptb_transactions,
        app_ptb_total = summary.ptb_app_transactions,
        package_ok = package_ok,
        package_total = packages.len(),
        candidate_ok = candidate_ok,
        candidate_total = candidates.len(),
        exec_total = executions.len(),
        exec_ok = exec_ok,
    );

    std::fs::write(args.out_dir.join("README.md"), content)
        .with_context(|| format!("write {}", args.out_dir.join("README.md").display()))?;

    Ok(())
}
