//! Python bindings for Sui Move package analysis, checkpoint replay, view function
//! execution, and Move function fuzzing.
//!
//! **All functions are standalone** — `pip install sui-sandbox` is all you need:
//! - `extract_interface`: Extract full Move package interface from bytecode or GraphQL
//! - `get_latest_checkpoint`: Get latest Walrus checkpoint number
//! - `get_checkpoint`: Fetch and summarize a Walrus checkpoint
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
//! - `FlowSession`: In-memory prepared context + replay helper for interactive workflows
//! - `json_to_bcs`: Convert Sui object JSON to BCS bytes
//! - `transaction_json_to_bcs`: Convert Snowflake/canonical TransactionData JSON to BCS bytes
//! - `call_view_function`: Execute a Move view function in the local VM
//! - `deepbook_margin_state`: One-call DeepBook manager_state historical replay helper
//! - `fuzz_function`: Fuzz a Move function with random inputs
//! - `replay`: Replay historical transactions (with optional analysis-only mode)
//! - `replay_transaction`: Opinionated replay helper with compact signature
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
use sui_sandbox_core::checkpoint_discovery::{
    build_walrus_client as core_build_walrus_client,
    discover_checkpoint_targets as core_discover_checkpoint_targets,
    normalize_package_id as core_normalize_package_id,
    resolve_replay_target_from_discovery as core_resolve_replay_target_from_discovery,
    WalrusArchiveNetwork as CoreWalrusArchiveNetwork,
};
use sui_sandbox_core::orchestrator::ReplayOrchestrator;
use sui_sandbox_core::ptb_universe::{
    run_with_args as core_run_ptb_universe, Args as CorePtbUniverseArgs,
    CheckpointSource as CoreCheckpointSource, DEFAULT_LATEST as CORE_PTB_UNIVERSE_DEFAULT_LATEST,
    DEFAULT_MAX_PTBS as CORE_PTB_UNIVERSE_DEFAULT_MAX_PTBS,
    DEFAULT_STREAM_TIMEOUT_SECS as CORE_PTB_UNIVERSE_DEFAULT_STREAM_TIMEOUT_SECS,
    DEFAULT_TOP_PACKAGES as CORE_PTB_UNIVERSE_DEFAULT_TOP_PACKAGES,
};
use sui_sandbox_core::resolver::ModuleProvider;
use sui_sandbox_core::utilities::unresolved_package_dependencies_for_modules;
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

const PROTOCOL_DEEPBOOK_MARGIN_PACKAGE: &str =
    "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";
const DEEPBOOK_SPOT_PACKAGE: &str =
    "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497";
const DEEPBOOK_MARGIN_DEFAULT_VERSIONS_FILE: &str =
    "examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json";
const DEEPBOOK_MARGIN_REGISTRY: &str =
    "0x0e40998b359a9ccbab22a98ed21bd4346abf19158bc7980c8291908086b3a742";
const DEEPBOOK_MARGIN_TARGET_MANAGER: &str =
    "0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80";
const DEEPBOOK_MARGIN_POOL: &str =
    "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407";
const DEEPBOOK_MARGIN_BASE_POOL: &str =
    "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344";
const DEEPBOOK_MARGIN_QUOTE_POOL: &str =
    "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f";
const DEEPBOOK_MARGIN_CLOCK: &str = "0x6";
const PTB_UNIVERSE_DEFAULT_OUT_DIR: &str = "examples/out/walrus_ptb_universe";
const DEEPBOOK_MARGIN_SUI_PYTH_PRICE_INFO: &str =
    "0x801dbc2f0053d34734814b2d6df491ce7807a725fe9a01ad74a07e9c51396c37";
const DEEPBOOK_MARGIN_USDC_PYTH_PRICE_INFO: &str =
    "0x5dec622733a204ca27f5a90d8c2fad453cc6665186fd5dff13a83d0b6c9027ab";
const DEEPBOOK_MARGIN_SUI_TYPE: &str = "0x2::sui::SUI";
const DEEPBOOK_MARGIN_USDC_TYPE: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";
const DEEPBOOK_MARGIN_REQUIRED_OBJECTS: [&str; 8] = [
    DEEPBOOK_MARGIN_TARGET_MANAGER,
    DEEPBOOK_MARGIN_REGISTRY,
    DEEPBOOK_MARGIN_SUI_PYTH_PRICE_INFO,
    DEEPBOOK_MARGIN_USDC_PYTH_PRICE_INFO,
    DEEPBOOK_MARGIN_POOL,
    DEEPBOOK_MARGIN_BASE_POOL,
    DEEPBOOK_MARGIN_QUOTE_POOL,
    DEEPBOOK_MARGIN_CLOCK,
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_py_err(e: anyhow::Error) -> PyErr {
    PyRuntimeError::new_err(format!("{:#}", e))
}

fn default_local_cache_dir() -> PathBuf {
    std::env::var("SUI_SANDBOX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sui-sandbox")
        })
        .join("cache")
        .join("local")
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

fn inferred_module_name(bytes: &[u8], idx: usize) -> String {
    CompiledModule::deserialize_with_defaults(bytes)
        .ok()
        .map(|module| module.self_id().name().to_string())
        .unwrap_or_else(|| format!("module_{}", idx))
}

fn decode_context_packages_value(
    value: &serde_json::Value,
) -> Result<HashMap<AccountAddress, PackageData>> {
    let mut out = HashMap::new();
    match value {
        serde_json::Value::Null => return Ok(out),
        serde_json::Value::Object(map) => {
            // Python/native shape: {"packages": {"0x..": ["base64...", ...]}}
            for (address, bytecodes_value) in map {
                let addr = AccountAddress::from_hex_literal(address)
                    .with_context(|| format!("invalid package address in context: {}", address))?;
                let bytecodes = bytecodes_value.as_array().ok_or_else(|| {
                    anyhow!("package {} in context must map to an array", address)
                })?;
                let mut modules = Vec::with_capacity(bytecodes.len());
                for (idx, item) in bytecodes.iter().enumerate() {
                    let encoded = item.as_str().ok_or_else(|| {
                        anyhow!("package {} module #{} is not a base64 string", address, idx)
                    })?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .with_context(|| {
                            format!("invalid base64 for package {} module #{}", address, idx)
                        })?;
                    modules.push((inferred_module_name(&bytes, idx), bytes));
                }
                out.insert(
                    addr,
                    PackageData {
                        address: addr,
                        version: 0,
                        modules,
                        linkage: HashMap::new(),
                        original_id: None,
                    },
                );
            }
        }
        serde_json::Value::Array(items) => {
            // CLI v2 shape: {"packages": [{"address":"0x..","modules":[...],"bytecodes":[...]}]}
            for (pkg_idx, item) in items.iter().enumerate() {
                let address = item
                    .get("address")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow!("context package #{} missing `address`", pkg_idx))?;
                let addr = AccountAddress::from_hex_literal(address)
                    .with_context(|| format!("invalid package address in context: {}", address))?;
                let encoded = item
                    .get("bytecodes")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| {
                        anyhow!("context package {} missing `bytecodes` array", address)
                    })?;
                let module_names: Vec<String> = item
                    .get("modules")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut modules = Vec::with_capacity(encoded.len());
                for (idx, item) in encoded.iter().enumerate() {
                    let encoded = item.as_str().ok_or_else(|| {
                        anyhow!(
                            "context package {} module #{} is not a string",
                            address,
                            idx
                        )
                    })?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .with_context(|| {
                            format!("invalid base64 for package {} module #{}", address, idx)
                        })?;
                    let name = module_names
                        .get(idx)
                        .cloned()
                        .unwrap_or_else(|| inferred_module_name(&bytes, idx));
                    modules.push((name, bytes));
                }
                out.insert(
                    addr,
                    PackageData {
                        address: addr,
                        version: 0,
                        modules,
                        linkage: HashMap::new(),
                        original_id: None,
                    },
                );
            }
        }
        _ => {
            return Err(anyhow!(
                "unsupported context `packages` format (expected object or array)"
            ));
        }
    }
    Ok(out)
}

fn load_context_packages_from_file(path: &Path) -> Result<HashMap<AccountAddress, PackageData>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read context file {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse context JSON {}", path.display()))?;
    let packages_value = value.get("packages").cloned().unwrap_or_default();
    decode_context_packages_value(&packages_value)
}

fn merge_context_packages(
    replay_state: &mut ReplayState,
    context_packages: &HashMap<AccountAddress, PackageData>,
) -> usize {
    let mut inserted = 0usize;
    for (address, package) in context_packages {
        if replay_state.packages.contains_key(address) {
            continue;
        }
        replay_state.packages.insert(*address, package.clone());
        inserted += 1;
    }
    inserted
}

fn write_temp_context_file(payload: &serde_json::Value) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "sui_sandbox_flow_context_{}_{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&path, serde_json::to_string(payload)?)
        .with_context(|| format!("Failed to write temp context file {}", path.display()))?;
    Ok(path)
}

/// Fetch a package's modules via GraphQL, returning (module_name, bytecode_bytes) pairs.
fn fetch_package_modules(
    graphql: &GraphQLClient,
    package_id: &str,
) -> Result<Vec<(String, Vec<u8>)>> {
    let pkg = graphql
        .fetch_package(package_id)
        .with_context(|| format!("fetch package {}", package_id))?;
    sui_transport::decode_graphql_modules(package_id, &pkg.modules)
}

/// Build a LocalModuleResolver with the Sui framework loaded, then fetch a target
/// package and its transitive dependencies via GraphQL.
fn build_resolver_with_deps(
    package_id: &str,
    extra_type_refs: &[String],
) -> Result<(
    sui_sandbox_core::resolver::LocalModuleResolver,
    HashSet<AccountAddress>,
)> {
    let mut resolver = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;
    let mut loaded_packages = HashSet::new();
    for fw in ["0x1", "0x2", "0x3"] {
        loaded_packages.insert(AccountAddress::from_hex_literal(fw).unwrap());
    }

    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
    let graphql = GraphQLClient::new(&graphql_endpoint);

    let mut to_fetch: VecDeque<AccountAddress> = VecDeque::new();
    let target_addr = AccountAddress::from_hex_literal(package_id)
        .with_context(|| format!("invalid target package: {}", package_id))?;
    if !loaded_packages.contains(&target_addr) {
        to_fetch.push_back(target_addr);
    }

    // Also fetch packages referenced in type strings
    for type_str in extra_type_refs {
        for pkg_id in sui_sandbox_core::utilities::extract_package_ids_from_type(type_str) {
            if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                if !loaded_packages.contains(&addr) && !is_framework_address(&addr) {
                    to_fetch.push_back(addr);
                }
            }
        }
    }

    // BFS fetch dependencies
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
                resolver.load_package_at(modules, addr)?;
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

    Ok((resolver, loaded_packages))
}

fn synthesize_missing_inputs_py(
    missing: &[sui_sandbox_core::tx_replay::MissingInputObject],
    cached_objects: &mut HashMap<String, String>,
    version_map: &mut HashMap<String, u64>,
    resolver: &sui_sandbox_core::resolver::LocalModuleResolver,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    graphql: &GraphQLClient,
    verbose: bool,
) -> Result<Vec<String>> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
    if modules.is_empty() {
        return Err(anyhow!("no modules loaded for synthesis"));
    }
    let type_model = sui_sandbox_core::mm2::TypeModel::from_modules(modules)
        .map_err(|e| anyhow!("failed to build type model: {}", e))?;
    let mut synthesizer = sui_sandbox_core::mm2::TypeSynthesizer::new(&type_model);

    let mut logs = Vec::new();
    for entry in missing {
        let object_id = entry.object_id.as_str();
        let version = entry.version;
        let mut type_string = graphql
            .fetch_object_at_version(object_id, version)
            .ok()
            .and_then(|obj| obj.type_string)
            .or_else(|| {
                graphql
                    .fetch_object(object_id)
                    .ok()
                    .and_then(|obj| obj.type_string)
            });

        let Some(type_str) = type_string.take() else {
            if verbose {
                logs.push(format!(
                    "missing_type object={} version={} (skipped)",
                    object_id, version
                ));
            }
            continue;
        };

        let mut synth_type = type_str.clone();
        if let Ok(tag) = sui_sandbox_core::types::parse_type_tag(&type_str) {
            let rewritten = sui_sandbox_core::utilities::rewrite_type_tag(tag, aliases);
            synth_type = sui_sandbox_core::types::format_type_tag(&rewritten);
        }

        let mut result = synthesizer.synthesize_with_fallback(&synth_type);
        if let Ok(id) = AccountAddress::from_hex_literal(object_id) {
            if result.bytes.len() >= 32 {
                result.bytes[..32].copy_from_slice(id.as_ref());
            }
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(&result.bytes);
        let normalized = sui_sandbox_core::utilities::normalize_address(object_id);
        cached_objects.insert(normalized.clone(), encoded.clone());
        cached_objects.insert(object_id.to_string(), encoded.clone());
        if let Some(short) = sui_sandbox_core::types::normalize_address_short(object_id) {
            cached_objects.insert(short, encoded.clone());
        }
        version_map.insert(normalized.clone(), version);

        logs.push(format!(
            "synthesized object={} version={} type={} stub={} ({})",
            normalized, version, synth_type, result.is_stub, result.description
        ));
    }

    Ok(logs)
}

fn build_mm2_summary_from_modules(
    modules: Vec<CompiledModule>,
    verbose: bool,
) -> (Option<bool>, Option<String>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sui_sandbox_core::mm2::TypeModel::from_modules(modules)
    }));
    match result {
        Ok(Ok(_)) => (Some(true), None),
        Ok(Err(err)) => {
            if verbose {
                eprintln!("[mm2] type model build failed: {}", err);
            }
            (Some(false), Some(err.to_string()))
        }
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            if verbose {
                eprintln!("[mm2] type model panicked: {}", msg);
            }
            (Some(false), Some(format!("mm2 panic: {}", msg)))
        }
    }
}

