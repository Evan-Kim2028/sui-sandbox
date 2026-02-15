//! Python bindings for Sui Move package analysis, checkpoint replay, view function
//! execution, and Move function fuzzing.
//!
//! **All functions are standalone** — `pip install sui-sandbox` is all you need:
//! - `extract_interface`: Extract full Move package interface from bytecode or GraphQL
//! - `get_latest_checkpoint`: Get latest Walrus checkpoint number
//! - `get_checkpoint`: Fetch and summarize a Walrus checkpoint
//! - `doctor`: Run endpoint/environment preflight checks
//! - `session_status` / `session_reset` / `session_clean`: CLI-parity session lifecycle APIs
//! - `snapshot_save` / `snapshot_load` / `snapshot_list` / `snapshot_delete`: Snapshot lifecycle APIs
//! - `ptb_universe`: Run checkpoint-source PTB universe generation/execution
//! - `discover_checkpoint_targets`: Discover digest/package Move-call targets from checkpoints
//! - `fetch_object_bcs`: Fetch object BCS (optionally at historical version) via gRPC
//! - `fetch_historical_package_bytecodes`: Fetch checkpoint-pinned package bytecodes via gRPC
//! - `fetch_package_bytecodes`: Fetch package bytecodes via GraphQL
//! - `context_prepare` / `prepare_package_context`: Fetch package closure for two-step replay flows
//! - `context_run` / `adapter_run` / `protocol_run`: First-class replay orchestration wrappers
//! - `context_discover` / `adapter_discover` / `protocol_discover`: Replay target discovery helpers
//! - `pipeline_validate` / `workflow_validate`: Validate typed pipeline/workflow specs
//! - `pipeline_init` / `workflow_init`: Generate typed pipeline/workflow specs
//! - `pipeline_auto` / `workflow_auto`: Auto-generate package-first draft adapters
//! - `pipeline_run` / `workflow_run`: Execute typed specs natively from Python
//! - `pipeline_run_inline` / `workflow_run_inline`: Execute typed specs from in-memory Python objects
//! - `OrchestrationSession`: In-memory prepared context + replay helper for interactive workflows
//! - `json_to_bcs`: Convert Sui object JSON to BCS bytes
//! - `transaction_json_to_bcs`: Convert Snowflake/canonical TransactionData JSON to BCS bytes
//! - `call_view_function`: Execute a Move view function in the local VM
//! - `historical_view_from_versions`: Generic historical view execution from versions snapshots
//! - `historical_decode_returns_typed`: Decode historical command return values by type tags
//! - `historical_decode_with_schema`: Decode historical command return values via named schema
//! - `fuzz_function`: Fuzz a Move function with random inputs
//! - `replay`: Replay historical transactions (with optional analysis-only mode)
//! - `replay_transaction`: Opinionated replay helper with compact signature
//! - `analyze_replay` / `replay_analyze`: Replay hydration/readiness analysis
//! - `replay_effects`: Replay execution summary with effects-focused output
//! - `classify_replay_result`: Structured replay failure classification and hints
//! - `dynamic_field_diagnostics`: Compare hydration with/without DF prefetch and report gaps
//! - `import_state`: Import replay data files into local cache
//! - `deserialize_transaction`: Decode raw transaction BCS
//! - `deserialize_package`: Decode raw package BCS

#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict};

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
use sui_sandbox_core::orchestrator::{ReplayOrchestrator, ReturnDecodeField};
use sui_sandbox_core::ptb_universe::{
    run_with_args as core_run_ptb_universe, Args as CorePtbUniverseArgs,
    CheckpointSource as CoreCheckpointSource, DEFAULT_LATEST as CORE_PTB_UNIVERSE_DEFAULT_LATEST,
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
use sui_transport::grpc::{historical_endpoint_and_api_key_from_env, GrpcClient, GrpcOwner};
use sui_transport::network::resolve_graphql_endpoint;
use sui_transport::walrus::WalrusClient;

mod replay_api;
mod replay_core;
mod replay_output;
mod session_api;
mod transport_helpers;
mod workflow_api;
mod workflow_native;
use replay_api::*;
use replay_core::*;
use replay_output::{
    build_analyze_replay_output, build_replay_output, classify_replay_output,
    deserialize_package_inner, deserialize_transaction_inner, import_state_inner,
    load_replay_state_from_file,
};
use session_api::*;
use transport_helpers::*;
use workflow_api::*;
use workflow_native::*;

const PTB_UNIVERSE_DEFAULT_OUT_DIR: &str = "examples/out/walrus_ptb_universe";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_py_err(e: anyhow::Error) -> PyErr {
    PyRuntimeError::new_err(format!("{:#}", e))
}

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

/// Convert a serde_json::Value to a Python object via JSON round-trip.
fn json_value_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<PyObject> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| PyRuntimeError::new_err(format!("JSON serialization failed: {}", e)))?;
    let json_mod = py.import("json")?;
    let result = json_mod.call_method1("loads", (json_str,))?;
    Ok(result.into())
}

