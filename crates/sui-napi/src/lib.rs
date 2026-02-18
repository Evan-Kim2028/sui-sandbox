// ---------------------------------------------------------------------------
// sui-napi: Node.js/TypeScript bindings for Sui Move package analysis and
//           transaction replay via NAPI-RS.
//
// This crate mirrors the Python bindings in `crates/sui-python/` with 1:1 API
// parity. Key simplifications over PyO3:
//   - No GIL management — NAPI-RS handles threading.
//   - No `json_value_to_py()` — NAPI's `serde-json` feature converts
//     `serde_json::Value` to JS objects automatically.
//   - No manual module registration — `#[napi]` auto-registers exports.
//   - Native async support — async fns return Promises without manual
//     `tokio::Runtime` wiring.
// ---------------------------------------------------------------------------

#[macro_use]
extern crate napi_derive;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context as AnyhowContext, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde_json;

// ---------------------------------------------------------------------------
// Workspace crate re-exports (mirroring crates/sui-python/src/lib.rs)
// ---------------------------------------------------------------------------

use sui_package_extractor::bytecode::{
    build_bytecode_interface_value_from_compiled_modules, read_local_compiled_modules,
    resolve_local_package_id,
};
use sui_package_extractor::extract_module_dependency_ids as extract_dependency_addrs;
use sui_package_extractor::utils::is_framework_address;

use sui_sandbox_core::adapter::{
    resolve_discovery_package_filter as core_resolve_discovery_package_filter,
    resolve_required_package_id as core_resolve_required_package_id,
    ProtocolAdapter as CoreProtocolAdapter,
};
use sui_sandbox_core::checkpoint_discovery::{
    build_walrus_client as core_build_walrus_client,
    discover_checkpoint_targets as core_discover_checkpoint_targets,
    resolve_replay_target_from_discovery as core_resolve_replay_target_from_discovery,
    WalrusArchiveNetwork as CoreWalrusArchiveNetwork,
};
use sui_sandbox_core::context_contract::{
    context_packages_from_package_map, decode_context_package_modules, decode_context_packages,
    parse_context_payload, ContextPackage, ContextPayloadV2,
};
use sui_sandbox_core::health::{run_doctor as core_run_doctor, DoctorConfig as CoreDoctorConfig};
use sui_sandbox_core::historical_view::{
    execute_historical_view_from_versions as core_execute_historical_view_from_versions,
    HistoricalViewRequest as CoreHistoricalViewRequest,
};
use sui_sandbox_core::orchestrator::{
    HistoricalSeriesExecutionOptions as CoreHistoricalSeriesExecutionOptions,
    HistoricalSeriesPoint as CoreHistoricalSeriesPoint, ReplayOrchestrator, ReturnDecodeField,
};
use sui_sandbox_core::ptb_universe::{
    run_with_args as core_run_ptb_universe, Args as CorePtbUniverseArgs,
    CheckpointSource as CoreCheckpointSource,
    DEFAULT_LATEST as CORE_PTB_UNIVERSE_DEFAULT_LATEST,
    DEFAULT_MAX_PTBS as CORE_PTB_UNIVERSE_DEFAULT_MAX_PTBS,
    DEFAULT_STREAM_TIMEOUT_SECS as CORE_PTB_UNIVERSE_DEFAULT_STREAM_TIMEOUT_SECS,
    DEFAULT_TOP_PACKAGES as CORE_PTB_UNIVERSE_DEFAULT_TOP_PACKAGES,
};
use sui_sandbox_core::replay_reporting::{
    build_replay_analysis_summary as core_build_replay_analysis_summary,
    build_replay_diagnostics as core_build_replay_diagnostics,
    classify_replay_output as core_classify_replay_output,
    missing_input_objects_from_state as core_missing_input_objects_from_state,
    ReplayDiagnosticsOptions as CoreReplayDiagnosticsOptions,
};
use sui_sandbox_core::resolver::ModuleProvider;
use sui_sandbox_core::simulation::{
    CoinMetadata, PersistentState, StateMetadata, SUI_COIN_TYPE, SUI_DECIMALS, SUI_SYMBOL,
};
use sui_sandbox_core::utilities::unresolved_package_dependencies_for_modules;
use sui_sandbox_core::vm::SimulationConfig;
use sui_sandbox_core::workflow::{
    normalize_command_args, WorkflowAnalyzeReplayStep, WorkflowCommandStep, WorkflowDefaults,
    WorkflowFetchStrategy, WorkflowReplayProfile, WorkflowReplayStep, WorkflowSource, WorkflowSpec,
    WorkflowStep, WorkflowStepAction,
};
use sui_sandbox_core::workflow_adapter::{
    build_builtin_workflow, BuiltinWorkflowInput, BuiltinWorkflowTemplate,
};
use sui_sandbox_core::workflow_runner::{
    run_prepared_workflow_steps, WorkflowPreparedStep, WorkflowStepExecution,
};

use sui_state_fetcher::{
    bcs_codec, build_aliases, checkpoint_to_replay_state, import_replay_states,
    parse_replay_states_file, FileStateProvider, HistoricalStateProvider, ImportSpec, PackageData,
    ReplayState,
};

use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{resolve_historical_endpoint_and_api_key, GrpcClient, GrpcOwner};
use sui_transport::network::resolve_graphql_endpoint;
use sui_transport::walrus::WalrusClient;

// ---------------------------------------------------------------------------
// Submodules
// ---------------------------------------------------------------------------

mod replay_api;
mod replay_core;
mod replay_output;
mod session_api;
mod transport_helpers;
mod workflow_api;
mod workflow_native;

// Re-export public #[napi] items from submodules.
// NAPI-RS auto-discovers #[napi] items in submodules.
pub use replay_api::*;
pub use session_api::*;
pub use workflow_api::*;

// ---------------------------------------------------------------------------
// Internal imports from submodules
// ---------------------------------------------------------------------------

use replay_core::*;
use replay_output::*;
use transport_helpers::*;
use workflow_native::*;

// ---------------------------------------------------------------------------
// Error conversion helper
// ---------------------------------------------------------------------------

fn to_napi_err(e: anyhow::Error) -> napi::Error {
    napi::Error::from_reason(format!("{:#}", e))
}

const PTB_UNIVERSE_DEFAULT_OUT_DIR: &str = "examples/out/walrus_ptb_universe";

// ---------------------------------------------------------------------------
// Internal helpers (mirroring crates/sui-python/src/lib.rs helpers)
// ---------------------------------------------------------------------------

fn sandbox_home_dir() -> PathBuf {
    std::env::var("SUI_SANDBOX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sui-sandbox")
        })
}

fn default_local_cache_dir() -> PathBuf {
    sandbox_home_dir().join("cache").join("local")
}

fn default_state_file_path() -> PathBuf {
    sandbox_home_dir().join("state.json")
}

fn default_snapshot_dir() -> PathBuf {
    sandbox_home_dir().join("snapshots")
}

fn sanitize_snapshot_name(name: &str) -> String {
    let filtered: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if filtered.is_empty() {
        "snapshot".to_string()
    } else {
        filtered
    }
}

fn snapshot_path(name: &str) -> PathBuf {
    default_snapshot_dir().join(format!("{}.json", sanitize_snapshot_name(name)))
}

fn default_persistent_state() -> PersistentState {
    let mut coin_registry = HashMap::new();
    coin_registry.insert(
        SUI_COIN_TYPE.to_string(),
        CoinMetadata {
            decimals: SUI_DECIMALS,
            symbol: SUI_SYMBOL.to_string(),
            name: "Sui".to_string(),
            type_tag: SUI_COIN_TYPE.to_string(),
        },
    );

    let now = chrono::Utc::now().to_rfc3339();

    PersistentState {
        version: PersistentState::CURRENT_VERSION,
        objects: Vec::new(),
        object_history: Vec::new(),
        modules: Vec::new(),
        packages: Vec::new(),
        coin_registry,
        sender: "0x0".to_string(),
        id_counter: 0,
        timestamp_ms: None,
        dynamic_fields: Vec::new(),
        pending_receives: Vec::new(),
        config: Some(SimulationConfig::default()),
        metadata: Some(StateMetadata {
            description: None,
            created_at: Some(now),
            modified_at: None,
            tags: Vec::new(),
        }),
        fetcher_config: None,
    }
}

fn load_or_create_state(path: &Path) -> Result<PersistentState> {
    if !path.exists() {
        return Ok(default_persistent_state());
    }
    let raw = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut state: PersistentState = serde_json::from_slice(&raw)
        .with_context(|| format!("Failed to parse state file {}", path.display()))?;
    if state.version > PersistentState::CURRENT_VERSION {
        return Err(anyhow!(
            "state file version {} is newer than supported {}",
            state.version,
            PersistentState::CURRENT_VERSION
        ));
    }
    state.version = PersistentState::CURRENT_VERSION;
    Ok(state)
}