fn attach_mm2_summary_fields(
    output: &mut serde_json::Value,
    modules: Vec<CompiledModule>,
    verbose: bool,
) {
    let (mm2_ok, mm2_error) = build_mm2_summary_from_modules(modules, verbose);
    if let Some(analysis) = output
        .get_mut("analysis")
        .and_then(serde_json::Value::as_object_mut)
    {
        analysis.insert("mm2_model_ok".to_string(), serde_json::json!(mm2_ok));
        analysis.insert("mm2_error".to_string(), serde_json::json!(mm2_error));
    }
    if let Some(object) = output.as_object_mut() {
        object.insert("mm2_model_ok".to_string(), serde_json::json!(mm2_ok));
        object.insert("mm2_error".to_string(), serde_json::json!(mm2_error));
    }
}

fn enable_self_heal_fetchers(
    harness: &mut sui_sandbox_core::vm::VMHarness,
    graphql: &GraphQLClient,
    checkpoint: Option<u64>,
    max_version: u64,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    modules: &[CompiledModule],
) {
    let graphql_for_versioned = graphql.clone();
    harness.set_versioned_child_fetcher(Box::new(move |_parent, child_id| {
        let child_hex = child_id.to_hex_literal();
        let object = checkpoint
            .and_then(|cp| {
                graphql_for_versioned
                    .fetch_object_at_checkpoint(&child_hex, cp)
                    .ok()
            })
            .or_else(|| graphql_for_versioned.fetch_object(&child_hex).ok())?;

        if object.version > max_version {
            return None;
        }
        let (type_str, bcs_b64) = (object.type_string?, object.bcs_base64?);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bcs_b64.as_bytes())
            .ok()?;
        let tag = sui_sandbox_core::types::parse_type_tag(&type_str).ok()?;
        Some((tag, bytes, object.version))
    }));

    let graphql_for_key = graphql.clone();
    let aliases_for_key = aliases.clone();
    let modules_for_synth = Arc::new(modules.to_vec());
    harness.set_key_based_child_fetcher(Box::new(
        move |parent, _child_id, _key_type, key_bytes| {
            let parent_hex = parent.to_hex_literal();
            let field = graphql_for_key
                .find_dynamic_field_by_bcs(&parent_hex, key_bytes, checkpoint, 1000)
                .ok()
                .flatten()?;

            let value_type = field.value_type?;
            let parsed = sui_sandbox_core::types::parse_type_tag(&value_type).ok()?;
            let rewritten = sui_sandbox_core::utilities::rewrite_type_tag(parsed, &aliases_for_key);

            if let Some(bcs_b64) = field.value_bcs.as_deref() {
                if let Ok(bytes) =
                    base64::engine::general_purpose::STANDARD.decode(bcs_b64.as_bytes())
                {
                    return Some((rewritten, bytes));
                }
            }

            let synth_type = sui_sandbox_core::types::format_type_tag(&rewritten);
            let type_model =
                sui_sandbox_core::mm2::TypeModel::from_modules(modules_for_synth.as_ref().clone())
                    .ok()?;
            let mut synthesizer = sui_sandbox_core::mm2::TypeSynthesizer::new(&type_model);
            let mut result = synthesizer.synthesize_with_fallback(&synth_type);
            if let Some(obj_id) = field
                .object_id
                .as_deref()
                .and_then(|id| AccountAddress::from_hex_literal(id).ok())
            {
                if result.bytes.len() >= 32 {
                    result.bytes[..32].copy_from_slice(obj_id.as_ref());
                }
            }
            Some((rewritten, result.bytes))
        },
    ));
}

// ---------------------------------------------------------------------------
// extract_interface (native)
// ---------------------------------------------------------------------------

fn extract_interface_inner(
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
) -> Result<serde_json::Value> {
    if package_id.is_none() && bytecode_dir.is_none() {
        return Err(anyhow!(
            "Either package_id or bytecode_dir must be provided"
        ));
    }
    if package_id.is_some() && bytecode_dir.is_some() {
        return Err(anyhow!(
            "Provide either package_id or bytecode_dir, not both"
        ));
    }

    if let Some(dir) = bytecode_dir {
        let dir_path = PathBuf::from(dir);
        let compiled = read_local_compiled_modules(&dir_path)?;
        let pkg_id = resolve_local_package_id(&dir_path)?;
        let (_, interface_value) =
            build_bytecode_interface_value_from_compiled_modules(&pkg_id, &compiled)?;
        return Ok(interface_value);
    }

    let pkg_id_str = package_id.unwrap();
    let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
    let graphql = GraphQLClient::new(&graphql_endpoint);
    let pkg = graphql
        .fetch_package(pkg_id_str)
        .with_context(|| format!("fetch package {}", pkg_id_str))?;

    let raw_modules = sui_transport::decode_graphql_modules(pkg_id_str, &pkg.modules)?;
    let compiled_modules: Vec<CompiledModule> = raw_modules
        .into_iter()
        .map(|(name, bytes)| {
            CompiledModule::deserialize_with_defaults(&bytes)
                .map_err(|e| anyhow!("deserialize {}::{}: {:?}", pkg_id_str, name, e))
        })
        .collect::<Result<_>>()?;

    let (_, interface_value) =
        build_bytecode_interface_value_from_compiled_modules(pkg_id_str, &compiled_modules)?;
    Ok(interface_value)
}

// ---------------------------------------------------------------------------
// replay (native — unified analyze + execute)
// ---------------------------------------------------------------------------

