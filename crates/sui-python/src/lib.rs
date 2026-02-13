//! Python bindings for Sui Move package analysis, checkpoint replay, view function
//! execution, and Move function fuzzing.
//!
//! **All functions are standalone** — `pip install sui-sandbox` is all you need:
//! - `extract_interface`: Extract full Move package interface from bytecode or GraphQL
//! - `get_latest_checkpoint`: Get latest Walrus checkpoint number
//! - `get_checkpoint`: Fetch and summarize a Walrus checkpoint
//! - `fetch_package_bytecodes`: Fetch package bytecodes via GraphQL
//! - `json_to_bcs`: Convert Sui object JSON to BCS bytes
//! - `transaction_json_to_bcs`: Convert Snowflake/canonical TransactionData JSON to BCS bytes
//! - `call_view_function`: Execute a Move view function in the local VM
//! - `fuzz_function`: Fuzz a Move function with random inputs
//! - `replay`: Replay historical transactions (with optional analysis-only mode)
//! - `import_state`: Import replay data files into local cache
//! - `deserialize_transaction`: Decode raw transaction BCS
//! - `deserialize_package`: Decode raw package BCS

#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

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
use sui_package_extractor::extract_module_dependency_ids as extract_dependency_addrs;
use sui_package_extractor::utils::is_framework_address;
use sui_sandbox_core::resolver::ModuleProvider;
use sui_state_fetcher::{
    bcs_codec, build_aliases, checkpoint_to_replay_state, import_replay_states,
    parse_replay_states_file, FileStateProvider, HistoricalStateProvider, ImportSpec, ReplayState,
};
use sui_transport::graphql::GraphQLClient;
use sui_transport::network::resolve_graphql_endpoint;
use sui_transport::walrus::WalrusClient;

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
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    verbose: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::replay_support;
    use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};

    // ---------------------------------------------------------------
    // 1. Fetch ReplayState
    // ---------------------------------------------------------------
    let replay_state: ReplayState;
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
        return build_analyze_output(
            &replay_state,
            &effective_source,
            allow_fallback,
            auto_system_objects,
            !no_prefetch,
            prefetch_depth,
            prefetch_limit,
            verbose,
        );
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

    let reconcile_policy = EffectsReconcilePolicy::Strict;
    let replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &maps.cached_objects,
        &pkg_aliases.aliases,
        Some(&maps.versions_str),
        reconcile_policy,
    );

    // ---------------------------------------------------------------
    // 4. Build output JSON
    // ---------------------------------------------------------------
    build_replay_output(
        &replay_state,
        replay_result,
        source,
        &effective_source,
        allow_fallback,
        auto_system_objects,
        !no_prefetch,
        prefetch_depth,
        prefetch_limit,
        "graphql_dependency_closure",
        fetched_deps,
        compare,
    )
}