fn save_state(path: &Path, state: &PersistentState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(state).context("Failed to serialize state")?;
    std::fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn normalize_address_like_cli(raw: &str) -> String {
    match AccountAddress::from_hex_literal(raw) {
        Ok(addr) => addr.to_hex_literal(),
        Err(_) => raw.to_ascii_lowercase(),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SnapshotFile {
    schema_version: u32,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    created_at: String,
    state: PersistentState,
}

#[derive(Clone, Copy)]
enum WorkflowOutputFormat {
    Json,
    Yaml,
}

impl WorkflowOutputFormat {
    fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

fn context_packages_to_package_data(
    packages: &[ContextPackage],
) -> Result<HashMap<AccountAddress, PackageData>> {
    let mut out = HashMap::new();
    for package in packages {
        let address = AccountAddress::from_hex_literal(&package.address).with_context(|| {
            format!(
                "invalid package address in context payload: {}",
                package.address
            )
        })?;
        let modules = decode_context_package_modules(package).with_context(|| {
            format!(
                "failed to decode package modules from context payload for {}",
                package.address
            )
        })?;
        out.insert(
            address,
            PackageData {
                address,
                version: 0,
                modules,
                linkage: HashMap::new(),
                original_id: None,
            },
        );
    }
    Ok(out)
}

fn decode_context_packages_value(
    value: &serde_json::Value,
) -> Result<HashMap<AccountAddress, PackageData>> {
    let packages = decode_context_packages(value)?;
    context_packages_to_package_data(&packages)
}

fn load_context_packages_from_file(path: &Path) -> Result<HashMap<AccountAddress, PackageData>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read context file {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse context JSON {}", path.display()))?;
    let parsed = parse_context_payload(&value)
        .with_context(|| format!("Invalid context payload in {}", path.display()))?;
    context_packages_to_package_data(&parsed.packages)
}

// ---------------------------------------------------------------------------
// Checkpoint functions
// ---------------------------------------------------------------------------

fn get_latest_checkpoint_inner() -> Result<u64> {
    let walrus = WalrusClient::mainnet();
    walrus.get_latest_checkpoint()
}

fn get_checkpoint_inner(checkpoint: u64) -> Result<serde_json::Value> {
    let walrus = WalrusClient::mainnet();
    let data = walrus.get_checkpoint(checkpoint)?;
    serde_json::to_value(data).context("Failed to serialize checkpoint data")
}

/// Get the latest archived checkpoint number from Walrus.
///
/// No API keys or authentication required.
#[napi]
pub fn get_latest_checkpoint() -> napi::Result<u32> {
    // Note: napi doesn't natively support u64 — use f64 or return as i64/u32.
    // Checkpoint numbers fit in u32 for the foreseeable future.
    // For safety, we'll return i64 which napi supports.
    get_latest_checkpoint_inner()
        .map(|v| v as u32)
        .map_err(to_napi_err)
}

/// Get the latest archived checkpoint number (as bigint for full u64 range).
#[napi]
pub fn get_latest_checkpoint_bigint() -> napi::Result<i64> {
    get_latest_checkpoint_inner()
        .map(|v| v as i64)
        .map_err(to_napi_err)
}

/// Fetch a checkpoint from Walrus and return a summary object.
#[napi]
pub async fn get_checkpoint(checkpoint: u32) -> napi::Result<serde_json::Value> {
    get_checkpoint_inner(checkpoint as u64).map_err(to_napi_err)
}

/// Build and execute a checkpoint-source PTB universe run via core engine.
#[napi]
pub async fn ptb_universe(
    source: Option<String>,
    latest: Option<u32>,
    top_packages: Option<u32>,
    max_ptbs: Option<u32>,
    out_dir: Option<String>,
    grpc_endpoint: Option<String>,
    stream_timeout_secs: Option<u32>,
) -> napi::Result<serde_json::Value> {
    let source_str = source.as_deref().unwrap_or("walrus");
    let source_parsed = CoreCheckpointSource::parse(source_str).map_err(to_napi_err)?;
    let latest_val = latest.map(|v| v as u64).unwrap_or(CORE_PTB_UNIVERSE_DEFAULT_LATEST);
    let top_packages_val = top_packages.map(|v| v as usize).unwrap_or(CORE_PTB_UNIVERSE_DEFAULT_TOP_PACKAGES);
    let max_ptbs_val = max_ptbs.map(|v| v as usize).unwrap_or(CORE_PTB_UNIVERSE_DEFAULT_MAX_PTBS);
    let stream_timeout_val = stream_timeout_secs.map(|v| v as u64).unwrap_or(CORE_PTB_UNIVERSE_DEFAULT_STREAM_TIMEOUT_SECS);
    let out_dir_path = PathBuf::from(
        out_dir
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(PTB_UNIVERSE_DEFAULT_OUT_DIR),
    );
    let grpc_endpoint_owned = grpc_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned);

    let args = CorePtbUniverseArgs {
        source: source_parsed,
        latest: latest_val,
        top_packages: top_packages_val,
        max_ptbs: max_ptbs_val,
        out_dir: out_dir_path.clone(),
        grpc_endpoint: grpc_endpoint_owned.clone(),
        stream_timeout_secs: stream_timeout_val,
    };

    core_run_ptb_universe(args).map_err(to_napi_err)?;

    Ok(serde_json::json!({
        "success": true,
        "source": source_parsed.as_str(),
        "latest": latest_val,
        "top_packages": top_packages_val,
        "max_ptbs": max_ptbs_val,
        "grpc_endpoint": grpc_endpoint_owned,
        "stream_timeout_secs": stream_timeout_val,
        "out_dir": out_dir_path.display().to_string(),
        "artifacts": {
            "summary": out_dir_path.join("universe_summary.json").display().to_string(),
            "package_downloads": out_dir_path.join("package_downloads.json").display().to_string(),
            "function_candidates": out_dir_path.join("function_candidates.json").display().to_string(),
            "ptb_execution_results": out_dir_path.join("ptb_execution_results.json").display().to_string(),
        }
    }))
}

// ---------------------------------------------------------------------------
// Discovery functions
// ---------------------------------------------------------------------------

/// Discover replay candidates from checkpoint Move calls.
#[napi]
pub async fn discover_checkpoint_targets(
    checkpoint: Option<String>,
    latest: Option<u32>,
    package_id: Option<String>,
    include_framework: Option<bool>,
    limit: Option<u32>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    discover_checkpoint_targets_inner(
        checkpoint.as_deref(),
        latest.map(|v| v as u64),
        package_id.as_deref(),
        include_framework.unwrap_or(false),
        limit.map(|v| v as usize).unwrap_or(200),
        walrus_network.as_deref().unwrap_or("mainnet"),
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
    )
    .map_err(to_napi_err)
}

/// Protocol-first replay-target discovery from checkpoints.
#[napi]
pub async fn protocol_discover(
    protocol: Option<String>,
    package_id: Option<String>,
    checkpoint: Option<String>,
    latest: Option<u32>,
    include_framework: Option<bool>,
    limit: Option<u32>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    let protocol_str = protocol.as_deref().unwrap_or("generic");
    let filter = resolve_protocol_discovery_package_filter(protocol_str, package_id.as_deref())
        .map_err(to_napi_err)?;
    discover_checkpoint_targets_inner(
        checkpoint.as_deref(),
        latest.map(|v| v as u64),
        filter.as_deref(),
        include_framework.unwrap_or(false),
        limit.map(|v| v as usize).unwrap_or(200),
        walrus_network.as_deref().unwrap_or("mainnet"),
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
    )
    .map_err(to_napi_err)
}

/// Alias for `discover_checkpoint_targets`.
#[napi]
pub async fn context_discover(
    checkpoint: Option<String>,
    latest: Option<u32>,
    package_id: Option<String>,
    include_framework: Option<bool>,
    limit: Option<u32>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    discover_checkpoint_targets(
        checkpoint,
        latest,
        package_id,
        include_framework,
        limit,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
    .await
}

/// Alias for `protocol_discover`.
#[napi]
pub async fn adapter_discover(
    protocol: Option<String>,
    package_id: Option<String>,
    checkpoint: Option<String>,
    latest: Option<u32>,
    include_framework: Option<bool>,
    limit: Option<u32>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    protocol_discover(
        protocol,
        package_id,
        checkpoint,
        latest,
        include_framework,
        limit,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
    .await
}

// ---------------------------------------------------------------------------
// Object / BCS / bytecode functions
// ---------------------------------------------------------------------------

fn fetch_object_bcs_inner(
    object_id: &str,
    version: Option<u64>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> Result<serde_json::Value> {
    let (grpc_endpoint, grpc_api_key) = resolve_grpc_endpoint_and_key(endpoint, api_key);
    let object_id_owned = object_id.to_string();

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    let object = rt.block_on(async {
        let grpc = GrpcClient::with_api_key(&grpc_endpoint, grpc_api_key)
            .await
            .context("Failed to create gRPC client")?;
        grpc.get_object_at_version(&object_id_owned, version)
            .await
            .context("Failed to fetch object via gRPC")
    })?;

    let object = object.ok_or_else(|| {
        anyhow!(
            "Object {} not found at version {:?}",
            object_id_owned,
            version
        )
    })?;

    let bcs = object
        .bcs
        .ok_or_else(|| anyhow!("Object {} missing BCS payload", object_id_owned))?;
    let type_tag = object.type_string.clone().ok_or_else(|| {
        anyhow!(
            "Object {} missing type string; cannot build view input",
            object_id_owned
        )
    })?;

    let (owner_kind, is_shared, is_immutable) = match object.owner {
        GrpcOwner::Shared { .. } => ("shared", true, false),
        GrpcOwner::Immutable => ("immutable", false, true),
        GrpcOwner::Address(_) => ("address_owned", false, false),
        GrpcOwner::Object(_) => ("object_owned", false, false),
        GrpcOwner::Unknown => ("unknown", false, false),
    };

    Ok(serde_json::json!({
        "object_id": object_id_owned,
        "requested_version": version,
        "version": object.version,
        "endpoint_used": grpc_endpoint,
        "type_tag": type_tag,
        "bcs_base64": base64::engine::general_purpose::STANDARD.encode(&bcs),
        "is_shared": is_shared,
        "is_immutable": is_immutable,
        "owner_kind": owner_kind,
    }))
}

/// Fetch object BCS via gRPC, optionally pinned to a historical version.
#[napi]
pub async fn fetch_object_bcs(
    object_id: String,
    version: Option<u32>,
    endpoint: Option<String>,
    api_key: Option<String>,
) -> napi::Result<serde_json::Value> {
    fetch_object_bcs_inner(
        &object_id,
        version.map(|v| v as u64),
        endpoint.as_deref(),
        api_key.as_deref(),
    )
    .map_err(to_napi_err)
}

/// Extract the full interface JSON for a Sui Move package.
#[napi]
pub async fn extract_interface(
    package_id: Option<String>,
    bytecode_dir: Option<String>,
    rpc_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    extract_interface_inner(
        package_id.as_deref(),
        bytecode_dir.as_deref(),
        rpc_url.as_deref().unwrap_or("https://fullnode.mainnet.sui.io:443"),
    )
    .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// Replay function (core entrypoint)
// ---------------------------------------------------------------------------

/// Replay a historical Sui transaction locally with the Move VM.
///
/// When `checkpoint` is provided, uses Walrus as data source (no API key needed).
/// Otherwise uses gRPC/hybrid (requires `SUI_GRPC_API_KEY` env var).
#[napi]
pub async fn replay(
    digest: Option<String>,
    rpc_url: Option<String>,
    source: Option<String>,
    checkpoint: Option<u32>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    analyze_only: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let rpc = rpc_url.as_deref().unwrap_or("https://fullnode.mainnet.sui.io:443");
    let source_str = source.as_deref().unwrap_or("hybrid");
    let vm_only_val = vm_only.unwrap_or(false);
    let allow_fallback_val = if vm_only_val { false } else { allow_fallback.unwrap_or(true) };
    let no_prefetch_val = no_prefetch.unwrap_or(false);
    let verbose_val = verbose.unwrap_or(false);
    let compare_val = compare.unwrap_or(false);
    let analyze_only_val = analyze_only.unwrap_or(false);
    let synthesize_missing_val = synthesize_missing.unwrap_or(false);
    let self_heal_val = self_heal_dynamic_fields.unwrap_or(false);
    let analyze_mm2_val = analyze_mm2.unwrap_or(false);
    let auto_system_val = auto_system_objects.unwrap_or(true);
    let depth = prefetch_depth.map(|v| v as usize).unwrap_or(3);
    let limit = prefetch_limit.map(|v| v as usize).unwrap_or(200);

    let profile_parsed = parse_replay_profile(profile.as_deref()).map_err(to_napi_err)?;
    let _profile_env = workflow_apply_profile_env(profile_parsed);
    let fetch_strategy_parsed = parse_replay_fetch_strategy(fetch_strategy.as_deref()).map_err(to_napi_err)?;
    let no_prefetch_effective = no_prefetch_val || fetch_strategy_parsed == WorkflowFetchStrategy::Eager;

    let source_is_local = source_str.eq_ignore_ascii_case("local");
    let use_local_cache = source_is_local || cache_dir.is_some();
    let context_packages = if let Some(path) = context_path.as_ref() {
        Some(load_context_packages_from_file(Path::new(path)).map_err(to_napi_err)?)
    } else {
        None
    };

    if state_file.is_some() && use_local_cache {
        return Err(to_napi_err(anyhow!(
            "state_file cannot be combined with cache_dir/source='local'"
        )));
    }

    if let Some(ref state_path) = state_file {
        let replay_state = load_replay_state_from_file(Path::new(state_path), digest.as_deref())
            .map_err(to_napi_err)?;
        return replay_loaded_state_inner(
            replay_state,
            "state_file",
            "state_json",
            context_packages.as_ref(),
            allow_fallback_val,
            auto_system_val,
            self_heal_val,
            vm_only_val,
            compare_val,
            analyze_only_val,
            synthesize_missing_val,
            analyze_mm2_val,
            rpc,
            verbose_val,
        )
        .map_err(to_napi_err);
    }

    if use_local_cache {
        let d = digest
            .as_deref()
            .ok_or_else(|| to_napi_err(anyhow!("digest is required when replaying from cache_dir/source='local'")))?;
        let cache = cache_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(default_local_cache_dir);
        let provider = FileStateProvider::new(&cache)
            .with_context(|| format!("Failed to open local replay cache {}", cache.display()))
            .map_err(to_napi_err)?;
        let replay_state = provider.get_state(d).map_err(to_napi_err)?;
        return replay_loaded_state_inner(
            replay_state,
            source_str,
            "local_cache",
            context_packages.as_ref(),
            allow_fallback_val,
            auto_system_val,
            self_heal_val,
            vm_only_val,
            compare_val,
            analyze_only_val,
            synthesize_missing_val,
            analyze_mm2_val,
            rpc,
            verbose_val,
        )
        .map_err(to_napi_err);
    }

    let d = digest
        .as_deref()
        .ok_or_else(|| to_napi_err(anyhow!("digest is required")))?;
    replay_inner(
        d,
        rpc,
        source_str,
        checkpoint.map(|v| v as u64),
        context_packages.as_ref(),
        allow_fallback_val,
        depth,
        limit,
        auto_system_val,
        no_prefetch_effective,
        synthesize_missing_val,
        self_heal_val,
        vm_only_val,
        compare_val,
        analyze_only_val,
        analyze_mm2_val,
        verbose_val,
    )
    .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// Import / Deserialize functions
// ---------------------------------------------------------------------------

/// Import replay data files into a local replay cache directory.
#[napi]
pub async fn import_state(
    state: Option<String>,
    transactions: Option<String>,
    objects: Option<String>,
    packages: Option<String>,
    cache_dir: Option<String>,
) -> napi::Result<serde_json::Value> {
    import_state_inner(
        state.as_deref(),
        transactions.as_deref(),
        objects.as_deref(),
        packages.as_deref(),
        cache_dir.as_deref(),
    )
    .map_err(to_napi_err)
}

/// Deserialize transaction BCS bytes into structured replay transaction JSON.
#[napi]
pub fn deserialize_transaction(raw_bcs: napi::bindgen_prelude::Buffer) -> napi::Result<serde_json::Value> {
    deserialize_transaction_inner(&raw_bcs).map_err(to_napi_err)
}

/// Deserialize package BCS bytes into structured package JSON.
#[napi]
pub fn deserialize_package(bcs: napi::bindgen_prelude::Buffer) -> napi::Result<serde_json::Value> {
    deserialize_package_inner(&bcs).map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// BCS conversion functions
// ---------------------------------------------------------------------------

fn json_to_bcs_inner(
    type_str: &str,
    object_json: &str,
    package_bytecodes: Vec<Vec<u8>>,
) -> Result<Vec<u8>> {
    let json_value: serde_json::Value =
        serde_json::from_str(object_json).context("Failed to parse object_json")?;

    let mut converter = sui_sandbox_core::utilities::JsonToBcsConverter::new();
    converter.add_modules_from_bytes(&package_bytecodes)?;
    converter.convert(type_str, &json_value)
}

fn transaction_json_to_bcs_inner(transaction_json: &str) -> Result<Vec<u8>> {
    bcs_codec::transaction_json_to_bcs(transaction_json)
}

/// Convert Sui object JSON to BCS bytes using struct layouts from bytecode.
#[napi]
pub fn json_to_bcs(
    type_str: String,
    object_json: String,
    package_bytecodes: Vec<napi::bindgen_prelude::Buffer>,
) -> napi::Result<napi::bindgen_prelude::Buffer> {
    let bytecodes: Vec<Vec<u8>> = package_bytecodes.into_iter().map(|b| b.to_vec()).collect();
    let bytes = json_to_bcs_inner(&type_str, &object_json, bytecodes).map_err(to_napi_err)?;
    Ok(napi::bindgen_prelude::Buffer::from(bytes))
}

/// Convert Sui TransactionData JSON into raw transaction BCS bytes.
#[napi]
pub fn transaction_json_to_bcs(transaction_json: String) -> napi::Result<napi::bindgen_prelude::Buffer> {
    let bytes = transaction_json_to_bcs_inner(&transaction_json).map_err(to_napi_err)?;
    Ok(napi::bindgen_prelude::Buffer::from(bytes))
}

// ---------------------------------------------------------------------------
// Bytecode / context functions
// ---------------------------------------------------------------------------

fn fetch_package_bytecodes_inner(
    package_id: &str,
    resolve_deps: bool,
) -> Result<serde_json::Value> {
    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
    let graphql = GraphQLClient::new(&graphql_endpoint);

    let mut packages = serde_json::Map::new();

    if resolve_deps {
        let mut to_fetch: VecDeque<AccountAddress> = VecDeque::new();
        let mut visited = HashSet::new();
        let root = AccountAddress::from_hex_literal(package_id)
            .with_context(|| format!("invalid package address: {}", package_id))?;
        for fw in ["0x1", "0x2", "0x3"] {
            let fw_addr = AccountAddress::from_hex_literal(fw).unwrap();
            if fw_addr != root {
                visited.insert(fw_addr);
            }
        }
        to_fetch.push_back(root);

        const MAX_DEP_ROUNDS: usize = 20;
        let mut rounds = 0;
        while let Some(addr) = to_fetch.pop_front() {
            if visited.contains(&addr) || (is_framework_address(&addr) && addr != root) {
                continue;
            }
            rounds += 1;
            if rounds > MAX_DEP_ROUNDS {
                eprintln!(
                    "Warning: dependency resolution hit max depth ({} packages fetched), stopping.",
                    MAX_DEP_ROUNDS
                );
                break;
            }
            visited.insert(addr);

            let hex = addr.to_hex_literal();
            let modules = fetch_package_modules(&graphql, &hex)?;
            let bytecodes: Vec<String> = modules
                .iter()
                .map(|(_, bytes)| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect();
            let dep_addrs = extract_dependency_addrs(&modules);
            packages.insert(hex, serde_json::json!(bytecodes));

            for dep_addr in dep_addrs {
                if !visited.contains(&dep_addr) && !is_framework_address(&dep_addr) {
                    to_fetch.push_back(dep_addr);
                }
            }
        }
    } else {
        let modules = fetch_package_modules(&graphql, package_id)?;
        let bytecodes: Vec<String> = modules
            .iter()
            .map(|(_, bytes)| base64::engine::general_purpose::STANDARD.encode(bytes))
            .collect();
        packages.insert(package_id.to_string(), serde_json::json!(bytecodes));
    }

    Ok(serde_json::json!({
        "packages": packages,
        "count": packages.len(),
    }))
}

fn prepare_package_context_inner(
    package_id: &str,
    resolve_deps: bool,
    output_path: Option<&str>,
) -> Result<serde_json::Value> {
    let fetched = fetch_package_bytecodes_inner(package_id, resolve_deps)?;
    let generated_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let packages_map = fetched
        .get("packages")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("fetch package output missing `packages` map"))?;
    let packages = context_packages_from_package_map(packages_map)
        .context("failed to convert package map into context package payload")?;
    let context_payload =
        ContextPayloadV2::new(package_id, resolve_deps, generated_at_ms, None, packages);
    let mut payload = serde_json::to_value(context_payload)?;

    if let Some(path) = output_path {
        let path_buf = PathBuf::from(path);
        if let Some(parent) = path_buf.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create context output directory {}",
                        parent.display()
                    )
                })?;
            }
        }
        std::fs::write(&path_buf, serde_json::to_string_pretty(&payload)?)
            .with_context(|| format!("Failed to write context file {}", path_buf.display()))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "context_path".to_string(),
                serde_json::json!(path_buf.display().to_string()),
            );
        }
    }

    Ok(payload)
}

/// Fetch package bytecodes via GraphQL, optionally resolving transitive dependencies.
#[napi]
pub async fn fetch_package_bytecodes(
    package_id: String,
    resolve_deps: Option<bool>,
) -> napi::Result<serde_json::Value> {
    fetch_package_bytecodes_inner(&package_id, resolve_deps.unwrap_or(true)).map_err(to_napi_err)
}

/// Prepare a generic package context by fetching package bytecodes (+deps by default).
#[napi]
pub async fn prepare_package_context(
    package_id: String,
    resolve_deps: Option<bool>,
    output_path: Option<String>,
) -> napi::Result<serde_json::Value> {
    prepare_package_context_inner(
        &package_id,
        resolve_deps.unwrap_or(true),
        output_path.as_deref(),
    )
    .map_err(to_napi_err)
}

/// Alias for `prepare_package_context`.
#[napi]
pub async fn context_prepare(
    package_id: String,
    resolve_deps: Option<bool>,
    output_path: Option<String>,
) -> napi::Result<serde_json::Value> {
    prepare_package_context(package_id, resolve_deps, output_path).await
}

/// Protocol-first package-context preparation.
#[napi]
pub async fn protocol_prepare(
    protocol: Option<String>,
    package_id: Option<String>,
    resolve_deps: Option<bool>,
    output_path: Option<String>,
) -> napi::Result<serde_json::Value> {
    let proto = protocol.as_deref().unwrap_or("generic");
    let resolved = resolve_protocol_package_id(proto, package_id.as_deref()).map_err(to_napi_err)?;
    prepare_package_context_inner(
        &resolved,
        resolve_deps.unwrap_or(true),
        output_path.as_deref(),
    )
    .map_err(to_napi_err)
}

/// Alias for `protocol_prepare`.
#[napi]
pub async fn adapter_prepare(
    protocol: Option<String>,
    package_id: Option<String>,
    resolve_deps: Option<bool>,
    output_path: Option<String>,
) -> napi::Result<serde_json::Value> {
    protocol_prepare(protocol, package_id, resolve_deps, output_path).await
}

// ---------------------------------------------------------------------------
// Historical package bytecodes
// ---------------------------------------------------------------------------

fn fetch_historical_package_bytecodes_inner(
    package_ids: &[String],
    type_refs: &[String],
    checkpoint: Option<u64>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> Result<serde_json::Value> {
    let (grpc_endpoint, grpc_api_key) = resolve_grpc_endpoint_and_key(endpoint, api_key);
    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");

    let mut package_roots: Vec<AccountAddress> = Vec::new();
    for id in package_ids {
        package_roots.push(AccountAddress::from_hex_literal(id)?);
    }
    for type_str in type_refs {
        for pkg_id in sui_sandbox_core::utilities::extract_package_ids_from_type(type_str) {
            if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                if !is_framework_address(&addr) {
                    package_roots.push(addr);
                }
            }
        }
    }

    let rt = tokio::runtime::Runtime::new()?;
    let packages = rt.block_on(async {
        let grpc = GrpcClient::with_api_key(&grpc_endpoint, grpc_api_key)
            .await
            .context("Failed to create gRPC client")?;
        let graphql = GraphQLClient::new(&graphql_endpoint);
        let provider = HistoricalStateProvider::with_clients(grpc, graphql);
        provider
            .fetch_packages_with_deps(&package_roots, None, checkpoint)
            .await
            .context("Failed to fetch historical packages with deps")
    })?;

    let mut package_map = serde_json::Map::new();
    let mut aliases = serde_json::Map::new();
    let mut linkage_upgrades = serde_json::Map::new();
    let mut package_runtime_ids = serde_json::Map::new();
    let mut package_linkage = serde_json::Map::new();
    let mut package_versions = serde_json::Map::new();

    for (addr, pkg) in &packages {
        let encoded_modules: Vec<String> = pkg
            .modules
            .iter()
            .map(|(_, bytes)| base64::engine::general_purpose::STANDARD.encode(bytes))
            .collect();
        let storage_id = addr.to_hex_literal();
        let inferred_runtime_id = pkg
            .modules
            .iter()
            .find_map(|(_, bytes)| {
                CompiledModule::deserialize_with_defaults(bytes)
                    .ok()
                    .map(|module| *module.self_id().address())
            })
            .unwrap_or_else(|| pkg.runtime_id());
        let runtime_id = inferred_runtime_id.to_hex_literal();
        package_map.insert(storage_id.clone(), serde_json::json!(encoded_modules));
        package_runtime_ids.insert(storage_id.clone(), serde_json::json!(runtime_id.clone()));
        package_versions.insert(storage_id.clone(), serde_json::json!(pkg.version));

        if storage_id != runtime_id {
            aliases.insert(storage_id.clone(), serde_json::json!(runtime_id.clone()));
            linkage_upgrades.insert(runtime_id.clone(), serde_json::json!(storage_id.clone()));
        }

        let mut lm = serde_json::Map::new();
        for (dep_runtime, dep_storage) in &pkg.linkage {
            let drt = dep_runtime.to_hex_literal();
            let dst = dep_storage.to_hex_literal();
            lm.insert(drt.clone(), serde_json::json!(dst.clone()));
            if drt != dst {
                linkage_upgrades.insert(drt, serde_json::json!(dst));
            }
        }
        package_linkage.insert(storage_id, serde_json::Value::Object(lm));
    }

    Ok(serde_json::json!({
        "checkpoint": checkpoint,
        "endpoint_used": grpc_endpoint,
        "packages": package_map,
        "aliases": aliases,
        "linkage_upgrades": linkage_upgrades,
        "package_runtime_ids": package_runtime_ids,
        "package_linkage": package_linkage,
        "package_versions": package_versions,
        "count": package_map.len(),
    }))
}

/// Fetch historical package bytecodes with transitive dependency resolution.
#[napi]
pub async fn fetch_historical_package_bytecodes(
    package_ids: Vec<String>,
    type_refs: Option<Vec<String>>,
    checkpoint: Option<u32>,
    endpoint: Option<String>,
    api_key: Option<String>,
) -> napi::Result<serde_json::Value> {
    fetch_historical_package_bytecodes_inner(
        &package_ids,
        &type_refs.unwrap_or_default(),
        checkpoint.map(|v| v as u64),
        endpoint.as_deref(),
        api_key.as_deref(),
    )
    .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// View function execution
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn call_view_function_inner(
    package_id: &str,
    module: &str,
    function: &str,
    type_args: Vec<String>,
    object_inputs: Vec<(String, Vec<u8>, String, bool, bool)>,
    pure_inputs: Vec<Vec<u8>>,
    child_objects: HashMap<String, Vec<(String, Vec<u8>, String)>>,
    historical_versions: HashMap<String, u64>,
    fetch_child_objects: bool,
    grpc_endpoint: Option<String>,
    grpc_api_key: Option<String>,
    package_bytecodes: HashMap<String, Vec<Vec<u8>>>,
    package_aliases: HashMap<String, String>,
    linkage_upgrades: HashMap<String, String>,
    package_runtime_ids: HashMap<String, String>,
    package_linkage: HashMap<String, HashMap<String, String>>,
    package_versions: HashMap<String, u64>,
    fetch_deps: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::ptb::{Argument, Command, ObjectInput, PTBExecutor};
    use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
    use move_core_types::identifier::Identifier;

    // 1. Build LocalModuleResolver with sui framework
    let mut resolver = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;

    // 2. Load provided package bytecodes
    let mut loaded_packages = HashSet::new();
    loaded_packages.insert(AccountAddress::from_hex_literal("0x1").unwrap());
    loaded_packages.insert(AccountAddress::from_hex_literal("0x2").unwrap());
    loaded_packages.insert(AccountAddress::from_hex_literal("0x3").unwrap());

    let mut skipped_original_packages: HashSet<String> = HashSet::new();
    for (original, upgraded) in &linkage_upgrades {
        if original != upgraded
            && package_bytecodes.contains_key(original)
            && package_bytecodes.contains_key(upgraded)
        {
            skipped_original_packages.insert(original.clone());
        }
    }

    let mut package_entries: Vec<(&String, &Vec<Vec<u8>>)> = package_bytecodes.iter().collect();
    package_entries.sort_by(|a, b| a.0.cmp(b.0));
    for (pkg_id_str, module_bytecodes) in package_entries {
        if skipped_original_packages.contains(pkg_id_str) {
            continue;
        }
        let addr = AccountAddress::from_hex_literal(pkg_id_str)
            .with_context(|| format!("invalid package address: {}", pkg_id_str))?;
        if is_framework_address(&addr) {
            continue;
        }
        let modules: Vec<(String, Vec<u8>)> = module_bytecodes
            .iter()
            .enumerate()
            .map(|(i, bytes)| {
                if let Ok(compiled) = CompiledModule::deserialize_with_defaults(bytes) {
                    let name = compiled.self_id().name().to_string();
                    (name, bytes.clone())
                } else {
                    (format!("module_{}", i), bytes.clone())
                }
            })
            .collect();
        resolver.add_package_modules_at(modules, Some(addr))?;
        loaded_packages.insert(addr);
    }

    for (storage_str, runtime_str) in &package_aliases {
        let storage = AccountAddress::from_hex_literal(storage_str)
            .with_context(|| format!("invalid package alias storage id: {}", storage_str))?;
        let runtime = AccountAddress::from_hex_literal(runtime_str)
            .with_context(|| format!("invalid package alias runtime id: {}", runtime_str))?;
        resolver.add_address_alias(storage, runtime);
    }

    for (original_str, upgraded_str) in &linkage_upgrades {
        let original = AccountAddress::from_hex_literal(original_str)
            .with_context(|| format!("invalid linkage original id: {}", original_str))?;
        let upgraded = AccountAddress::from_hex_literal(upgraded_str)
            .with_context(|| format!("invalid linkage upgraded id: {}", upgraded_str))?;
        resolver.add_linkage_upgrade(original, upgraded);
    }

    for (storage_str, linkage_entries) in &package_linkage {
        if skipped_original_packages.contains(storage_str) {
            continue;
        }
        let storage = AccountAddress::from_hex_literal(storage_str)
            .with_context(|| format!("invalid linkage storage id: {}", storage_str))?;
        let runtime = if let Some(runtime_str) = package_runtime_ids.get(storage_str) {
            AccountAddress::from_hex_literal(runtime_str)
                .with_context(|| format!("invalid package runtime id: {}", runtime_str))?
        } else {
            storage
        };

        let mut linkage_map: HashMap<AccountAddress, AccountAddress> = HashMap::new();
        for (dep_runtime_str, dep_storage_str) in linkage_entries {
            let dep_runtime = AccountAddress::from_hex_literal(dep_runtime_str)
                .with_context(|| format!("invalid dep runtime id: {}", dep_runtime_str))?;
            let dep_storage = AccountAddress::from_hex_literal(dep_storage_str)
                .with_context(|| format!("invalid dep storage id: {}", dep_storage_str))?;
            linkage_map.insert(dep_runtime, dep_storage);
        }
        resolver.add_package_linkage(storage, runtime, &linkage_map);
    }

    for (storage_str, runtime_str) in &package_runtime_ids {
        if skipped_original_packages.contains(storage_str) {
            continue;
        }
        if package_linkage.contains_key(storage_str) {
            continue;
        }
        let storage = AccountAddress::from_hex_literal(storage_str)
            .with_context(|| format!("invalid package runtime storage id: {}", storage_str))?;
        let runtime = AccountAddress::from_hex_literal(runtime_str)
            .with_context(|| format!("invalid package runtime id: {}", runtime_str))?;
        resolver.add_package_linkage(storage, runtime, &HashMap::new());
    }

    // 3. If fetch_deps, resolve transitive dependencies via GraphQL
    if fetch_deps {
        let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
        let graphql = GraphQLClient::new(&graphql_endpoint);

        let mut to_fetch: VecDeque<AccountAddress> = VecDeque::new();

        let target_addr = AccountAddress::from_hex_literal(package_id)
            .with_context(|| format!("invalid target package: {}", package_id))?;
        if !loaded_packages.contains(&target_addr) {
            to_fetch.push_back(target_addr);
        }

        for ta_str in &type_args {
            for pkg_id in sui_sandbox_core::utilities::extract_package_ids_from_type(ta_str) {
                if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                    if !loaded_packages.contains(&addr) && !is_framework_address(&addr) {
                        to_fetch.push_back(addr);
                    }
                }
            }
        }
        for (_, _, type_tag_str, _, _) in &object_inputs {
            for pkg_id in sui_sandbox_core::utilities::extract_package_ids_from_type(type_tag_str) {
                if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                    if !loaded_packages.contains(&addr) && !is_framework_address(&addr) {
                        to_fetch.push_back(addr);
                    }
                }
            }
        }

        for module_bytecodes in package_bytecodes.values() {
            let modules: Vec<(String, Vec<u8>)> = module_bytecodes
                .iter()
                .enumerate()
                .map(|(i, b)| (format!("m{}", i), b.clone()))
                .collect();
            for dep_addr in extract_dependency_addrs(&modules) {
                if !loaded_packages.contains(&dep_addr) && !is_framework_address(&dep_addr) {
                    to_fetch.push_back(dep_addr);
                }
            }
        }

        const MAX_DEP_ROUNDS: usize = 8;
        let mut visited = loaded_packages.clone();
        let mut rounds = 0;
        while let Some(addr) = to_fetch.pop_front() {
            if visited.contains(&addr) || is_framework_address(&addr) {
                continue;
            }
            rounds += 1;
            if rounds > MAX_DEP_ROUNDS {
                eprintln!(
                    "Warning: dependency resolution hit max depth ({} packages fetched), \
                     stopping. Some transitive deps may be missing.",
                    MAX_DEP_ROUNDS
                );
                break;
            }
            visited.insert(addr);

            let hex = addr.to_hex_literal();
            match fetch_package_modules(&graphql, &hex) {
                Ok(modules) => {
                    let dep_addrs = extract_dependency_addrs(&modules);
                    resolver.add_package_modules_at(modules, Some(addr))?;
                    loaded_packages.insert(addr);

                    for dep_addr in dep_addrs {
                        if !visited.contains(&dep_addr) && !is_framework_address(&dep_addr) {
                            to_fetch.push_back(dep_addr);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: failed to fetch package {}: {:#}", hex, e);
                }
            }
        }
    }

    // 4. Create VMHarness with simulation config
    let config = SimulationConfig::default();
    let mut vm = VMHarness::with_config(&resolver, false, config)?;
    let mut alias_map: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    for (storage_str, runtime_str) in &package_aliases {
        let storage = AccountAddress::from_hex_literal(storage_str)
            .with_context(|| format!("invalid alias storage id: {}", storage_str))?;
        let runtime = AccountAddress::from_hex_literal(runtime_str)
            .with_context(|| format!("invalid alias runtime id: {}", runtime_str))?;
        if storage != runtime {
            alias_map.insert(storage, runtime);
        }
    }
    if alias_map.is_empty() {
        alias_map = resolver.get_all_aliases().into_iter().collect();
    }
    if !alias_map.is_empty() {
        vm.set_address_aliases_with_versions(alias_map, package_versions.clone());
    }

    // 5. Set up child fetcher
    if !child_objects.is_empty() || fetch_child_objects {
        let mut child_map: HashMap<(AccountAddress, AccountAddress), (move_core_types::language_storage::TypeTag, Vec<u8>)> =
            HashMap::new();
        for (parent_id_str, children) in &child_objects {
            let parent_addr = AccountAddress::from_hex_literal(parent_id_str)
                .with_context(|| format!("invalid parent_id: {}", parent_id_str))?;
            for (child_id_str, bcs_bytes, type_tag_str) in children {
                let child_addr = AccountAddress::from_hex_literal(child_id_str)
                    .with_context(|| format!("invalid child_id: {}", child_id_str))?;
                let type_tag = sui_sandbox_core::types::parse_type_tag(type_tag_str)
                    .with_context(|| format!("invalid type tag: {}", type_tag_str))?;
                child_map.insert((parent_addr, child_addr), (type_tag, bcs_bytes.clone()));
            }
        }

        let grpc_child_config: Option<Arc<(String, Option<String>)>> = if fetch_child_objects {
            let (resolved_endpoint, resolved_api_key) =
                resolve_grpc_endpoint_and_key(grpc_endpoint.as_deref(), grpc_api_key.as_deref());
            Some(Arc::new((resolved_endpoint, resolved_api_key)))
        } else {
            None
        };

        let child_map = Arc::new(child_map);
        let historical_versions_for_fetcher = Arc::new(historical_versions.clone());
        let fetcher: sui_sandbox_core::sandbox_runtime::ChildFetcherFn =
            Box::new(move |parent, child| {
                if let Some(found) = child_map.get(&(parent, child)).cloned() {
                    return Some(found);
                }

                let grpc_cfg = grpc_child_config.as_ref()?;
                let child_id_str = child.to_hex_literal();
                let historical_version =
                    historical_versions_for_fetcher.get(&child_id_str).copied();

                let rt = tokio::runtime::Runtime::new().ok()?;
                let fetched = rt.block_on(async {
                    let client = GrpcClient::with_api_key(&grpc_cfg.0, grpc_cfg.1.clone())
                        .await
                        .ok()?;
                    client
                        .get_object_at_version(&child_id_str, historical_version)
                        .await
                        .ok()
                        .flatten()
                });

                let object = fetched?;
                let type_tag_str = object.type_string?;
                let bcs = object.bcs?;
                let type_tag = sui_sandbox_core::types::parse_type_tag(&type_tag_str).ok()?;
                Some((type_tag, bcs))
            });
        vm.set_child_fetcher(fetcher);
    }

    // 6. Build PTB and execute
    let mut executor = PTBExecutor::new(&mut vm);

    let mut input_indices = Vec::new();
    for (obj_id_str, bcs_bytes, type_tag_str, is_shared, mutable) in &object_inputs {
        let id = AccountAddress::from_hex_literal(obj_id_str)
            .with_context(|| format!("invalid object_id: {}", obj_id_str))?;
        let type_tag = sui_sandbox_core::types::parse_type_tag(type_tag_str)
            .with_context(|| format!("invalid type tag: {}", type_tag_str))?;
        let obj_version = historical_versions.get(obj_id_str).copied();

        let obj_input = if *is_shared {
            ObjectInput::Shared {
                id,
                bytes: bcs_bytes.clone(),
                type_tag: Some(type_tag),
                version: obj_version,
                mutable: *mutable,
            }
        } else {
            ObjectInput::ImmRef {
                id,
                bytes: bcs_bytes.clone(),
                type_tag: Some(type_tag),
                version: obj_version,
            }
        };

        let idx = executor
            .add_object_input(obj_input)
            .with_context(|| format!("add object input {}", obj_id_str))?;
        input_indices.push(idx);
    }

    for pure_bytes in &pure_inputs {
        let idx = executor
            .add_pure_input(pure_bytes.clone())
            .context("add pure input")?;
        input_indices.push(idx);
    }

    let mut parsed_type_args = Vec::new();
    for ta_str in &type_args {
        let tt = sui_sandbox_core::types::parse_type_tag(ta_str)
            .with_context(|| format!("invalid type arg: {}", ta_str))?;
        parsed_type_args.push(tt);
    }

    let args: Vec<Argument> = (0..input_indices.len() as u16)
        .map(Argument::Input)
        .collect();

    let target_addr = AccountAddress::from_hex_literal(package_id)
        .with_context(|| format!("invalid package address: {}", package_id))?;
    let commands = vec![Command::MoveCall {
        package: target_addr,
        module: Identifier::new(module).context("invalid module name")?,
        function: Identifier::new(function).context("invalid function name")?,
        type_args: parsed_type_args,
        args,
    }];

    // 7. Execute
    let effects = executor.execute_commands(&commands)?;

    // 8. Build result
    let return_values: Vec<Vec<String>> = effects
        .return_values
        .iter()
        .map(|cmd_returns| {
            cmd_returns
                .iter()
                .map(|rv_bytes| base64::engine::general_purpose::STANDARD.encode(rv_bytes))
                .collect()
        })
        .collect();

    let return_type_tags: Vec<Vec<Option<String>>> = effects
        .return_type_tags
        .iter()
        .map(|cmd_types| {
            cmd_types
                .iter()
                .map(|type_tag| type_tag.as_ref().map(|t| t.to_canonical_string(true)))
                .collect()
        })
        .collect();

    Ok(serde_json::json!({
        "success": effects.success,
        "error": effects.error,
        "return_values": return_values,
        "return_type_tags": return_type_tags,
        "gas_used": effects.gas_used,
    }))
}

/// Execute a view function via local Move VM.
///
/// object_inputs: Array of {objectId, bcsBytes, typeTag, isShared?, mutable?, owner?}
/// pure_inputs: Array of BCS-encoded pure argument buffers
/// child_objects: Map parent_id -> [{childId, bcsBytes, typeTag}]
/// historical_versions: Map object_id -> version
/// package_bytecodes: Map package_id -> [module_bytes_buffer or base64_string]
///   OR full payload from fetchHistoricalPackageBytecodes(...)
#[napi]
pub async fn call_view_function(
    package_id: String,
    module: String,
    function: String,
    type_args: Option<Vec<String>>,
    object_inputs: Option<Vec<serde_json::Value>>,
    pure_inputs: Option<Vec<napi::bindgen_prelude::Buffer>>,
    child_objects: Option<serde_json::Value>,
    historical_versions: Option<serde_json::Value>,
    fetch_child_objects: Option<bool>,
    grpc_endpoint: Option<String>,
    grpc_api_key: Option<String>,
    package_bytecodes: Option<serde_json::Value>,
    fetch_deps: Option<bool>,
) -> napi::Result<serde_json::Value> {
    // Parse object_inputs from JSON
    let mut parsed_obj_inputs: Vec<(String, Vec<u8>, String, bool, bool)> = Vec::new();
    if let Some(inputs) = object_inputs {
        for obj in &inputs {
            let object_id = obj.get("objectId").or(obj.get("object_id"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| to_napi_err(anyhow!("missing 'objectId' in objectInputs")))?
                .to_string();
            let bcs_b64 = obj.get("bcsBytes").or(obj.get("bcs_bytes"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| to_napi_err(anyhow!("missing 'bcsBytes' in objectInputs")))?;
            let bcs_bytes = base64::engine::general_purpose::STANDARD
                .decode(bcs_b64)
                .map_err(|e| to_napi_err(anyhow!("invalid base64 in bcsBytes: {}", e)))?;
            let type_tag = obj.get("typeTag").or(obj.get("type_tag"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| to_napi_err(anyhow!("missing 'typeTag' in objectInputs")))?
                .to_string();
            let is_shared = obj.get("isShared").or(obj.get("is_shared"))
                .and_then(|v| v.as_bool()).unwrap_or(false);
            let mutable = obj.get("mutable")
                .and_then(|v| v.as_bool()).unwrap_or(false);

            // Legacy owner field
            let owner = obj.get("owner").and_then(|v| v.as_str());
            let (final_shared, final_mutable) = if obj.get("isShared").is_none() && obj.get("is_shared").is_none() {
                match owner.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
                    Some("shared") => (true, obj.get("mutable").and_then(|v| v.as_bool()).unwrap_or(true)),
                    Some("immutable") | Some("address_owned") => (false, mutable),
                    None => (is_shared, mutable),
                    Some(other) => return Err(to_napi_err(anyhow!(
                        "invalid 'owner' in objectInputs: {} (expected immutable|shared|address_owned)", other
                    ))),
                }
            } else {
                (is_shared, mutable)
            };

            parsed_obj_inputs.push((object_id, bcs_bytes, type_tag, final_shared, final_mutable));
        }
    }

    let parsed_pure: Vec<Vec<u8>> = pure_inputs
        .unwrap_or_default()
        .into_iter()
        .map(|b| b.to_vec())
        .collect();

    // Parse child_objects
    let mut parsed_children: HashMap<String, Vec<(String, Vec<u8>, String)>> = HashMap::new();
    if let Some(co) = child_objects {
        if let Some(obj) = co.as_object() {
            for (parent_id, children_val) in obj {
                if let Some(children) = children_val.as_array() {
                    let mut child_vec = Vec::new();
                    for child in children {
                        let child_id = child.get("childId").or(child.get("child_id"))
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| to_napi_err(anyhow!("missing 'childId'")))?
                            .to_string();
                        let bcs_b64 = child.get("bcsBytes").or(child.get("bcs_bytes"))
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| to_napi_err(anyhow!("missing 'bcsBytes'")))?;
                        let bcs = base64::engine::general_purpose::STANDARD
                            .decode(bcs_b64)
                            .map_err(|e| to_napi_err(anyhow!("invalid base64: {}", e)))?;
                        let tt = child.get("typeTag").or(child.get("type_tag"))
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| to_napi_err(anyhow!("missing 'typeTag'")))?
                            .to_string();
                        child_vec.push((child_id, bcs, tt));
                    }
                    parsed_children.insert(parent_id.clone(), child_vec);
                }
            }
        }
    }

    // Parse historical_versions
    let mut parsed_hist_versions: HashMap<String, u64> = HashMap::new();
    if let Some(hv) = historical_versions {
        if let Some(obj) = hv.as_object() {
            for (key, val) in obj {
                if let Some(v) = val.as_u64() {
                    parsed_hist_versions.insert(key.clone(), v);
                }
            }
        }
    }

    // Parse package_bytecodes
    let mut parsed_pkg_bytes: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    let mut parsed_aliases: HashMap<String, String> = HashMap::new();
    let mut parsed_linkage_upgrades: HashMap<String, String> = HashMap::new();
    let mut parsed_runtime_ids: HashMap<String, String> = HashMap::new();
    let mut parsed_pkg_linkage: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut parsed_pkg_versions: HashMap<String, u64> = HashMap::new();
    let mut historical_payload_mode = false;

    if let Some(pb) = &package_bytecodes {
        if let Some(obj) = pb.as_object() {
            let packages_obj = if let Some(pkgs) = obj.get("packages").and_then(|v| v.as_object()) {
                historical_payload_mode = true;
                pkgs
            } else {
                obj
            };

            for (pkg_id, modules_val) in packages_obj {
                if let Some(modules) = modules_val.as_array() {
                    let mut bytecodes = Vec::new();
                    for m in modules {
                        if let Some(s) = m.as_str() {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(s)
                                .map_err(|e| to_napi_err(anyhow!("invalid base64 module: {}", e)))?;
                            bytecodes.push(bytes);
                        }
                    }
                    parsed_pkg_bytes.insert(pkg_id.clone(), bytecodes);
                }
            }

            if let Some(a) = obj.get("aliases").and_then(|v| v.as_object()) {
                for (k, v) in a { if let Some(s) = v.as_str() { parsed_aliases.insert(k.clone(), s.to_string()); } }
            }
            if let Some(lu) = obj.get("linkage_upgrades").and_then(|v| v.as_object()) {
                for (k, v) in lu { if let Some(s) = v.as_str() { parsed_linkage_upgrades.insert(k.clone(), s.to_string()); } }
            }
            if let Some(ri) = obj.get("package_runtime_ids").and_then(|v| v.as_object()) {
                for (k, v) in ri { if let Some(s) = v.as_str() { parsed_runtime_ids.insert(k.clone(), s.to_string()); } }
            }
            if let Some(pl) = obj.get("package_linkage").and_then(|v| v.as_object()) {
                for (k, v) in pl {
                    if let Some(inner) = v.as_object() {
                        let mut map = HashMap::new();
                        for (ik, iv) in inner { if let Some(s) = iv.as_str() { map.insert(ik.clone(), s.to_string()); } }
                        parsed_pkg_linkage.insert(k.clone(), map);
                    }
                }
            }
            if let Some(pv) = obj.get("package_versions").and_then(|v| v.as_object()) {
                for (k, v) in pv { if let Some(n) = v.as_u64() { parsed_pkg_versions.insert(k.clone(), n); } }
            }
        }
    }

    let effective_fetch_deps = if historical_payload_mode { false } else { fetch_deps.unwrap_or(true) };

    call_view_function_inner(
        &package_id,
        &module,
        &function,
        type_args.unwrap_or_default(),
        parsed_obj_inputs,
        parsed_pure,
        parsed_children,
        parsed_hist_versions,
        fetch_child_objects.unwrap_or(false),
        grpc_endpoint,
        grpc_api_key,
        parsed_pkg_bytes,
        parsed_aliases,
        parsed_linkage_upgrades,
        parsed_runtime_ids,
        parsed_pkg_linkage,
        parsed_pkg_versions,
        effective_fetch_deps,
    )
    .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// Historical view functions
// ---------------------------------------------------------------------------

/// Execute a generic historical Move view function from a versions snapshot.
#[napi]
pub async fn historical_view_from_versions(
    versions_file: String,
    package_id: String,
    module: String,
    function: String,
    required_objects: Vec<String>,
    type_args: Option<Vec<String>>,
    package_roots: Option<Vec<String>>,
    type_refs: Option<Vec<String>>,
    fetch_child_objects: Option<bool>,
    grpc_endpoint: Option<String>,
    grpc_api_key: Option<String>,
) -> napi::Result<serde_json::Value> {
    let versions_path = PathBuf::from(&versions_file);
    let request = CoreHistoricalViewRequest {
        package_id,
        module,
        function,
        type_args: type_args.unwrap_or_default(),
        required_objects,
        package_roots: package_roots.unwrap_or_default(),
        type_refs: type_refs.unwrap_or_default(),
        fetch_child_objects: fetch_child_objects.unwrap_or(true),
    };
    let output = core_execute_historical_view_from_versions(
        &versions_path,
        &request,
        grpc_endpoint.as_deref(),
        grpc_api_key.as_deref(),
    )
    .map_err(to_napi_err)?;
    serde_json::to_value(output)
        .context("Failed to serialize historical view output")
        .map_err(to_napi_err)
}

/// Execute historical series across labeled checkpoint/version points.
#[napi]
pub async fn historical_series_from_points(
    points: serde_json::Value,
    package_id: String,
    module: String,
    function: String,
    required_objects: Vec<String>,
    type_args: Option<Vec<String>>,
    package_roots: Option<Vec<String>>,
    type_refs: Option<Vec<String>>,
    fetch_child_objects: Option<bool>,
    schema: Option<serde_json::Value>,
    command_index: Option<u32>,
    grpc_endpoint: Option<String>,
    grpc_api_key: Option<String>,
    max_concurrency: Option<u32>,
) -> napi::Result<serde_json::Value> {
    let parsed_points: Vec<CoreHistoricalSeriesPoint> = serde_json::from_value(points)
        .map_err(|e| to_napi_err(anyhow!("invalid historical series points payload: {}", e)))?;
    let schema_fields: Option<Vec<ReturnDecodeField>> = schema
        .map(|v| serde_json::from_value(v))
        .transpose()
        .map_err(|e| to_napi_err(anyhow!("invalid historical series schema: {}", e)))?;

    let cmd_idx = command_index.unwrap_or(0) as usize;
    let request = CoreHistoricalViewRequest {
        package_id,
        module,
        function,
        type_args: type_args.unwrap_or_default(),
        required_objects,
        package_roots: package_roots.unwrap_or_default(),
        type_refs: type_refs.unwrap_or_default(),
        fetch_child_objects: fetch_child_objects.unwrap_or(true),
    };
    let options = CoreHistoricalSeriesExecutionOptions {
        max_concurrency: Some(max_concurrency.map(|v| v as usize).unwrap_or(1).max(1)),
    };

    let runs = if let Some(fields) = schema_fields.as_ref() {
        ReplayOrchestrator::execute_historical_series_with_schema_and_options(
            &parsed_points, &request, cmd_idx, fields,
            grpc_endpoint.as_deref(), grpc_api_key.as_deref(), &options,
        ).map_err(to_napi_err)?
    } else {
        ReplayOrchestrator::execute_historical_series_with_options(
            &parsed_points, &request,
            grpc_endpoint.as_deref(), grpc_api_key.as_deref(), &options,
        ).map_err(to_napi_err)?
    };
    let summary = ReplayOrchestrator::summarize_historical_series_runs(&runs);
    Ok(serde_json::json!({
        "request": request,
        "points": parsed_points.len(),
        "summary": summary,
        "runs": runs,
    }))
}

/// Execute historical series from request/series/schema files.
#[napi]
pub async fn historical_series_from_files(
    request_file: String,
    series_file: String,
    schema_file: Option<String>,
    command_index: Option<u32>,
    grpc_endpoint: Option<String>,
    grpc_api_key: Option<String>,
    max_concurrency: Option<u32>,
) -> napi::Result<serde_json::Value> {
    let options = CoreHistoricalSeriesExecutionOptions {
        max_concurrency: Some(max_concurrency.map(|v| v as usize).unwrap_or(1).max(1)),
    };
    let report = ReplayOrchestrator::execute_historical_series_from_files_with_options(
        Path::new(&request_file),
        Path::new(&series_file),
        schema_file.as_deref().map(Path::new),
        command_index.unwrap_or(0) as usize,
        grpc_endpoint.as_deref(),
        grpc_api_key.as_deref(),
        &options,
    )
    .map_err(to_napi_err)?;
    serde_json::to_value(report)
        .context("Failed to serialize historical series file-run report")
        .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// Historical decode helpers
// ---------------------------------------------------------------------------

fn decode_historical_result_with_raw_fallback<T, F>(
    raw: &serde_json::Value,
    mut decode: F,
) -> Result<Option<T>>
where
    F: FnMut(&serde_json::Value) -> Result<Option<T>>,
{
    match decode(raw) {
        Ok(Some(value)) => Ok(Some(value)),
        Ok(None) => {
            if let Some(inner) = raw.get("raw") {
                decode(inner)
            } else {
                Ok(None)
            }
        }
        Err(primary_err) => {
            if let Some(inner) = raw.get("raw") {
                decode(inner).or(Err(primary_err))
            } else {
                Err(primary_err)
            }
        }
    }
}

/// Decode one u64 return value from historical view output.
#[napi]
pub fn historical_decode_return_u64(
    result: serde_json::Value,
    command_index: Option<u32>,
    value_index: u32,
) -> napi::Result<Option<f64>> {
    let cmd_idx = command_index.unwrap_or(0) as usize;
    let val_idx = value_index as usize;
    decode_historical_result_with_raw_fallback(&result, |candidate| {
        ReplayOrchestrator::decode_command_return_u64(candidate, cmd_idx, val_idx)
    })
    .map(|opt| opt.map(|v| v as f64))
    .map_err(to_napi_err)
}

/// Decode all return values from a command into u64 values.
#[napi]
pub fn historical_decode_return_u64s(
    result: serde_json::Value,
    command_index: Option<u32>,
) -> napi::Result<Option<Vec<Option<f64>>>> {
    let cmd_idx = command_index.unwrap_or(0) as usize;
    let decoded = decode_historical_result_with_raw_fallback(&result, |candidate| {
        ReplayOrchestrator::decode_command_return_values(candidate, cmd_idx).map(|opt| {
            opt.map(|values| {
                values
                    .into_iter()
                    .map(|bytes| {
                        if bytes.len() < 8 { return None; }
                        let mut buf = [0u8; 8];
                        buf.copy_from_slice(&bytes[..8]);
                        Some(u64::from_le_bytes(buf) as f64)
                    })
                    .collect::<Vec<_>>()
            })
        })
    })
    .map_err(to_napi_err)?;
    Ok(decoded)
}

/// Decode all return values from a historical-view command into typed JSON values.
#[napi]
pub fn historical_decode_returns_typed(
    result: serde_json::Value,
    command_index: Option<u32>,
) -> napi::Result<Option<serde_json::Value>> {
    let cmd_idx = command_index.unwrap_or(0) as usize;
    let decoded = decode_historical_result_with_raw_fallback(&result, |candidate| {
        ReplayOrchestrator::decode_command_return_values_typed(candidate, cmd_idx)
    })
    .map_err(to_napi_err)?;
    match decoded {
        Some(values) => {
            let value = serde_json::to_value(values).map_err(|e| {
                to_napi_err(anyhow!(
                    "failed to serialize typed return decode output: {}",
                    e
                ))
            })?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

/// Decode historical-view return values into a named object with field schema.
#[napi]
pub fn historical_decode_with_schema(
    result: serde_json::Value,
    schema: serde_json::Value,
    command_index: Option<u32>,
) -> napi::Result<Option<serde_json::Value>> {
    let cmd_idx = command_index.unwrap_or(0) as usize;
    let fields: Vec<ReturnDecodeField> = serde_json::from_value(schema)
        .map_err(|e| to_napi_err(anyhow!("invalid return decode schema: {}", e)))?;

    let decoded = decode_historical_result_with_raw_fallback(&result, |candidate| {
        ReplayOrchestrator::decode_command_return_schema(candidate, cmd_idx, &fields)
    })
    .map_err(to_napi_err)?;
    Ok(decoded.map(serde_json::Value::Object))
}

// ---------------------------------------------------------------------------
// Fuzzing
// ---------------------------------------------------------------------------

fn fuzz_function_inner(
    package_id: &str,
    module: &str,
    function: &str,
    iterations: u64,
    seed: u64,
    sender: &str,
    gas_budget: u64,
    type_args: Vec<String>,
    fail_fast: bool,
    max_vector_len: usize,
    dry_run: bool,
    fetch_deps: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::fuzz::{classify_params, FuzzConfig, FuzzRunner};

    let (resolver, _loaded) = if fetch_deps {
        build_resolver_with_deps(package_id, &type_args)?
    } else {
        let r = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;
        let mut loaded = HashSet::new();
        for fw in ["0x1", "0x2", "0x3"] {
            loaded.insert(AccountAddress::from_hex_literal(fw).unwrap());
        }
        (r, loaded)
    };

    let target_addr = AccountAddress::from_hex_literal(package_id)
        .with_context(|| format!("invalid package address: {}", package_id))?;

    let compiled_module = resolver
        .get_module_by_addr_name(&target_addr, module)
        .ok_or_else(|| anyhow!("Module '{}::{}' not found", package_id, module))?;

    let sig = resolver
        .get_function_signature(&target_addr, module, function)
        .ok_or_else(|| {
            anyhow!(
                "Function '{}::{}::{}' not found",
                package_id, module, function
            )
        })?;

    let classification = classify_params(compiled_module, &sig.parameter_types);
    let target = format!("{}::{}::{}", package_id, module, function);

    if dry_run {
        return Ok(serde_json::json!({
            "target": target,
            "classification": classification,
            "verdict": if classification.is_fully_fuzzable { "FULLY_FUZZABLE" } else { "NOT_FUZZABLE" },
        }));
    }

    if !classification.is_fully_fuzzable {
        return Ok(serde_json::json!({
            "target": target,
            "classification": classification,
            "verdict": "NOT_FUZZABLE",
            "reason": format!(
                "Function has {} object and {} unfuzzable parameter(s)",
                classification.object_count, classification.unfuzzable_count
            ),
        }));
    }

    let sender_addr = AccountAddress::from_hex_literal(sender).context("Invalid sender address")?;
    let parsed_type_args = type_args
        .iter()
        .map(|s| sui_sandbox_core::types::parse_type_tag(s))
        .collect::<Result<Vec<_>>>()?;

    let config = FuzzConfig {
        iterations,
        seed,
        sender: sender_addr,
        gas_budget,
        type_args: parsed_type_args,
        fail_fast,
        max_vector_len,
    };

    let runner = FuzzRunner::new(&resolver);
    let report = runner.run(target_addr, module, function, &classification, &config)?;

    serde_json::to_value(&report).map_err(|e| anyhow!("Failed to serialize fuzz report: {}", e))
}

/// Fuzz a Move function with randomly generated inputs.
#[napi]
pub async fn fuzz_function(
    package_id: String,
    module: String,
    function: String,
    iterations: Option<u32>,
    seed: Option<f64>,
    sender: Option<String>,
    gas_budget: Option<f64>,
    type_args: Option<Vec<String>>,
    fail_fast: Option<bool>,
    max_vector_len: Option<u32>,
    dry_run: Option<bool>,
    fetch_deps: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let actual_seed = seed.map(|v| v as u64).unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    });

    fuzz_function_inner(
        &package_id,
        &module,
        &function,
        iterations.map(|v| v as u64).unwrap_or(100),
        actual_seed,
        sender.as_deref().unwrap_or("0x0"),
        gas_budget.map(|v| v as u64).unwrap_or(50_000_000_000),
        type_args.unwrap_or_default(),
        fail_fast.unwrap_or(false),
        max_vector_len.map(|v| v as usize).unwrap_or(32),
        dry_run.unwrap_or(false),
        fetch_deps.unwrap_or(true),
    )
    .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// OrchestrationSession class
// ---------------------------------------------------------------------------

/// Interactive two-step flow helper for Node.js.
///
/// Keeps prepared package context in memory and reuses it across replays.
#[napi(js_name = "OrchestrationSession")]
pub struct OrchestrationSession {
    context: Option<serde_json::Value>,
    package_id: Option<String>,
}

#[napi]
impl OrchestrationSession {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            context: None,
            package_id: None,
        }
    }

    #[napi]
    pub fn prepare(
        &mut self,
        package_id: String,
        resolve_deps: Option<bool>,
        output_path: Option<String>,
    ) -> napi::Result<serde_json::Value> {
        let value = prepare_package_context_inner(
            &package_id,
            resolve_deps.unwrap_or(true),
            output_path.as_deref(),
        )
        .map_err(to_napi_err)?;
        self.package_id = Some(package_id);
        self.context = Some(value.clone());
        Ok(value)
    }

    #[napi]
    pub fn load_context(&mut self, context_path: String) -> napi::Result<serde_json::Value> {
        let path = PathBuf::from(&context_path);
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read context file {}", path.display()))
            .map_err(to_napi_err)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("Invalid context JSON in {}", path.display()))
            .map_err(to_napi_err)?;
        self.package_id = value
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        self.context = Some(value.clone());
        Ok(value)
    }

    #[napi]
    pub fn save_context(&self, context_path: String) -> napi::Result<()> {
        let value = self.context.as_ref().ok_or_else(|| {
            to_napi_err(anyhow!(
                "OrchestrationSession has no context; call prepare() or loadContext()"
            ))
        })?;
        let path = PathBuf::from(&context_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    to_napi_err(anyhow!(
                        "Failed to create context directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }
        let serialized =
            serde_json::to_string_pretty(value).map_err(|e| to_napi_err(anyhow!(e)))?;
        std::fs::write(&path, serialized).map_err(|e| {
            to_napi_err(anyhow!(
                "Failed to write context file {}: {}",
                path.display(),
                e
            ))
        })?;
        Ok(())
    }

    #[napi]
    pub fn has_context(&self) -> bool {
        self.context.is_some()
    }

    #[napi(getter)]
    pub fn get_package_id(&self) -> Option<String> {
        self.package_id.clone()
    }

    #[napi(getter)]
    pub fn get_context(&self) -> Option<serde_json::Value> {
        self.context.clone()
    }

    #[napi]
    pub fn replay(
        &self,
        digest: Option<String>,
        checkpoint: Option<u32>,
        discover_latest: Option<u32>,
        source: Option<String>,
        state_file: Option<String>,
        cache_dir: Option<String>,
        walrus_network: Option<String>,
        walrus_caching_url: Option<String>,
        walrus_aggregator_url: Option<String>,
        rpc_url: Option<String>,
        profile: Option<String>,
        fetch_strategy: Option<String>,
        vm_only: Option<bool>,
        allow_fallback: Option<bool>,
        prefetch_depth: Option<u32>,
        prefetch_limit: Option<u32>,
        auto_system_objects: Option<bool>,
        no_prefetch: Option<bool>,
        compare: Option<bool>,
        analyze_only: Option<bool>,
        synthesize_missing: Option<bool>,
        self_heal_dynamic_fields: Option<bool>,
        analyze_mm2: Option<bool>,
        verbose: Option<bool>,
    ) -> napi::Result<serde_json::Value> {
        let context_tmp = match self.context.as_ref() {
            Some(value) => Some(write_temp_context_file(value).map_err(to_napi_err)?),
            None => None,
        };

        let discover_package_id = if discover_latest.is_some() {
            self.package_id.clone()
        } else {
            None
        };

        let result = replay_api::replay_transaction(
            digest,
            checkpoint,
            discover_latest,
            discover_package_id,
            source,
            state_file,
            context_tmp.as_ref().and_then(|p| p.to_str()).map(String::from),
            cache_dir,
            walrus_network,
            walrus_caching_url,
            walrus_aggregator_url,
            rpc_url,
            profile,
            fetch_strategy,
            vm_only,
            allow_fallback,
            prefetch_depth,
            prefetch_limit,
            auto_system_objects,
            no_prefetch,
            compare,
            analyze_only,
            synthesize_missing,
            self_heal_dynamic_fields,
            analyze_mm2,
            verbose,
        );

        if let Some(path) = context_tmp {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Version constant
// ---------------------------------------------------------------------------

/// Get the native addon version.
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