fn replay_inner(
    digest: &str,
    rpc_url: &str,
    source: &str,
    checkpoint: Option<u64>,
    context_packages: Option<&HashMap<AccountAddress, PackageData>>,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    vm_only: bool,
    compare: bool,
    analyze_only: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::replay_support;
    use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};

    // ---------------------------------------------------------------
    // 1. Fetch ReplayState
    // ---------------------------------------------------------------
    let mut replay_state: ReplayState;
    let graphql_client: GraphQLClient;
    let effective_source: String;

    if let Some(cp) = checkpoint {
        // Walrus path — no API key needed
        if verbose {
            eprintln!("[walrus] fetching checkpoint {} for digest {}", cp, digest);
        }
        let checkpoint_data = WalrusClient::mainnet()
            .get_checkpoint(cp)
            .context("Failed to fetch checkpoint from Walrus")?;
        replay_state = checkpoint_to_replay_state(&checkpoint_data, digest)
            .context("Failed to convert checkpoint to replay state")?;
        let gql_endpoint = resolve_graphql_endpoint(rpc_url);
        graphql_client = GraphQLClient::new(&gql_endpoint);
        effective_source = "walrus".to_string();
    } else {
        // gRPC/hybrid path — requires API key
        let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

        let gql_endpoint = resolve_graphql_endpoint(rpc_url);
        graphql_client = GraphQLClient::new(&gql_endpoint);

        let (grpc_endpoint, api_key) =
            sui_transport::grpc::historical_endpoint_and_api_key_from_env();

        let provider = rt.block_on(async {
            let grpc = sui_transport::grpc::GrpcClient::with_api_key(&grpc_endpoint, api_key)
                .await
                .context("Failed to create gRPC client")?;
            let mut provider = HistoricalStateProvider::with_clients(grpc, graphql_client.clone());

            // Enable Walrus for hybrid/walrus sources
            if source == "walrus" || source == "hybrid" {
                provider = provider
                    .with_walrus_from_env()
                    .with_local_object_store_from_env();
            }

            Ok::<HistoricalStateProvider, anyhow::Error>(provider)
        })?;

        let prefetch_dynamic_fields = !no_prefetch;
        replay_state = rt.block_on(async {
            provider
                .replay_state_builder()
                .with_config(sui_state_fetcher::ReplayStateConfig {
                    prefetch_dynamic_fields,
                    df_depth: prefetch_depth,
                    df_limit: prefetch_limit,
                    auto_system_objects,
                })
                .build(digest)
                .await
                .context("Failed to fetch replay state")
        })?;
        effective_source = source.to_string();
    }

    if let Some(context_packages) = context_packages {
        let merged = merge_context_packages(&mut replay_state, context_packages);
        if verbose && merged > 0 {
            eprintln!(
                "[context] merged {} package(s) from prepared context before replay",
                merged
            );
        }
    }

    if verbose {
        eprintln!(
            "  Sender: {}",
            replay_state.transaction.sender.to_hex_literal()
        );
        eprintln!("  Commands: {}", replay_state.transaction.commands.len());
        eprintln!("  Inputs: {}", replay_state.transaction.inputs.len());
        eprintln!(
            "  Objects: {}, Packages: {}",
            replay_state.objects.len(),
            replay_state.packages.len()
        );
    }

    // ---------------------------------------------------------------
    // 2. Analyze-only: return state summary without VM execution
    // ---------------------------------------------------------------
    if analyze_only {
        let mut output = build_analyze_replay_output(
            &replay_state,
            source,
            &effective_source,
            vm_only,
            allow_fallback,
            auto_system_objects,
            !no_prefetch,
            prefetch_depth,
            prefetch_limit,
            verbose,
        )?;
        if analyze_mm2 {
            let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
            let mut resolver = replay_support::hydrate_resolver_from_replay_state(
                &replay_state,
                &pkg_aliases.linkage_upgrades,
                &pkg_aliases.aliases,
            )?;
            let _ = replay_support::fetch_dependency_closure(
                &mut resolver,
                &graphql_client,
                replay_state.checkpoint,
                verbose,
            );
            let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
            attach_mm2_summary_fields(&mut output, modules, verbose);
        }
        return Ok(output);
    }

    // ---------------------------------------------------------------
    // 3. Full replay: build resolver, fetch deps, execute VM
    // ---------------------------------------------------------------
    let pkg_aliases = build_aliases(
        &replay_state.packages,
        None, // no provider ref needed for PyO3 path
        replay_state.checkpoint,
    );

    let mut resolver = replay_support::hydrate_resolver_from_replay_state(
        &replay_state,
        &pkg_aliases.linkage_upgrades,
        &pkg_aliases.aliases,
    )?;

    let fetched_deps = replay_support::fetch_dependency_closure(
        &mut resolver,
        &graphql_client,
        replay_state.checkpoint,
        verbose,
    )
    .unwrap_or(0);
    if verbose && fetched_deps > 0 {
        eprintln!("[deps] fetched {} dependency packages", fetched_deps);
    }

    let mut maps = replay_support::build_replay_object_maps(&replay_state, &pkg_aliases.versions);
    replay_support::maybe_patch_replay_objects(
        &resolver,
        &replay_state,
        &pkg_aliases.versions,
        &pkg_aliases.aliases,
        &mut maps,
        verbose,
    );

    let config = replay_support::build_simulation_config(&replay_state);
    let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
    harness
        .set_address_aliases_with_versions(pkg_aliases.aliases.clone(), maps.versions_str.clone());
    if self_heal_dynamic_fields {
        let max_version = maps.version_map.values().copied().max().unwrap_or(0);
        let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
        if !modules.is_empty() {
            let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            enable_self_heal_fetchers(
                &mut harness,
                &graphql,
                replay_state.checkpoint,
                max_version,
                &pkg_aliases.aliases,
                &modules,
            );
        }
    }
    if self_heal_dynamic_fields {
        let max_version = maps.version_map.values().copied().max().unwrap_or(0);
        let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
        if !modules.is_empty() {
            enable_self_heal_fetchers(
                &mut harness,
                &graphql_client,
                replay_state.checkpoint,
                max_version,
                &pkg_aliases.aliases,
                &modules,
            );
        }
    }

    let reconcile_policy = EffectsReconcilePolicy::Strict;
    let mut replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &maps.cached_objects,
        &pkg_aliases.aliases,
        Some(&maps.versions_str),
        reconcile_policy,
    );
    let mut synthetic_inputs = 0usize;
    if synthesize_missing
        && replay_result
            .as_ref()
            .map(|result| !result.result.local_success)
            .unwrap_or(true)
    {
        let missing =
            tx_replay::find_missing_input_objects(&replay_state.transaction, &maps.cached_objects);
        if !missing.is_empty() {
            match synthesize_missing_inputs_py(
                &missing,
                &mut maps.cached_objects,
                &mut maps.version_map,
                &resolver,
                &pkg_aliases.aliases,
                &graphql_client,
                verbose,
            ) {
                Ok(logs) => {
                    synthetic_inputs = logs.len();
                    if verbose && synthetic_inputs > 0 {
                        eprintln!(
                            "[replay_fallback] synthesized {} missing input object(s)",
                            synthetic_inputs
                        );
                    }
                    if synthetic_inputs > 0 {
                        replay_result =
                            tx_replay::replay_with_version_tracking_with_policy_with_effects(
                                &replay_state.transaction,
                                &mut harness,
                                &maps.cached_objects,
                                &pkg_aliases.aliases,
                                Some(&maps.versions_str),
                                reconcile_policy,
                            );
                    }
                }
                Err(err) => {
                    if verbose {
                        eprintln!("[replay_fallback] synthesis failed: {}", err);
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------
    // 4. Build output JSON
    // ---------------------------------------------------------------
    build_replay_output(
        &replay_state,
        replay_result,
        source,
        &effective_source,
        vm_only,
        allow_fallback,
        auto_system_objects,
        !no_prefetch,
        prefetch_depth,
        prefetch_limit,
        "graphql_dependency_closure",
        fetched_deps,
        synthetic_inputs,
        compare,
    )
}

fn replay_loaded_state_inner(
    mut replay_state: ReplayState,
    requested_source: &str,
    effective_source: &str,
    context_packages: Option<&HashMap<AccountAddress, PackageData>>,
    allow_fallback: bool,
    auto_system_objects: bool,
    self_heal_dynamic_fields: bool,
    vm_only: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    analyze_mm2: bool,
    rpc_url: &str,
    verbose: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::replay_support;
    use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};

    if let Some(context_packages) = context_packages {
        let merged = merge_context_packages(&mut replay_state, context_packages);
        if verbose && merged > 0 {
            eprintln!(
                "[context] merged {} package(s) from prepared context before replay",
                merged
            );
        }
    }

    if analyze_only {
        let mut output = build_analyze_replay_output(
            &replay_state,
            requested_source,
            effective_source,
            vm_only,
            allow_fallback,
            auto_system_objects,
            false,
            0,
            0,
            verbose,
        )?;
        if analyze_mm2 {
            let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
            let resolver = replay_support::hydrate_resolver_from_replay_state(
                &replay_state,
                &pkg_aliases.linkage_upgrades,
                &pkg_aliases.aliases,
            )?;
            let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
            attach_mm2_summary_fields(&mut output, modules, verbose);
        }
        return Ok(output);
    }

    let pkg_aliases = build_aliases(&replay_state.packages, None, replay_state.checkpoint);
    let resolver = replay_support::hydrate_resolver_from_replay_state(
        &replay_state,
        &pkg_aliases.linkage_upgrades,
        &pkg_aliases.aliases,
    )?;

    let mut maps = replay_support::build_replay_object_maps(&replay_state, &pkg_aliases.versions);
    replay_support::maybe_patch_replay_objects(
        &resolver,
        &replay_state,
        &pkg_aliases.versions,
        &pkg_aliases.aliases,
        &mut maps,
        verbose,
    );

    let config = replay_support::build_simulation_config(&replay_state);
    let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
    harness
        .set_address_aliases_with_versions(pkg_aliases.aliases.clone(), maps.versions_str.clone());
    if self_heal_dynamic_fields {
        let max_version = maps.version_map.values().copied().max().unwrap_or(0);
        let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
        if !modules.is_empty() {
            let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            enable_self_heal_fetchers(
                &mut harness,
                &graphql,
                replay_state.checkpoint,
                max_version,
                &pkg_aliases.aliases,
                &modules,
            );
        }
    }

    let mut replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &maps.cached_objects,
        &pkg_aliases.aliases,
        Some(&maps.versions_str),
        EffectsReconcilePolicy::Strict,
    );
    let mut synthetic_inputs = 0usize;
    if synthesize_missing
        && replay_result
            .as_ref()
            .map(|result| !result.result.local_success)
            .unwrap_or(true)
    {
        let missing =
            tx_replay::find_missing_input_objects(&replay_state.transaction, &maps.cached_objects);
        if !missing.is_empty() {
            let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
            let graphql = GraphQLClient::new(&graphql_endpoint);
            match synthesize_missing_inputs_py(
                &missing,
                &mut maps.cached_objects,
                &mut maps.version_map,
                &resolver,
                &pkg_aliases.aliases,
                &graphql,
                verbose,
            ) {
                Ok(logs) => {
                    synthetic_inputs = logs.len();
                    if verbose && synthetic_inputs > 0 {
                        eprintln!(
                            "[replay_fallback] synthesized {} missing input object(s)",
                            synthetic_inputs
                        );
                    }
                    if synthetic_inputs > 0 {
                        replay_result =
                            tx_replay::replay_with_version_tracking_with_policy_with_effects(
                                &replay_state.transaction,
                                &mut harness,
                                &maps.cached_objects,
                                &pkg_aliases.aliases,
                                Some(&maps.versions_str),
                                EffectsReconcilePolicy::Strict,
                            );
                    }
                }
                Err(err) => {
                    if verbose {
                        eprintln!("[replay_fallback] synthesis failed: {}", err);
                    }
                }
            }
        }
    }

    build_replay_output(
        &replay_state,
        replay_result,
        requested_source,
        effective_source,
        vm_only,
        allow_fallback,
        auto_system_objects,
        false,
        0,
        0,
        effective_source,
        0,
        synthetic_inputs,
        compare,
    )
}

fn load_replay_state_from_file(path: &Path, digest: Option<&str>) -> Result<ReplayState> {
    let states = parse_replay_states_file(path)?;
    if states.is_empty() {
        return Err(anyhow!(
            "Replay state file '{}' did not contain any states",
            path.display()
        ));
    }
    if states.len() == 1 {
        return Ok(states.into_iter().next().expect("single replay state"));
    }
    let digest = digest.ok_or_else(|| {
        anyhow!(
            "Replay state file '{}' contains multiple states; provide digest",
            path.display()
        )
    })?;
    states
        .into_iter()
        .find(|state| state.transaction.digest.0 == digest)
        .ok_or_else(|| {
            anyhow!(
                "Replay state file '{}' does not contain digest '{}'",
                path.display(),
                digest
            )
        })
}

fn import_state_inner(
    state: Option<&str>,
    transactions: Option<&str>,
    objects: Option<&str>,
    packages: Option<&str>,
    cache_dir: Option<&str>,
) -> Result<serde_json::Value> {
    let cache_dir = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_local_cache_dir);

    let spec = ImportSpec {
        state: state.map(PathBuf::from),
        transactions: transactions.map(PathBuf::from),
        objects: objects.map(PathBuf::from),
        packages: packages.map(PathBuf::from),
    };
    let summary = import_replay_states(&cache_dir, &spec)?;
    Ok(serde_json::json!({
        "cache_dir": summary.cache_dir,
        "states_imported": summary.states_imported,
        "objects_imported": summary.objects_imported,
        "packages_imported": summary.packages_imported,
        "digests": summary.digests,
    }))
}

fn deserialize_transaction_inner(raw_bcs: &[u8]) -> Result<serde_json::Value> {
    let decoded = bcs_codec::deserialize_transaction(raw_bcs, "decoded_tx", None, None, None)?;
    serde_json::to_value(decoded).context("Failed to serialize decoded transaction")
}

fn deserialize_package_inner(raw_bcs: &[u8]) -> Result<serde_json::Value> {
    let decoded = bcs_codec::deserialize_package(raw_bcs)?;
    serde_json::to_value(decoded).context("Failed to serialize decoded package")
}

/// Build JSON output for analyze-only mode (no VM execution).
fn build_analyze_output(
    replay_state: &sui_state_fetcher::ReplayState,
    source: &str,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    verbose: bool,
) -> Result<serde_json::Value> {
    let mut modules_total = 0usize;
    for pkg in replay_state.packages.values() {
        modules_total += pkg.modules.len();
    }

    let package_ids: Vec<String> = replay_state
        .packages
        .keys()
        .map(|id| id.to_hex_literal())
        .collect();
    let object_ids: Vec<String> = replay_state
        .objects
        .keys()
        .map(|id| id.to_hex_literal())
        .collect();

    // Summarize commands
    let command_summaries: Vec<serde_json::Value> = replay_state
        .transaction
        .commands
        .iter()
        .map(|cmd| {
            use sui_sandbox_types::PtbCommand;
            match cmd {
                PtbCommand::MoveCall {
                    package,
                    module,
                    function,
                    type_arguments,
                    arguments,
                } => serde_json::json!({
                    "kind": "MoveCall",
                    "target": format!("{}::{}::{}", package, module, function),
                    "type_args": type_arguments.len(),
                    "args": arguments.len(),
                }),
                PtbCommand::SplitCoins { amounts, .. } => serde_json::json!({
                    "kind": "SplitCoins",
                    "args": 1 + amounts.len(),
                }),
                PtbCommand::MergeCoins { sources, .. } => serde_json::json!({
                    "kind": "MergeCoins",
                    "args": 1 + sources.len(),
                }),
                PtbCommand::TransferObjects { objects, .. } => serde_json::json!({
                    "kind": "TransferObjects",
                    "args": 1 + objects.len(),
                }),
                PtbCommand::MakeMoveVec { elements, .. } => serde_json::json!({
                    "kind": "MakeMoveVec",
                    "args": elements.len(),
                }),
                PtbCommand::Publish { dependencies, .. } => serde_json::json!({
                    "kind": "Publish",
                    "args": dependencies.len(),
                }),
                PtbCommand::Upgrade { package, .. } => serde_json::json!({
                    "kind": "Upgrade",
                    "target": package,
                }),
            }
        })
        .collect();

    // Summarize inputs
    let mut pure = 0usize;
    let mut owned = 0usize;
    let mut shared_mutable = 0usize;
    let mut shared_immutable = 0usize;
    let mut immutable = 0usize;
    let mut receiving = 0usize;

    for input in &replay_state.transaction.inputs {
        use sui_sandbox_types::TransactionInput;
        match input {
            TransactionInput::Pure { .. } => pure += 1,
            TransactionInput::Object { .. } => owned += 1,
            TransactionInput::SharedObject { mutable, .. } => {
                if *mutable {
                    shared_mutable += 1;
                } else {
                    shared_immutable += 1;
                }
            }
            TransactionInput::ImmutableObject { .. } => immutable += 1,
            TransactionInput::Receiving { .. } => receiving += 1,
        }
    }

    let mut result = serde_json::json!({
        "digest": replay_state.transaction.digest.0,
        "sender": replay_state.transaction.sender.to_hex_literal(),
        "commands": replay_state.transaction.commands.len(),
        "inputs": replay_state.transaction.inputs.len(),
        "objects": replay_state.objects.len(),
        "packages": replay_state.packages.len(),
        "modules": modules_total,
        "input_summary": {
            "total": replay_state.transaction.inputs.len(),
            "pure": pure,
            "owned": owned,
            "shared_mutable": shared_mutable,
            "shared_immutable": shared_immutable,
            "immutable": immutable,
            "receiving": receiving,
        },
        "command_summaries": command_summaries,
        "hydration": {
            "source": source,
            "allow_fallback": allow_fallback,
            "auto_system_objects": auto_system_objects,
            "dynamic_field_prefetch": dynamic_field_prefetch,
            "prefetch_depth": prefetch_depth,
            "prefetch_limit": prefetch_limit,
        },
        "epoch": replay_state.epoch,
        "protocol_version": replay_state.protocol_version,
    });

    if let Some(cp) = replay_state.checkpoint {
        result["checkpoint"] = serde_json::json!(cp);
    }
    if let Some(rgp) = replay_state.reference_gas_price {
        result["reference_gas_price"] = serde_json::json!(rgp);
    }
    if verbose {
        result["package_ids"] = serde_json::json!(package_ids);
        result["object_ids"] = serde_json::json!(object_ids);
    }

    Ok(result)
}

/// Build envelope JSON for analyze-only replay mode.
///
/// This mirrors CLI replay shape (`local_success`, `execution_path`, `analysis`) while
/// retaining top-level summary keys for backwards compatibility.
fn build_analyze_replay_output(
    replay_state: &ReplayState,
    requested_source: &str,
    effective_source: &str,
    vm_only: bool,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    verbose: bool,
) -> Result<serde_json::Value> {
    let analysis = build_analyze_output(
        replay_state,
        effective_source,
        allow_fallback,
        auto_system_objects,
        dynamic_field_prefetch,
        prefetch_depth,
        prefetch_limit,
        verbose,
    )?;

    let execution_path = serde_json::json!({
        "requested_source": requested_source,
        "effective_source": effective_source,
        "vm_only": vm_only,
        "allow_fallback": allow_fallback,
        "auto_system_objects": auto_system_objects,
        "fallback_used": false,
        "dynamic_field_prefetch": dynamic_field_prefetch,
        "prefetch_depth": prefetch_depth,
        "prefetch_limit": prefetch_limit,
        "dependency_fetch_mode": "hydration_only",
        "dependency_packages_fetched": 0,
        "synthetic_inputs": 0,
    });

    let mut output = serde_json::json!({
        "digest": replay_state.transaction.digest.0,
        "local_success": true,
        "execution_path": execution_path,
        "analysis": analysis.clone(),
        "commands_executed": 0,
    });

    // Backwards-compatible access for existing scripts:
    // expose summary fields at the top level in addition to `analysis`.
    if let Some(summary) = analysis.as_object() {
        for (key, value) in summary {
            if output.get(key).is_none() {
                output[key] = value.clone();
            }
        }
    }

    Ok(output)
}

/// Build JSON output for full replay (VM execution results).
fn build_replay_diagnostics_py(replay_state: &ReplayState) -> Option<serde_json::Value> {
    use std::collections::BTreeSet;
    use sui_sandbox_types::TransactionInput;

    let mut missing_inputs = Vec::new();
    for input in &replay_state.transaction.inputs {
        let object_id = match input {
            TransactionInput::Object { object_id, .. } => Some(object_id),
            TransactionInput::SharedObject { object_id, .. } => Some(object_id),
            TransactionInput::ImmutableObject { object_id, .. } => Some(object_id),
            TransactionInput::Receiving { object_id, .. } => Some(object_id),
            TransactionInput::Pure { .. } => None,
        };
        if let Some(object_id) = object_id {
            if let Ok(address) = AccountAddress::from_hex_literal(object_id) {
                if !replay_state.objects.contains_key(&address) {
                    missing_inputs.push(address.to_hex_literal());
                }
            } else {
                missing_inputs.push(object_id.clone());
            }
        }
    }

    let mut required_packages: BTreeSet<AccountAddress> = BTreeSet::new();
    for cmd in &replay_state.transaction.commands {
        use sui_sandbox_types::PtbCommand;
        match cmd {
            PtbCommand::MoveCall {
                package,
                type_arguments,
                ..
            } => {
                if let Ok(address) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(address);
                }
                for ty in type_arguments {
                    for pkg in sui_sandbox_core::utilities::extract_package_ids_from_type(ty) {
                        if let Ok(address) = AccountAddress::from_hex_literal(&pkg) {
                            required_packages.insert(address);
                        }
                    }
                }
            }
            PtbCommand::Upgrade { package, .. } => {
                if let Ok(address) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(address);
                }
            }
            PtbCommand::Publish { dependencies, .. } => {
                for dep in dependencies {
                    if let Ok(address) = AccountAddress::from_hex_literal(dep) {
                        required_packages.insert(address);
                    }
                }
            }
            _ => {}
        }
    }
    let missing_packages = required_packages
        .into_iter()
        .filter(|address| !replay_state.packages.contains_key(address))
        .map(|address| address.to_hex_literal())
        .collect::<Vec<_>>();

    let mut suggestions = Vec::new();
    if !missing_inputs.is_empty() {
        suggestions.push(
            "Missing input objects detected; provide full object state via state_file or better hydration source."
                .to_string(),
        );
    }
    if !missing_packages.is_empty() {
        suggestions.push(
            "Missing package bytecode detected; prepare a package context and replay with context_path."
                .to_string(),
        );
    }

    if missing_inputs.is_empty() && missing_packages.is_empty() && suggestions.is_empty() {
        None
    } else {
        Some(serde_json::json!({
            "missing_input_objects": missing_inputs,
            "missing_packages": missing_packages,
            "suggestions": suggestions,
        }))
    }
}

