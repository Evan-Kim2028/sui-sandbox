//! Python bindings for Sui Move package analysis and transaction replay.
//!
//! Exposes two main capabilities:
//! - `analyze_package`: Extract Move package interface from bytecode or GraphQL
//! - `analyze_replay`: Inspect replay state hydration for a transaction digest

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use sui_package_extractor::bytecode::{
    build_bytecode_interface_value_from_compiled_modules, extract_sanity_counts,
    read_local_compiled_modules,
};
use sui_state_fetcher::checkpoint_to_replay_state;
use sui_transport::graphql::GraphQLClient;
use sui_transport::network::{infer_network, resolve_graphql_endpoint};
use sui_transport::walrus::{self, WalrusClient};
use sui_types::transaction::TransactionDataAPI;

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

// ---------------------------------------------------------------------------
// Tokio runtime
// ---------------------------------------------------------------------------

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")
}

// ---------------------------------------------------------------------------
// analyze_package
// ---------------------------------------------------------------------------

/// Result of analyzing a Move package.
fn analyze_package_inner(
    package_id: Option<&str>,
    bytecode_dir: Option<&str>,
    rpc_url: &str,
    list_modules: bool,
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
        let pkg_id = dir_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("local")
            .to_string();
        let (module_names, interface_value) =
            build_bytecode_interface_value_from_compiled_modules(&pkg_id, &compiled)?;
        let counts = extract_sanity_counts(
            interface_value
                .get("modules")
                .unwrap_or(&serde_json::Value::Null),
        );

        let mut result = serde_json::json!({
            "source": "local-bytecode",
            "package_id": pkg_id,
            "modules": counts.modules,
            "structs": counts.structs,
            "functions": counts.functions,
            "key_structs": counts.key_structs,
        });
        if list_modules {
            result["module_names"] = serde_json::json!(module_names);
        }
        return Ok(result);
    }

    // Remote package via GraphQL
    let pkg_id_str = package_id.unwrap();
    let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
    let graphql = GraphQLClient::new(&graphql_endpoint);
    let pkg = graphql
        .fetch_package(pkg_id_str)
        .with_context(|| format!("fetch package {}", pkg_id_str))?;

    let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
    let mut names = Vec::with_capacity(pkg.modules.len());
    for module in pkg.modules {
        names.push(module.name.clone());
        let Some(b64) = module.bytecode_base64 else {
            continue;
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("decode module bytecode")?;
        let compiled =
            CompiledModule::deserialize_with_defaults(&bytes).context("deserialize module")?;
        compiled_modules.push(compiled);
    }
    names.sort();

    let (_, interface_value) =
        build_bytecode_interface_value_from_compiled_modules(&pkg.address, &compiled_modules)?;
    let counts = extract_sanity_counts(
        interface_value
            .get("modules")
            .unwrap_or(&serde_json::Value::Null),
    );

    let mut result = serde_json::json!({
        "source": "graphql",
        "package_id": pkg.address,
        "modules": counts.modules,
        "structs": counts.structs,
        "functions": counts.functions,
        "key_structs": counts.key_structs,
    });
    if list_modules {
        result["module_names"] = serde_json::json!(names);
    }
    Ok(result)
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
        let pkg_id = dir_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("local")
            .to_string();
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

    let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
    for module in pkg.modules {
        let Some(b64) = module.bytecode_base64 else {
            continue;
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("decode module bytecode")?;
        let compiled =
            CompiledModule::deserialize_with_defaults(&bytes).context("deserialize module")?;
        compiled_modules.push(compiled);
    }

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
    let rt = runtime()?;
    rt.block_on(async {
        let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
        let network = infer_network(rpc_url, &graphql_endpoint);

        // Build cache directory
        let cache_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".sui-sandbox")
            .join("cache")
            .join(&network);
        let cache = Arc::new(
            sui_state_fetcher::VersionedCache::with_storage(cache_dir)
                .context("Failed to create cache")?,
        );

        // Read gRPC config from env (same as CLI)
        let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
            .or_else(|_| std::env::var("SUI_GRPC_ARCHIVE_ENDPOINT"))
            .or_else(|_| std::env::var("SUI_GRPC_HISTORICAL_ENDPOINT"))
            .unwrap_or_else(|_| rpc_url.to_string());
        let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

        let grpc_client =
            sui_transport::grpc::GrpcClient::with_api_key(&grpc_endpoint, api_key).await?;
        let graphql_client = GraphQLClient::new(&graphql_endpoint);

        let mut provider =
            sui_state_fetcher::HistoricalStateProvider::with_clients(grpc_client, graphql_client)
                .with_cache(cache);

        if source == "walrus" || source == "hybrid" {
            provider = provider
                .with_walrus_from_env()
                .with_local_object_store_from_env();
        }

        let provider = Arc::new(provider);

        let replay_config = sui_state_fetcher::ReplayStateConfig {
            prefetch_dynamic_fields: !no_prefetch,
            df_depth: prefetch_depth,
            df_limit: prefetch_limit,
            auto_system_objects,
        };

        let replay_state = provider
            .replay_state_builder()
            .with_config(replay_config)
            .build(digest)
            .await
            .context("Failed to fetch replay state")?;

        // Summarize inputs
        let mut input_summary = serde_json::json!({
            "total": replay_state.transaction.inputs.len(),
            "pure": 0,
            "owned": 0,
            "shared_mutable": 0,
            "shared_immutable": 0,
            "immutable": 0,
            "receiving": 0,
        });
        let mut input_objects: Vec<serde_json::Value> = Vec::new();

        for input in &replay_state.transaction.inputs {
            match input {
                sui_sandbox_types::TransactionInput::Pure { .. } => {
                    input_summary["pure"] =
                        serde_json::json!(input_summary["pure"].as_u64().unwrap_or(0) + 1);
                }
                sui_sandbox_types::TransactionInput::Object { object_id, .. } => {
                    input_summary["owned"] =
                        serde_json::json!(input_summary["owned"].as_u64().unwrap_or(0) + 1);
                    if verbose {
                        input_objects.push(serde_json::json!({
                            "id": object_id,
                            "kind": "owned",
                        }));
                    }
                }
                sui_sandbox_types::TransactionInput::SharedObject {
                    object_id, mutable, ..
                } => {
                    if *mutable {
                        input_summary["shared_mutable"] = serde_json::json!(
                            input_summary["shared_mutable"].as_u64().unwrap_or(0) + 1
                        );
                    } else {
                        input_summary["shared_immutable"] = serde_json::json!(
                            input_summary["shared_immutable"].as_u64().unwrap_or(0) + 1
                        );
                    }
                    if verbose {
                        input_objects.push(serde_json::json!({
                            "id": object_id,
                            "kind": "shared",
                            "mutable": mutable,
                        }));
                    }
                }
                sui_sandbox_types::TransactionInput::ImmutableObject { object_id, .. } => {
                    input_summary["immutable"] =
                        serde_json::json!(input_summary["immutable"].as_u64().unwrap_or(0) + 1);
                    if verbose {
                        input_objects.push(serde_json::json!({
                            "id": object_id,
                            "kind": "immutable",
                        }));
                    }
                }
                sui_sandbox_types::TransactionInput::Receiving { object_id, .. } => {
                    input_summary["receiving"] =
                        serde_json::json!(input_summary["receiving"].as_u64().unwrap_or(0) + 1);
                    if verbose {
                        input_objects.push(serde_json::json!({
                            "id": object_id,
                            "kind": "receiving",
                        }));
                    }
                }
            }
        }

        // Summarize commands
        let mut command_summaries: Vec<serde_json::Value> = Vec::new();
        for cmd in &replay_state.transaction.commands {
            let summary = match cmd {
                sui_sandbox_types::PtbCommand::MoveCall {
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
                sui_sandbox_types::PtbCommand::SplitCoins { amounts, .. } => serde_json::json!({
                    "kind": "SplitCoins",
                    "type_args": 0,
                    "args": 1 + amounts.len(),
                }),
                sui_sandbox_types::PtbCommand::MergeCoins { sources, .. } => serde_json::json!({
                    "kind": "MergeCoins",
                    "type_args": 0,
                    "args": 1 + sources.len(),
                }),
                sui_sandbox_types::PtbCommand::TransferObjects { objects, .. } => {
                    serde_json::json!({
                        "kind": "TransferObjects",
                        "type_args": 0,
                        "args": 1 + objects.len(),
                    })
                }
                sui_sandbox_types::PtbCommand::MakeMoveVec { elements, type_arg } => {
                    serde_json::json!({
                        "kind": "MakeMoveVec",
                        "type_args": usize::from(type_arg.is_some()),
                        "args": elements.len(),
                    })
                }
                sui_sandbox_types::PtbCommand::Publish { dependencies, .. } => {
                    serde_json::json!({
                        "kind": "Publish",
                        "type_args": 0,
                        "args": dependencies.len(),
                    })
                }
                sui_sandbox_types::PtbCommand::Upgrade { package, .. } => serde_json::json!({
                    "kind": "Upgrade",
                    "target": package,
                    "type_args": 0,
                    "args": 1,
                }),
            };
            command_summaries.push(summary);
        }

        // Count modules
        let mut modules_total = 0usize;
        for pkg in replay_state.packages.values() {
            modules_total += pkg.modules.len();
        }

        // Check missing inputs / packages
        let mut missing_inputs = Vec::new();
        for input in &replay_state.transaction.inputs {
            let id = match input {
                sui_sandbox_types::TransactionInput::Object { object_id, .. } => Some(object_id),
                sui_sandbox_types::TransactionInput::SharedObject { object_id, .. } => {
                    Some(object_id)
                }
                sui_sandbox_types::TransactionInput::ImmutableObject { object_id, .. } => {
                    Some(object_id)
                }
                sui_sandbox_types::TransactionInput::Receiving { object_id, .. } => Some(object_id),
                sui_sandbox_types::TransactionInput::Pure { .. } => None,
            };
            if let Some(id) = id {
                if let Ok(addr) = AccountAddress::from_hex_literal(id) {
                    if !replay_state.objects.contains_key(&addr) {
                        missing_inputs.push(addr.to_hex_literal());
                    }
                }
            }
        }

        let mut required_packages: BTreeSet<AccountAddress> = BTreeSet::new();
        for cmd in &replay_state.transaction.commands {
            if let sui_sandbox_types::PtbCommand::MoveCall {
                package,
                type_arguments,
                ..
            } = cmd
            {
                if let Ok(addr) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(addr);
                }
                for ty in type_arguments {
                    for pkg in sui_sandbox_core::utilities::extract_package_ids_from_type(ty) {
                        if let Ok(addr) = AccountAddress::from_hex_literal(&pkg) {
                            required_packages.insert(addr);
                        }
                    }
                }
            }
        }
        let mut missing_packages = Vec::new();
        for addr in &required_packages {
            if !replay_state.packages.contains_key(addr) {
                missing_packages.push(addr.to_hex_literal());
            }
        }

        // Object types (verbose)
        let object_types: Option<Vec<serde_json::Value>> = if verbose {
            Some(
                replay_state
                    .objects
                    .values()
                    .map(|obj| {
                        serde_json::json!({
                            "id": obj.id.to_hex_literal(),
                            "type_tag": obj.type_tag,
                            "version": obj.version,
                            "shared": obj.is_shared,
                            "immutable": obj.is_immutable,
                        })
                    })
                    .collect(),
            )
        } else {
            None
        };

        let package_ids: Option<Vec<String>> = if verbose {
            Some(
                replay_state
                    .packages
                    .keys()
                    .map(|id| id.to_hex_literal())
                    .collect(),
            )
        } else {
            None
        };

        let mut result = serde_json::json!({
            "digest": replay_state.transaction.digest.0,
            "sender": replay_state.transaction.sender.to_hex_literal(),
            "commands": replay_state.transaction.commands.len(),
            "inputs": replay_state.transaction.inputs.len(),
            "objects": replay_state.objects.len(),
            "packages": replay_state.packages.len(),
            "modules": modules_total,
            "input_summary": input_summary,
            "command_summaries": command_summaries,
            "hydration": {
                "source": source,
                "allow_fallback": allow_fallback,
                "auto_system_objects": auto_system_objects,
                "dynamic_field_prefetch": !no_prefetch,
                "prefetch_depth": prefetch_depth,
                "prefetch_limit": prefetch_limit,
            },
            "missing_inputs": missing_inputs,
            "missing_packages": missing_packages,
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
            if let Some(objs) = input_objects.into() {
                result["input_objects"] = serde_json::json!(objs);
            }
            if let Some(types) = object_types {
                result["object_types"] = serde_json::json!(types);
            }
            if let Some(ids) = package_ids {
                result["package_ids"] = serde_json::json!(ids);
            }
        }

        Ok(result)
    })
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
    // Use the CLI binary as a subprocess — the VM harness setup is too tightly
    // coupled to the binary's internal modules to replicate here cleanly.
    // This gives us the exact same behavior as `sui-sandbox replay --json`.
    let mut cmd = std::process::Command::new("sui-sandbox");
    cmd.arg("replay")
        .arg(digest)
        .arg("--json")
        .arg("--rpc-url")
        .arg(rpc_url);
    if compare {
        cmd.arg("--compare");
    }
    if verbose {
        cmd.arg("--verbose");
    }

    let output = cmd
        .output()
        .context("Failed to execute sui-sandbox binary. Is it installed and in PATH?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // Try to extract JSON from stderr or stdout
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
// Walrus functions
// ---------------------------------------------------------------------------

/// Get the latest checkpoint number from Walrus.
fn get_latest_checkpoint_inner() -> Result<u64> {
    let client = WalrusClient::mainnet();
    client.get_latest_checkpoint()
}

/// Fetch a checkpoint from Walrus and return a summary.
fn get_checkpoint_inner(checkpoint_num: u64) -> Result<serde_json::Value> {
    let client = WalrusClient::mainnet();
    let checkpoint_data = client.get_checkpoint(checkpoint_num)?;

    let seq = checkpoint_data.checkpoint_summary.sequence_number;
    let epoch = checkpoint_data.checkpoint_summary.epoch;
    let timestamp_ms = checkpoint_data.checkpoint_summary.timestamp_ms;

    // Extract transaction digests and summaries
    let mut transactions = Vec::new();
    for tx in &checkpoint_data.transactions {
        let digest = tx.transaction.digest().to_string();
        let tx_data = tx.transaction.data().transaction_data();
        let sender = format!("{}", tx_data.sender());

        // Count commands
        let command_count = match tx_data.kind() {
            sui_types::transaction::TransactionKind::ProgrammableTransaction(ptb) => {
                ptb.commands.len()
            }
            _ => 0,
        };

        // Count input/output objects
        let input_objects = tx.input_objects.len();
        let output_objects = tx.output_objects.len();

        transactions.push(serde_json::json!({
            "digest": digest,
            "sender": sender,
            "commands": command_count,
            "input_objects": input_objects,
            "output_objects": output_objects,
        }));
    }

    // Extract object versions
    let versions = walrus::extract_object_versions_from_checkpoint(&checkpoint_data);

    Ok(serde_json::json!({
        "checkpoint": seq,
        "epoch": epoch,
        "timestamp_ms": timestamp_ms,
        "transaction_count": checkpoint_data.transactions.len(),
        "transactions": transactions,
        "object_versions_count": versions.len(),
    }))
}

/// Walrus-first analyze replay: fetch checkpoint, convert to replay state, summarize.
/// No gRPC or API keys needed.
fn analyze_replay_walrus_inner(
    digest: &str,
    checkpoint_num: u64,
    verbose: bool,
) -> Result<serde_json::Value> {
    let client = WalrusClient::mainnet();
    let checkpoint_data = client.get_checkpoint(checkpoint_num)?;

    let replay_state = checkpoint_to_replay_state(&checkpoint_data, digest)?;

    // Build the same summary structure as analyze_replay_inner
    let mut input_summary = serde_json::json!({
        "total": replay_state.transaction.inputs.len(),
        "pure": 0, "owned": 0, "shared_mutable": 0,
        "shared_immutable": 0, "immutable": 0, "receiving": 0,
    });
    let mut input_objects: Vec<serde_json::Value> = Vec::new();

    for input in &replay_state.transaction.inputs {
        match input {
            sui_sandbox_types::TransactionInput::Pure { .. } => {
                input_summary["pure"] =
                    serde_json::json!(input_summary["pure"].as_u64().unwrap_or(0) + 1);
            }
            sui_sandbox_types::TransactionInput::Object { object_id, .. } => {
                input_summary["owned"] =
                    serde_json::json!(input_summary["owned"].as_u64().unwrap_or(0) + 1);
                if verbose {
                    input_objects.push(serde_json::json!({"id": object_id, "kind": "owned"}));
                }
            }
            sui_sandbox_types::TransactionInput::SharedObject {
                object_id, mutable, ..
            } => {
                if *mutable {
                    input_summary["shared_mutable"] = serde_json::json!(
                        input_summary["shared_mutable"].as_u64().unwrap_or(0) + 1
                    );
                } else {
                    input_summary["shared_immutable"] = serde_json::json!(
                        input_summary["shared_immutable"].as_u64().unwrap_or(0) + 1
                    );
                }
                if verbose {
                    input_objects.push(
                        serde_json::json!({"id": object_id, "kind": "shared", "mutable": mutable}),
                    );
                }
            }
            sui_sandbox_types::TransactionInput::ImmutableObject { object_id, .. } => {
                input_summary["immutable"] =
                    serde_json::json!(input_summary["immutable"].as_u64().unwrap_or(0) + 1);
                if verbose {
                    input_objects.push(serde_json::json!({"id": object_id, "kind": "immutable"}));
                }
            }
            sui_sandbox_types::TransactionInput::Receiving { object_id, .. } => {
                input_summary["receiving"] =
                    serde_json::json!(input_summary["receiving"].as_u64().unwrap_or(0) + 1);
                if verbose {
                    input_objects.push(serde_json::json!({"id": object_id, "kind": "receiving"}));
                }
            }
        }
    }

    // Summarize commands
    let mut command_summaries: Vec<serde_json::Value> = Vec::new();
    for cmd in &replay_state.transaction.commands {
        let summary = match cmd {
            sui_sandbox_types::PtbCommand::MoveCall {
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
            sui_sandbox_types::PtbCommand::SplitCoins { amounts, .. } => serde_json::json!({
                "kind": "SplitCoins", "type_args": 0, "args": 1 + amounts.len(),
            }),
            sui_sandbox_types::PtbCommand::MergeCoins { sources, .. } => serde_json::json!({
                "kind": "MergeCoins", "type_args": 0, "args": 1 + sources.len(),
            }),
            sui_sandbox_types::PtbCommand::TransferObjects { objects, .. } => serde_json::json!({
                "kind": "TransferObjects", "type_args": 0, "args": 1 + objects.len(),
            }),
            sui_sandbox_types::PtbCommand::MakeMoveVec { elements, type_arg } => {
                serde_json::json!({
                    "kind": "MakeMoveVec",
                    "type_args": usize::from(type_arg.is_some()),
                    "args": elements.len(),
                })
            }
            sui_sandbox_types::PtbCommand::Publish { dependencies, .. } => serde_json::json!({
                "kind": "Publish", "type_args": 0, "args": dependencies.len(),
            }),
            sui_sandbox_types::PtbCommand::Upgrade { package, .. } => serde_json::json!({
                "kind": "Upgrade", "target": package, "type_args": 0, "args": 1,
            }),
        };
        command_summaries.push(summary);
    }

    let mut modules_total = 0usize;
    for pkg in replay_state.packages.values() {
        modules_total += pkg.modules.len();
    }

    let mut result = serde_json::json!({
        "digest": replay_state.transaction.digest.0,
        "sender": replay_state.transaction.sender.to_hex_literal(),
        "commands": replay_state.transaction.commands.len(),
        "inputs": replay_state.transaction.inputs.len(),
        "objects": replay_state.objects.len(),
        "packages": replay_state.packages.len(),
        "modules": modules_total,
        "input_summary": input_summary,
        "command_summaries": command_summaries,
        "hydration": {
            "source": "walrus",
            "checkpoint": checkpoint_num,
        },
        "epoch": replay_state.epoch,
        "protocol_version": replay_state.protocol_version,
        "checkpoint": checkpoint_num,
    });

    if verbose {
        if !input_objects.is_empty() {
            result["input_objects"] = serde_json::json!(input_objects);
        }
        let package_ids: Vec<String> = replay_state
            .packages
            .keys()
            .map(|id| id.to_hex_literal())
            .collect();
        result["package_ids"] = serde_json::json!(package_ids);

        let object_types: Vec<serde_json::Value> = replay_state
            .objects
            .values()
            .map(|obj| {
                serde_json::json!({
                    "id": obj.id.to_hex_literal(),
                    "type_tag": obj.type_tag,
                    "version": obj.version,
                    "shared": obj.is_shared,
                    "immutable": obj.is_immutable,
                })
            })
            .collect();
        result["object_types"] = serde_json::json!(object_types);
    }

    Ok(result)
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
    let value = extract_interface_inner(package_id, bytecode_dir, rpc_url).map_err(to_py_err)?;
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
    Ok(())
}
