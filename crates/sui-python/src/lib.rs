//! Python bindings for Sui Move package analysis and transaction replay.
//!
//! Exposes two main capabilities:
//! - `analyze_package`: Extract Move package interface from bytecode or GraphQL
//! - `analyze_replay`: Inspect replay state hydration for a transaction digest

#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use sui_package_extractor::bytecode::{
    build_bytecode_interface_value_from_compiled_modules, read_local_compiled_modules,
    resolve_local_package_id,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::network::resolve_graphql_endpoint;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_py_err(e: anyhow::Error) -> PyErr {
    PyRuntimeError::new_err(format!("{:#}", e))
}

/// Convert a serde_json::Value to a Python object via JSON round-trip.
fn json_value_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<PyObject> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| PyRuntimeError::new_err(format!("JSON serialization failed: {}", e)))?;
    let json_mod = py.import("json")?;
    let result = json_mod.call_method1("loads", (json_str,))?;
    Ok(result.into())
}

/// Run a `sui-sandbox --json` CLI command and return parsed JSON output.
fn run_cli(args: &[&str]) -> Result<serde_json::Value> {
    let mut cmd = std::process::Command::new("sui-sandbox");
    cmd.arg("--json");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .context("Failed to execute sui-sandbox binary. Is it installed and in PATH?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            return Ok(v);
        }
        return Err(anyhow!(
            "sui-sandbox failed (exit {}): {}",
            output.status,
            stderr
        ));
    }

    serde_json::from_str(&stdout).context("Failed to parse sui-sandbox JSON output")
}

// ---------------------------------------------------------------------------
// analyze_package (subprocess wrapper)
// ---------------------------------------------------------------------------

fn analyze_package_inner(
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
    list_modules: bool,
) -> Result<serde_json::Value> {
    let mut args: Vec<&str> = vec!["analyze", "package"];
    if let Some(pkg) = package_id {
        args.push("--package-id");
        args.push(pkg);
    }
    if let Some(dir) = bytecode_dir {
        args.push("--bytecode-dir");
        args.push(dir);
    }
    if list_modules {
        args.push("--list-modules");
    }

    let mut cmd = std::process::Command::new("sui-sandbox");
    cmd.arg("--json");
    if rpc_url != "https://fullnode.mainnet.sui.io:443" {
        cmd.arg("--rpc-url").arg(rpc_url);
    }
    for arg in &args {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .context("Failed to execute sui-sandbox binary. Is it installed and in PATH?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            return Ok(v);
        }
        return Err(anyhow!(
            "sui-sandbox analyze package failed (exit {}): {}",
            output.status,
            stderr
        ));
    }

    serde_json::from_str(&stdout).context("Failed to parse sui-sandbox JSON output")
}

/// Extract the full interface JSON (all structs, functions, type params, etc.)
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
// analyze_replay
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn analyze_replay_inner(
    digest: &str,
    rpc_url: &str,
    source: &str,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    verbose: bool,
) -> Result<serde_json::Value> {
    let depth_str = prefetch_depth.to_string();
    let limit_str = prefetch_limit.to_string();

    let mut cmd = std::process::Command::new("sui-sandbox");
    cmd.arg("--json");
    if rpc_url != "https://fullnode.mainnet.sui.io:443" {
        cmd.arg("--rpc-url").arg(rpc_url);
    }
    if verbose {
        cmd.arg("--verbose");
    }
    cmd.arg("analyze").arg("replay").arg(digest);
    cmd.arg("--source").arg(source);
    cmd.arg("--allow-fallback")
        .arg(if allow_fallback { "true" } else { "false" });
    cmd.arg("--prefetch-depth").arg(&depth_str);
    cmd.arg("--prefetch-limit").arg(&limit_str);
    cmd.arg("--auto-system-objects")
        .arg(if auto_system_objects { "true" } else { "false" });
    if no_prefetch {
        cmd.arg("--no-prefetch");
    }

    let output = cmd
        .output()
        .context("Failed to execute sui-sandbox binary. Is it installed and in PATH?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            return Ok(v);
        }
        return Err(anyhow!(
            "sui-sandbox analyze replay failed (exit {}): {}",
            output.status,
            stderr
        ));
    }

    serde_json::from_str(&stdout).context("Failed to parse sui-sandbox JSON output")
}