fn build_replay_output(
    replay_state: &sui_state_fetcher::ReplayState,
    replay_result: Result<sui_sandbox_core::tx_replay::ReplayExecution>,
    requested_source: &str,
    effective_source: &str,
    vm_only: bool,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    dependency_fetch_mode: &str,
    dependency_packages_fetched: usize,
    synthetic_inputs: usize,
    compare: bool,
) -> Result<serde_json::Value> {
    let execution_path = serde_json::json!({
        "requested_source": requested_source,
        "effective_source": effective_source,
        "vm_only": vm_only,
        "allow_fallback": allow_fallback,
        "auto_system_objects": auto_system_objects,
        "fallback_used": false,
        "dynamic_field_prefetch": dynamic_field_prefetch,
        "prefetch_depth": prefetch_depth,
        "prefetch_limit": prefetch_limit,
        "dependency_fetch_mode": dependency_fetch_mode,
        "dependency_packages_fetched": dependency_packages_fetched,
        "synthetic_inputs": synthetic_inputs,
    });

    match replay_result {
        Ok(execution) => {
            let result = execution.result;
            let effects = &execution.effects;
            let diagnostics = if result.local_success {
                None
            } else {
                build_replay_diagnostics_py(replay_state)
            };

            let effects_summary = serde_json::json!({
                "success": effects.success,
                "error": effects.error,
                "gas_used": effects.gas_used,
                "created": effects.created.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "mutated": effects.mutated.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "deleted": effects.deleted.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "wrapped": effects.wrapped.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "unwrapped": effects.unwrapped.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "transferred": effects.transferred.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "received": effects.received.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "events_count": effects.events.len(),
                "failed_command_index": effects.failed_command_index,
                "failed_command_description": effects.failed_command_description,
                "commands_succeeded": effects.commands_succeeded,
                "return_values": effects.return_values.iter().map(|v| v.len()).collect::<Vec<_>>(),
            });

            let comparison = if compare {
                result.comparison.map(|c| {
                    serde_json::json!({
                        "status_match": c.status_match,
                        "created_match": c.created_count_match,
                        "mutated_match": c.mutated_count_match,
                        "deleted_match": c.deleted_count_match,
                        "on_chain_status": if c.status_match && result.local_success {
                            "success"
                        } else if c.status_match && !result.local_success {
                            "failed"
                        } else {
                            "unknown"
                        },
                        "local_status": if result.local_success { "success" } else { "failed" },
                        "notes": c.notes,
                    })
                })
            } else {
                None
            };

            let mut output = serde_json::json!({
                "digest": replay_state.transaction.digest.0,
                "local_success": result.local_success,
                "execution_path": execution_path,
                "effects": effects_summary,
                "commands_executed": result.commands_executed,
            });

            if let Some(err) = &result.local_error {
                output["local_error"] = serde_json::json!(err);
            }
            if let Some(diagnostics) = diagnostics {
                output["diagnostics"] = diagnostics;
            }
            if let Some(cmp) = comparison {
                output["comparison"] = cmp;
            }

            Ok(output)
        }
        Err(e) => {
            let mut output = serde_json::json!({
                "digest": replay_state.transaction.digest.0,
                "local_success": false,
                "local_error": e.to_string(),
                "execution_path": execution_path,
                "commands_executed": 0,
            });
            if let Some(diagnostics) = build_replay_diagnostics_py(replay_state) {
                output["diagnostics"] = diagnostics;
            }
            Ok(output)
        }
    }
}

// ---------------------------------------------------------------------------
// get_latest_checkpoint (native — Walrus)
// ---------------------------------------------------------------------------

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

fn parse_walrus_archive_network(network: &str) -> Result<CoreWalrusArchiveNetwork> {
    CoreWalrusArchiveNetwork::parse(network)
}

fn build_walrus_client(
    network: CoreWalrusArchiveNetwork,
    caching_url: Option<&str>,
    aggregator_url: Option<&str>,
) -> Result<WalrusClient> {
    core_build_walrus_client(network, caching_url, aggregator_url)
}

fn normalize_package_id(package: &str) -> Result<String> {
    core_normalize_package_id(package)
}

fn normalize_protocol_name(protocol: &str) -> Result<String> {
    let normalized = protocol.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "generic" | "deepbook" | "cetus" | "suilend" | "scallop" => Ok(normalized),
        _ => Err(anyhow!(
            "invalid protocol '{}': expected one of generic, deepbook, cetus, suilend, scallop",
            protocol
        )),
    }
}

fn protocol_default_package_id(protocol: &str) -> Option<&'static str> {
    match protocol {
        "deepbook" => Some(PROTOCOL_DEEPBOOK_MARGIN_PACKAGE),
        _ => None,
    }
}

fn resolve_protocol_package_id(protocol: &str, package_id: Option<&str>) -> Result<String> {
    let normalized = normalize_protocol_name(protocol)?;
    let raw = match package_id {
        Some(value) => value,
        None => protocol_default_package_id(&normalized).ok_or_else(|| {
            anyhow!(
                "protocol '{}' has no default package id; provide package_id",
                normalized
            )
        })?,
    };
    normalize_package_id(raw)
}

fn resolve_protocol_discovery_package_filter(
    protocol: &str,
    package_id: Option<&str>,
) -> Result<Option<String>> {
    if let Some(raw) = package_id {
        return normalize_package_id(raw).map(Some);
    }
    let normalized = normalize_protocol_name(protocol)?;
    if normalized == "generic" {
        return Ok(None);
    }
    protocol_default_package_id(&normalized)
        .map(normalize_package_id)
        .transpose()?
        .ok_or_else(|| {
            anyhow!(
                "protocol '{}' has no default package id; provide package_id",
                normalized
            )
        })
        .map(Some)
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

fn parse_workflow_template(template: &str) -> Result<BuiltinWorkflowTemplate> {
    match template.trim().to_ascii_lowercase().as_str() {
        "generic" => Ok(BuiltinWorkflowTemplate::Generic),
        "cetus" => Ok(BuiltinWorkflowTemplate::Cetus),
        "suilend" => Ok(BuiltinWorkflowTemplate::Suilend),
        "scallop" => Ok(BuiltinWorkflowTemplate::Scallop),
        other => Err(anyhow!(
            "invalid template '{}': expected one of generic, cetus, suilend, scallop",
            other
        )),
    }
}

fn parse_workflow_output_format(format: Option<&str>) -> Result<Option<WorkflowOutputFormat>> {
    let Some(format) = format else {
        return Ok(None);
    };
    match format.trim().to_ascii_lowercase().as_str() {
        "json" => Ok(Some(WorkflowOutputFormat::Json)),
        "yaml" | "yml" => Ok(Some(WorkflowOutputFormat::Yaml)),
        other => Err(anyhow!(
            "invalid format '{}': expected 'json' or 'yaml'",
            other
        )),
    }
}

fn short_package_id(package_id: &str) -> String {
    let trimmed = package_id.trim_start_matches("0x");
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.chars().take(12).collect()
    }
}

fn write_workflow_spec(
    path: &Path,
    spec: &WorkflowSpec,
    format: WorkflowOutputFormat,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create workflow output directory {}",
                    parent.display()
                )
            })?;
        }
    }
    let serialized = match format {
        WorkflowOutputFormat::Json => serde_json::to_string_pretty(spec)?,
        WorkflowOutputFormat::Yaml => serde_yaml::to_string(spec)?,
    };
    std::fs::write(path, serialized)
        .with_context(|| format!("Failed to write workflow spec {}", path.display()))?;
    Ok(())
}

fn probe_package_modules_for_workflow(package_id: &str) -> Result<(usize, Vec<String>)> {
    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
    let graphql = GraphQLClient::new(&graphql_endpoint);
    let modules = fetch_package_modules(&graphql, package_id)?;
    let names = modules
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    Ok((names.len(), names))
}

fn probe_dependency_closure_for_workflow(package_id: &str) -> Result<(usize, Vec<String>)> {
    let fetched = fetch_package_bytecodes_inner(package_id, true)?;
    let packages_value = fetched
        .get("packages")
        .ok_or_else(|| anyhow!("fetch package probe output missing `packages` field"))?;
    let decoded = decode_context_packages_value(packages_value)?;
    let unresolved = unresolved_package_dependencies_for_modules(
        decoded
            .iter()
            .map(|(id, pkg)| (*id, pkg.modules.clone()))
            .collect(),
    )?;
    Ok((
        decoded.len(),
        unresolved
            .into_iter()
            .map(|address| address.to_hex_literal())
            .collect(),
    ))
}

#[derive(Debug, Clone)]
struct WorkflowTemplateInference {
    template: BuiltinWorkflowTemplate,
    confidence: &'static str,
    source: &'static str,
    reason: Option<String>,
}

fn infer_workflow_template_from_modules(module_names: &[String]) -> WorkflowTemplateInference {
    if module_names.is_empty() {
        return WorkflowTemplateInference {
            template: BuiltinWorkflowTemplate::Generic,
            confidence: "low",
            source: "fallback",
            reason: Some("no module names available from package probe".to_string()),
        };
    }

    let cetus_keywords = ["cetus", "clmm", "dlmm", "pool_script", "position_manager"];
    let suilend_keywords = ["suilend", "lending", "reserve", "obligation", "liquidation"];
    let scallop_keywords = ["scallop", "scoin", "spool", "collateral", "market"];

    let mut cetus_score = 0usize;
    let mut suilend_score = 0usize;
    let mut scallop_score = 0usize;
    for name in module_names.iter().map(|value| value.to_ascii_lowercase()) {
        if cetus_keywords.iter().any(|kw| name.contains(kw)) {
            cetus_score += 1;
        }
        if suilend_keywords.iter().any(|kw| name.contains(kw)) {
            suilend_score += 1;
        }
        if scallop_keywords.iter().any(|kw| name.contains(kw)) {
            scallop_score += 1;
        }
    }

    let mut ranked = vec![
        (BuiltinWorkflowTemplate::Cetus, cetus_score),
        (BuiltinWorkflowTemplate::Suilend, suilend_score),
        (BuiltinWorkflowTemplate::Scallop, scallop_score),
        (BuiltinWorkflowTemplate::Generic, 0usize),
    ];
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let (top_template, top_score) = ranked[0];
    let second_score = ranked[1].1;

    if top_score == 0 {
        return WorkflowTemplateInference {
            template: BuiltinWorkflowTemplate::Generic,
            confidence: "low",
            source: "module_probe",
            reason: Some("no template keyword matches found in module names".to_string()),
        };
    }
    if top_score == second_score {
        return WorkflowTemplateInference {
            template: BuiltinWorkflowTemplate::Generic,
            confidence: "low",
            source: "module_probe",
            reason: Some(format!(
                "ambiguous module matches (cetus={}, suilend={}, scallop={})",
                cetus_score, suilend_score, scallop_score
            )),
        };
    }

    let confidence = if top_score >= 4 {
        "high"
    } else if top_score >= 2 {
        "medium"
    } else {
        "low"
    };
    WorkflowTemplateInference {
        template: top_template,
        confidence,
        source: "module_probe",
        reason: Some(format!(
            "module keyword matches: cetus={}, suilend={}, scallop={}",
            cetus_score, suilend_score, scallop_score
        )),
    }
}

#[derive(Debug, Clone)]
struct WorkflowDiscoveryTarget {
    digest: String,
    checkpoint: u64,
}

fn discover_latest_target_for_workflow(
    package_id: &str,
    latest: u64,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<WorkflowDiscoveryTarget> {
    if latest == 0 {
        return Err(anyhow!("discover_latest must be greater than zero"));
    }
    let discovered = discover_checkpoint_targets_inner(
        None,
        Some(latest),
        Some(package_id),
        false,
        1,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )?;
    let target = discovered
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .and_then(|targets| targets.first())
        .ok_or_else(|| {
            anyhow!(
                "no candidate transactions discovered for package {} in latest {} checkpoint(s)",
                package_id,
                latest
            )
        })?;
    let digest = target
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("discovery target missing digest"))?
        .to_string();
    let checkpoint = target
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("discovery target missing checkpoint"))?;
    Ok(WorkflowDiscoveryTarget { digest, checkpoint })
}

#[derive(Debug, Clone)]
struct WorkflowRunStepExecution {
    exit_code: i32,
    output: serde_json::Value,
}

fn workflow_step_kind(action: &WorkflowStepAction) -> &'static str {
    match action {
        WorkflowStepAction::Replay(_) => "replay",
        WorkflowStepAction::AnalyzeReplay(_) => "analyze_replay",
        WorkflowStepAction::Command(_) => "command",
    }
}

fn workflow_step_label(step: &WorkflowStep, index: usize) -> String {
    if let Some(id) = step.id.as_deref() {
        if !id.trim().is_empty() {
            return format!("{index}:{id}");
        }
    }
    if let Some(name) = step.name.as_deref() {
        if !name.trim().is_empty() {
            return format!("{index}:{name}");
        }
    }
    index.to_string()
}

fn workflow_first_nonempty_output_line(bytes: &[u8]) -> Option<String> {
    const MAX_LEN: usize = 240;
    let text = String::from_utf8_lossy(bytes);
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)?;

    if line.chars().count() > MAX_LEN {
        let truncated: String = line.chars().take(MAX_LEN).collect();
        return Some(format!("{truncated}..."));
    }
    Some(line)
}

fn workflow_summarize_failure_output(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    workflow_first_nonempty_output_line(stderr)
        .or_else(|| workflow_first_nonempty_output_line(stdout))
}

fn workflow_build_step_command(
    defaults: &WorkflowDefaults,
    step: &WorkflowStep,
) -> Result<Vec<String>> {
    match &step.action {
        WorkflowStepAction::Replay(replay) => Ok(workflow_build_replay_command(defaults, replay)),
        WorkflowStepAction::AnalyzeReplay(analyze) => {
            Ok(workflow_build_analyze_replay_command(defaults, analyze))
        }
        WorkflowStepAction::Command(command) => normalize_command_args(&command.args),
    }
}

fn workflow_build_replay_command(
    defaults: &WorkflowDefaults,
    replay: &WorkflowReplayStep,
) -> Vec<String> {
    ReplayOrchestrator::build_replay_command(defaults, replay)
}

fn workflow_build_analyze_replay_command(
    defaults: &WorkflowDefaults,
    analyze: &WorkflowAnalyzeReplayStep,
) -> Vec<String> {
    ReplayOrchestrator::build_analyze_replay_command(defaults, analyze)
}

