//! Generic historical view execution helper shared by Rust and Python.
//!
//! Protocol-specific logic should live in callers (examples/adapters), while this
//! module provides reusable tooling to:
//! - load a versions snapshot
//! - hydrate required historical objects + package closure
//! - execute a local Move view function

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
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

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "no" | "off")
        }
        Err(_) => default,
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn block_on_optional<F, T>(future: F) -> Option<T>
where
    F: Future<Output = Option<T>> + Send + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().ok()?;
            rt.block_on(future)
        })
        .join()
        .ok()
        .flatten();
    }

    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(future)
}

fn block_on_result<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    rt.block_on(future)
}

impl HistoricalViewRequest {
    /// Create a generic historical-view request.
    pub fn new(
        package_id: impl Into<String>,
        module: impl Into<String>,
        function: impl Into<String>,
    ) -> Self {
        Self {
            package_id: package_id.into(),
            module: module.into(),
            function: function.into(),
            type_args: Vec::new(),
            required_objects: Vec::new(),
            package_roots: Vec::new(),
            type_refs: Vec::new(),
            fetch_child_objects: true,
        }
    }

    pub fn with_type_args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.type_args = values.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_required_objects<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.required_objects = values.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_package_roots<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.package_roots = values.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_type_refs<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.type_refs = values.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_fetch_child_objects(mut self, enabled: bool) -> Self {
        self.fetch_child_objects = enabled;
        self
    }

    pub fn validate(&self) -> Result<()> {
        validate_request(self)
    }
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
    request.validate()?;

    let checkpoint = snapshot.checkpoint;
    let mut historical_versions = snapshot.versions.clone();
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

    let object_inputs = block_on_result(fetch_required_object_inputs(
        &resolved_endpoint,
        resolved_api_key.clone(),
        &historical_versions,
        &request.required_objects,
    ))?;
    let mut prefetched_object_inputs = Vec::new();
    if request.fetch_child_objects && env_bool("SUI_HISTORICAL_AUTO_HYDRATE_DYNAMIC_FIELDS", true) {
        let (dynamic_inputs, dynamic_versions) =
            block_on_result(fetch_dynamic_field_object_inputs(
                &resolved_endpoint,
                resolved_api_key.clone(),
                checkpoint,
                &historical_versions,
                &request.required_objects,
            ))
            .with_context(|| {
                format!(
                    "failed auto-hydrating dynamic fields at checkpoint {}",
                    checkpoint
                )
            })?;

        prefetched_object_inputs = filter_unique_object_inputs(&object_inputs, dynamic_inputs);
        for (object_id, version) in dynamic_versions {
            insert_object_version_aliases(&mut historical_versions, &object_id, version);
        }
        if env_bool("SUI_HISTORICAL_DYNAMIC_FIELD_LOG", false)
            && !prefetched_object_inputs.is_empty()
        {
            eprintln!(
                "  INFO: auto-hydrated {} dynamic field object(s) for historical view replay",
                prefetched_object_inputs.len()
            );
        }
    }
    let packages = block_on_result(fetch_historical_packages(
        &resolved_endpoint,
        resolved_api_key.clone(),
        checkpoint,
        &package_roots,
        &type_refs,
    ))?;