// ---------------------------------------------------------------------------
// replay (full VM execution)
// ---------------------------------------------------------------------------

fn replay_inner(
    digest: &str,
    rpc_url: &str,
    compare: bool,
    verbose: bool,
) -> Result<serde_json::Value> {
    let mut cmd = std::process::Command::new("sui-sandbox");
    cmd.arg("--json");
    if rpc_url != "https://fullnode.mainnet.sui.io:443" {
        cmd.arg("--rpc-url").arg(rpc_url);
    }
    if verbose {
        cmd.arg("--verbose");
    }
    cmd.arg("replay").arg(digest);
    if compare {
        cmd.arg("--compare");
    }

    let output = cmd
        .output()
        .context("Failed to execute sui-sandbox binary. Is it installed and in PATH?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            return Ok(v);
        }
        return Err(anyhow!(
            "sui-sandbox replay failed (exit {}): {}",
            output.status,
            stderr
        ));
    }

    serde_json::from_str(&stdout).context("Failed to parse sui-sandbox replay JSON output")
}

// ---------------------------------------------------------------------------
// Walrus functions (subprocess wrappers)
// ---------------------------------------------------------------------------

fn get_latest_checkpoint_inner() -> Result<u64> {
    let value = run_cli(&["fetch", "latest-checkpoint"])?;
    value["checkpoint"]
        .as_u64()
        .ok_or_else(|| anyhow!("missing 'checkpoint' in CLI output"))
}

fn get_checkpoint_inner(checkpoint_num: u64) -> Result<serde_json::Value> {
    let seq_str = checkpoint_num.to_string();
    run_cli(&["fetch", "checkpoint", &seq_str])
}

fn analyze_replay_walrus_inner(
    digest: &str,
    checkpoint_num: u64,
    verbose: bool,
) -> Result<serde_json::Value> {
    let cp_str = checkpoint_num.to_string();
    let mut args = vec![
        "analyze",
        "replay",
        digest,
        "--source",
        "walrus",
        "--checkpoint",
        &cp_str,
    ];
    if verbose {
        args.insert(0, "--verbose");
    }
    run_cli(&args)
}

// ---------------------------------------------------------------------------
// json_to_bcs: Convert Sui object JSON to BCS bytes
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

// ---------------------------------------------------------------------------
// call_view_function: Execute a view function via local Move VM
// ---------------------------------------------------------------------------

use sui_package_extractor::extract_module_dependency_ids as extract_dependency_addrs;
use sui_package_extractor::utils::is_framework_address;

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