fn workflow_discover_target_for_replay(
    checkpoint: Option<&str>,
    latest: Option<u64>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<WorkflowDiscoveryTarget> {
    let discovered = discover_checkpoint_targets_inner(
        checkpoint,
        latest,
        None,
        false,
        1,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )?;
    let target = discovered
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .and_then(|targets| targets.first())
        .ok_or_else(|| anyhow!("workflow replay step discovery returned no targets"))?;
    let digest = target
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("workflow replay step discovery target missing digest"))?
        .to_string();
    let checkpoint = target
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("workflow replay step discovery target missing checkpoint"))?;
    Ok(WorkflowDiscoveryTarget { digest, checkpoint })
}

fn parse_inline_workflow_spec(py: Python<'_>, spec_obj: &Bound<'_, PyAny>) -> Result<WorkflowSpec> {
    let json_mod = py.import("json")?;
    let dumped = json_mod
        .call_method1("dumps", (spec_obj,))
        .context("failed to serialize inline workflow spec to JSON")?
        .extract::<String>()
        .context("failed to extract inline workflow JSON string")?;
    let spec: WorkflowSpec =
        serde_json::from_str(&dumped).context("invalid inline workflow spec JSON payload")?;
    spec.validate()?;
    Ok(spec)
}

fn workflow_parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    for (idx, arg) in args.iter().enumerate() {
        if arg == flag {
            return args.get(idx + 1).cloned();
        }
        let prefix = format!("{flag}=");
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

fn workflow_extract_interface_module_names(interface: &serde_json::Value) -> Vec<String> {
    let mut names = interface
        .get("modules")
        .and_then(serde_json::Value::as_object)
        .map(|modules| modules.keys().cloned().collect::<Vec<_>>())
        .or_else(|| {
            interface
                .as_object()
                .map(|modules| modules.keys().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn workflow_has_comparison_mismatch(replay_output: &serde_json::Value) -> bool {
    let Some(comparison) = replay_output.get("comparison") else {
        return false;
    };
    let read = |key: &str| comparison.get(key).and_then(serde_json::Value::as_bool);
    [
        read("status_match"),
        read("created_match"),
        read("mutated_match"),
        read("deleted_match"),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value)
}

struct WorkflowEnvGuard {
    previous: Vec<(String, Option<String>)>,
}

impl Drop for WorkflowEnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

fn workflow_profile_env_defaults(
    profile: WorkflowReplayProfile,
) -> &'static [(&'static str, &'static str)] {
    match profile {
        WorkflowReplayProfile::Safe => &[
            ("SUI_CHECKPOINT_LOOKUP_GRAPHQL", "1"),
            ("SUI_PACKAGE_LOOKUP_GRAPHQL", "1"),
            ("SUI_OBJECT_FETCH_CONCURRENCY", "8"),
            ("SUI_PACKAGE_FETCH_CONCURRENCY", "4"),
            ("SUI_PACKAGE_FETCH_PARALLEL", "1"),
        ],
        WorkflowReplayProfile::Balanced => &[],
        WorkflowReplayProfile::Fast => &[
            ("SUI_CHECKPOINT_LOOKUP_GRAPHQL", "0"),
            ("SUI_PACKAGE_LOOKUP_GRAPHQL", "0"),
            ("SUI_OBJECT_FETCH_CONCURRENCY", "32"),
            ("SUI_PACKAGE_FETCH_CONCURRENCY", "16"),
            ("SUI_PACKAGE_FETCH_PARALLEL", "1"),
        ],
    }
}

fn workflow_apply_profile_env(profile: WorkflowReplayProfile) -> WorkflowEnvGuard {
    let mut previous = Vec::new();
    for (key, value) in workflow_profile_env_defaults(profile) {
        if std::env::var(key).is_err() {
            previous.push(((*key).to_string(), None));
            std::env::set_var(key, value);
        }
    }
    WorkflowEnvGuard { previous }
}

fn parse_replay_profile(value: Option<&str>) -> Result<WorkflowReplayProfile> {
    let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(WorkflowReplayProfile::Balanced);
    };
    match raw.to_ascii_lowercase().as_str() {
        "safe" => Ok(WorkflowReplayProfile::Safe),
        "balanced" => Ok(WorkflowReplayProfile::Balanced),
        "fast" => Ok(WorkflowReplayProfile::Fast),
        other => Err(anyhow!(
            "invalid profile `{}` (expected one of: safe, balanced, fast)",
            other
        )),
    }
}

fn parse_replay_fetch_strategy(value: Option<&str>) -> Result<WorkflowFetchStrategy> {
    let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(WorkflowFetchStrategy::Full);
    };
    match raw.to_ascii_lowercase().as_str() {
        "eager" => Ok(WorkflowFetchStrategy::Eager),
        "full" => Ok(WorkflowFetchStrategy::Full),
        other => Err(anyhow!(
            "invalid fetch_strategy `{}` (expected one of: eager, full)",
            other
        )),
    }
}

fn workflow_execute_command_step(
    command: &WorkflowCommandStep,
    rpc_url: &str,
) -> Result<WorkflowRunStepExecution> {
    let normalized = normalize_command_args(&command.args)?;
    let Some(program) = normalized.first() else {
        return Err(anyhow!("command step args cannot be empty"));
    };

    if program == "status" {
        return Ok(WorkflowRunStepExecution {
            exit_code: 0,
            output: serde_json::json!({
                "success": true,
                "mode": "python_native",
                "status": "ready",
            }),
        });
    }

    if program == "analyze" && normalized.get(1).is_some_and(|value| value == "package") {
        let package_id = workflow_parse_flag_value(&normalized, "--package-id")
            .ok_or_else(|| anyhow!("`analyze package` requires --package-id"))?;
        let interface = extract_interface_inner(Some(&package_id), None, rpc_url)?;
        let module_names = workflow_extract_interface_module_names(&interface);
        let list_modules = normalized.iter().any(|value| value == "--list-modules");
        return Ok(WorkflowRunStepExecution {
            exit_code: 0,
            output: serde_json::json!({
                "success": true,
                "package_id": package_id,
                "modules": module_names.len(),
                "module_names": if list_modules { Some(module_names) } else { None },
            }),
        });
    }

    if program == "view" && normalized.get(1).is_some_and(|value| value == "object") {
        let object_id = normalized
            .get(2)
            .cloned()
            .ok_or_else(|| anyhow!("`view object` requires an object id argument"))?;
        let version = workflow_parse_flag_value(&normalized, "--version")
            .map(|raw| raw.parse::<u64>())
            .transpose()
            .map_err(|_| anyhow!("`view object --version` must be a u64"))?;
        let object = fetch_object_bcs_inner(&object_id, version, None, None)?;
        let bcs_bytes = object
            .get("bcs_base64")
            .and_then(serde_json::Value::as_str)
            .map(|value| value.len())
            .unwrap_or(0);
        return Ok(WorkflowRunStepExecution {
            exit_code: 0,
            output: serde_json::json!({
                "success": true,
                "object_id": object_id,
                "version": object.get("version").cloned().unwrap_or(serde_json::Value::Null),
                "type_tag": object.get("type_tag").cloned().unwrap_or(serde_json::Value::Null),
                "bcs_base64_len": bcs_bytes,
            }),
        });
    }

    let output = Command::new(program)
        .args(&normalized[1..])
        .output()
        .with_context(|| format!("failed to execute command step program `{}`", program))?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();
    let mut payload = serde_json::json!({
        "success": success,
        "program": program,
        "stdout": stdout,
        "stderr": stderr,
    });
    if !success {
        let summary = workflow_summarize_failure_output(&output.stdout, &output.stderr)
            .unwrap_or_else(|| {
                format!("command `{}` failed with exit code {}", program, exit_code)
            });
        if let Some(object) = payload.as_object_mut() {
            object.insert("error".to_string(), serde_json::json!(summary));
        }
    }

    Ok(WorkflowRunStepExecution {
        exit_code,
        output: payload,
    })
}

fn workflow_execute_replay_step(
    defaults: &WorkflowDefaults,
    replay: &WorkflowReplayStep,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> Result<WorkflowRunStepExecution> {
    let profile = replay
        .profile
        .or(defaults.profile)
        .unwrap_or(WorkflowReplayProfile::Balanced);
    let _profile_env = workflow_apply_profile_env(profile);
    let fetch_strategy = replay
        .fetch_strategy
        .or(defaults.fetch_strategy)
        .unwrap_or(WorkflowFetchStrategy::Full);
    let vm_only = replay.vm_only.or(defaults.vm_only).unwrap_or(false);
    let synthesize_missing = replay
        .synthesize_missing
        .or(defaults.synthesize_missing)
        .unwrap_or(false);
    let self_heal_dynamic_fields = replay
        .self_heal_dynamic_fields
        .or(defaults.self_heal_dynamic_fields)
        .unwrap_or(false);

    let source = replay
        .source
        .or(defaults.source)
        .unwrap_or(WorkflowSource::Hybrid);

    let mut allow_fallback = replay
        .allow_fallback
        .or(defaults.allow_fallback)
        .unwrap_or(true);
    if vm_only {
        allow_fallback = false;
    }
    let auto_system_objects = replay
        .auto_system_objects
        .or(defaults.auto_system_objects)
        .unwrap_or(true);
    let no_prefetch_requested = replay.no_prefetch.or(defaults.no_prefetch).unwrap_or(false);
    let no_prefetch = no_prefetch_requested || fetch_strategy == WorkflowFetchStrategy::Eager;
    let prefetch_depth = replay
        .prefetch_depth
        .or(defaults.prefetch_depth)
        .unwrap_or(3);
    let prefetch_limit = replay
        .prefetch_limit
        .or(defaults.prefetch_limit)
        .unwrap_or(200);
    let compare = replay.compare.or(defaults.compare).unwrap_or(false);
    let strict = replay.strict.or(defaults.strict).unwrap_or(false);

    let mut digest = replay
        .digest
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if replay.latest.is_some() && digest.is_some() {
        return Err(anyhow!(
            "workflow replay cannot combine `digest` and `latest` in python native mode"
        ));
    }

    let mut checkpoint = None;
    if let Some(checkpoint_raw) = replay
        .checkpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(parsed) = checkpoint_raw.parse::<u64>() {
            checkpoint = Some(parsed);
        } else if digest.is_some() {
            return Err(anyhow!(
                "workflow replay checkpoint `{}` must be numeric when digest is provided in python native mode",
                checkpoint_raw
            ));
        } else {
            let discovered = workflow_discover_target_for_replay(
                Some(checkpoint_raw),
                None,
                walrus_network,
                walrus_caching_url,
                walrus_aggregator_url,
            )?;
            digest = Some(discovered.digest);
            checkpoint = Some(discovered.checkpoint);
        }
    }
    if let Some(latest) = replay.latest {
        if latest == 0 {
            return Err(anyhow!("workflow replay latest must be >= 1"));
        }
        let discovered = workflow_discover_target_for_replay(
            None,
            Some(latest),
            walrus_network,
            walrus_caching_url,
            walrus_aggregator_url,
        )?;
        digest = Some(discovered.digest);
        checkpoint = Some(discovered.checkpoint);
    }
    if digest.is_none() && replay.state_json.is_none() {
        return Err(anyhow!(
            "workflow replay requires digest or state_json in python native mode"
        ));
    }

    let source_str = source.as_cli_value();
    let mut output = if let Some(state_json) = replay.state_json.as_ref() {
        let replay_state = load_replay_state_from_file(state_json, digest.as_deref())?;
        replay_loaded_state_inner(
            replay_state,
            source_str,
            "state_json",
            None,
            allow_fallback,
            auto_system_objects,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            false,
            synthesize_missing,
            false,
            rpc_url,
            verbose,
        )?
    } else if source == WorkflowSource::Local {
        let digest = digest
            .as_deref()
            .ok_or_else(|| anyhow!("workflow replay missing digest for local source"))?;
        let cache_dir = default_local_cache_dir();
        let provider = FileStateProvider::new(&cache_dir).with_context(|| {
            format!(
                "failed to open workflow local replay cache {}",
                cache_dir.display()
            )
        })?;
        let replay_state = provider.get_state(digest)?;
        replay_loaded_state_inner(
            replay_state,
            source_str,
            "local_cache",
            None,
            allow_fallback,
            auto_system_objects,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            false,
            synthesize_missing,
            false,
            rpc_url,
            verbose,
        )?
    } else {
        replay_inner(
            digest
                .as_deref()
                .ok_or_else(|| anyhow!("workflow replay missing digest"))?,
            rpc_url,
            source_str,
            checkpoint,
            None,
            allow_fallback,
            prefetch_depth,
            prefetch_limit,
            auto_system_objects,
            no_prefetch,
            synthesize_missing,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            false,
            false,
            verbose,
        )?
    };

    let local_success = output
        .get("local_success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut exit_code = if local_success { 0 } else { 1 };
    if strict && local_success && compare && workflow_has_comparison_mismatch(&output) {
        exit_code = 1;
        if let Some(object) = output.as_object_mut() {
            object.insert(
                "strict_error".to_string(),
                serde_json::json!("comparison mismatch under strict replay"),
            );
        }
    }
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "workflow_source".to_string(),
            serde_json::json!(source.as_cli_value()),
        );
        object.insert(
            "workflow_profile".to_string(),
            serde_json::json!(profile.as_cli_value()),
        );
        object.insert(
            "workflow_fetch_strategy".to_string(),
            serde_json::json!(fetch_strategy.as_cli_value()),
        );
    }
    if let Some(execution_path) = output
        .get_mut("execution_path")
        .and_then(serde_json::Value::as_object_mut)
    {
        execution_path.insert("vm_only".to_string(), serde_json::json!(vm_only));
        execution_path.insert(
            "allow_fallback".to_string(),
            serde_json::json!(allow_fallback),
        );
        execution_path.insert(
            "dynamic_field_prefetch".to_string(),
            serde_json::json!(!no_prefetch),
        );
        execution_path.insert(
            "self_heal_dynamic_fields".to_string(),
            serde_json::json!(self_heal_dynamic_fields),
        );
    }

    Ok(WorkflowRunStepExecution { exit_code, output })
}

fn workflow_execute_analyze_replay_step(
    defaults: &WorkflowDefaults,
    analyze: &WorkflowAnalyzeReplayStep,
    rpc_url: &str,
    verbose: bool,
) -> Result<WorkflowRunStepExecution> {
    let mm2_enabled = analyze.mm2.or(defaults.mm2).unwrap_or(false);
    let digest = analyze.digest.trim();
    if digest.is_empty() {
        return Err(anyhow!("workflow analyze_replay digest cannot be empty"));
    }
    let profile = defaults.profile.unwrap_or(WorkflowReplayProfile::Balanced);
    let _profile_env = workflow_apply_profile_env(profile);
    let source = analyze
        .source
        .or(defaults.source)
        .unwrap_or(WorkflowSource::Hybrid);
    let allow_fallback = analyze
        .allow_fallback
        .or(defaults.allow_fallback)
        .unwrap_or(true);
    let auto_system_objects = analyze
        .auto_system_objects
        .or(defaults.auto_system_objects)
        .unwrap_or(true);
    let no_prefetch = analyze
        .no_prefetch
        .or(defaults.no_prefetch)
        .unwrap_or(false);
    let prefetch_depth = analyze
        .prefetch_depth
        .or(defaults.prefetch_depth)
        .unwrap_or(3);
    let prefetch_limit = analyze
        .prefetch_limit
        .or(defaults.prefetch_limit)
        .unwrap_or(200);
    let mut output = if source == WorkflowSource::Local {
        let cache_dir = default_local_cache_dir();
        let provider = FileStateProvider::new(&cache_dir).with_context(|| {
            format!(
                "failed to open workflow local replay cache {}",
                cache_dir.display()
            )
        })?;
        let replay_state = provider.get_state(digest)?;
        replay_loaded_state_inner(
            replay_state,
            source.as_cli_value(),
            "local_cache",
            None,
            allow_fallback,
            auto_system_objects,
            false,
            false,
            false,
            true,
            false,
            mm2_enabled,
            rpc_url,
            verbose,
        )?
    } else {
        replay_inner(
            digest,
            rpc_url,
            source.as_cli_value(),
            analyze.checkpoint,
            None,
            allow_fallback,
            prefetch_depth,
            prefetch_limit,
            auto_system_objects,
            no_prefetch,
            false,
            false,
            false,
            false,
            true,
            mm2_enabled,
            verbose,
        )?
    };
    let local_success = output
        .get("local_success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let exit_code = if local_success { 0 } else { 1 };
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "workflow_source".to_string(),
            serde_json::json!(source.as_cli_value()),
        );
        object.insert(
            "workflow_profile".to_string(),
            serde_json::json!(profile.as_cli_value()),
        );
    }
    Ok(WorkflowRunStepExecution { exit_code, output })
}