fn replay_loaded_state_inner(
    replay_state: ReplayState,
    requested_source: &str,
    effective_source: &str,
    allow_fallback: bool,
    auto_system_objects: bool,
    compare: bool,
    analyze_only: bool,
    verbose: bool,
) -> Result<serde_json::Value> {
    use sui_sandbox_core::replay_support;
    use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};

    if analyze_only {
        return build_analyze_output(
            &replay_state,
            effective_source,
            allow_fallback,
            auto_system_objects,
            false,
            0,
            0,
            verbose,
        );
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

    let replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &maps.cached_objects,
        &pkg_aliases.aliases,
        Some(&maps.versions_str),
        EffectsReconcilePolicy::Strict,
    );

    build_replay_output(
        &replay_state,
        replay_result,
        requested_source,
        effective_source,
        allow_fallback,
        auto_system_objects,
        false,
        0,
        0,
        effective_source,
        0,
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

/// Build JSON output for full replay (VM execution results).
fn build_replay_output(
    replay_state: &sui_state_fetcher::ReplayState,
    replay_result: Result<sui_sandbox_core::tx_replay::ReplayExecution>,
    requested_source: &str,
    effective_source: &str,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    dependency_fetch_mode: &str,
    dependency_packages_fetched: usize,
    compare: bool,
) -> Result<serde_json::Value> {
    let execution_path = serde_json::json!({
        "requested_source": requested_source,
        "effective_source": effective_source,
        "vm_only": false,
        "allow_fallback": allow_fallback,
        "auto_system_objects": auto_system_objects,
        "fallback_used": false,
        "dynamic_field_prefetch": dynamic_field_prefetch,
        "prefetch_depth": prefetch_depth,
        "prefetch_limit": prefetch_limit,
        "dependency_fetch_mode": dependency_fetch_mode,
        "dependency_packages_fetched": dependency_packages_fetched,
        "synthetic_inputs": 0,
    });

    match replay_result {
        Ok(execution) => {
            let result = execution.result;
            let effects = &execution.effects;

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
            if let Some(cmp) = comparison {
                output["comparison"] = cmp;
            }

            Ok(output)
        }
        Err(e) => Ok(serde_json::json!({
            "digest": replay_state.transaction.digest.0,
            "local_success": false,
            "local_error": e.to_string(),
            "execution_path": execution_path,
            "commands_executed": 0,
        })),
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
    package_bytecodes: HashMap<String, Vec<Vec<u8>>>,
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
        for fw in ["0x1", "0x2", "0x3"] {
            visited.insert(AccountAddress::from_hex_literal(fw).unwrap());
        }
        to_fetch.push_back(
            AccountAddress::from_hex_literal(package_id)
                .with_context(|| format!("invalid package address: {}", package_id))?,
        );

        const MAX_DEP_ROUNDS: usize = 20;
        let mut rounds = 0;
        while let Some(addr) = to_fetch.pop_front() {
            if visited.contains(&addr) || is_framework_address(&addr) {
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
///     prefetch_depth: Dynamic field prefetch depth
///     prefetch_limit: Dynamic field prefetch limit per parent
///     auto_system_objects: Auto-inject Clock/Random when missing
///     no_prefetch: Disable dynamic field prefetch
///     compare: Compare local execution with on-chain effects
///     analyze_only: Skip VM execution, just inspect state hydration
///     verbose: Enable verbose logging to stderr
///
/// Returns: dict with replay results (or analysis summary if analyze_only)
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    source="hybrid",
    checkpoint=None,
    state_file=None,
    cache_dir=None,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    verbose=false,
))]
fn replay(
    py: Python<'_>,
    digest: Option<&str>,
    rpc_url: &str,
    source: &str,
    checkpoint: Option<u64>,
    state_file: Option<&str>,
    cache_dir: Option<&str>,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let digest_owned = digest.map(|s| s.to_string());
    let rpc_url_owned = rpc_url.to_string();
    let source_owned = source.to_string();
    let state_file_owned = state_file.map(PathBuf::from);
    let cache_dir_owned = cache_dir.map(PathBuf::from);
    let value = py
        .allow_threads(move || {
            let digest = digest_owned.as_deref();
            let source_is_local = source_owned.eq_ignore_ascii_case("local");
            let use_local_cache = source_is_local || cache_dir_owned.is_some();

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
                    allow_fallback,
                    auto_system_objects,
                    compare,
                    analyze_only,
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
                    allow_fallback,
                    auto_system_objects,
                    compare,
                    analyze_only,
                    verbose,
                );
            }

            let digest = digest.ok_or_else(|| anyhow!("digest is required"))?;
            replay_inner(
                digest,
                &rpc_url_owned,
                &source_owned,
                checkpoint,
                allow_fallback,
                prefetch_depth,
                prefetch_limit,
                auto_system_objects,
                no_prefetch,
                compare,
                analyze_only,
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
///     package_bytecodes: Dict mapping package_id -> list of module bytecodes
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

    // Parse package_bytecodes from Python dict
    let mut parsed_pkg_bytes: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    if let Some(ref pb) = package_bytecodes {
        for (key, value) in pb.iter() {
            let pkg_id: String = key.extract()?;
            let bytecodes: Vec<Vec<u8>> = value.extract()?;
            parsed_pkg_bytes.insert(pkg_id, bytecodes);
        }
    }

    // Release GIL during VM execution
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
    m.add_function(wrap_pyfunction!(import_state, m)?)?;
    m.add_function(wrap_pyfunction!(deserialize_transaction, m)?)?;
    m.add_function(wrap_pyfunction!(deserialize_package, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_package_bytecodes, m)?)?;
    m.add_function(wrap_pyfunction!(json_to_bcs, m)?)?;
    m.add_function(wrap_pyfunction!(transaction_json_to_bcs, m)?)?;
    m.add_function(wrap_pyfunction!(call_view_function, m)?)?;
    m.add_function(wrap_pyfunction!(fuzz_function, m)?)?;
    m.add_function(wrap_pyfunction!(replay, m)?)?;
    Ok(())
}