fn call_view_function_inner(
    package_id: &str,
    module: &str,
    function: &str,
    type_args: Vec<String>,
    object_inputs: Vec<(String, Vec<u8>, String, bool, bool)>, // (object_id, bcs_bytes, type_tag_str, is_shared, mutable)
    pure_inputs: Vec<Vec<u8>>,
    child_objects: HashMap<String, Vec<(String, Vec<u8>, String)>>, // parent_id -> [(child_id, bcs, type_tag)]
    package_bytecodes: HashMap<String, Vec<Vec<u8>>>, // package_id -> [module_bytecodes]
    fetch_deps: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::ptb::{Argument, Command, ObjectInput, PTBExecutor};
    use sui_sandbox_core::resolver::ModuleProvider;
    use sui_sandbox_core::vm::{SimulationConfig, VMHarness};

    // 1. Build LocalModuleResolver with sui framework
    let mut resolver = sui_sandbox_core::resolver::LocalModuleResolver::with_sui_framework()?;

    // 2. Load provided package bytecodes
    let mut loaded_packages = HashSet::new();
    // Mark framework as loaded
    loaded_packages.insert(AccountAddress::from_hex_literal("0x1").unwrap());
    loaded_packages.insert(AccountAddress::from_hex_literal("0x2").unwrap());
    loaded_packages.insert(AccountAddress::from_hex_literal("0x3").unwrap());

    for (pkg_id_str, module_bytecodes) in &package_bytecodes {
        let addr = AccountAddress::from_hex_literal(pkg_id_str)
            .with_context(|| format!("invalid package address: {}", pkg_id_str))?;
        if is_framework_address(&addr) {
            continue;
        }
        let modules: Vec<(String, Vec<u8>)> = module_bytecodes
            .iter()
            .enumerate()
            .map(|(i, bytes)| {
                // Try to extract module name from bytecode
                if let Ok(compiled) = CompiledModule::deserialize_with_defaults(bytes) {
                    let name = compiled.self_id().name().to_string();
                    (name, bytes.clone())
                } else {
                    (format!("module_{}", i), bytes.clone())
                }
            })
            .collect();
        resolver.load_package_at(modules, addr)?;
        loaded_packages.insert(addr);
    }

    // 3. If fetch_deps, resolve transitive dependencies via GraphQL
    if fetch_deps {
        let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
        let graphql = GraphQLClient::new(&graphql_endpoint);

        // Collect all packages we need to resolve
        let mut to_fetch: VecDeque<AccountAddress> = VecDeque::new();

        // Start with the target package if not already loaded
        let target_addr = AccountAddress::from_hex_literal(package_id)
            .with_context(|| format!("invalid target package: {}", package_id))?;
        if !loaded_packages.contains(&target_addr) {
            to_fetch.push_back(target_addr);
        }

        // Also fetch packages referenced in type_args and object type_tags
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

        // Also check packages from provided bytecodes for their dependencies
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

        // BFS fetch dependencies (max 8 rounds to prevent runaway resolution)
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
                    // Extract deps before loading
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
                    // Log warning but continue — some deps may be optional
                    eprintln!("Warning: failed to fetch package {}: {:#}", hex, e);
                }
            }
        }
    }

    // 4. Create VMHarness with simulation config
    let config = SimulationConfig::default();
    let mut vm = VMHarness::with_config(&resolver, false, config)?;

    // 5. Set up child fetcher if child_objects provided
    if !child_objects.is_empty() {
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

        let fetcher: sui_sandbox_core::sandbox_runtime::ChildFetcherFn =
            Box::new(move |parent, child| child_map.get(&(parent, child)).cloned());
        vm.set_child_fetcher(fetcher);
    }

    // 6. Build PTB and execute
    let mut executor = PTBExecutor::new(&mut vm);

    // Add object inputs
    let mut input_indices = Vec::new();
    for (obj_id_str, bcs_bytes, type_tag_str, is_shared, mutable) in &object_inputs {
        let id = AccountAddress::from_hex_literal(obj_id_str)
            .with_context(|| format!("invalid object_id: {}", obj_id_str))?;
        let type_tag = sui_sandbox_core::types::parse_type_tag(type_tag_str)
            .with_context(|| format!("invalid type tag: {}", type_tag_str))?;

        let obj_input = if *is_shared {
            ObjectInput::Shared {
                id,
                bytes: bcs_bytes.clone(),
                type_tag: Some(type_tag),
                version: None,
                mutable: *mutable,
            }
        } else {
            ObjectInput::ImmRef {
                id,
                bytes: bcs_bytes.clone(),
                type_tag: Some(type_tag),
                version: None,
            }
        };

        let idx = executor
            .add_object_input(obj_input)
            .with_context(|| format!("add object input {}", obj_id_str))?;
        input_indices.push(idx);
    }

    // Add pure inputs
    for pure_bytes in &pure_inputs {
        let idx = executor
            .add_pure_input(pure_bytes.clone())
            .context("add pure input")?;
        input_indices.push(idx);
    }

    // Parse type arguments
    let mut parsed_type_args = Vec::new();
    for ta_str in &type_args {
        let tt = sui_sandbox_core::types::parse_type_tag(ta_str)
            .with_context(|| format!("invalid type arg: {}", ta_str))?;
        parsed_type_args.push(tt);
    }

    // Build args list: all inputs as Argument::Input
    let args: Vec<Argument> = (0..input_indices.len() as u16)
        .map(Argument::Input)
        .collect();

    // Build move call command
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
// fetch_package_bytecodes: Fetch package bytecode via GraphQL
// ---------------------------------------------------------------------------