fn write_workflow_run_report(path: &Path, report: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create workflow report output directory {}",
                    parent.display()
                )
            })?;
        }
    }
    let payload = serde_json::to_string_pretty(report)?;
    std::fs::write(path, payload)
        .with_context(|| format!("Failed to write workflow report {}", path.display()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn workflow_run_spec_inner(
    spec: WorkflowSpec,
    spec_label: String,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<String>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> Result<serde_json::Value> {
    let prepared_steps = spec
        .steps
        .iter()
        .enumerate()
        .map(|(idx, step)| WorkflowPreparedStep {
            index: idx + 1,
            id: step.id.clone(),
            name: step.name.clone(),
            kind: workflow_step_kind(&step.action).to_string(),
            continue_on_error: step.continue_on_error,
            command: workflow_build_step_command(&spec.defaults, step)
                .map_err(|err| err.to_string()),
        })
        .collect::<Vec<_>>();

    let report_struct = run_prepared_workflow_steps(
        spec_label,
        &spec,
        prepared_steps,
        dry_run,
        continue_on_error,
        |step, prepared| {
            if verbose {
                eprintln!(
                    "[workflow:{}] {}",
                    workflow_step_label(step, prepared.index),
                    prepared.command_display()
                );
            }
        },
        |step, _prepared| {
            let step_output = match &step.action {
                WorkflowStepAction::Replay(replay) => workflow_execute_replay_step(
                    &spec.defaults,
                    replay,
                    rpc_url,
                    walrus_network,
                    walrus_caching_url,
                    walrus_aggregator_url,
                    verbose,
                )?,
                WorkflowStepAction::AnalyzeReplay(analyze) => {
                    workflow_execute_analyze_replay_step(&spec.defaults, analyze, rpc_url, verbose)?
                }
                WorkflowStepAction::Command(command_step) => {
                    workflow_execute_command_step(command_step, rpc_url)?
                }
            };

            let error = if step_output.exit_code == 0 {
                None
            } else {
                step_output
                    .output
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        step_output
                            .output
                            .get("local_error")
                            .and_then(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                    })
            };

            Ok(WorkflowStepExecution {
                exit_code: step_output.exit_code,
                output: Some(step_output.output),
                error,
            })
        },
    );

    let mut report = serde_json::to_value(&report_struct)?;

    if let Some(path) = report_path.as_deref() {
        let report_path = PathBuf::from(path);
        write_workflow_run_report(&report_path, &report)?;
        if let Some(object) = report.as_object_mut() {
            object.insert(
                "report_file".to_string(),
                serde_json::json!(report_path.display().to_string()),
            );
        }
    }

    Ok(report)
}

fn discover_checkpoint_targets_inner(
    checkpoint: Option<&str>,
    latest: Option<u64>,
    package_id: Option<&str>,
    include_framework: bool,
    limit: usize,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<serde_json::Value> {
    let network = parse_walrus_archive_network(walrus_network)?;
    let walrus = build_walrus_client(network, walrus_caching_url, walrus_aggregator_url)?;
    let output = core_discover_checkpoint_targets(
        &walrus,
        checkpoint,
        latest,
        package_id,
        include_framework,
        limit,
    )?;
    serde_json::to_value(output).context("failed to serialize checkpoint discovery output")
}

fn resolve_replay_target_from_discovery(
    digest: Option<&str>,
    checkpoint: Option<u64>,
    state_file: Option<&str>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<(Option<String>, Option<u64>)> {
    let network = parse_walrus_archive_network(walrus_network)?;
    let walrus = build_walrus_client(network, walrus_caching_url, walrus_aggregator_url)?;
    core_resolve_replay_target_from_discovery(
        digest,
        checkpoint,
        state_file.is_some(),
        discover_latest,
        discover_package_id,
        &walrus,
    )
}

fn resolve_grpc_endpoint_and_key(
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> (String, Option<String>) {
    const MAINNET_ARCHIVE_GRPC: &str = "https://archive.mainnet.sui.io:443";
    let endpoint_explicit = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some();
    let (default_endpoint, default_api_key) = historical_endpoint_and_api_key_from_env();
    let mut resolved_endpoint = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or(default_endpoint);
    if resolved_endpoint
        .to_ascii_lowercase()
        .contains("fullnode.mainnet.sui.io")
    {
        resolved_endpoint = MAINNET_ARCHIVE_GRPC.to_string();
    }
    let explicit_api_key = api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let sui_api_key = std::env::var("SUI_GRPC_API_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let resolved_api_key = if endpoint_explicit {
        // Keep explicit endpoint behavior predictable:
        // explicit arg > SUI_GRPC_API_KEY > no key.
        explicit_api_key.or(sui_api_key)
    } else {
        explicit_api_key.or(default_api_key)
    };
    (resolved_endpoint, resolved_api_key)
}

fn load_deepbook_versions_snapshot(path: &Path) -> Result<(u64, HashMap<String, u64>)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read DeepBook versions file {}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid JSON in versions file {}", path.display()))?;

    let checkpoint = json
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("versions file missing numeric `checkpoint`"))?;
    let objects = json
        .get("objects")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("versions file missing `objects` map"))?;

    let mut versions = HashMap::new();
    for (object_id, meta) in objects {
        let version = meta
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow!("object {} missing numeric `version`", object_id))?;
        versions.insert(object_id.to_string(), version);
    }

    Ok((checkpoint, versions))
}

fn parse_string_map_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<HashMap<String, String>> {
    let Some(value) = payload.get(field) else {
        return Ok(HashMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("historical package payload `{}` must be a map", field))?;
    let mut out = HashMap::new();
    for (k, v) in object {
        let val = v.as_str().ok_or_else(|| {
            anyhow!(
                "historical package payload `{}` entry `{}` is not a string",
                field,
                k
            )
        })?;
        out.insert(k.clone(), val.to_string());
    }
    Ok(out)
}

fn parse_u64_map_field(payload: &serde_json::Value, field: &str) -> Result<HashMap<String, u64>> {
    let Some(value) = payload.get(field) else {
        return Ok(HashMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("historical package payload `{}` must be a map", field))?;
    let mut out = HashMap::new();
    for (k, v) in object {
        let val = v.as_u64().ok_or_else(|| {
            anyhow!(
                "historical package payload `{}` entry `{}` is not a u64",
                field,
                k
            )
        })?;
        out.insert(k.clone(), val);
    }
    Ok(out)
}

fn parse_nested_string_map_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<HashMap<String, HashMap<String, String>>> {
    let Some(value) = payload.get(field) else {
        return Ok(HashMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("historical package payload `{}` must be a map", field))?;
    let mut out = HashMap::new();
    for (k, v) in object {
        let nested_obj = v.as_object().ok_or_else(|| {
            anyhow!(
                "historical package payload `{}` entry `{}` is not a map",
                field,
                k
            )
        })?;
        let mut nested = HashMap::new();
        for (nk, nv) in nested_obj {
            let nval = nv.as_str().ok_or_else(|| {
                anyhow!(
                    "historical package payload `{}` entry `{}.{}` is not a string",
                    field,
                    k,
                    nk
                )
            })?;
            nested.insert(nk.clone(), nval.to_string());
        }
        out.insert(k.clone(), nested);
    }
    Ok(out)
}

#[allow(clippy::type_complexity)]
fn parse_historical_package_payload(
    payload: &serde_json::Value,
) -> Result<(
    HashMap<String, Vec<Vec<u8>>>,
    HashMap<String, String>,
    HashMap<String, String>,
    HashMap<String, String>,
    HashMap<String, HashMap<String, String>>,
    HashMap<String, u64>,
)> {
    let packages_obj = payload
        .get("packages")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("historical package payload missing `packages` map"))?;
    let mut package_bytecodes: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    for (package_id, modules) in packages_obj {
        let modules_array = modules.as_array().ok_or_else(|| {
            anyhow!(
                "historical package `{}` modules is not an array",
                package_id
            )
        })?;
        let mut decoded_modules = Vec::with_capacity(modules_array.len());
        for (idx, module_b64) in modules_array.iter().enumerate() {
            let encoded = module_b64.as_str().ok_or_else(|| {
                anyhow!(
                    "historical package `{}` module {} is not base64 string",
                    package_id,
                    idx
                )
            })?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .with_context(|| {
                    format!(
                        "invalid base64 module payload for package {} module {}",
                        package_id, idx
                    )
                })?;
            decoded_modules.push(bytes);
        }
        package_bytecodes.insert(package_id.clone(), decoded_modules);
    }

    let aliases = parse_string_map_field(payload, "aliases")?;
    let linkage_upgrades = parse_string_map_field(payload, "linkage_upgrades")?;
    let package_runtime_ids = parse_string_map_field(payload, "package_runtime_ids")?;
    let package_linkage = parse_nested_string_map_field(payload, "package_linkage")?;
    let package_versions = parse_u64_map_field(payload, "package_versions")?;

    Ok((
        package_bytecodes,
        aliases,
        linkage_upgrades,
        package_runtime_ids,
        package_linkage,
        package_versions,
    ))
}

fn decode_u64_le(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    if bytes.len() < 8 {
        return 0;
    }
    buf.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(buf)
}

fn decode_deepbook_margin_state(result: &serde_json::Value) -> Result<Option<serde_json::Value>> {
    if !result
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let return_values = match result
        .get("return_values")
        .and_then(serde_json::Value::as_array)
    {
        Some(values) => values,
        None => return Ok(None),
    };
    let Some(first_command_values) = return_values.first().and_then(serde_json::Value::as_array)
    else {
        return Ok(None);
    };
    if first_command_values.len() < 12 {
        return Ok(None);
    }

    let mut decoded = Vec::with_capacity(first_command_values.len());
    for (idx, value) in first_command_values.iter().enumerate() {
        let encoded = value
            .as_str()
            .ok_or_else(|| anyhow!("manager_state return value {} is not base64 string", idx))?;
        decoded.push(
            base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .with_context(|| format!("invalid base64 manager_state return value {}", idx))?,
        );
    }

    let risk_ratio = decode_u64_le(&decoded[2]);
    let base_asset = decode_u64_le(&decoded[3]);
    let quote_asset = decode_u64_le(&decoded[4]);
    let base_debt = decode_u64_le(&decoded[5]);
    let quote_debt = decode_u64_le(&decoded[6]);
    let current_price = decode_u64_le(&decoded[11]);

    Ok(Some(serde_json::json!({
        "risk_ratio_pct": risk_ratio as f64 / 1e9_f64 * 100.0_f64,
        "base_asset_sui": base_asset as f64 / 1e9_f64,
        "quote_asset_usdc": quote_asset as f64 / 1e6_f64,
        "base_debt_sui": base_debt as f64 / 1e9_f64,
        "quote_debt_usdc": quote_debt as f64 / 1e6_f64,
        "current_price": current_price as f64 / 1e6_f64,
    })))
}

fn deepbook_archive_hint(error: &str, endpoint: &str) -> Option<String> {
    let lower = error.to_ascii_lowercase();
    let looks_like_archive_gap = (lower.contains("contractabort") && lower.contains("abort_code"))
        || (lower.contains("major_status: aborted") && lower.contains("dynamic_field"));
    if !looks_like_archive_gap {
        return None;
    }

    let endpoint_lower = endpoint.to_ascii_lowercase();
    if endpoint_lower.contains("archive.mainnet.sui.io")
        || endpoint_lower.contains("fullnode.mainnet.sui.io")
    {
        return Some(
            "Likely archive runtime-object gap; retry with \
SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443"
                .to_string(),
        );
    }
    None
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

    let mut payload = serde_json::json!({
        "version": 1,
        "package_id": package_id,
        "resolve_deps": resolve_deps,
        "generated_at_ms": generated_at_ms,
        "packages": fetched.get("packages").cloned().unwrap_or_else(|| serde_json::json!({})),
        "count": fetched.get("count").cloned().unwrap_or_else(|| serde_json::json!(0)),
    });

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
/// Applies protocol package defaults when available, with optional override.
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

/// Validate a typed workflow spec (JSON or YAML) and return step counts.
#[pyfunction]
fn workflow_validate(py: Python<'_>, spec_path: &str) -> PyResult<PyObject> {
    let spec_path_owned = spec_path.to_string();
    let value = py
        .allow_threads(move || {
            let path = PathBuf::from(&spec_path_owned);
            let spec = WorkflowSpec::load_from_path(&path)?;
            let mut replay_steps = 0usize;
            let mut analyze_replay_steps = 0usize;
            let mut command_steps = 0usize;
            for step in &spec.steps {
                match step.action {
                    WorkflowStepAction::Replay(_) => replay_steps += 1,
                    WorkflowStepAction::AnalyzeReplay(_) => analyze_replay_steps += 1,
                    WorkflowStepAction::Command(_) => command_steps += 1,
                }
            }

            Ok(serde_json::json!({
                "spec_file": path.display().to_string(),
                "version": spec.version,
                "name": spec.name,
                "steps": spec.steps.len(),
                "replay_steps": replay_steps,
                "analyze_replay_steps": analyze_replay_steps,
                "command_steps": command_steps,
            }))
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Generate a typed workflow spec from a built-in template.
#[pyfunction]
#[pyo3(signature = (
    *,
    template="generic",
    output_path=None,
    format=None,
    digest=None,
    checkpoint=None,
    include_analyze_step=true,
    strict_replay=true,
    name=None,
    package_id=None,
    view_objects=vec![],
    force=false,
))]
fn workflow_init(
    py: Python<'_>,
    template: &str,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    include_analyze_step: bool,
    strict_replay: bool,
    name: Option<&str>,
    package_id: Option<&str>,
    view_objects: Vec<String>,
    force: bool,
) -> PyResult<PyObject> {
    let template_owned = template.to_string();
    let output_path_owned = output_path.map(ToOwned::to_owned);
    let format_owned = format.map(ToOwned::to_owned);
    let digest_owned = digest.map(ToOwned::to_owned);
    let name_owned = name.map(ToOwned::to_owned);
    let package_id_owned = package_id.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let template = parse_workflow_template(&template_owned)?;
            let digest = digest_owned
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| template.default_digest().to_string());
            let checkpoint = checkpoint.unwrap_or(template.default_checkpoint());

            let mut spec = build_builtin_workflow(
                template,
                &BuiltinWorkflowInput {
                    digest: Some(digest.clone()),
                    checkpoint: Some(checkpoint),
                    include_analyze_step,
                    include_replay_step: true,
                    strict_replay,
                    package_id: package_id_owned
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    view_objects: view_objects.clone(),
                },
            )?;
            if let Some(name) = name_owned
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                spec.name = Some(name.to_string());
            }
            spec.validate()?;

            let parsed_format = parse_workflow_output_format(format_owned.as_deref())?;
            let format_hint = parsed_format.unwrap_or(WorkflowOutputFormat::Json);
            let output_path = output_path_owned
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "workflow.{}.{}",
                        template.key(),
                        format_hint.extension()
                    ))
                });
            let output_format = parsed_format
                .or_else(|| WorkflowOutputFormat::from_path(&output_path))
                .unwrap_or(format_hint);

            if output_path.exists() && !force {
                return Err(anyhow!(
                    "Refusing to overwrite existing workflow spec at {} (pass force=True)",
                    output_path.display()
                ));
            }

            write_workflow_spec(&output_path, &spec, output_format)?;

            Ok(serde_json::json!({
                "template": template.key(),
                "output_file": output_path.display().to_string(),
                "format": output_format.as_str(),
                "digest": digest,
                "checkpoint": checkpoint,
                "include_analyze_step": include_analyze_step,
                "strict_replay": strict_replay,
                "package_id": package_id_owned,
                "view_objects": view_objects.len(),
                "workflow_name": spec.name,
                "steps": spec.steps.len(),
            }))
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Auto-generate a draft adapter workflow from a package id.
///
/// This mirrors CLI `workflow auto` behavior:
/// - dependency closure validation (fail closed unless `best_effort=True`)
/// - template inference from module names (or explicit override)
/// - scaffold-only output when replay seed is unavailable
/// - replay-capable output with `digest` or `discover_latest`
#[pyfunction]
#[pyo3(signature = (
    package_id,
    *,
    template=None,
    output_path=None,
    format=None,
    digest=None,
    discover_latest=None,
    checkpoint=None,
    name=None,
    best_effort=false,
    force=false,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
