//! Generic historical view execution helper shared by Rust and Python.
//!
//! Protocol-specific logic should live in callers (examples/adapters), while this
//! module provides reusable tooling to:
//! - load a versions snapshot
//! - hydrate required historical objects + package closure
//! - execute a local Move view function

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use serde::{Deserialize, Serialize};
use sui_resolver::address::normalize_address;
use sui_state_fetcher::{HistoricalStateProvider, PackageData};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{historical_endpoint_and_api_key_from_env, GrpcClient, GrpcOwner};
use sui_transport::network::resolve_graphql_endpoint;

use crate::bootstrap::archive_runtime_gap_hint;
use crate::ptb::{Argument, Command, ObjectInput, PTBExecutor};
use crate::resolver::LocalModuleResolver;
use crate::utilities::collect_required_package_roots_from_type_strings;
use crate::vm::{SimulationConfig, VMHarness};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalViewRequest {
    pub package_id: String,
    pub module: String,
    pub function: String,
    #[serde(default)]
    pub type_args: Vec<String>,
    #[serde(default)]
    pub required_objects: Vec<String>,
    #[serde(default)]
    pub package_roots: Vec<String>,
    #[serde(default)]
    pub type_refs: Vec<String>,
    #[serde(default = "default_fetch_child_objects")]
    pub fetch_child_objects: bool,
}

fn default_fetch_child_objects() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalViewOutput {
    pub versions_file: String,
    pub checkpoint: u64,
    pub grpc_endpoint: String,
    pub success: bool,
    pub gas_used: Option<u64>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalVersionsSnapshot {
    pub checkpoint: u64,
    pub versions: HashMap<String, u64>,
}

#[derive(Debug, Clone)]
struct ViewObjectInput {
    object_id: String,
    bcs_bytes: Vec<u8>,
    type_tag: String,
    is_shared: bool,
}

pub fn execute_historical_view_from_versions(
    versions_file: &Path,
    request: &HistoricalViewRequest,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
) -> Result<HistoricalViewOutput> {
    let (checkpoint, versions) = load_versions_snapshot(versions_file)?;
    let snapshot = HistoricalVersionsSnapshot {
        checkpoint,
        versions,
    };
    execute_historical_view_from_snapshot_with_label(
        &snapshot,
        versions_file.display().to_string(),
        request,
        grpc_endpoint,
        grpc_api_key,
    )
}

pub fn execute_historical_view_from_snapshot(
    snapshot: &HistoricalVersionsSnapshot,
    request: &HistoricalViewRequest,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
) -> Result<HistoricalViewOutput> {
    execute_historical_view_from_snapshot_with_label(
        snapshot,
        format!("<in-memory-checkpoint:{}>", snapshot.checkpoint),
        request,
        grpc_endpoint,
        grpc_api_key,
    )
}

fn execute_historical_view_from_snapshot_with_label(
    snapshot: &HistoricalVersionsSnapshot,
    versions_label: String,
    request: &HistoricalViewRequest,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
) -> Result<HistoricalViewOutput> {
    validate_request(request)?;

    let checkpoint = snapshot.checkpoint;
    let historical_versions = snapshot.versions.clone();
    let (resolved_endpoint, resolved_api_key) =
        resolve_grpc_endpoint_and_key(grpc_endpoint, grpc_api_key);

    let package_roots = if request.package_roots.is_empty() {
        vec![request.package_id.clone()]
    } else {
        request.package_roots.clone()
    };
    let type_refs = if request.type_refs.is_empty() {
        request.type_args.clone()
    } else {
        request.type_refs.clone()
    };

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    let object_inputs = rt.block_on(fetch_required_object_inputs(
        &resolved_endpoint,
        resolved_api_key.clone(),
        &historical_versions,
        &request.required_objects,
    ))?;
    let packages = rt.block_on(fetch_historical_packages(
        &resolved_endpoint,
        resolved_api_key.clone(),
        checkpoint,
        &package_roots,
        &type_refs,
    ))?;
    drop(rt);

    let raw = execute_view_call(
        request,
        object_inputs,
        historical_versions,
        &resolved_endpoint,
        resolved_api_key,
        packages,
    )?;

    let success = raw
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let gas_used = raw.get("gas_used").and_then(serde_json::Value::as_u64);
    let error = raw
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let hint = error
        .as_deref()
        .and_then(|err| archive_runtime_gap_hint(err, Some(&resolved_endpoint)));

    Ok(HistoricalViewOutput {
        versions_file: versions_label,
        checkpoint,
        grpc_endpoint: resolved_endpoint,
        success,
        gas_used,
        error,
        hint,
        raw,
    })
}

pub fn load_versions_snapshot(path: &Path) -> Result<(u64, HashMap<String, u64>)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read versions file {}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid JSON in {}", path.display()))?;

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
        versions.insert(normalize_address(object_id), version);
        if let Ok(addr) = AccountAddress::from_hex_literal(object_id) {
            versions.insert(addr.to_hex_literal(), version);
        }
    }

    Ok((checkpoint, versions))
}