fn fetch_package_bytecodes_inner(
    package_id: &str,
    resolve_deps: bool,
) -> Result<serde_json::Value> {
    let mut args = vec!["fetch", "package", package_id, "--bytecodes"];
    if resolve_deps {
        args.push("--with-deps");
    }
    let result = run_cli(&args)?;

    // Transform CLI output shape to match the Python API's expected shape.
    // CLI returns { packages_fetched: [{ address, bytecodes: [...] }] }
    // Python callers expect { packages: { pkg_id: [base64...] }, count: N }
    let packages_fetched = result["packages_fetched"]
        .as_array()
        .ok_or_else(|| anyhow!("missing 'packages_fetched' in CLI output"))?;

    let mut packages = serde_json::Map::new();
    for pkg in packages_fetched {
        let address = pkg["address"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'address' in package"))?;
        let bytecodes = pkg
            .get("bytecodes")
            .cloned()
            .unwrap_or(serde_json::json!([]));
        packages.insert(address.to_string(), bytecodes);
    }

    Ok(serde_json::json!({
        "packages": packages,
        "count": packages.len(),
    }))
}

// ---------------------------------------------------------------------------
// Python module
// ---------------------------------------------------------------------------

/// Get the latest archived checkpoint number from Walrus.
///
/// No API keys or authentication required.
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
/// No API keys or authentication required.
#[pyfunction]
fn get_checkpoint(py: Python<'_>, checkpoint: u64) -> PyResult<PyObject> {
    let value = get_checkpoint_inner(checkpoint).map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Analyze replay state for a transaction using Walrus only.
///
/// Fetches the checkpoint from Walrus, extracts the transaction,
/// and builds a replay state summary. No gRPC or API keys needed.
///
/// Args:
///     digest: Transaction digest
///     checkpoint: Checkpoint number containing the transaction
///     verbose: Include detailed object/package info
#[pyfunction]
#[pyo3(signature = (digest, checkpoint, *, verbose=false))]
fn walrus_analyze_replay(
    py: Python<'_>,
    digest: &str,
    checkpoint: u64,
    verbose: bool,
) -> PyResult<PyObject> {
    let value = analyze_replay_walrus_inner(digest, checkpoint, verbose).map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Analyze a Sui Move package interface.
///
/// Provide either `package_id` (fetched via GraphQL) or `bytecode_dir`
/// (local directory with `bytecode_modules/*.mv`), but not both.
///
/// Returns a dict with: source, package_id, modules, structs, functions,
/// key_structs, and optionally module_names.
#[pyfunction]
#[pyo3(signature = (*, package_id=None, bytecode_dir=None, rpc_url="https://fullnode.mainnet.sui.io:443", list_modules=false))]
fn analyze_package(
    py: Python<'_>,
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
    list_modules: bool,
) -> PyResult<PyObject> {
    let value = analyze_package_inner(package_id, bytecode_dir, rpc_url, list_modules)
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
#[pyfunction]
#[pyo3(signature = (*, package_id=None, bytecode_dir=None, rpc_url="https://fullnode.mainnet.sui.io:443"))]
fn extract_interface(
    py: Python<'_>,
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
) -> PyResult<PyObject> {
    // Release GIL during GraphQL fetching so Python threads can run concurrently
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

/// Analyze replay state hydration for a transaction digest.
///
/// Fetches the transaction and all required objects/packages, then
/// summarizes the hydration state without executing the transaction.
///
/// Set `SUI_GRPC_API_KEY` and optionally `SUI_GRPC_ENDPOINT` in env
/// for gRPC access to historical data.
#[pyfunction]
#[pyo3(signature = (
    digest,
    *,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    source="hybrid",
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    verbose=false,
))]
#[allow(clippy::too_many_arguments)]
fn analyze_replay(
    py: Python<'_>,
    digest: &str,
    rpc_url: &str,
    source: &str,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let value = analyze_replay_inner(
        digest,
        rpc_url,
        source,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        verbose,
    )
    .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Replay a historical Sui transaction locally with the Move VM.
///
/// This shells out to the `sui-sandbox` CLI binary (must be in PATH)
/// with `--json` output and returns the parsed result.
///
/// For full native replay without subprocess, use the Rust library directly.
#[pyfunction]
#[pyo3(signature = (
    digest,
    *,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    compare=false,
    verbose=false,
))]
fn replay(
    py: Python<'_>,
    digest: &str,
    rpc_url: &str,
    compare: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let value = replay_inner(digest, rpc_url, compare, verbose).map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Convert Sui object JSON to BCS bytes using struct layouts from bytecode.
///
/// Accepts the standard Sui object JSON format used by the RPC, GraphQL,
/// Snowflake, and other data providers.
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
    // Release GIL during computation so Python threads can run concurrently
    let type_str_owned = type_str.to_string();
    let object_json_owned = object_json.to_string();
    let bcs_bytes = py
        .allow_threads(move || {
            json_to_bcs_inner(&type_str_owned, &object_json_owned, package_bytecodes)
        })
        .map_err(to_py_err)?;
    Ok(PyBytes::new(py, &bcs_bytes))
}