fn workflow_auto(
    py: Python<'_>,
    package_id: &str,
    template: Option<&str>,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    discover_latest: Option<u64>,
    checkpoint: Option<u64>,
    name: Option<&str>,
    best_effort: bool,
    force: bool,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    let package_id_owned = package_id.to_string();
    let template_owned = template.map(ToOwned::to_owned);
    let output_path_owned = output_path.map(ToOwned::to_owned);
    let format_owned = format.map(ToOwned::to_owned);
    let digest_owned = digest.map(ToOwned::to_owned);
    let name_owned = name.map(ToOwned::to_owned);
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_owned = walrus_aggregator_url.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let package_id = package_id_owned.trim();
            if package_id.is_empty() {
                return Err(anyhow!("package_id cannot be empty"));
            }

            let mut dependency_packages_fetched = None;
            let mut unresolved_dependencies = Vec::new();
            let mut dependency_probe_error = None;
            match probe_dependency_closure_for_workflow(package_id) {
                Ok((fetched_packages, unresolved)) => {
                    dependency_packages_fetched = Some(fetched_packages);
                    unresolved_dependencies = unresolved;
                }
                Err(err) => {
                    if best_effort {
                        dependency_probe_error = Some(err.to_string());
                    } else {
                        return Err(anyhow!(
                            "AUTO_CLOSURE_INCOMPLETE: dependency closure probe failed for package {}: {}\nHint: resolve package fetch issues, or rerun with best_effort=True to emit scaffold output.",
                            package_id,
                            err
                        ));
                    }
                }
            }
            if !unresolved_dependencies.is_empty() && !best_effort {
                return Err(anyhow!(
                    "AUTO_CLOSURE_INCOMPLETE: unresolved package dependencies after closure fetch for package {}: {}\nHint: ensure transitive package bytecode is available, or rerun with best_effort=True to emit scaffold output.",
                    package_id,
                    unresolved_dependencies.join(", ")
                ));
            }

            let mut package_module_count = None;
            let mut module_names = Vec::new();
            let mut package_module_probe_error = None;
            match probe_package_modules_for_workflow(package_id) {
                Ok((count, names)) => {
                    package_module_count = Some(count);
                    module_names = names;
                }
                Err(err) => {
                    package_module_probe_error = Some(err.to_string());
                }
            }

            let inference = if let Some(template_raw) = template_owned.as_deref() {
                WorkflowTemplateInference {
                    template: parse_workflow_template(template_raw)?,
                    confidence: "manual",
                    source: "user",
                    reason: None,
                }
            } else {
                infer_workflow_template_from_modules(&module_names)
            };
            let template = inference.template;

            let explicit_digest = digest_owned
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let mut discovery_probe_error = None;
            let discovered_target = if let Some(latest) = discover_latest {
                match discover_latest_target_for_workflow(
                    package_id,
                    latest,
                    &walrus_network_owned,
                    walrus_caching_owned.as_deref(),
                    walrus_aggregator_owned.as_deref(),
                ) {
                    Ok(target) => Some(target),
                    Err(err) => {
                        if best_effort {
                            discovery_probe_error = Some(err.to_string());
                            None
                        } else {
                            return Err(anyhow!(
                                "AUTO_DISCOVERY_EMPTY: failed to auto-discover replay target for package {}: {}\nHint: rerun with a larger discover_latest window, provide digest explicitly, or use best_effort=True for scaffold-only output.",
                                package_id,
                                err
                            ));
                        }
                    }
                }
            } else {
                None
            };

            let digest = explicit_digest
                .clone()
                .or_else(|| discovered_target.as_ref().map(|target| target.digest.clone()));
            let include_replay = digest.is_some();
            let checkpoint = if include_replay {
                if let Some(target) = discovered_target.as_ref() {
                    Some(target.checkpoint)
                } else {
                    Some(checkpoint.unwrap_or(template.default_checkpoint()))
                }
            } else {
                None
            };
            let replay_seed_source = if explicit_digest.is_some() {
                "digest"
            } else if discovered_target.is_some() {
                "discover_latest"
            } else {
                "none"
            };

            let mut missing_inputs = Vec::new();
            if !include_replay {
                if discover_latest.is_some() {
                    missing_inputs.push(
                        "auto-discovery target (rerun with larger discover_latest window)"
                            .to_string(),
                    );
                } else {
                    missing_inputs.push("digest".to_string());
                    missing_inputs
                        .push("checkpoint (optional; default inferred per template)".to_string());
                }
            }

            let mut spec = build_builtin_workflow(
                template,
                &BuiltinWorkflowInput {
                    digest,
                    checkpoint,
                    include_analyze_step: include_replay,
                    include_replay_step: include_replay,
                    strict_replay: true,
                    package_id: Some(package_id.to_string()),
                    view_objects: Vec::new(),
                },
            )?;

            let pkg_suffix = short_package_id(package_id);
            spec.name = Some(name_owned.unwrap_or_else(|| {
                format!("auto_{}_{}", template.key(), pkg_suffix)
            }));
            spec.description = Some(format!(
                "Auto draft adapter generated from package {} (template: {}).",
                package_id,
                template.key()
            ));
            spec.validate()?;

            let parsed_format = parse_workflow_output_format(format_owned.as_deref())?;
            let format_hint = parsed_format.unwrap_or(WorkflowOutputFormat::Json);
            let output_path = output_path_owned
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "workflow.auto.{}.{}.{}",
                        template.key(),
                        pkg_suffix,
                        format_hint.extension()
                    ))
                });
            let output_format = parsed_format
                .or_else(|| WorkflowOutputFormat::from_path(&output_path))
                .unwrap_or(format_hint);

            if output_path.exists() && !force {
                return Err(anyhow!(
                    "Refusing to overwrite existing workflow spec at {} (pass force=True)",
                    output_path.display()
                ));
            }
            write_workflow_spec(&output_path, &spec, output_format)?;

            Ok(serde_json::json!({
                "package_id": package_id,
                "template": template.key(),
                "inference_source": inference.source,
                "inference_confidence": inference.confidence,
                "inference_reason": inference.reason,
                "output_file": output_path.display().to_string(),
                "format": output_format.as_str(),
                "replay_steps_included": include_replay,
                "replay_seed_source": replay_seed_source,
                "discover_latest": discover_latest,
                "discovered_checkpoint": discovered_target.as_ref().map(|target| target.checkpoint),
                "discovery_probe_error": discovery_probe_error,
                "missing_inputs": missing_inputs,
                "package_module_count": package_module_count,
                "package_module_probe_error": package_module_probe_error,
                "dependency_packages_fetched": dependency_packages_fetched,
                "unresolved_dependencies": unresolved_dependencies,
                "dependency_probe_error": dependency_probe_error,
                "steps": spec.steps.len(),
            }))
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Run a typed workflow spec natively via Python bindings.
///
/// Supports replay, analyze_replay, and command steps without shelling out to
/// `sui-sandbox workflow run`.
#[pyfunction]
#[pyo3(signature = (
    spec_path,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