fn validate_request(request: &HistoricalViewRequest) -> Result<()> {
    if request.package_id.trim().is_empty() {
        return Err(anyhow!("package_id is required"));
    }
    if request.module.trim().is_empty() {
        return Err(anyhow!("module is required"));
    }
    if request.function.trim().is_empty() {
        return Err(anyhow!("function is required"));
    }
    if request.required_objects.is_empty() {
        return Err(anyhow!("required_objects must not be empty"));
    }
    Ok(())
}

async fn fetch_required_object_inputs(
    grpc_endpoint: &str,
    grpc_api_key: Option<String>,
    historical_versions: &HashMap<String, u64>,
    required_objects: &[String],
) -> Result<Vec<ViewObjectInput>> {
    let grpc = GrpcClient::with_api_key(grpc_endpoint, grpc_api_key)
        .await
        .context("Failed to create gRPC client")?;

    let mut inputs = Vec::with_capacity(required_objects.len());
    for object_id in required_objects {
        let version = lookup_version(historical_versions, object_id).ok_or_else(|| {
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
            .ok_or_else(|| anyhow!("object {} not found at version {}", object_id, version))?;
        let bcs_bytes = fetched
            .bcs
            .ok_or_else(|| anyhow!("object {} missing BCS payload", object_id))?;
        let type_tag = fetched.type_string.ok_or_else(|| {
            anyhow!(
                "object {} missing type string; cannot build view input",
                object_id
            )
        })?;
        let is_shared = matches!(fetched.owner, GrpcOwner::Shared { .. });
        inputs.push(ViewObjectInput {
            object_id: object_id.clone(),
            bcs_bytes,
            type_tag,
            is_shared,
        });
    }

    Ok(inputs)
}

async fn fetch_historical_packages(
    grpc_endpoint: &str,
    grpc_api_key: Option<String>,
    checkpoint: u64,
    package_roots: &[String],
    type_refs: &[String],
) -> Result<HashMap<AccountAddress, PackageData>> {
    let mut explicit_roots = Vec::with_capacity(package_roots.len());
    for package_id in package_roots {
        explicit_roots.push(
            AccountAddress::from_hex_literal(package_id)
                .with_context(|| format!("invalid package id: {}", package_id))?,
        );
    }
    let package_roots: Vec<AccountAddress> =
        collect_required_package_roots_from_type_strings(&explicit_roots, type_refs)?
            .into_iter()
            .collect();

    let grpc = GrpcClient::with_api_key(grpc_endpoint, grpc_api_key)
        .await
        .context("Failed to create gRPC client")?;
    let graphql = GraphQLClient::new(&resolve_graphql_endpoint(
        "https://fullnode.mainnet.sui.io:443",
    ));
    let provider = HistoricalStateProvider::with_clients(grpc, graphql);
    provider
        .fetch_packages_with_deps(&package_roots, None, Some(checkpoint))
        .await
        .context("Failed to fetch historical packages with deps")
}

fn execute_view_call(
    request: &HistoricalViewRequest,
    object_inputs: Vec<ViewObjectInput>,
    historical_versions: HashMap<String, u64>,
    grpc_endpoint: &str,
    grpc_api_key: Option<String>,
    packages: HashMap<AccountAddress, PackageData>,
) -> Result<serde_json::Value> {
    let mut resolver = LocalModuleResolver::with_sui_framework()?;
    let package_versions = register_packages_with_metadata(&mut resolver, &packages)?;

    let mut vm = VMHarness::with_config(&resolver, false, SimulationConfig::default())?;
    let aliases: HashMap<AccountAddress, AccountAddress> =
        resolver.get_all_aliases().into_iter().collect();
    if !aliases.is_empty() {
        vm.set_address_aliases_with_versions(aliases, package_versions);
    }

    if request.fetch_child_objects {
        let historical_versions_for_fetcher = Arc::new(historical_versions.clone());
        let grpc_config = Arc::new((grpc_endpoint.to_string(), grpc_api_key));
        let fetcher: crate::sandbox_runtime::ChildFetcherFn = Box::new(move |_parent, child| {
            let child_id = child.to_hex_literal();
            let version_hint = lookup_version(&historical_versions_for_fetcher, &child_id);
            let rt = tokio::runtime::Runtime::new().ok()?;
            let fetched = rt.block_on(async {
                let client = GrpcClient::with_api_key(&grpc_config.0, grpc_config.1.clone())
                    .await
                    .ok()?;
                client
                    .get_object_at_version(&child_id, version_hint)
                    .await
                    .ok()
                    .flatten()
            })?;
            let type_tag = crate::types::parse_type_tag(fetched.type_string.as_deref()?).ok()?;
            let bcs = fetched.bcs?;
            Some((type_tag, bcs))
        });
        vm.set_child_fetcher(fetcher);
    }

    let mut executor = PTBExecutor::new(&mut vm);
    let mut input_indices = Vec::new();
    for obj in &object_inputs {
        let id = AccountAddress::from_hex_literal(&obj.object_id)
            .with_context(|| format!("invalid object id: {}", obj.object_id))?;
        let type_tag = crate::types::parse_type_tag(&obj.type_tag)
            .with_context(|| format!("invalid type tag: {}", obj.type_tag))?;
        let version = lookup_version(&historical_versions, &obj.object_id);
        let input = if obj.is_shared {
            ObjectInput::Shared {
                id,
                bytes: obj.bcs_bytes.clone(),
                type_tag: Some(type_tag),
                version,
                mutable: false,
            }
        } else {
            ObjectInput::ImmRef {
                id,
                bytes: obj.bcs_bytes.clone(),
                type_tag: Some(type_tag),
                version,
            }
        };
        input_indices.push(
            executor
                .add_object_input(input)
                .with_context(|| format!("add object input {}", obj.object_id))?,
        );
    }

    let mut parsed_type_args = Vec::with_capacity(request.type_args.len());
    for type_arg in &request.type_args {
        parsed_type_args.push(
            crate::types::parse_type_tag(type_arg)
                .with_context(|| format!("invalid type arg: {}", type_arg))?,
        );
    }

    let args: Vec<Argument> = (0..input_indices.len() as u16)
        .map(Argument::Input)
        .collect();

    let command = Command::MoveCall {
        package: AccountAddress::from_hex_literal(&request.package_id)
            .with_context(|| format!("invalid package_id: {}", request.package_id))?,
        module: Identifier::new(request.module.as_str())
            .with_context(|| format!("invalid module: {}", request.module))?,
        function: Identifier::new(request.function.as_str())
            .with_context(|| format!("invalid function: {}", request.function))?,
        type_args: parsed_type_args,
        args,
    };

    let effects = executor.execute_commands(&[command])?;

    let return_values: Vec<Vec<String>> = effects
        .return_values
        .iter()
        .map(|cmd_returns| {
            cmd_returns
                .iter()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect()
        })
        .collect();
    let return_type_tags: Vec<Vec<Option<String>>> = effects
        .return_type_tags
        .iter()
        .map(|cmd_types| {
            cmd_types
                .iter()
                .map(|tt| tt.as_ref().map(|t| t.to_canonical_string(true)))
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

fn register_packages_with_metadata(
    resolver: &mut LocalModuleResolver,
    packages: &HashMap<AccountAddress, PackageData>,
) -> Result<HashMap<String, u64>> {
    let mut package_versions: HashMap<String, u64> = HashMap::new();
    let mut runtime_ids: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    for (storage, pkg) in packages {
        let runtime = infer_runtime_id(pkg);
        runtime_ids.insert(*storage, runtime);
        package_versions.insert(storage.to_hex_literal(), pkg.version);
    }

    let mut skipped_original: HashSet<AccountAddress> = HashSet::new();
    for (storage, runtime) in &runtime_ids {
        if storage != runtime && packages.contains_key(runtime) {
            skipped_original.insert(*runtime);
        }
    }

    let mut package_ids: Vec<AccountAddress> = packages.keys().copied().collect();
    package_ids.sort();
    for storage in &package_ids {
        if skipped_original.contains(storage) || is_framework_address(storage) {
            continue;
        }
        if let Some(pkg) = packages.get(storage) {
            resolver.add_package_modules_at(pkg.modules.clone(), Some(*storage))?;
        }
    }

    for storage in &package_ids {
        if skipped_original.contains(storage) {
            continue;
        }
        let Some(pkg) = packages.get(storage) else {
            continue;
        };
        let runtime = runtime_ids.get(storage).copied().unwrap_or(*storage);
        if storage != &runtime {
            resolver.add_address_alias(*storage, runtime);
            resolver.add_linkage_upgrade(runtime, *storage);
        }
        resolver.add_package_linkage(*storage, runtime, &pkg.linkage);
    }

    Ok(package_versions)
}

fn infer_runtime_id(pkg: &PackageData) -> AccountAddress {
    pkg.modules
        .iter()
        .find_map(|(_, bytes)| {
            CompiledModule::deserialize_with_defaults(bytes)
                .ok()
                .map(|module| *module.self_id().address())
        })
        .unwrap_or_else(|| pkg.runtime_id())
}

fn is_framework_address(addr: &AccountAddress) -> bool {
    matches!(addr.to_hex_literal().as_str(), "0x1" | "0x2" | "0x3")
}

fn lookup_version(historical_versions: &HashMap<String, u64>, object_id: &str) -> Option<u64> {
    historical_versions
        .get(object_id)
        .copied()
        .or_else(|| {
            historical_versions
                .get(&normalize_address(object_id))
                .copied()
        })
        .or_else(|| {
            AccountAddress::from_hex_literal(object_id)
                .ok()
                .and_then(|addr| historical_versions.get(&addr.to_hex_literal()).copied())
        })
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
    let env_api_key = std::env::var("SUI_GRPC_API_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let resolved_api_key = if endpoint_explicit {
        explicit_api_key.or(env_api_key)
    } else {
        explicit_api_key.or(default_api_key)
    };
    (resolved_endpoint, resolved_api_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_versions_snapshot_parses_checkpoint_and_versions() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(
            tmp.path(),
            serde_json::json!({
                "checkpoint": 1234,
                "objects": {
                    "0x6": { "version": 42 },
                    "0x0000000000000000000000000000000000000000000000000000000000000006": { "version": 42 }
                }
            })
            .to_string(),
        )
        .expect("write fixture");
        let (checkpoint, versions) = load_versions_snapshot(tmp.path()).expect("load snapshot");
        assert_eq!(checkpoint, 1234);
        assert_eq!(lookup_version(&versions, "0x6"), Some(42));
        assert_eq!(
            lookup_version(
                &versions,
                "0x0000000000000000000000000000000000000000000000000000000000000006"
            ),
            Some(42)
        );
    }
}