/// Parse package module bytecodes from Python input.
///
/// Accepts either:
/// - `List[bytes]`
/// - `List[str]` where each string is base64-encoded module bytecode
fn decode_package_module_bytes(value: &Bound<'_, pyo3::PyAny>) -> PyResult<Vec<Vec<u8>>> {
    if let Ok(raw) = value.extract::<Vec<Vec<u8>>>() {
        return Ok(raw);
    }
    if let Ok(encoded) = value.extract::<Vec<String>>() {
        let mut out = Vec::with_capacity(encoded.len());
        for module_b64 in encoded {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(module_b64.as_bytes())
                .map_err(|e| {
                    PyRuntimeError::new_err(format!("invalid base64 package module bytecode: {e}"))
                })?;
            out.push(bytes);
        }
        return Ok(out);
    }
    Err(PyRuntimeError::new_err(
        "package module list must be List[bytes] or List[str] (base64)",
    ))
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

fn get_latest_checkpoint_inner() -> Result<u64> {
    WalrusClient::mainnet().get_latest_checkpoint()
}

// ---------------------------------------------------------------------------
// get_checkpoint (native — Walrus)
// ---------------------------------------------------------------------------

fn get_checkpoint_inner(checkpoint_num: u64) -> Result<serde_json::Value> {
    use sui_transport::walrus;
    use sui_types::transaction::TransactionDataAPI;

    let client = WalrusClient::mainnet();
    let checkpoint_data = client.get_checkpoint(checkpoint_num)?;

    let epoch = checkpoint_data.checkpoint_summary.epoch;
    let timestamp_ms = checkpoint_data.checkpoint_summary.timestamp_ms;

    let mut transactions = Vec::new();
    for tx in &checkpoint_data.transactions {
        let digest = tx.transaction.digest().to_string();
        let tx_data = tx.transaction.data().transaction_data();
        let sender = format!("{}", tx_data.sender());

        let command_count = match tx_data.kind() {
            sui_types::transaction::TransactionKind::ProgrammableTransaction(ptb) => {
                ptb.commands.len()
            }
            _ => 0,
        };

        transactions.push(serde_json::json!({
            "digest": digest,
            "sender": sender,
            "commands": command_count,
            "input_objects": tx.input_objects.len(),
            "output_objects": tx.output_objects.len(),
        }));
    }

    let versions = walrus::extract_object_versions_from_checkpoint(&checkpoint_data);

    Ok(serde_json::json!({
        "checkpoint": checkpoint_num,
        "epoch": epoch,
        "timestamp_ms": timestamp_ms,
        "transaction_count": checkpoint_data.transactions.len(),
        "transactions": transactions,
        "object_versions_count": versions.len(),
    }))
}

#[derive(Debug, Clone, Copy)]
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

    fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

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

// ---------------------------------------------------------------------------
// json_to_bcs (native)
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

// ---------------------------------------------------------------------------
// call_view_function (native)
// ---------------------------------------------------------------------------

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

    // 1. Build LocalModuleResolver with sui framework
    let mut resolver = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;

    // 2. Load provided package bytecodes
    let mut loaded_packages = HashSet::new();
    loaded_packages.insert(AccountAddress::from_hex_literal("0x1").unwrap());
    loaded_packages.insert(AccountAddress::from_hex_literal("0x2").unwrap());
    loaded_packages.insert(AccountAddress::from_hex_literal("0x3").unwrap());

    // If both original and upgraded storage packages are present, skip loading
    // the original package bytes so upgraded bytecode wins deterministically.
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

    // Apply explicit package metadata from historical fetchers:
    // - aliases: storage -> runtime (bytecode) IDs
    // - linkage_upgrades: runtime -> storage upgrades
    // - package_runtime_ids + package_linkage: per-package linkage tables
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

    // 5. Set up child fetcher:
    //    - static preloaded children (if provided)
    //    - optional on-demand gRPC fetch for missing child objects
    if !child_objects.is_empty() || fetch_child_objects {
        let mut child_map: HashMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)> =
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
            if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
                eprintln!(
                    "[py_child_fetcher] init endpoint={} api_key_present={}",
                    resolved_endpoint,
                    resolved_api_key.is_some()
                );
            }
            Some(Arc::new((resolved_endpoint, resolved_api_key)))
        } else {
            None
        };

        let child_map = Arc::new(child_map);
        let historical_versions_for_fetcher = Arc::new(historical_versions.clone());
        let fetcher: sui_sandbox_core::sandbox_runtime::ChildFetcherFn =
            Box::new(move |parent, child| {
                let debug_child_fetch =
                    std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1");
                if let Some(found) = child_map.get(&(parent, child)).cloned() {
                    if debug_child_fetch {
                        eprintln!(
                            "[py_child_fetcher] HIT static parent={} child={}",
                            parent.to_hex_literal(),
                            child.to_hex_literal()
                        );
                    }
                    return Some(found);
                }

                let grpc_cfg = grpc_child_config.as_ref()?;
                let child_id_str = child.to_hex_literal();
                let historical_version =
                    historical_versions_for_fetcher.get(&child_id_str).copied();
                if debug_child_fetch {
                    eprintln!(
                        "[py_child_fetcher] FETCH parent={} child={} version_hint={:?}",
                        parent.to_hex_literal(),
                        child_id_str,
                        historical_version
                    );
                }

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
                if fetched.is_none() {
                    if debug_child_fetch {
                        eprintln!(
                            "[py_child_fetcher] MISS grpc child={} version_hint={:?}",
                            child_id_str, historical_version
                        );
                    }
                    return None;
                }
                let object = fetched?;
                if debug_child_fetch && (object.type_string.is_none() || object.bcs.is_none()) {
                    eprintln!(
                        "[py_child_fetcher] MISS payload child={} has_type={} has_bcs={}",
                        child_id_str,
                        object.type_string.is_some(),
                        object.bcs.is_some()
                    );
                }

                let type_tag_str = object.type_string?;
                let bcs = object.bcs?;
                let type_tag = sui_sandbox_core::types::parse_type_tag(&type_tag_str).ok()?;
                if debug_child_fetch {
                    eprintln!(
                        "[py_child_fetcher] HIT grpc child={} type={}",
                        child_id_str, type_tag_str
                    );
                }
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

// ---------------------------------------------------------------------------
// fetch_package_bytecodes (native — GraphQL)
// ---------------------------------------------------------------------------

fn fetch_historical_package_bytecodes_inner(
    package_ids: &[String],
    type_refs: &[String],
    checkpoint: Option<u64>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> Result<serde_json::Value> {
    let mut explicit_roots = Vec::new();
    for package_id in package_ids {
        let addr = AccountAddress::from_hex_literal(package_id)
            .with_context(|| format!("invalid package id: {}", package_id))?;
        if !explicit_roots.contains(&addr) {
            explicit_roots.push(addr);
        }
    }
    let package_roots: Vec<AccountAddress> =
        sui_sandbox_core::utilities::collect_required_package_roots_from_type_strings(
            &explicit_roots,
            type_refs,
        )?
        .into_iter()
        .collect();

    let (grpc_endpoint, grpc_api_key) = resolve_grpc_endpoint_and_key(endpoint, api_key);
    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
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
    let mut aliases = serde_json::Map::new(); // storage -> runtime
    let mut linkage_upgrades = serde_json::Map::new(); // runtime -> storage
    let mut package_runtime_ids = serde_json::Map::new(); // storage -> runtime
    let mut package_linkage = serde_json::Map::new(); // storage -> {runtime_dep -> storage_dep}
    let mut package_versions = serde_json::Map::new(); // storage -> version

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

        let mut linkage_map = serde_json::Map::new();
        for (dep_runtime, dep_storage) in &pkg.linkage {
            let dep_runtime_id = dep_runtime.to_hex_literal();
            let dep_storage_id = dep_storage.to_hex_literal();
            linkage_map.insert(
                dep_runtime_id.clone(),
                serde_json::json!(dep_storage_id.clone()),
            );
            if dep_runtime_id != dep_storage_id {
                linkage_upgrades.insert(dep_runtime_id, serde_json::json!(dep_storage_id));
            }
        }
        package_linkage.insert(storage_id, serde_json::Value::Object(linkage_map));
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

// ---------------------------------------------------------------------------
// fuzz_function (native)
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

    // 1. Build resolver and fetch deps
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

    // 2. Get compiled module and function signature
    let compiled_module = resolver
        .get_module_by_addr_name(&target_addr, module)
        .ok_or_else(|| anyhow!("Module '{}::{}' not found", package_id, module))?;

    let sig = resolver
        .get_function_signature(&target_addr, module, function)
        .ok_or_else(|| {
            anyhow!(
                "Function '{}::{}::{}' not found",
                package_id,
                module,
                function
            )
        })?;

    // 3. Classify parameters
    let classification = classify_params(compiled_module, &sig.parameter_types);

    let target = format!("{}::{}::{}", package_id, module, function);

    // 4. If dry_run, return classification only
    if dry_run {
        return Ok(serde_json::json!({
            "target": target,
            "classification": classification,
            "verdict": if classification.is_fully_fuzzable { "FULLY_FUZZABLE" } else { "NOT_FUZZABLE" },
        }));
    }

    // 5. Check fuzzability
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

    // 6. Parse type args and build config
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

    // 7. Run fuzzer
    let runner = FuzzRunner::new(&resolver);
    let report = runner.run(target_addr, module, function, &classification, &config)?;

    serde_json::to_value(&report).map_err(|e| anyhow!("Failed to serialize fuzz report: {}", e))
}

// ---------------------------------------------------------------------------
// Python module functions
// ---------------------------------------------------------------------------

/// Get the latest archived checkpoint number from Walrus.
///
/// No API keys or authentication required. Standalone — no CLI binary needed.
#[pyfunction]
fn get_latest_checkpoint() -> PyResult<u64> {
    get_latest_checkpoint_inner().map_err(to_py_err)
}

/// Fetch a checkpoint from Walrus and return a summary dict.
///
/// Returns: checkpoint, epoch, timestamp_ms, transaction_count,
/// transactions (list of {digest, sender, commands, input_objects, output_objects}),
/// and object_versions_count.
///
/// No API keys or authentication required. Standalone — no CLI binary needed.
#[pyfunction]
fn get_checkpoint(py: Python<'_>, checkpoint: u64) -> PyResult<PyObject> {
    // Release GIL during Walrus fetch
    let value = py
        .allow_threads(move || get_checkpoint_inner(checkpoint))
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Build and execute a checkpoint-source PTB universe run via core engine.
///
/// This is the same reusable engine used by the Rust example wrapper
/// (`examples/walrus_ptb_universe.rs`), exposed as a first-class Python API.
#[pyfunction]
#[pyo3(signature = (
    *,
    source="walrus",
    latest=CORE_PTB_UNIVERSE_DEFAULT_LATEST,
    top_packages=CORE_PTB_UNIVERSE_DEFAULT_TOP_PACKAGES,
    max_ptbs=CORE_PTB_UNIVERSE_DEFAULT_MAX_PTBS,
    out_dir=None,
    grpc_endpoint=None,
    stream_timeout_secs=CORE_PTB_UNIVERSE_DEFAULT_STREAM_TIMEOUT_SECS,
))]
fn ptb_universe(
    py: Python<'_>,
    source: &str,
    latest: u64,
    top_packages: usize,
    max_ptbs: usize,
    out_dir: Option<&str>,
    grpc_endpoint: Option<&str>,
    stream_timeout_secs: u64,
) -> PyResult<PyObject> {
    let source_parsed = CoreCheckpointSource::parse(source).map_err(to_py_err)?;
    let out_dir_path = PathBuf::from(
        out_dir
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(PTB_UNIVERSE_DEFAULT_OUT_DIR),
    );
    let grpc_endpoint_owned = grpc_endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let args = CorePtbUniverseArgs {
        source: source_parsed,
        latest,
        top_packages,
        max_ptbs,
        out_dir: out_dir_path.clone(),
        grpc_endpoint: grpc_endpoint_owned.clone(),
        stream_timeout_secs,
    };

    py.allow_threads(move || core_run_ptb_universe(args))
        .map_err(to_py_err)?;

    let value = serde_json::json!({
        "success": true,
        "source": source_parsed.as_str(),
        "latest": latest,
        "top_packages": top_packages,
        "max_ptbs": max_ptbs,
        "grpc_endpoint": grpc_endpoint_owned,
        "stream_timeout_secs": stream_timeout_secs,
        "out_dir": out_dir_path.display().to_string(),
        "artifacts": {
            "summary": out_dir_path.join("universe_summary.json").display().to_string(),
            "package_downloads": out_dir_path.join("package_downloads.json").display().to_string(),
            "function_candidates": out_dir_path.join("function_candidates.json").display().to_string(),
            "ptb_execution_results": out_dir_path.join("ptb_execution_results.json").display().to_string(),
            "readme": out_dir_path.join("README.md").display().to_string(),
        }
    });
    json_value_to_py(py, &value)
}

/// Discover replay candidates from checkpoint Move calls.
///
/// Returns digests + package/module/function call summaries for programmable
/// transactions across one or more checkpoints.
///
/// By default this uses Walrus mainnet. Set `walrus_network="testnet"` or
/// pass both `walrus_caching_url` and `walrus_aggregator_url` for custom
/// archive endpoints.
#[pyfunction]
#[pyo3(signature = (
    *,
    checkpoint=None,
    latest=None,
    package_id=None,
    include_framework=false,
    limit=200,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
fn discover_checkpoint_targets(
    py: Python<'_>,
    checkpoint: Option<&str>,
    latest: Option<u64>,
    package_id: Option<&str>,
    include_framework: bool,
    limit: usize,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    let checkpoint_owned = checkpoint.map(ToOwned::to_owned);
    let package_id_owned = package_id.map(ToOwned::to_owned);
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_url_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_url_owned = walrus_aggregator_url.map(ToOwned::to_owned);
    let value = py
        .allow_threads(move || {
            discover_checkpoint_targets_inner(
                checkpoint_owned.as_deref(),
                latest,
                package_id_owned.as_deref(),
                include_framework,
                limit,
                &walrus_network_owned,
                walrus_caching_url_owned.as_deref(),
                walrus_aggregator_url_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Protocol-first replay-target discovery from checkpoints.
///
/// Non-generic protocols require `package_id` so package selection stays explicit.
#[pyfunction]
#[pyo3(signature = (
    *,
    protocol="generic",
    package_id=None,
    checkpoint=None,
    latest=None,
    include_framework=false,
    limit=200,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
fn protocol_discover(
    py: Python<'_>,
    protocol: &str,
    package_id: Option<&str>,
    checkpoint: Option<&str>,
    latest: Option<u64>,
    include_framework: bool,
    limit: usize,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    let protocol_owned = protocol.to_string();
    let package_id_owned = package_id.map(ToOwned::to_owned);
    let checkpoint_owned = checkpoint.map(ToOwned::to_owned);
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_url_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_url_owned = walrus_aggregator_url.map(ToOwned::to_owned);
    let value = py
        .allow_threads(move || {
            let filter = resolve_protocol_discovery_package_filter(
                &protocol_owned,
                package_id_owned.as_deref(),
            )?;
            discover_checkpoint_targets_inner(
                checkpoint_owned.as_deref(),
                latest,
                filter.as_deref(),
                include_framework,
                limit,
                &walrus_network_owned,
                walrus_caching_url_owned.as_deref(),
                walrus_aggregator_url_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Canonical alias for `discover_checkpoint_targets`.
#[pyfunction]
#[pyo3(signature = (
    *,
    checkpoint=None,
    latest=None,
    package_id=None,
    include_framework=false,
    limit=200,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
fn context_discover(
    py: Python<'_>,
    checkpoint: Option<&str>,
    latest: Option<u64>,
    package_id: Option<&str>,
    include_framework: bool,
    limit: usize,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    discover_checkpoint_targets(
        py,
        checkpoint,
        latest,
        package_id,
        include_framework,
        limit,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
}

/// Canonical alias for `protocol_discover`.
#[pyfunction]
#[pyo3(signature = (
    *,
    protocol="generic",
    package_id=None,
    checkpoint=None,
    latest=None,
    include_framework=false,
    limit=200,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
fn adapter_discover(
    py: Python<'_>,
    protocol: &str,
    package_id: Option<&str>,
    checkpoint: Option<&str>,
    latest: Option<u64>,
    include_framework: bool,
    limit: usize,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    protocol_discover(
        py,
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
}

/// Fetch object BCS via gRPC, optionally pinned to a historical version.
///
/// Useful for constructing deterministic `call_view_function` object inputs.
#[pyfunction]
#[pyo3(signature = (
    object_id,
    *,
    version=None,
    endpoint=None,
    api_key=None,
))]
fn fetch_object_bcs(
    py: Python<'_>,
    object_id: &str,
    version: Option<u64>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> PyResult<PyObject> {
    let object_id_owned = object_id.to_string();
    let endpoint_owned = endpoint.map(|s| s.to_string());
    let api_key_owned = api_key.map(|s| s.to_string());
    let value = py
        .allow_threads(move || {
            fetch_object_bcs_inner(
                &object_id_owned,
                version,
                endpoint_owned.as_deref(),
                api_key_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Extract the full interface JSON for a Sui Move package.
///
/// Returns the complete interface with all modules, structs, functions,
/// type parameters, abilities, fields, etc.
///
/// Provide either `package_id` (fetched via GraphQL) or `bytecode_dir`
/// (local directory with `bytecode_modules/*.mv`), but not both.
///
/// Standalone — no CLI binary needed.
#[pyfunction]
#[pyo3(signature = (*, package_id=None, bytecode_dir=None, rpc_url="https://fullnode.mainnet.sui.io:443"))]
fn extract_interface(
    py: Python<'_>,
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
) -> PyResult<PyObject> {
    let pkg_id_owned = package_id.map(|s| s.to_string());
    let bytecode_dir_owned = bytecode_dir.map(|s| s.to_string());
    let rpc_url_owned = rpc_url.to_string();
    let value = py
        .allow_threads(move || {
            extract_interface_inner(
                pkg_id_owned.as_deref(),
                bytecode_dir_owned.as_deref(),
                &rpc_url_owned,
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Replay a historical Sui transaction locally with the Move VM.
///
/// Standalone — no CLI binary needed. All data is fetched directly.
///
/// By default, executes the transaction in the local Move VM and returns
/// execution results. Use `analyze_only=True` to inspect state hydration
/// without executing.
///
/// When `checkpoint` is provided, uses Walrus as data source (no API key needed).
/// Otherwise uses gRPC/hybrid (requires `SUI_GRPC_API_KEY` env var).
///
/// Args:
///     digest: Transaction digest to replay
///     rpc_url: Sui RPC endpoint
///     source: Data source — "hybrid", "grpc", or "walrus"
///     checkpoint: Walrus checkpoint number (auto-uses walrus, no API key needed)
///     allow_fallback: Allow fallback to secondary data sources
///     context_path: Optional prepared package context JSON from prepare_package_context(...)
///     profile: Runtime defaults profile ("safe"|"balanced"|"fast")
///     fetch_strategy: Dynamic-field fetch strategy ("eager"|"full")
///     vm_only: Disable fallback paths and force VM-only behavior
///     prefetch_depth: Dynamic field prefetch depth
///     prefetch_limit: Dynamic field prefetch limit per parent
///     auto_system_objects: Auto-inject Clock/Random when missing
///     no_prefetch: Disable dynamic field prefetch
///     compare: Compare local execution with on-chain effects
///     analyze_only: Skip VM execution, just inspect state hydration
///     synthesize_missing: Retry with synthetic object bytes when inputs are missing
///     self_heal_dynamic_fields: Enable dynamic field child fetchers during VM execution
///     analyze_mm2: Build MM2 type-model diagnostics (analyze-only mode)
///     verbose: Enable verbose logging to stderr
///
/// Returns: dict replay envelope. In `analyze_only=True` mode, `analysis` contains
/// the hydration summary (with compatibility mirror fields also exposed at top level).
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    source="hybrid",
    checkpoint=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    analyze_mm2=false,
    verbose=false,
))]
fn replay(
    py: Python<'_>,
    digest: Option<&str>,
    rpc_url: &str,
    source: &str,
    checkpoint: Option<u64>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let digest_owned = digest.map(|s| s.to_string());
    let rpc_url_owned = rpc_url.to_string();
    let source_owned = source.to_string();
    let state_file_owned = state_file.map(PathBuf::from);
    let context_path_owned = context_path.map(PathBuf::from);
    let cache_dir_owned = cache_dir.map(PathBuf::from);
    let profile_owned = profile.map(ToOwned::to_owned);
    let fetch_strategy_owned = fetch_strategy.map(ToOwned::to_owned);
    let value = py
        .allow_threads(move || {
            let profile = parse_replay_profile(profile_owned.as_deref())?;
            let _profile_env = workflow_apply_profile_env(profile);
            let fetch_strategy = parse_replay_fetch_strategy(fetch_strategy_owned.as_deref())?;
            let allow_fallback = if vm_only { false } else { allow_fallback };
            let no_prefetch = no_prefetch || fetch_strategy == WorkflowFetchStrategy::Eager;

            let digest = digest_owned.as_deref();
            let source_is_local = source_owned.eq_ignore_ascii_case("local");
            let use_local_cache = source_is_local || cache_dir_owned.is_some();
            let context_packages = if let Some(path) = context_path_owned.as_ref() {
                Some(load_context_packages_from_file(path)?)
            } else {
                None
            };

            if state_file_owned.is_some() && use_local_cache {
                return Err(anyhow!(
                    "state_file cannot be combined with cache_dir/source='local'"
                ));
            }

            if let Some(state_path) = state_file_owned.as_ref() {
                let replay_state = load_replay_state_from_file(state_path, digest)?;
                return replay_loaded_state_inner(
                    replay_state,
                    "state_file",
                    "state_json",
                    context_packages.as_ref(),
                    allow_fallback,
                    auto_system_objects,
                    self_heal_dynamic_fields,
                    vm_only,
                    compare,
                    analyze_only,
                    synthesize_missing,
                    analyze_mm2,
                    &rpc_url_owned,
                    verbose,
                );
            }

            if use_local_cache {
                let digest = digest.ok_or_else(|| {
                    anyhow!("digest is required when replaying from cache_dir/source='local'")
                })?;
                let cache_dir = cache_dir_owned
                    .clone()
                    .unwrap_or_else(default_local_cache_dir);
                let provider = FileStateProvider::new(&cache_dir).with_context(|| {
                    format!("Failed to open local replay cache {}", cache_dir.display())
                })?;
                let replay_state = provider.get_state(digest)?;
                return replay_loaded_state_inner(
                    replay_state,
                    &source_owned,
                    "local_cache",
                    context_packages.as_ref(),
                    allow_fallback,
                    auto_system_objects,
                    self_heal_dynamic_fields,
                    vm_only,
                    compare,
                    analyze_only,
                    synthesize_missing,
                    analyze_mm2,
                    &rpc_url_owned,
                    verbose,
                );
            }

            let digest = digest.ok_or_else(|| anyhow!("digest is required"))?;
            replay_inner(
                digest,
                &rpc_url_owned,
                &source_owned,
                checkpoint,
                context_packages.as_ref(),
                allow_fallback,
                prefetch_depth,
                prefetch_limit,
                auto_system_objects,
                no_prefetch,
                synthesize_missing,
                self_heal_dynamic_fields,
                vm_only,
                compare,
                analyze_only,
                analyze_mm2,
                verbose,
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Import replay data files into a local replay cache directory.
#[pyfunction]
#[pyo3(signature = (
    *,
    state=None,
    transactions=None,
    objects=None,
    packages=None,
    cache_dir=None,
))]
fn import_state(
    py: Python<'_>,
    state: Option<&str>,
    transactions: Option<&str>,
    objects: Option<&str>,
    packages: Option<&str>,
    cache_dir: Option<&str>,
) -> PyResult<PyObject> {
    let state_owned = state.map(|s| s.to_string());
    let transactions_owned = transactions.map(|s| s.to_string());
    let objects_owned = objects.map(|s| s.to_string());
    let packages_owned = packages.map(|s| s.to_string());
    let cache_owned = cache_dir.map(|s| s.to_string());
    let value = py
        .allow_threads(move || {
            import_state_inner(
                state_owned.as_deref(),
                transactions_owned.as_deref(),
                objects_owned.as_deref(),
                packages_owned.as_deref(),
                cache_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Deserialize transaction BCS bytes into structured replay transaction JSON.
#[pyfunction]
fn deserialize_transaction(py: Python<'_>, raw_bcs: Vec<u8>) -> PyResult<PyObject> {
    let value = py
        .allow_threads(move || deserialize_transaction_inner(&raw_bcs))
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Deserialize package BCS bytes into structured package JSON.
#[pyfunction]
fn deserialize_package(py: Python<'_>, bcs: Vec<u8>) -> PyResult<PyObject> {
    let value = py
        .allow_threads(move || deserialize_package_inner(&bcs))
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Convert Sui object JSON to BCS bytes using struct layouts from bytecode.
///
/// Standalone — no CLI binary needed.
///
/// Args:
///     type_str: Full Sui type string (e.g., "0x2::coin::Coin<0x2::sui::SUI>")
///     object_json: JSON string of the decoded object data
///     package_bytecodes: List of raw bytecode bytes for all needed package modules
///
/// Returns: BCS-encoded bytes
#[pyfunction]
#[pyo3(signature = (type_str, object_json, package_bytecodes))]
fn json_to_bcs<'py>(
    py: Python<'py>,
    type_str: &str,
    object_json: &str,
    package_bytecodes: Vec<Vec<u8>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let type_str_owned = type_str.to_string();
    let object_json_owned = object_json.to_string();
    let bcs_bytes = py
        .allow_threads(move || {
            json_to_bcs_inner(&type_str_owned, &object_json_owned, package_bytecodes)
        })
        .map_err(to_py_err)?;
    Ok(PyBytes::new(py, &bcs_bytes))
}

/// Convert Snowflake TRANSACTION_JSON (Sui TransactionData JSON) into raw transaction BCS bytes.
///
/// Accepts canonical Sui `TransactionData` JSON and Snowflake-style variants
/// (for example StructTag `type_args` and non-`0x` hex addresses).
#[pyfunction]
#[pyo3(signature = (transaction_json))]
fn transaction_json_to_bcs<'py>(
    py: Python<'py>,
    transaction_json: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let tx_json_owned = transaction_json.to_string();
    let bcs_bytes = py
        .allow_threads(move || transaction_json_to_bcs_inner(&tx_json_owned))
        .map_err(to_py_err)?;
    Ok(PyBytes::new(py, &bcs_bytes))
}

/// Execute a view function via local Move VM.
///
/// Standalone — no CLI binary needed.
///
/// Args:
///     package_id: Package containing the view function
///     module: Module name
///     function: Function name
///     type_args: List of type argument strings (e.g., ["0x2::sui::SUI"])
///     object_inputs: List of dicts with keys: object_id, bcs_bytes, type_tag
///         optional: is_shared/mutable, or legacy owner ("immutable"|"shared"|"address_owned")
///     pure_inputs: List of BCS-encoded pure argument bytes
///     child_objects: Dict mapping parent_id -> list of {child_id, bcs_bytes, type_tag}
///     historical_versions: Optional object_id -> version map for on-demand child fetches
///     fetch_child_objects: If True, fetch child objects on-demand via gRPC
///     grpc_endpoint: Optional gRPC endpoint override for child fetches
///     grpc_api_key: Optional gRPC API key override for child fetches
///     package_bytecodes: Either:
///         - Dict[package_id -> list[module_bytes or module_base64]]
///         - Full payload returned by fetch_historical_package_bytecodes(...)
///     fetch_deps: If True, automatically resolve transitive deps via GraphQL
///
/// Returns: Dict with success, error, return_values, return_type_tags, gas_used
#[pyfunction]
#[pyo3(signature = (
    package_id,
    module,
    function,
    *,
    type_args=vec![],
    object_inputs=vec![],
    pure_inputs=vec![],
    child_objects=None,
    historical_versions=None,
    fetch_child_objects=false,
    grpc_endpoint=None,
    grpc_api_key=None,
    package_bytecodes=None,
    fetch_deps=true,
))]
fn call_view_function(
    py: Python<'_>,
    package_id: &str,
    module: &str,
    function: &str,
    type_args: Vec<String>,
    object_inputs: Vec<Bound<'_, PyDict>>,
    pure_inputs: Vec<Vec<u8>>,
    child_objects: Option<Bound<'_, PyDict>>,
    historical_versions: Option<Bound<'_, PyDict>>,
    fetch_child_objects: bool,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
    package_bytecodes: Option<Bound<'_, PyDict>>,
    fetch_deps: bool,
) -> PyResult<PyObject> {
    // Parse object_inputs from Python dicts
    let mut parsed_obj_inputs: Vec<(String, Vec<u8>, String, bool, bool)> = Vec::new();
    for dict in &object_inputs {
        let obj_id: String = dict
            .get_item("object_id")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'object_id' in object_inputs"))?
            .extract()?;
        let bcs_bytes: Vec<u8> = dict
            .get_item("bcs_bytes")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'bcs_bytes' in object_inputs"))?
            .extract()?;
        let type_tag: String = dict
            .get_item("type_tag")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'type_tag' in object_inputs"))?
            .extract()?;

        let explicit_is_shared = dict.get_item("is_shared")?;
        let explicit_mutable = dict.get_item("mutable")?;
        let owner = dict
            .get_item("owner")?
            .map(|v| v.extract::<String>())
            .transpose()?;

        let mut is_shared: bool = explicit_is_shared
            .as_ref()
            .map(|v| v.extract().unwrap_or(false))
            .unwrap_or(false);
        let mut mutable: bool = explicit_mutable
            .as_ref()
            .map(|v| v.extract().unwrap_or(false))
            .unwrap_or(false);

        // Backward-compatible alias used in earlier examples:
        // owner = "immutable" | "shared" | "address_owned"
        if explicit_is_shared.is_none() {
            if let Some(owner) = owner {
                match owner.trim().to_ascii_lowercase().as_str() {
                    "shared" => {
                        is_shared = true;
                        if explicit_mutable.is_none() {
                            // Shared objects are typically mutable unless explicitly overridden.
                            mutable = true;
                        }
                    }
                    "immutable" | "address_owned" => {
                        is_shared = false;
                    }
                    other => {
                        return Err(PyRuntimeError::new_err(format!(
                            "invalid 'owner' in object_inputs: {other} (expected immutable|shared|address_owned)"
                        )));
                    }
                }
            }
        }
        parsed_obj_inputs.push((obj_id, bcs_bytes, type_tag, is_shared, mutable));
    }

    // Parse child_objects from Python dict
    let mut parsed_children: HashMap<String, Vec<(String, Vec<u8>, String)>> = HashMap::new();
    if let Some(ref co) = child_objects {
        for (key, value) in co.iter() {
            let parent_id: String = key.extract()?;
            let children_list: Vec<Bound<'_, PyDict>> = value.extract()?;
            let mut children = Vec::new();
            for child_dict in &children_list {
                let child_id: String = child_dict
                    .get_item("child_id")?
                    .ok_or_else(|| PyRuntimeError::new_err("missing 'child_id'"))?
                    .extract()?;
                let bcs: Vec<u8> = child_dict
                    .get_item("bcs_bytes")?
                    .ok_or_else(|| PyRuntimeError::new_err("missing 'bcs_bytes'"))?
                    .extract()?;
                let tt: String = child_dict
                    .get_item("type_tag")?
                    .ok_or_else(|| PyRuntimeError::new_err("missing 'type_tag'"))?
                    .extract()?;
                children.push((child_id, bcs, tt));
            }
            parsed_children.insert(parent_id, children);
        }
    }

    // Parse historical_versions map from Python dict
    let mut parsed_historical_versions: HashMap<String, u64> = HashMap::new();
    if let Some(ref hv) = historical_versions {
        for (key, value) in hv.iter() {
            let object_id: String = key.extract()?;
            let version: u64 = value.extract()?;
            parsed_historical_versions.insert(object_id, version);
        }
    }

    // Parse package_bytecodes from Python dict
    let mut parsed_pkg_bytes: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    let mut parsed_package_aliases: HashMap<String, String> = HashMap::new();
    let mut parsed_linkage_upgrades: HashMap<String, String> = HashMap::new();
    let mut parsed_package_runtime_ids: HashMap<String, String> = HashMap::new();
    let mut parsed_package_linkage: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut parsed_package_versions: HashMap<String, u64> = HashMap::new();
    let mut historical_payload_mode = false;
    if let Some(ref pb) = package_bytecodes {
        let packages_dict: Bound<'_, PyDict> =
            if let Some(packages_any) = pb.get_item("packages")? {
                historical_payload_mode = true;
                packages_any.extract()?
            } else {
                pb.clone()
            };

        for (key, value) in packages_dict.iter() {
            let pkg_id: String = key.extract()?;
            let bytecodes = decode_package_module_bytes(&value)?;
            parsed_pkg_bytes.insert(pkg_id, bytecodes);
        }

        if let Some(aliases_any) = pb.get_item("aliases")? {
            parsed_package_aliases = aliases_any.extract().map_err(|_| {
                PyRuntimeError::new_err(
                    "package_bytecodes.aliases must be Dict[str, str] (storage -> runtime)",
                )
            })?;
        }
        if let Some(linkage_any) = pb.get_item("linkage_upgrades")? {
            parsed_linkage_upgrades = linkage_any.extract().map_err(|_| {
                PyRuntimeError::new_err(
                    "package_bytecodes.linkage_upgrades must be Dict[str, str] (runtime -> storage)",
                )
            })?;
        }
        if let Some(runtime_ids_any) = pb.get_item("package_runtime_ids")? {
            parsed_package_runtime_ids = runtime_ids_any.extract().map_err(|_| {
                PyRuntimeError::new_err(
                    "package_bytecodes.package_runtime_ids must be Dict[str, str] (storage -> runtime)",
                )
            })?;
        }
        if let Some(pkg_linkage_any) = pb.get_item("package_linkage")? {
            parsed_package_linkage = pkg_linkage_any.extract().map_err(|_| {
                PyRuntimeError::new_err(
                    "package_bytecodes.package_linkage must be Dict[str, Dict[str, str]]",
                )
            })?;
        }
        if let Some(pkg_versions_any) = pb.get_item("package_versions")? {
            parsed_package_versions = pkg_versions_any.extract().map_err(|_| {
                PyRuntimeError::new_err("package_bytecodes.package_versions must be Dict[str, int]")
            })?;
        }
    }

    // Release GIL during VM execution
    let pkg_id_owned = package_id.to_string();
    let module_owned = module.to_string();
    let function_owned = function.to_string();
    let grpc_endpoint_owned = grpc_endpoint.map(|s| s.to_string());
    let grpc_api_key_owned = grpc_api_key.map(|s| s.to_string());
    let effective_fetch_deps = if historical_payload_mode {
        false
    } else {
        fetch_deps
    };
    let value = py
        .allow_threads(move || {
            call_view_function_inner(
                &pkg_id_owned,
                &module_owned,
                &function_owned,
                type_args,
                parsed_obj_inputs,
                pure_inputs,
                parsed_children,
                parsed_historical_versions,
                fetch_child_objects,
                grpc_endpoint_owned,
                grpc_api_key_owned,
                parsed_pkg_bytes,
                parsed_package_aliases,
                parsed_linkage_upgrades,
                parsed_package_runtime_ids,
                parsed_package_linkage,
                parsed_package_versions,
                effective_fetch_deps,
            )
        })
        .map_err(to_py_err)?;

    json_value_to_py(py, &value)
}

/// Execute a generic historical Move view function from a versions snapshot.
///
/// Protocol-specific logic (object selection, type args, decoding) should be
/// authored by callers on top of this generic primitive.
#[pyfunction]
#[pyo3(signature = (
    *,
    versions_file,
    package_id,
    module,
    function,
    required_objects,
    type_args=vec![],
    package_roots=vec![],
    type_refs=vec![],
    fetch_child_objects=true,
    grpc_endpoint=None,
    grpc_api_key=None,
))]
fn historical_view_from_versions(
    py: Python<'_>,
    versions_file: &str,
    package_id: &str,
    module: &str,
    function: &str,
    required_objects: Vec<String>,
    type_args: Vec<String>,
    package_roots: Vec<String>,
    type_refs: Vec<String>,
    fetch_child_objects: bool,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
) -> PyResult<PyObject> {
    let versions_file_owned = versions_file.to_string();
    let package_id_owned = package_id.to_string();
    let module_owned = module.to_string();
    let function_owned = function.to_string();
    let endpoint_owned = grpc_endpoint.map(ToOwned::to_owned);
    let api_key_owned = grpc_api_key.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let versions_path = PathBuf::from(&versions_file_owned);
            let request = CoreHistoricalViewRequest {
                package_id: package_id_owned,
                module: module_owned,
                function: function_owned,
                type_args,
                required_objects,
                package_roots,
                type_refs,
                fetch_child_objects,
            };
            let output = core_execute_historical_view_from_versions(
                &versions_path,
                &request,
                endpoint_owned.as_deref(),
                api_key_owned.as_deref(),
            )?;
            serde_json::to_value(output).context("Failed to serialize historical view output")
        })
        .map_err(to_py_err)?;

    json_value_to_py(py, &value)
}

fn py_json_value(py: Python<'_>, value: &Bound<'_, PyAny>) -> Result<serde_json::Value> {
    let json_mod = py
        .import("json")
        .context("failed to import python json module")?;
    let dumped_obj = json_mod
        .call_method1("dumps", (value,))
        .context("failed to serialize python object to JSON string")?;
    let dumped: String = dumped_obj
        .extract()
        .context("failed to extract serialized JSON string")?;
    serde_json::from_str(&dumped).context("invalid JSON payload")
}

/// Decode one u64 return value from `historical_view_from_versions` output.
///
/// Returns `None` when the execution failed or the return index is missing.
#[pyfunction]
#[pyo3(signature = (result, *, command_index=0, value_index))]
fn historical_decode_return_u64(
    py: Python<'_>,
    result: &Bound<'_, PyAny>,
    command_index: usize,
    value_index: usize,
) -> PyResult<Option<u64>> {
    let raw = py_json_value(py, result).map_err(to_py_err)?;
    ReplayOrchestrator::decode_command_return_u64(&raw, command_index, value_index)
        .map_err(to_py_err)
}

/// Decode all return values from a command into `u64` values where possible.
///
/// Returns `None` when the execution failed or command return values are missing.
#[pyfunction]
#[pyo3(signature = (result, *, command_index=0))]
fn historical_decode_return_u64s(
    py: Python<'_>,
    result: &Bound<'_, PyAny>,
    command_index: usize,
) -> PyResult<Option<Vec<Option<u64>>>> {
    let raw = py_json_value(py, result).map_err(to_py_err)?;
    let decoded = ReplayOrchestrator::decode_command_return_values(&raw, command_index)
        .map_err(to_py_err)?
        .map(|values| {
            values
                .into_iter()
                .map(|bytes| {
                    if bytes.len() < 8 {
                        return None;
                    }
                    let mut buf = [0u8; 8];
                    buf.copy_from_slice(&bytes[..8]);
                    Some(u64::from_le_bytes(buf))
                })
                .collect::<Vec<_>>()
        });
    Ok(decoded)
}

/// Decode all return values from a historical-view command into typed JSON values.
///
/// Returns `None` when execution failed or command return values are missing.
#[pyfunction]
#[pyo3(signature = (result, *, command_index=0))]
fn historical_decode_returns_typed(
    py: Python<'_>,
    result: &Bound<'_, PyAny>,
    command_index: usize,
) -> PyResult<Option<PyObject>> {
    let raw = py_json_value(py, result).map_err(to_py_err)?;
    let decoded = ReplayOrchestrator::decode_command_return_values_typed(&raw, command_index)
        .map_err(to_py_err)?;
    match decoded {
        Some(values) => {
            let value = serde_json::to_value(values).map_err(|e| {
                to_py_err(anyhow!(
                    "failed to serialize typed return decode output: {}",
                    e
                ))
            })?;
            json_value_to_py(py, &value).map(Some)
        }
        None => Ok(None),
    }
}

/// Decode historical-view return values into a named object with field schema.
///
/// `schema` must be a JSON-serializable list of:
/// `{ "index": int, "name": str, "type_hint": str|None, "scale": float|None }`.
#[pyfunction]
#[pyo3(signature = (result, schema, *, command_index=0))]
fn historical_decode_with_schema(
    py: Python<'_>,
    result: &Bound<'_, PyAny>,
    schema: &Bound<'_, PyAny>,
    command_index: usize,
) -> PyResult<Option<PyObject>> {
    let raw = py_json_value(py, result).map_err(to_py_err)?;
    let schema_json = py_json_value(py, schema).map_err(to_py_err)?;
    let fields: Vec<ReturnDecodeField> = serde_json::from_value(schema_json)
        .map_err(|e| to_py_err(anyhow!("invalid return decode schema: {}", e)))?;

    let decoded = ReplayOrchestrator::decode_command_return_schema(&raw, command_index, &fields)
        .map_err(to_py_err)?;
    match decoded {
        Some(map) => json_value_to_py(py, &serde_json::Value::Object(map)).map(Some),
        None => Ok(None),
    }
}

/// Fetch historical package bytecodes with transitive dependency resolution.
///
/// Standalone — no CLI binary needed.
///
/// Args:
///     package_ids: Root package IDs to fetch
///     type_refs: Optional type strings to infer additional package roots
///     checkpoint: Optional checkpoint to pin historical package versions
///     endpoint: Optional gRPC endpoint override
///     api_key: Optional gRPC API key override
///
/// Returns: Dict with packages (pkg_id -> [base64 module bytes]), count, endpoint_used
#[pyfunction]
#[pyo3(signature = (
    package_ids,
    *,
    type_refs=vec![],
    checkpoint=None,
    endpoint=None,
    api_key=None,
))]
fn fetch_historical_package_bytecodes(
    py: Python<'_>,
    package_ids: Vec<String>,
    type_refs: Vec<String>,
    checkpoint: Option<u64>,
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> PyResult<PyObject> {
    let endpoint_owned = endpoint.map(|s| s.to_string());
    let api_key_owned = api_key.map(|s| s.to_string());
    let value = py
        .allow_threads(move || {
            fetch_historical_package_bytecodes_inner(
                &package_ids,
                &type_refs,
                checkpoint,
                endpoint_owned.as_deref(),
                api_key_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Fetch package bytecodes via GraphQL, optionally resolving transitive dependencies.
///
/// Standalone — no CLI binary needed.
///
/// Args:
///     package_id: The package to fetch
///     resolve_deps: If True, recursively fetch all dependency packages
///
/// Returns: Dict with packages (pkg_id -> [base64 module bytes]) and count
#[pyfunction]
#[pyo3(signature = (package_id, *, resolve_deps=true))]
fn fetch_package_bytecodes(
    py: Python<'_>,
    package_id: &str,
    resolve_deps: bool,
) -> PyResult<PyObject> {
    let pkg_id_owned = package_id.to_string();
    let value = py
        .allow_threads(move || fetch_package_bytecodes_inner(&pkg_id_owned, resolve_deps))
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Prepare a generic package context by fetching package bytecodes (+deps by default).
///
/// This is step 1 of a simple two-step developer flow:
/// 1) `prepare_package_context(...)`
/// 2) `replay_transaction(...)`
///
/// Args:
///     package_id: Root package id (0x...)
///     resolve_deps: If True, fetch transitive dependency closure (default: True)
///     output_path: Optional JSON path to persist the context payload
///
/// Returns: Dict with `package_id`, `packages`, and `count`
#[pyfunction]
#[pyo3(signature = (package_id, *, resolve_deps=true, output_path=None))]
fn prepare_package_context(
    py: Python<'_>,
    package_id: &str,
    resolve_deps: bool,
    output_path: Option<&str>,
) -> PyResult<PyObject> {
    let package_id_owned = package_id.to_string();
    let output_path_owned = output_path.map(|s| s.to_string());
    let value = py
        .allow_threads(move || {
            prepare_package_context_inner(
                &package_id_owned,
                resolve_deps,
                output_path_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Protocol-first package-context preparation.
///
/// `package_id` is required for all non-generic protocols.
#[pyfunction]
#[pyo3(signature = (
    *,
    protocol="generic",
    package_id=None,
    resolve_deps=true,
    output_path=None,
))]
fn protocol_prepare(
    py: Python<'_>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    output_path: Option<&str>,
) -> PyResult<PyObject> {
    let protocol_owned = protocol.to_string();
    let package_id_owned = package_id.map(ToOwned::to_owned);
    let output_path_owned = output_path.map(ToOwned::to_owned);
    let value = py
        .allow_threads(move || {
            let resolved =
                resolve_protocol_package_id(&protocol_owned, package_id_owned.as_deref())?;
            prepare_package_context_inner(&resolved, resolve_deps, output_path_owned.as_deref())
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Canonical alias for `prepare_package_context`.
#[pyfunction]
#[pyo3(signature = (package_id, *, resolve_deps=true, output_path=None))]
fn context_prepare(
    py: Python<'_>,
    package_id: &str,
    resolve_deps: bool,
    output_path: Option<&str>,
) -> PyResult<PyObject> {
    prepare_package_context(py, package_id, resolve_deps, output_path)
}

/// Canonical alias for `protocol_prepare`.
#[pyfunction]
#[pyo3(signature = (
    *,
    protocol="generic",
    package_id=None,
    resolve_deps=true,
    output_path=None,
))]
fn adapter_prepare(
    py: Python<'_>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    output_path: Option<&str>,
) -> PyResult<PyObject> {
    protocol_prepare(py, protocol, package_id, resolve_deps, output_path)
}

/// Interactive two-step flow helper for Python.
///
/// Keeps prepared package context in memory and reuses it across replays.
#[pyclass(name = "OrchestrationSession", module = "sui_sandbox")]
struct OrchestrationSession {
    context: Option<serde_json::Value>,
    package_id: Option<String>,
}

#[pymethods]
impl OrchestrationSession {
    #[new]
    fn new() -> Self {
        Self {
            context: None,
            package_id: None,
        }
    }

    #[pyo3(signature = (package_id, *, resolve_deps=true, output_path=None))]
    fn prepare(
        &mut self,
        py: Python<'_>,
        package_id: &str,
        resolve_deps: bool,
        output_path: Option<&str>,
    ) -> PyResult<PyObject> {
        let package_id_owned = package_id.to_string();
        let output_path_owned = output_path.map(|s| s.to_string());
        let value = py
            .allow_threads(move || {
                prepare_package_context_inner(
                    &package_id_owned,
                    resolve_deps,
                    output_path_owned.as_deref(),
                )
            })
            .map_err(to_py_err)?;
        self.package_id = Some(package_id.to_string());
        self.context = Some(value.clone());
        json_value_to_py(py, &value)
    }

    fn load_context(&mut self, py: Python<'_>, context_path: &str) -> PyResult<PyObject> {
        let path = PathBuf::from(context_path);
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read context file {}", path.display()))
            .map_err(to_py_err)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("Invalid context JSON in {}", path.display()))
            .map_err(to_py_err)?;
        self.package_id = value
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        self.context = Some(value.clone());
        json_value_to_py(py, &value)
    }

    fn save_context(&self, context_path: &str) -> PyResult<()> {
        let value = self.context.as_ref().ok_or_else(|| {
            PyRuntimeError::new_err(
                "OrchestrationSession has no context; call prepare() or load_context()",
            )
        })?;
        let path = PathBuf::from(context_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    PyRuntimeError::new_err(format!(
                        "Failed to create context directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }
        let serialized = serde_json::to_string_pretty(value).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize context payload: {}", e))
        })?;
        std::fs::write(&path, serialized).map_err(|e| {
            PyRuntimeError::new_err(format!(
                "Failed to write context file {}: {}",
                path.display(),
                e
            ))
        })?;
        Ok(())
    }

    fn has_context(&self) -> bool {
        self.context.is_some()
    }

    fn package_id(&self) -> Option<String> {
        self.package_id.clone()
    }

    fn context(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        match self.context.as_ref() {
            Some(value) => Ok(Some(json_value_to_py(py, value)?)),
            None => Ok(None),
        }
    }

    #[pyo3(signature = (
        digest=None,
        *,
        checkpoint=None,
        discover_latest=None,
        source=None,
        state_file=None,
        cache_dir=None,
        walrus_network="mainnet",
        walrus_caching_url=None,
        walrus_aggregator_url=None,
        rpc_url="https://fullnode.mainnet.sui.io:443",
        profile=None,
        fetch_strategy=None,
        vm_only=false,
        allow_fallback=true,
        prefetch_depth=3,
        prefetch_limit=200,
        auto_system_objects=true,
        no_prefetch=false,
        compare=false,
        analyze_only=false,
        synthesize_missing=false,
        self_heal_dynamic_fields=false,
        analyze_mm2=false,
        verbose=false,
    ))]
    fn replay(
        &self,
        py: Python<'_>,
        digest: Option<&str>,
        checkpoint: Option<u64>,
        discover_latest: Option<u64>,
        source: Option<&str>,
        state_file: Option<&str>,
        cache_dir: Option<&str>,
        walrus_network: &str,
        walrus_caching_url: Option<&str>,
        walrus_aggregator_url: Option<&str>,
        rpc_url: &str,
        profile: Option<&str>,
        fetch_strategy: Option<&str>,
        vm_only: bool,
        allow_fallback: bool,
        prefetch_depth: usize,
        prefetch_limit: usize,
        auto_system_objects: bool,
        no_prefetch: bool,
        compare: bool,
        analyze_only: bool,
        synthesize_missing: bool,
        self_heal_dynamic_fields: bool,
        analyze_mm2: bool,
        verbose: bool,
    ) -> PyResult<PyObject> {
        let context_tmp = match self.context.as_ref() {
            Some(value) => Some(write_temp_context_file(value).map_err(to_py_err)?),
            None => None,
        };
        let result = replay_transaction(
            py,
            digest,
            checkpoint,
            discover_latest,
            if discover_latest.is_some() {
                self.package_id.as_deref()
            } else {
                None
            },
            source,
            state_file,
            context_tmp.as_ref().and_then(|p| p.to_str()),
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

/// Fuzz a Move function with randomly generated inputs.
///
/// Standalone — no CLI binary needed.
///
/// Generates random valid inputs for a Move function's pure parameter types
/// and executes it repeatedly against the local VM. Reports aborts, errors,
/// gas exhaustion, and gas usage profiles.
///
/// Args:
///     package_id: Package address (e.g., "0x2")
///     module: Module name
///     function: Function name
///     iterations: Number of fuzz iterations (default: 100)
///     seed: Random seed for reproducibility (default: random)
///     sender: Sender address (default: "0x0")
///     gas_budget: Gas budget per execution (default: 50_000_000_000)
///     type_args: Type argument strings (e.g., ["0x2::sui::SUI"])
///     fail_fast: Stop on first abort/error (default: False)
///     max_vector_len: Max length for generated vectors (default: 32)
///     dry_run: Only analyze signature, don't execute (default: False)
///     fetch_deps: Auto-resolve transitive deps via GraphQL (default: True)
///
/// Returns: Dict with target, total_iterations, seed, outcomes, gas_profile,
///          interesting_cases, etc. If dry_run=True, returns classification only.
#[pyfunction]
#[pyo3(signature = (
    package_id,
    module,
    function,
    *,
    iterations=100,
    seed=None,
    sender="0x0",
    gas_budget=50_000_000_000u64,
    type_args=vec![],
    fail_fast=false,
    max_vector_len=32,
    dry_run=false,
    fetch_deps=true,
))]
fn fuzz_function(
    py: Python<'_>,
    package_id: &str,
    module: &str,
    function: &str,
    iterations: u64,
    seed: Option<u64>,
    sender: &str,
    gas_budget: u64,
    type_args: Vec<String>,
    fail_fast: bool,
    max_vector_len: usize,
    dry_run: bool,
    fetch_deps: bool,
) -> PyResult<PyObject> {
    let actual_seed = seed.unwrap_or_else(|| {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    });

    let pkg_id_owned = package_id.to_string();
    let module_owned = module.to_string();
    let function_owned = function.to_string();
    let sender_owned = sender.to_string();
    let value = py
        .allow_threads(move || {
            fuzz_function_inner(
                &pkg_id_owned,
                &module_owned,
                &function_owned,
                iterations,
                actual_seed,
                &sender_owned,
                gas_budget,
                type_args,
                fail_fast,
                max_vector_len,
                dry_run,
                fetch_deps,
            )
        })
        .map_err(to_py_err)?;

    json_value_to_py(py, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use sui_sandbox_core::workflow::{
        WorkflowAnalyzeReplayStep, WorkflowDefaults, WorkflowFetchStrategy, WorkflowReplayProfile,
        WorkflowReplayStep, WorkflowSource, WorkflowSpec,
    };

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(relative)
    }

    fn synthetic_state_fixture() -> PathBuf {
        fixture_path("examples/data/state_json_synthetic_ptb_demo.json")
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), ts))
    }

    fn with_sandbox_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let previous: Option<OsString> = std::env::var_os("SUI_SANDBOX_HOME");
        std::env::set_var("SUI_SANDBOX_HOME", home);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Some(value) = previous {
            std::env::set_var("SUI_SANDBOX_HOME", value);
        } else {
            std::env::remove_var("SUI_SANDBOX_HOME");
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn has_flag(args: &[String], flag: &str) -> bool {
        args.iter().any(|arg| arg == flag)
    }

    #[test]
    fn workflow_build_replay_command_includes_native_replay_flags() {
        let defaults = WorkflowDefaults {
            source: Some(WorkflowSource::Hybrid),
            profile: Some(WorkflowReplayProfile::Fast),
            fetch_strategy: Some(WorkflowFetchStrategy::Eager),
            vm_only: Some(true),
            synthesize_missing: Some(true),
            self_heal_dynamic_fields: Some(true),
            ..WorkflowDefaults::default()
        };
        let replay: WorkflowReplayStep = serde_json::from_value(json!({
            "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "checkpoint": "239615926"
        }))
        .expect("valid replay step");

        let args = workflow_build_replay_command(&defaults, &replay);

        assert!(has_flag(&args, "--profile"));
        assert!(has_flag(&args, "--fetch-strategy"));
        assert!(has_flag(&args, "--vm-only"));
        assert!(has_flag(&args, "--synthesize-missing"));
        assert!(has_flag(&args, "--self-heal-dynamic-fields"));
    }

    #[test]
    fn workflow_build_replay_command_honors_false_step_overrides() {
        let defaults = WorkflowDefaults {
            synthesize_missing: Some(true),
            self_heal_dynamic_fields: Some(true),
            ..WorkflowDefaults::default()
        };
        let replay: WorkflowReplayStep = serde_json::from_value(json!({
            "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "synthesize_missing": false,
            "self_heal_dynamic_fields": false
        }))
        .expect("valid replay step");

        let args = workflow_build_replay_command(&defaults, &replay);

        assert!(!has_flag(&args, "--synthesize-missing"));
        assert!(!has_flag(&args, "--self-heal-dynamic-fields"));
    }

    #[test]
    fn workflow_build_analyze_replay_command_honors_mm2_controls() {
        let defaults = WorkflowDefaults {
            mm2: Some(true),
            ..WorkflowDefaults::default()
        };
        let analyze_default: WorkflowAnalyzeReplayStep = serde_json::from_value(
            json!({ "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2" }),
        )
        .expect("valid analyze step");
        let args_default = workflow_build_analyze_replay_command(&defaults, &analyze_default);
        assert!(has_flag(&args_default, "--mm2"));

        let analyze_override: WorkflowAnalyzeReplayStep = serde_json::from_value(json!({
            "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "mm2": false
        }))
        .expect("valid analyze step override");
        let args_override = workflow_build_analyze_replay_command(&defaults, &analyze_override);
        assert!(!has_flag(&args_override, "--mm2"));
    }

    #[test]
    fn parse_replay_profile_defaults_and_rejects_invalid() {
        assert!(matches!(
            parse_replay_profile(None).expect("default profile"),
            WorkflowReplayProfile::Balanced
        ));
        assert!(matches!(
            parse_replay_profile(Some("fast")).expect("fast profile"),
            WorkflowReplayProfile::Fast
        ));
        assert!(parse_replay_profile(Some("invalid")).is_err());
    }

    #[test]
    fn parse_replay_fetch_strategy_defaults_and_rejects_invalid() {
        assert!(matches!(
            parse_replay_fetch_strategy(None).expect("default fetch strategy"),
            WorkflowFetchStrategy::Full
        ));
        assert!(matches!(
            parse_replay_fetch_strategy(Some("eager")).expect("eager fetch strategy"),
            WorkflowFetchStrategy::Eager
        ));
        assert!(parse_replay_fetch_strategy(Some("invalid")).is_err());
    }

    #[test]
    fn pipeline_validate_alias_matches_workflow_validate() {
        Python::with_gil(|py| {
            let spec = fixture_path("examples/data/workflow_replay_analyze_demo.json");
            let spec_str = spec.to_str().expect("utf8 spec path");

            let workflow = workflow_validate(py, spec_str).expect("workflow validate");
            let pipeline = pipeline_validate(py, spec_str).expect("pipeline validate");

            let workflow_json =
                py_json_value(py, workflow.bind(py).as_any()).expect("workflow json");
            let pipeline_json =
                py_json_value(py, pipeline.bind(py).as_any()).expect("pipeline json");

            assert_eq!(workflow_json, pipeline_json);
        });
    }

    #[test]
    fn pipeline_run_inline_alias_matches_workflow_run_inline() {
        Python::with_gil(|py| {
            let spec = serde_json::json!({
                "version": 1,
                "name": "alias_parity",
                "steps": [
                    { "id": "status", "kind": "command", "args": ["status"] }
                ]
            });
            let spec_obj = json_value_to_py(py, &spec).expect("spec object");

            let workflow = workflow_run_inline(
                py,
                spec_obj.bind(py).as_any(),
                true,
                false,
                None,
                "https://archive.mainnet.sui.io:443",
                "mainnet",
                None,
                None,
                false,
            )
            .expect("workflow run inline");
            let pipeline = pipeline_run_inline(
                py,
                spec_obj.bind(py).as_any(),
                true,
                false,
                None,
                "https://archive.mainnet.sui.io:443",
                "mainnet",
                None,
                None,
                false,
            )
            .expect("pipeline run inline");

            let workflow_json =
                py_json_value(py, workflow.bind(py).as_any()).expect("workflow json");
            let pipeline_json =
                py_json_value(py, pipeline.bind(py).as_any()).expect("pipeline json");

            assert_eq!(
                workflow_json.get("total_steps"),
                pipeline_json.get("total_steps")
            );
            assert_eq!(
                workflow_json.get("succeeded_steps"),
                pipeline_json.get("succeeded_steps")
            );
            assert_eq!(
                workflow_json.get("failed_steps"),
                pipeline_json.get("failed_steps")
            );
            assert_eq!(workflow_json.get("steps"), pipeline_json.get("steps"));
        });
    }

    #[test]
    fn adapter_prepare_alias_matches_protocol_prepare_error_shape() {
        Python::with_gil(|py| {
            let protocol = "generic";
            let protocol_err = protocol_prepare(py, protocol, None, true, None)
                .expect_err("protocol_prepare should require package id")
                .to_string();
            let adapter_err = adapter_prepare(py, protocol, None, true, None)
                .expect_err("adapter_prepare should require package id")
                .to_string();

            assert_eq!(protocol_err, adapter_err);
            assert!(protocol_err.contains("requires package_id"));
        });
    }

    #[test]
    fn adapter_run_alias_matches_protocol_run_error_shape() {
        Python::with_gil(|py| {
            let protocol = "generic";
            let protocol_err = protocol_run(
                py,
                None,
                protocol,
                None,
                true,
                None,
                None,
                None,
                None,
                None,
                None,
                "mainnet",
                None,
                None,
                "https://archive.mainnet.sui.io:443",
                None,
                None,
                false,
                true,
                3,
                200,
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            )
            .expect_err("protocol_run should require package id")
            .to_string();

            let adapter_err = adapter_run(
                py,
                None,
                protocol,
                None,
                true,
                None,
                None,
                None,
                None,
                None,
                None,
                "mainnet",
                None,
                None,
                "https://archive.mainnet.sui.io:443",
                None,
                None,
                false,
                true,
                3,
                200,
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            )
            .expect_err("adapter_run should require package id")
            .to_string();

            assert_eq!(protocol_err, adapter_err);
            assert!(protocol_err.contains("requires package_id"));
        });
    }

    #[test]
    fn classify_replay_output_matches_core_classifier() {
        let raw = json!({
            "local_success": false,
            "local_error": "historical object not found in archive",
            "diagnostics": {
                "missing_input_objects": [],
                "missing_packages": [],
                "suggestions": []
            },
            "effects": {
                "failed_command_index": 1,
                "failed_command_description": "MoveCall 1"
            }
        });
        let binding_output = classify_replay_output(&raw);
        let core_output =
            serde_json::to_value(core_classify_replay_output(&raw)).expect("core classification");
        assert_eq!(binding_output, core_output);
    }

    #[test]
    fn workflow_execute_replay_step_supports_local_cache_source() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        let state_file = synthetic_state_fixture();
        assert!(
            state_file.exists(),
            "missing synthetic fixture {}",
            state_file.display()
        );

        let replay_state =
            load_replay_state_from_file(&state_file, None).expect("load synthetic replay state");
        let digest = replay_state.transaction.digest.0;
        let temp_home = unique_temp_dir("sui_python_local_workflow_replay");
        fs::create_dir_all(&temp_home).expect("create temp sandbox home");

        let execution = with_sandbox_home(&temp_home, || {
            import_state_inner(
                Some(state_file.to_str().expect("utf8 fixture path")),
                None,
                None,
                None,
                None,
            )
            .expect("import synthetic replay state");

            let replay: WorkflowReplayStep = serde_json::from_value(json!({
                "digest": digest,
                "source": "local",
                "strict": true
            }))
            .expect("valid replay step");

            workflow_execute_replay_step(
                &WorkflowDefaults::default(),
                &replay,
                "https://fullnode.mainnet.sui.io:443",
                "mainnet",
                None,
                None,
                false,
            )
            .expect("workflow replay step from local cache")
        });

        assert_eq!(execution.exit_code, 0);
        assert_eq!(execution.output["local_success"], true);
        assert_eq!(
            execution.output["execution_path"]["effective_source"],
            "local_cache"
        );
        assert_eq!(execution.output["workflow_source"], "local");
        let _ = fs::remove_dir_all(&temp_home);
    }

    #[test]
    fn workflow_execute_analyze_replay_step_supports_local_cache_source() {
        let _guard = env_lock().lock().expect("env lock poisoned");
        let state_file = synthetic_state_fixture();
        assert!(
            state_file.exists(),
            "missing synthetic fixture {}",
            state_file.display()
        );

        let replay_state =
            load_replay_state_from_file(&state_file, None).expect("load synthetic replay state");
        let digest = replay_state.transaction.digest.0;
        let temp_home = unique_temp_dir("sui_python_local_workflow_analyze");
        fs::create_dir_all(&temp_home).expect("create temp sandbox home");

        let execution = with_sandbox_home(&temp_home, || {
            import_state_inner(
                Some(state_file.to_str().expect("utf8 fixture path")),
                None,
                None,
                None,
                None,
            )
            .expect("import synthetic replay state");

            let analyze: WorkflowAnalyzeReplayStep = serde_json::from_value(json!({
                "digest": digest,
                "source": "local"
            }))
            .expect("valid analyze step");

            workflow_execute_analyze_replay_step(
                &WorkflowDefaults::default(),
                &analyze,
                "https://fullnode.mainnet.sui.io:443",
                false,
            )
            .expect("workflow analyze step from local cache")
        });

        assert_eq!(execution.exit_code, 0);
        assert_eq!(execution.output["local_success"], true);
        assert_eq!(
            execution.output["execution_path"]["effective_source"],
            "local_cache"
        );
        assert_eq!(execution.output["workflow_source"], "local");
        assert_eq!(execution.output["analysis"]["commands"], 1);
        let _ = fs::remove_dir_all(&temp_home);
    }

    #[test]
    fn workflow_run_spec_inner_supports_inline_label_in_dry_run() {
        let spec: WorkflowSpec = serde_json::from_value(json!({
            "version": 1,
            "name": "inline_dry_run",
            "steps": [
                {
                    "id": "status",
                    "kind": "command",
                    "args": ["status"]
                }
            ]
        }))
        .expect("valid inline workflow spec");
        spec.validate().expect("spec validates");

        let report = workflow_run_spec_inner(
            spec,
            "<inline>".to_string(),
            true,
            false,
            None,
            "https://fullnode.mainnet.sui.io:443",
            "mainnet",
            None,
            None,
            false,
        )
        .expect("inline dry-run succeeds");

        assert_eq!(report["spec_file"], "<inline>");
        assert_eq!(report["name"], "inline_dry_run");
        assert_eq!(report["total_steps"], 1);
        assert_eq!(report["succeeded_steps"], 1);
        assert_eq!(report["failed_steps"], 0);
        assert_eq!(report["steps"][0]["kind"], "command");
    }

    #[test]
    fn load_versions_snapshot_reads_fixture() {
        let path = fixture_path(
            "examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json",
        );
        let (checkpoint, versions) =
            sui_sandbox_core::historical_view::load_versions_snapshot(&path)
                .expect("load versions fixture");
        assert_eq!(checkpoint, 240733000);
        assert!(versions
            .contains_key("0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80"));
        assert!(versions.contains_key("0x6"));
    }

    #[test]
    fn load_context_packages_accepts_cli_v2_wrapper() {
        let tmp = std::env::temp_dir().join(format!(
            "sui_python_context_v2_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let payload = json!({
            "version": 2,
            "package_id": "0x2",
            "with_deps": true,
            "packages": [
                {
                    "address": "0x2",
                    "modules": ["test_mod"],
                    "bytecodes": ["AQIDBA=="]
                }
            ]
        });
        fs::write(
            &tmp,
            serde_json::to_string(&payload).expect("serialize payload"),
        )
        .expect("write payload");
        let parsed = load_context_packages_from_file(&tmp).expect("parse v2 context");
        let key = AccountAddress::from_hex_literal("0x2").expect("address");
        assert!(parsed.contains_key(&key));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn load_context_packages_accepts_legacy_map_wrapper() {
        let tmp = std::env::temp_dir().join(format!(
            "sui_python_context_v1_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let payload = json!({
            "version": 1,
            "package_id": "0x2",
            "resolve_deps": true,
            "packages": {
                "0x2": ["AQIDBA=="]
            }
        });
        fs::write(
            &tmp,
            serde_json::to_string(&payload).expect("serialize payload"),
        )
        .expect("write payload");
        let parsed = load_context_packages_from_file(&tmp).expect("parse v1 context");
        let key = AccountAddress::from_hex_literal("0x2").expect("address");
        assert!(parsed.contains_key(&key));
        let _ = fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

/// Python module: sui_sandbox
#[pymodule]
fn sui_sandbox(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(extract_interface, m)?)?;
    m.add_function(wrap_pyfunction!(get_latest_checkpoint, m)?)?;
    m.add_function(wrap_pyfunction!(get_checkpoint, m)?)?;
    m.add_function(wrap_pyfunction!(doctor, m)?)?;
    m.add_function(wrap_pyfunction!(session_status, m)?)?;
    m.add_function(wrap_pyfunction!(session_reset, m)?)?;
    m.add_function(wrap_pyfunction!(session_clean, m)?)?;
    m.add_function(wrap_pyfunction!(snapshot_save, m)?)?;
    m.add_function(wrap_pyfunction!(snapshot_load, m)?)?;
    m.add_function(wrap_pyfunction!(snapshot_list, m)?)?;
    m.add_function(wrap_pyfunction!(snapshot_delete, m)?)?;
    m.add_function(wrap_pyfunction!(ptb_universe, m)?)?;
    m.add_function(wrap_pyfunction!(discover_checkpoint_targets, m)?)?;
    m.add_function(wrap_pyfunction!(context_discover, m)?)?;
    m.add_function(wrap_pyfunction!(protocol_discover, m)?)?;
    m.add_function(wrap_pyfunction!(adapter_discover, m)?)?;
    m.add_function(wrap_pyfunction!(pipeline_validate, m)?)?;
    m.add_function(wrap_pyfunction!(pipeline_init, m)?)?;
    m.add_function(wrap_pyfunction!(pipeline_auto, m)?)?;
    m.add_function(wrap_pyfunction!(pipeline_run, m)?)?;
    m.add_function(wrap_pyfunction!(pipeline_run_inline, m)?)?;
    m.add_function(wrap_pyfunction!(workflow_validate, m)?)?;
    m.add_function(wrap_pyfunction!(workflow_init, m)?)?;
    m.add_function(wrap_pyfunction!(workflow_auto, m)?)?;
    m.add_function(wrap_pyfunction!(workflow_run, m)?)?;
    m.add_function(wrap_pyfunction!(workflow_run_inline, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_object_bcs, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_historical_package_bytecodes, m)?)?;
    m.add_function(wrap_pyfunction!(import_state, m)?)?;
    m.add_function(wrap_pyfunction!(deserialize_transaction, m)?)?;
    m.add_function(wrap_pyfunction!(deserialize_package, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_package_bytecodes, m)?)?;
    m.add_function(wrap_pyfunction!(prepare_package_context, m)?)?;
    m.add_function(wrap_pyfunction!(context_prepare, m)?)?;
    m.add_function(wrap_pyfunction!(protocol_prepare, m)?)?;
    m.add_function(wrap_pyfunction!(adapter_prepare, m)?)?;
    m.add_function(wrap_pyfunction!(json_to_bcs, m)?)?;
    m.add_function(wrap_pyfunction!(transaction_json_to_bcs, m)?)?;
    m.add_function(wrap_pyfunction!(call_view_function, m)?)?;
    m.add_function(wrap_pyfunction!(historical_view_from_versions, m)?)?;
    m.add_function(wrap_pyfunction!(historical_decode_return_u64, m)?)?;
    m.add_function(wrap_pyfunction!(historical_decode_return_u64s, m)?)?;
    m.add_function(wrap_pyfunction!(historical_decode_returns_typed, m)?)?;
    m.add_function(wrap_pyfunction!(historical_decode_with_schema, m)?)?;
    m.add_function(wrap_pyfunction!(fuzz_function, m)?)?;
    m.add_function(wrap_pyfunction!(replay, m)?)?;
    m.add_function(wrap_pyfunction!(replay_transaction, m)?)?;
    m.add_function(wrap_pyfunction!(analyze_replay, m)?)?;
    m.add_function(wrap_pyfunction!(replay_analyze, m)?)?;
    m.add_function(wrap_pyfunction!(replay_effects, m)?)?;
    m.add_function(wrap_pyfunction!(classify_replay_result, m)?)?;
    m.add_function(wrap_pyfunction!(dynamic_field_diagnostics, m)?)?;
    m.add_function(wrap_pyfunction!(context_replay, m)?)?;
    m.add_function(wrap_pyfunction!(context_run, m)?)?;
    m.add_function(wrap_pyfunction!(protocol_run, m)?)?;
    m.add_function(wrap_pyfunction!(adapter_run, m)?)?;
    m.add_class::<OrchestrationSession>()?;
    let orchestration_session = m.getattr("OrchestrationSession")?;
    m.add("FlowSession", orchestration_session.clone())?;
    m.add("ContextSession", orchestration_session)?;
    Ok(())
}