    let raw = execute_view_call(
        request,
        object_inputs,
        prefetched_object_inputs,
        checkpoint,
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

fn insert_object_version_aliases(
    versions: &mut HashMap<String, u64>,
    object_id: &str,
    version: u64,
) {
    versions.insert(object_id.to_string(), version);
    versions.insert(normalize_address(object_id), version);
    if let Ok(addr) = AccountAddress::from_hex_literal(object_id) {
        versions.insert(addr.to_hex_literal(), version);
    }
}

fn filter_unique_object_inputs(
    base: &[ViewObjectInput],
    extra: Vec<ViewObjectInput>,
) -> Vec<ViewObjectInput> {
    let mut seen = HashSet::new();
    for input in base {
        seen.insert(normalize_address(&input.object_id));
    }

    let mut unique = Vec::new();
    for input in extra {
        let key = normalize_address(&input.object_id);
        if seen.insert(key) {
            unique.push(input);
        }
    }
    unique
}

fn normalize_object_id_candidate(raw: &str) -> Option<String> {
    let candidate = raw.trim();
    if !candidate.starts_with("0x") {
        return None;
    }
    if candidate.len() < 3 || candidate.len() > 66 {
        return None;
    }
    if !candidate[2..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    AccountAddress::from_hex_literal(candidate)
        .ok()
        .map(|addr| addr.to_hex_literal())
}

fn collect_object_ids_from_json(
    value: &serde_json::Value,
    out: &mut HashSet<String>,
    max_items: usize,
) {
    if out.len() >= max_items {
        return;
    }
    match value {
        serde_json::Value::String(s) => {
            if let Some(id) = normalize_object_id_candidate(s) {
                out.insert(id);
            }
        }
        serde_json::Value::Array(values) => {
            for item in values {
                if out.len() >= max_items {
                    break;
                }
                collect_object_ids_from_json(item, out, max_items);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                if out.len() >= max_items {
                    break;
                }
                collect_object_ids_from_json(item, out, max_items);
            }
        }
        _ => {}
    }
}

pub fn load_json_or_yaml_file<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read document {}", path.display()))?;
    parse_json_or_yaml_str(&raw, &path.display().to_string())
}

pub fn parse_json_or_yaml_str<T: DeserializeOwned>(raw: &str, label: &str) -> Result<T> {
    serde_json::from_str(raw)
        .or_else(|_| serde_yaml::from_str(raw))
        .with_context(|| format!("Failed to parse JSON/YAML document {}", label))
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
        insert_object_version_aliases(&mut versions, object_id, version);
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

async fn fetch_dynamic_field_object_inputs(
    grpc_endpoint: &str,
    grpc_api_key: Option<String>,
    checkpoint: u64,
    historical_versions: &HashMap<String, u64>,
    parent_object_ids: &[String],
) -> Result<(Vec<ViewObjectInput>, HashMap<String, u64>)> {
    let max_depth = env_usize("SUI_HISTORICAL_DYNAMIC_FIELD_DEPTH", 2).max(1);
    let per_parent_limit = env_usize("SUI_HISTORICAL_DYNAMIC_FIELD_LIMIT", 64).max(1);
    let max_objects = env_usize("SUI_HISTORICAL_DYNAMIC_FIELD_MAX_OBJECTS", 512).max(1);
    let parent_scan_limit = env_usize("SUI_HISTORICAL_DYNAMIC_FIELD_PARENT_SCAN_LIMIT", 512).max(1);

    let graphql = GraphQLClient::new(&resolve_graphql_endpoint(grpc_endpoint));
    let grpc = GrpcClient::with_api_key(grpc_endpoint, grpc_api_key)
        .await
        .context("Failed to create gRPC client for dynamic-field hydration")?;

    let mut collected_inputs = Vec::new();
    let mut collected_versions: HashMap<String, u64> = HashMap::new();
    let mut seen_object_ids: HashSet<String> = HashSet::new();
    let mut queried_parents: HashSet<String> = HashSet::new();
    let mut seed_parent_ids: Vec<String> = parent_object_ids.to_vec();
    if env_bool("SUI_HISTORICAL_DISCOVER_PARENT_OBJECTS", true) {
        let mut discovered = HashSet::new();
        for parent_id in parent_object_ids {
            let object = graphql
                .fetch_object_at_checkpoint(parent_id, checkpoint)
                .or_else(|_| graphql.fetch_object(parent_id));
            let Ok(object) = object else {
                continue;
            };
            if let Some(content_json) = object.content_json.as_ref() {
                collect_object_ids_from_json(content_json, &mut discovered, parent_scan_limit);
            }
            if discovered.len() >= parent_scan_limit {
                break;
            }
        }
        for id in discovered {
            if !seed_parent_ids
                .iter()
                .any(|existing| normalize_address(existing) == normalize_address(&id))
            {
                seed_parent_ids.push(id);
            }
        }
    }
    let mut frontier: Vec<String> = seed_parent_ids;

    for _depth in 0..max_depth {
        if frontier.is_empty() || collected_inputs.len() >= max_objects {
            break;
        }

        let current = std::mem::take(&mut frontier);
        let mut next_frontier = Vec::new();

        for parent_id in current {
            if !queried_parents.insert(normalize_address(&parent_id)) {
                continue;
            }

            let fields = match graphql.fetch_dynamic_fields_at_checkpoint(
                &parent_id,
                per_parent_limit,
                checkpoint,
            ) {
                Ok(values) => values,
                Err(_) => match graphql.fetch_dynamic_fields(&parent_id, per_parent_limit) {
                    Ok(values) => values,
                    Err(_) => continue,
                },
            };

            for field in fields {
                if collected_inputs.len() >= max_objects {
                    break;
                }
                let Some(object_id) = field.object_id else {
                    continue;
                };
                if !seen_object_ids.insert(normalize_address(&object_id)) {
                    continue;
                }

                let version_hint = field
                    .version
                    .or_else(|| lookup_version(historical_versions, &object_id))
                    .or_else(|| lookup_version(&collected_versions, &object_id));

                let mut fetched = grpc.get_object_at_version(&object_id, version_hint).await?;
                if fetched.is_none() && version_hint.is_some() {
                    fetched = grpc.get_object_at_version(&object_id, None).await?;
                }
                let Some(fetched) = fetched else {
                    continue;
                };
                let Some(type_tag) = fetched.type_string.clone() else {
                    continue;
                };
                let Some(bcs_bytes) = fetched.bcs.clone() else {
                    continue;
                };
                let is_shared = matches!(fetched.owner, GrpcOwner::Shared { .. });

                collected_inputs.push(ViewObjectInput {
                    object_id: object_id.clone(),
                    bcs_bytes,
                    type_tag,
                    is_shared,
                });
                insert_object_version_aliases(&mut collected_versions, &object_id, fetched.version);
                next_frontier.push(object_id);
            }
        }

        frontier = next_frontier;
    }

    Ok((collected_inputs, collected_versions))
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
    prefetched_object_inputs: Vec<ViewObjectInput>,
    checkpoint: u64,
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
        let historical_versions_for_fetcher = Arc::new(Mutex::new(historical_versions.clone()));
        let queried_dynamic_parents_for_fetcher = Arc::new(Mutex::new(HashSet::<String>::new()));
        let grpc_config = Arc::new((grpc_endpoint.to_string(), grpc_api_key));
        let graphql_client_for_fetcher =
            Arc::new(GraphQLClient::new(&resolve_graphql_endpoint(grpc_endpoint)));
        let dynamic_field_limit_for_fetcher =
            env_usize("SUI_HISTORICAL_DYNAMIC_FIELD_LIMIT", 64).max(1);
        let auto_hydrate_dynamic_for_fetcher =
            env_bool("SUI_HISTORICAL_AUTO_HYDRATE_DYNAMIC_FIELDS", true);
        let dynamic_field_log_for_fetcher = env_bool("SUI_HISTORICAL_DYNAMIC_FIELD_LOG", false);

        let fetcher: crate::sandbox_runtime::ChildFetcherFn = Box::new(move |parent, child| {
            let parent_id = parent.to_hex_literal();
            let child_id = child.to_hex_literal();
            let mut version_hint = historical_versions_for_fetcher
                .lock()
                .ok()
                .and_then(|versions| lookup_version(&versions, &child_id));

            let fetch_grpc_config = grpc_config.clone();
            let fetch_child_id = child_id.clone();
            let fetch_version_hint = version_hint;
            let mut fetched = block_on_optional(async move {
                let client =
                    GrpcClient::with_api_key(&fetch_grpc_config.0, fetch_grpc_config.1.clone())
                        .await
                        .ok()?;
                let mut fetched = client
                    .get_object_at_version(&fetch_child_id, fetch_version_hint)
                    .await
                    .ok()
                    .flatten();
                if fetched.is_none() && fetch_version_hint.is_some() {
                    fetched = client
                        .get_object_at_version(&fetch_child_id, None)
                        .await
                        .ok()
                        .flatten();
                }
                fetched
            });

            let needs_hydration = fetched
                .as_ref()
                .map(|obj| obj.bcs.is_none() || obj.type_string.is_none())
                .unwrap_or(true);

            if needs_hydration && auto_hydrate_dynamic_for_fetcher {
                let should_query_parent = queried_dynamic_parents_for_fetcher
                    .lock()
                    .ok()
                    .map(|mut queried| queried.insert(normalize_address(&parent_id)))
                    .unwrap_or(false);
                if should_query_parent {
                    let fields = graphql_client_for_fetcher
                        .fetch_dynamic_fields_at_checkpoint(
                            &parent_id,
                            dynamic_field_limit_for_fetcher,
                            checkpoint,
                        )
                        .or_else(|_| {
                            graphql_client_for_fetcher
                                .fetch_dynamic_fields(&parent_id, dynamic_field_limit_for_fetcher)
                        })
                        .unwrap_or_default();
                    if dynamic_field_log_for_fetcher {
                        eprintln!(
                            "  INFO: child-fetch hydration parent={} fields={} limit={} checkpoint={}",
                            parent_id,
                            fields.len(),
                            dynamic_field_limit_for_fetcher,
                            checkpoint
                        );
                    }
                    if let Ok(mut versions) = historical_versions_for_fetcher.lock() {
                        for field in fields {
                            if let (Some(object_id), Some(version)) =
                                (field.object_id, field.version)
                            {
                                insert_object_version_aliases(&mut versions, &object_id, version);
                            }
                        }
                    }
                }

                version_hint = historical_versions_for_fetcher
                    .lock()
                    .ok()
                    .and_then(|versions| lookup_version(&versions, &child_id));

                let retry_grpc_config = grpc_config.clone();
                let retry_child_id = child_id.clone();
                let retry_version_hint = version_hint;
                fetched = block_on_optional(async move {
                    let client =
                        GrpcClient::with_api_key(&retry_grpc_config.0, retry_grpc_config.1.clone())
                            .await
                            .ok()?;
                    let mut fetched = client
                        .get_object_at_version(&retry_child_id, retry_version_hint)
                        .await
                        .ok()
                        .flatten();
                    if fetched.is_none() && retry_version_hint.is_some() {
                        fetched = client
                            .get_object_at_version(&retry_child_id, None)
                            .await
                            .ok()
                            .flatten();
                    }
                    fetched
                });
            }

            let fetched = fetched.and_then(|obj| {
                if obj.bcs.is_some() && obj.type_string.is_some() {
                    Some(obj)
                } else {
                    None
                }
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

    for obj in &prefetched_object_inputs {
        let id = AccountAddress::from_hex_literal(&obj.object_id)
            .with_context(|| format!("invalid prefetched object id: {}", obj.object_id))?;
        let type_tag = crate::types::parse_type_tag(&obj.type_tag)
            .with_context(|| format!("invalid prefetched type tag: {}", obj.type_tag))?;
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
        let _ = executor
            .add_object_input(input)
            .with_context(|| format!("add prefetched object input {}", obj.object_id))?;
    }

    let mut parsed_type_args = Vec::with_capacity(request.type_args.len());
    for type_arg in &request.type_args {
        parsed_type_args.push(
            crate::types::parse_type_tag(type_arg)
                .with_context(|| format!("invalid type arg: {}", type_arg))?,
        );
    }

    let args: Vec<Argument> = input_indices.iter().copied().map(Argument::Input).collect();

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

    #[test]
    fn historical_view_request_builder_sets_fields_and_validates() {
        let request = HistoricalViewRequest::new("0x2", "coin", "supply")
            .with_type_args(["0x2::sui::SUI"])
            .with_required_objects(["0x6"])
            .with_package_roots(["0x2"])
            .with_type_refs(["0x2::sui::SUI"])
            .with_fetch_child_objects(false);
        request.validate().expect("request validates");
        assert_eq!(request.package_id, "0x2");
        assert_eq!(request.module, "coin");
        assert_eq!(request.function, "supply");
        assert_eq!(request.type_args, vec!["0x2::sui::SUI"]);
        assert_eq!(request.required_objects, vec!["0x6"]);
        assert_eq!(request.package_roots, vec!["0x2"]);
        assert_eq!(request.type_refs, vec!["0x2::sui::SUI"]);
        assert!(!request.fetch_child_objects);
    }
}