/// Execute a view function via local Move VM.
///
/// Args:
///     package_id: Package containing the view function
///     module: Module name
///     function: Function name
///     type_args: List of type argument strings (e.g., ["0x2::sui::SUI"])
///     object_inputs: List of dicts with keys: object_id, bcs_bytes, type_tag, is_shared
///     pure_inputs: List of BCS-encoded pure argument bytes
///     child_objects: Dict mapping parent_id -> list of {child_id, bcs_bytes, type_tag}
///     package_bytecodes: Dict mapping package_id -> list of module bytecodes
///     fetch_deps: If True, automatically resolve transitive deps via GraphQL
///
/// Returns: Dict with success, error, return_values (per-command base64), return_type_tags (per-command),
/// gas_used
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
        let is_shared: bool = dict
            .get_item("is_shared")?
            .map(|v| v.extract().unwrap_or(false))
            .unwrap_or(false);
        let mutable: bool = dict
            .get_item("mutable")?
            .map(|v| v.extract().unwrap_or(false))
            .unwrap_or(false);
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

    // Parse package_bytecodes from Python dict
    let mut parsed_pkg_bytes: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    if let Some(ref pb) = package_bytecodes {
        for (key, value) in pb.iter() {
            let pkg_id: String = key.extract()?;
            let bytecodes: Vec<Vec<u8>> = value.extract()?;
            parsed_pkg_bytes.insert(pkg_id, bytecodes);
        }
    }

    // Release GIL during VM execution so Python threads can run concurrently
    let pkg_id_owned = package_id.to_string();
    let module_owned = module.to_string();
    let function_owned = function.to_string();
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
                parsed_pkg_bytes,
                fetch_deps,
            )
        })
        .map_err(to_py_err)?;

    json_value_to_py(py, &value)
}

/// Fetch package bytecodes via GraphQL, optionally resolving transitive dependencies.
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
    // Release GIL during GraphQL fetching so Python threads can run concurrently
    let pkg_id_owned = package_id.to_string();
    let value = py
        .allow_threads(move || fetch_package_bytecodes_inner(&pkg_id_owned, resolve_deps))
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Python module: sui_sandbox
#[pymodule]
fn sui_sandbox(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(analyze_package, m)?)?;
    m.add_function(wrap_pyfunction!(extract_interface, m)?)?;
    m.add_function(wrap_pyfunction!(analyze_replay, m)?)?;
    m.add_function(wrap_pyfunction!(replay, m)?)?;
    // Walrus functions
    m.add_function(wrap_pyfunction!(get_latest_checkpoint, m)?)?;
    m.add_function(wrap_pyfunction!(get_checkpoint, m)?)?;
    m.add_function(wrap_pyfunction!(walrus_analyze_replay, m)?)?;
    // View function execution
    m.add_function(wrap_pyfunction!(json_to_bcs, m)?)?;
    m.add_function(wrap_pyfunction!(call_view_function, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_package_bytecodes, m)?)?;
    Ok(())
}