fn workflow_run(
    py: Python<'_>,
    spec_path: &str,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    let spec_path_owned = spec_path.to_string();
    let report_path_owned = report_path.map(ToOwned::to_owned);
    let rpc_url_owned = rpc_url.to_string();
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_owned = walrus_aggregator_url.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let spec_path = PathBuf::from(&spec_path_owned);
            let spec = WorkflowSpec::load_from_path(&spec_path)?;
            workflow_run_spec_inner(
                spec,
                spec_path.display().to_string(),
                dry_run,
                continue_on_error,
                report_path_owned,
                &rpc_url_owned,
                &walrus_network_owned,
                walrus_caching_owned.as_deref(),
                walrus_aggregator_owned.as_deref(),
                verbose,
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Run a typed workflow spec directly from an in-memory Python object (dict/list).
///
/// This avoids writing temporary spec files for ad-hoc or notebook workflows.
#[pyfunction]
#[pyo3(signature = (
    spec,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
fn workflow_run_inline(
    py: Python<'_>,
    spec: &Bound<'_, PyAny>,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    let inline_spec = parse_inline_workflow_spec(py, spec).map_err(to_py_err)?;
    let report_path_owned = report_path.map(ToOwned::to_owned);
    let rpc_url_owned = rpc_url.to_string();
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_owned = walrus_aggregator_url.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            workflow_run_spec_inner(
                inline_spec,
                "<inline>".to_string(),
                dry_run,
                continue_on_error,
                report_path_owned,
                &rpc_url_owned,
                &walrus_network_owned,
                walrus_caching_owned.as_deref(),
                walrus_aggregator_owned.as_deref(),
                verbose,
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Canonical alias for `workflow_validate`.
#[pyfunction]
fn pipeline_validate(py: Python<'_>, spec_path: &str) -> PyResult<PyObject> {
    workflow_validate(py, spec_path)
}

/// Canonical alias for `workflow_init`.
#[pyfunction]
#[pyo3(signature = (
    *,
    template="generic",
    output_path=None,
    format=None,
    digest=None,
    checkpoint=None,
    include_analyze_step=true,
    strict_replay=true,
    name=None,
    package_id=None,
    view_objects=vec![],
    force=false,
))]
fn pipeline_init(
    py: Python<'_>,
    template: &str,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    include_analyze_step: bool,
    strict_replay: bool,
    name: Option<&str>,
    package_id: Option<&str>,
    view_objects: Vec<String>,
    force: bool,
) -> PyResult<PyObject> {
    workflow_init(
        py,
        template,
        output_path,
        format,
        digest,
        checkpoint,
        include_analyze_step,
        strict_replay,
        name,
        package_id,
        view_objects,
        force,
    )
}

/// Canonical alias for `workflow_auto`.
#[pyfunction]
#[pyo3(signature = (
    package_id,
    *,
    template=None,
    output_path=None,
    format=None,
    digest=None,
    discover_latest=None,
    checkpoint=None,
    name=None,
    best_effort=false,
    force=false,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
fn pipeline_auto(
    py: Python<'_>,
    package_id: &str,
    template: Option<&str>,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    discover_latest: Option<u64>,
    checkpoint: Option<u64>,
    name: Option<&str>,
    best_effort: bool,
    force: bool,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    workflow_auto(
        py,
        package_id,
        template,
        output_path,
        format,
        digest,
        discover_latest,
        checkpoint,
        name,
        best_effort,
        force,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
}

/// Canonical alias for `workflow_run`.
#[pyfunction]
#[pyo3(signature = (
    spec_path,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
fn pipeline_run(
    py: Python<'_>,
    spec_path: &str,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    workflow_run(
        py,
        spec_path,
        dry_run,
        continue_on_error,
        report_path,
        rpc_url,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        verbose,
    )
}

/// Canonical alias for `workflow_run_inline`.
#[pyfunction]
#[pyo3(signature = (
    spec,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
fn pipeline_run_inline(
    py: Python<'_>,
    spec: &Bound<'_, PyAny>,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    workflow_run_inline(
        py,
        spec,
        dry_run,
        continue_on_error,
        report_path,
        rpc_url,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        verbose,
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

/// Execute DeepBook `margin_manager::manager_state` from a versions snapshot.
///
/// Native convenience API that mirrors the Rust historical margin example:
/// - load versions/checkpoint JSON
/// - fetch required object BCS at historical versions
/// - fetch checkpoint-pinned package closure
/// - execute `manager_state` in local VM and decode key fields
#[pyfunction]
#[pyo3(signature = (
    *,
    versions_file=DEEPBOOK_MARGIN_DEFAULT_VERSIONS_FILE,
    grpc_endpoint=None,
    grpc_api_key=None,
))]
fn deepbook_margin_state(
    py: Python<'_>,
    versions_file: &str,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
) -> PyResult<PyObject> {
    let versions_file_owned = versions_file.to_string();
    let endpoint_owned = grpc_endpoint.map(ToOwned::to_owned);
    let api_key_owned = grpc_api_key.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let versions_path = PathBuf::from(&versions_file_owned);
            let (checkpoint, historical_versions) = load_deepbook_versions_snapshot(&versions_path)?;

            let (resolved_endpoint, resolved_api_key) = resolve_grpc_endpoint_and_key(
                endpoint_owned.as_deref(),
                api_key_owned.as_deref(),
            );
            let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
            let object_inputs = rt.block_on(async {
                let grpc = GrpcClient::with_api_key(&resolved_endpoint, resolved_api_key.clone())
                    .await
                    .context("Failed to create gRPC client")?;
                let mut inputs: Vec<(String, Vec<u8>, String, bool, bool)> =
                    Vec::with_capacity(DEEPBOOK_MARGIN_REQUIRED_OBJECTS.len());
                for object_id in DEEPBOOK_MARGIN_REQUIRED_OBJECTS {
                    let version = historical_versions.get(object_id).copied().ok_or_else(|| {
                        anyhow!(
                            "versions file missing required object version for {}",
                            object_id
                        )
                    })?;
                    let fetched = grpc
                        .get_object_at_version(object_id, Some(version))
                        .await
                        .with_context(|| {
                            format!(
                                "failed to fetch object {} at version {} via gRPC",
                                object_id, version
                            )
                        })?
                        .ok_or_else(|| {
                            anyhow!(
                                "object {} not found at version {}",
                                object_id,
                                version
                            )
                        })?;
                    let bcs = fetched
                        .bcs
                        .ok_or_else(|| anyhow!("object {} missing BCS payload", object_id))?;
                    let type_tag = fetched.type_string.clone().ok_or_else(|| {
                        anyhow!(
                            "object {} missing type string; cannot build view input",
                            object_id
                        )
                    })?;
                    let is_shared = matches!(fetched.owner, GrpcOwner::Shared { .. });
                    inputs.push((object_id.to_string(), bcs, type_tag, is_shared, false));
                }
                Ok::<_, anyhow::Error>(inputs)
            })?;

            let package_payload = fetch_historical_package_bytecodes_inner(
                &vec![
                    PROTOCOL_DEEPBOOK_MARGIN_PACKAGE.to_string(),
                    DEEPBOOK_SPOT_PACKAGE.to_string(),
                ],
                &vec![
                    DEEPBOOK_MARGIN_SUI_TYPE.to_string(),
                    DEEPBOOK_MARGIN_USDC_TYPE.to_string(),
                ],
                Some(checkpoint),
                Some(&resolved_endpoint),
                resolved_api_key.as_deref(),
            )?;
            let (
                package_bytecodes,
                aliases,
                linkage_upgrades,
                package_runtime_ids,
                package_linkage,
                package_versions,
            ) = parse_historical_package_payload(&package_payload)?;

            let result = call_view_function_inner(
                PROTOCOL_DEEPBOOK_MARGIN_PACKAGE,
                "margin_manager",
                "manager_state",
                vec![
                    DEEPBOOK_MARGIN_SUI_TYPE.to_string(),
                    DEEPBOOK_MARGIN_USDC_TYPE.to_string(),
                ],
                object_inputs,
                Vec::new(),
                HashMap::new(),
                historical_versions.clone(),
                true,
                Some(resolved_endpoint.clone()),
                resolved_api_key.clone(),
                package_bytecodes,
                aliases,
                linkage_upgrades,
                package_runtime_ids,
                package_linkage,
                package_versions,
                false,
            )?;

            let decoded = decode_deepbook_margin_state(&result)?;
            let mut output = serde_json::json!({
                "versions_file": versions_path.display().to_string(),
                "checkpoint": checkpoint,
                "grpc_endpoint": resolved_endpoint,
                "success": result.get("success").and_then(serde_json::Value::as_bool).unwrap_or(false),
                "gas_used": result.get("gas_used").and_then(serde_json::Value::as_u64),
                "error": result.get("error").cloned(),
                "decoded_margin_state": decoded,
                "raw": result,
            });

            let error_for_hint = output
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned);
            let endpoint_for_hint = output
                .get("grpc_endpoint")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(err) = error_for_hint.as_deref() {
                if let Some(hint) = deepbook_archive_hint(err, &endpoint_for_hint) {
                    if let Some(obj) = output.as_object_mut() {
                        obj.insert("hint".to_string(), serde_json::json!(hint));
                    }
                }
            }

            Ok(output)
        })
        .map_err(to_py_err)?;

    json_value_to_py(py, &value)
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
/// Resolves a protocol default package id when `package_id` is omitted.
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

/// Replay a transaction with opinionated defaults for a compact native API.
///
/// Args:
///     digest: Transaction digest (optional when state_file contains a single transaction)
///     checkpoint: Optional checkpoint (if provided and source is omitted, source defaults to walrus)
///     discover_latest: Auto-discover digest from latest N checkpoints (requires discover_package_id)
///     discover_package_id: Package filter used for discovery when discover_latest is set
///     source: "hybrid", "grpc", or "walrus" (default: inferred)
///     state_file: Optional replay-state JSON for deterministic local input data
///     context_path: Optional prepared package context JSON to pre-seed package bytecode
///     cache_dir: Optional local replay cache when source="local"
///     walrus_network: Walrus network for discovery ("mainnet" or "testnet")
///     walrus_caching_url: Optional custom Walrus caching endpoint (requires walrus_aggregator_url)
///     walrus_aggregator_url: Optional custom Walrus aggregator endpoint (requires walrus_caching_url)
///     rpc_url: Sui RPC endpoint
///     allow_fallback: Allow fallback hydration paths
///     profile: Runtime defaults profile ("safe"|"balanced"|"fast")
///     fetch_strategy: Dynamic-field fetch strategy ("eager"|"full")
///     vm_only: Disable fallback paths and force VM-only behavior
///     prefetch_depth: Dynamic field prefetch depth
///     prefetch_limit: Dynamic field prefetch limit
///     auto_system_objects: Auto inject Clock/Random if missing
///     no_prefetch: Disable prefetch
///     compare: Compare local execution with on-chain effects
///     analyze_only: Hydration-only mode
///     synthesize_missing: Retry with synthetic object bytes when inputs are missing
///     self_heal_dynamic_fields: Enable dynamic field self-healing during VM execution
///     analyze_mm2: Build MM2 diagnostics (analyze-only mode)
///     verbose: Verbose replay logging
///
/// Returns: Replay result dict
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
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
fn replay_transaction(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
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
    let (effective_digest, effective_checkpoint) = resolve_replay_target_from_discovery(
        digest,
        checkpoint,
        state_file,
        discover_latest,
        discover_package_id,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
    .map_err(to_py_err)?;

    let source_owned = source.map(|s| s.to_string()).unwrap_or_else(|| {
        if effective_checkpoint.is_some() {
            "walrus".to_string()
        } else {
            "hybrid".to_string()
        }
    });
    replay(
        py,
        effective_digest.as_deref(),
        rpc_url,
        &source_owned,
        effective_checkpoint,
        state_file,
        context_path,
        cache_dir,
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
    )
}

/// Protocol-first run path: prepare context + replay in one call.
///
/// Protocol defaults are applied only for package selection. Runtime inputs
/// (objects, type args, historical choices) stay explicit by design.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    protocol="generic",
    package_id=None,
    resolve_deps=true,
    context_path=None,
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
fn protocol_run(
    py: Python<'_>,
    digest: Option<&str>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
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
    let protocol_owned = protocol.to_string();
    let resolved_package_id =
        resolve_protocol_package_id(&protocol_owned, package_id).map_err(to_py_err)?;
    let context_path_owned = context_path.map(ToOwned::to_owned);
    let prepared = py
        .allow_threads(move || {
            prepare_package_context_inner(
                &resolved_package_id,
                resolve_deps,
                context_path_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;

    let context_tmp = if context_path.is_some() {
        None
    } else {
        Some(write_temp_context_file(&prepared).map_err(to_py_err)?)
    };
    let effective_context = context_path
        .map(ToOwned::to_owned)
        .or_else(|| context_tmp.as_ref().map(|p| p.display().to_string()));

    let result = replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        if discover_latest.is_some() {
            prepared
                .get("package_id")
                .and_then(serde_json::Value::as_str)
        } else {
            None
        },
        source,
        state_file,
        effective_context.as_deref(),
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

/// Canonical alias for replaying against a prepared context.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
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
fn context_replay(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
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
    replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        discover_package_id,
        source,
        state_file,
        context_path,
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
    )
}

/// Canonical alias for `protocol_run`.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    protocol="generic",
    package_id=None,
    resolve_deps=true,
    context_path=None,
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
fn adapter_run(
    py: Python<'_>,
    digest: Option<&str>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
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
    protocol_run(
        py,
        digest,
        protocol,
        package_id,
        resolve_deps,
        context_path,
        checkpoint,
        discover_latest,
        source,
        state_file,
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
    )
}

/// Canonical context run wrapper: prepare context + replay in one call.
#[pyfunction]
#[pyo3(signature = (
    package_id,
    digest=None,
    *,
    resolve_deps=true,
    context_path=None,
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
fn context_run(
    py: Python<'_>,
    package_id: &str,
    digest: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
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
    protocol_run(
        py,
        digest,
        "generic",
        Some(package_id),
        resolve_deps,
        context_path,
        checkpoint,
        discover_latest,
        source,
        state_file,
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
    )
}

/// Interactive two-step flow helper for Python.
///
/// Keeps prepared package context in memory and reuses it across replays.
#[pyclass(module = "sui_sandbox")]
struct FlowSession {
    context: Option<serde_json::Value>,
    package_id: Option<String>,
}

#[pymethods]
impl FlowSession {
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
            PyRuntimeError::new_err("FlowSession has no context; call prepare() or load_context()")
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
    fn load_deepbook_versions_snapshot_reads_fixture() {
        let path = fixture_path(
            "examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json",
        );
        let (checkpoint, versions) =
            load_deepbook_versions_snapshot(&path).expect("load deepbook versions fixture");
        assert_eq!(checkpoint, 240733000);
        assert!(versions.contains_key(DEEPBOOK_MARGIN_TARGET_MANAGER));
        assert!(versions.contains_key(DEEPBOOK_MARGIN_CLOCK));
    }

    #[test]
    fn decode_deepbook_margin_state_decodes_expected_fields() {
        use base64::Engine as _;
        let enc =
            |value: u64| base64::engine::general_purpose::STANDARD.encode(value.to_le_bytes());
        let result = serde_json::json!({
            "success": true,
            "return_values": [[
                enc(0),
                enc(0),
                enc(250_000_000),   // risk_ratio
                enc(3_000_000_000), // base_asset
                enc(5_000_000),     // quote_asset
                enc(1_000_000_000), // base_debt
                enc(2_000_000),     // quote_debt
                enc(0),
                enc(0),
                enc(0),
                enc(0),
                enc(1_500_000),     // current_price
            ]]
        });
        let decoded = decode_deepbook_margin_state(&result)
            .expect("decode succeeds")
            .expect("decoded payload");

        assert_eq!(decoded["risk_ratio_pct"].as_f64().unwrap(), 25.0);
        assert_eq!(decoded["base_asset_sui"].as_f64().unwrap(), 3.0);
        assert_eq!(decoded["quote_asset_usdc"].as_f64().unwrap(), 5.0);
        assert_eq!(decoded["base_debt_sui"].as_f64().unwrap(), 1.0);
        assert_eq!(decoded["quote_debt_usdc"].as_f64().unwrap(), 2.0);
        assert_eq!(decoded["current_price"].as_f64().unwrap(), 1.5);
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
    m.add_function(wrap_pyfunction!(deepbook_margin_state, m)?)?;
    m.add_function(wrap_pyfunction!(fuzz_function, m)?)?;
    m.add_function(wrap_pyfunction!(replay, m)?)?;
    m.add_function(wrap_pyfunction!(replay_transaction, m)?)?;
    m.add_function(wrap_pyfunction!(context_replay, m)?)?;
    m.add_function(wrap_pyfunction!(context_run, m)?)?;
    m.add_function(wrap_pyfunction!(protocol_run, m)?)?;
    m.add_function(wrap_pyfunction!(adapter_run, m)?)?;
    m.add_class::<FlowSession>()?;
    let flow_session = m.getattr("FlowSession")?;
    m.add("ContextSession", flow_session)?;
    Ok(())
}
