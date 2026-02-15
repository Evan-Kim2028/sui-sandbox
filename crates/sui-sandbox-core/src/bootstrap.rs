//! Shared bootstrap and hydration helpers for replay workflows.
//!
//! These helpers were originally implemented in example glue code and are now
//! first-class core APIs so both CLI and bindings can share the same flow.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use tokio::runtime::Runtime;

use sui_state_fetcher::{HistoricalStateProvider, PackageData};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcOwner, GrpcTransaction};
use sui_types::digests::TransactionDigest as SuiTransactionDigest;

use crate::fetcher::GrpcFetcher;
use crate::sandbox_runtime::ChildFetcherFn;
use crate::simulation::{FetcherConfig, SimulationEnvironment};
use crate::utilities::{parse_type_tag, GenericObjectPatcher};
use crate::vm::{SimulationConfig, DEFAULT_PROTOCOL_VERSION};

/// Default mainnet archival gRPC endpoint used by replay/historical helpers.
pub const MAINNET_ARCHIVE_GRPC_ENDPOINT: &str = "https://archive.mainnet.sui.io:443";

/// Recommended fallback endpoint when public archive misses runtime objects.
pub const SURFLUX_MAINNET_GRPC_ENDPOINT: &str = "https://grpc.surflux.dev:443";

/// Common fetched object tuple used by replay/bootstrap flows:
/// `(bcs_bytes, type_string, version, is_shared)`.
pub type BootstrapFetchedObjectData = (Vec<u8>, Option<String>, u64, bool);

/// Create a child fetcher function for on-demand object loading.
///
/// The child fetcher is called by the VM when it needs to access a child object
/// that wasn't pre-loaded. It fetches the object via gRPC at the historical version.
pub fn create_child_fetcher(
    grpc: GrpcClient,
    historical_versions: HashMap<String, u64>,
    patcher: Option<GenericObjectPatcher>,
) -> ChildFetcherFn {
    let grpc_arc = Arc::new(grpc);
    let historical_arc = Arc::new(historical_versions);
    let patcher_arc = Arc::new(parking_lot::Mutex::new(patcher));

    Box::new(
        move |_parent_id: AccountAddress, child_id: AccountAddress| {
            let child_id_str = child_id.to_hex_literal();
            let version = historical_arc.get(&child_id_str).copied();

            let rt = tokio::runtime::Runtime::new().ok()?;
            let result =
                rt.block_on(async { grpc_arc.get_object_at_version(&child_id_str, version).await });

            if let Ok(Some(obj)) = result {
                if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                    let final_bcs = {
                        let mut guard = patcher_arc.lock();
                        if let Some(ref mut p) = *guard {
                            p.patch_object(type_str, bcs)
                        } else {
                            bcs.clone()
                        }
                    };

                    if let Some(type_tag) = parse_type_tag(type_str) {
                        return Some((type_tag, final_bcs));
                    }
                }
            }

            None
        },
    )
}

/// Fetch object BCS data at an optional historical version with optional latest fallback.
pub fn fetch_object_data(
    rt: &Runtime,
    grpc: &GrpcClient,
    object_id: &str,
    historical_version: Option<u64>,
    allow_latest_fallback: bool,
) -> Option<BootstrapFetchedObjectData> {
    let at_version = historical_version.and_then(|version| {
        rt.block_on(async { grpc.get_object_at_version(object_id, Some(version)).await })
            .ok()
            .flatten()
            .and_then(|obj| {
                let is_shared = matches!(obj.owner, GrpcOwner::Shared { .. });
                let bcs = obj.bcs?;
                Some((bcs, obj.type_string, obj.version, is_shared))
            })
    });
    if at_version.is_some() {
        return at_version;
    }
    if historical_version.is_some() && !allow_latest_fallback {
        return None;
    }

    rt.block_on(async { grpc.get_object(object_id).await })
        .ok()
        .flatten()
        .and_then(|obj| {
            let is_shared = matches!(obj.owner, GrpcOwner::Shared { .. });
            let bcs = obj.bcs?;
            Some((bcs, obj.type_string, obj.version, is_shared))
        })
}

/// Preload dynamic-field wrapper objects for a list of parent object IDs.
///
/// Returns a map of dynamic-field object ID to fetched object tuple.
pub fn preload_dynamic_field_objects(
    rt: &Runtime,
    graphql: &GraphQLClient,
    grpc: &GrpcClient,
    parent_ids: &[&str],
    limit: usize,
) -> HashMap<String, BootstrapFetchedObjectData> {
    let mut loaded = HashMap::new();
    for parent_id in parent_ids {
        if let Ok(fields) = graphql.fetch_dynamic_fields(parent_id, limit) {
            for field in fields {
                let Some(obj_id) = field.object_id else {
                    continue;
                };
                if loaded.contains_key(&obj_id) {
                    continue;
                }
                if let Some((bcs, type_str, version, _is_shared)) =
                    fetch_object_data(rt, grpc, &obj_id, None, false)
                {
                    // Dynamic field wrapper objects are loaded as owned/non-shared.
                    loaded.insert(obj_id, (bcs, type_str, version, false));
                }
            }
        }
    }
    loaded
}

/// Load fetched object blobs into a simulation environment.
///
/// When `fail_on_error` is true, returns the first load error.
/// When false, skips failing objects and continues.
pub fn load_fetched_objects_into_env(
    env: &mut SimulationEnvironment,
    fetched: &HashMap<String, BootstrapFetchedObjectData>,
    fail_on_error: bool,
) -> Result<usize> {
    let mut loaded = 0usize;
    for (obj_id, (bcs, type_str, version, is_shared)) in fetched {
        let result = env.load_object_from_data(
            obj_id,
            bcs.clone(),
            type_str.as_deref(),
            *is_shared,
            false,
            *version,
        );
        match result {
            Ok(_) => loaded += 1,
            Err(err) if fail_on_error => {
                return Err(anyhow!("failed loading object {}: {}", obj_id, err));
            }
            Err(_) => {}
        }
    }
    Ok(loaded)
}

/// Create a mainnet `HistoricalStateProvider` with practical endpoint defaults.
///
/// Behavior:
/// - Uses `SUI_GRPC_ENDPOINT` when explicitly set.
/// - Uses `https://archive.mainnet.sui.io:443` by default when no explicit endpoint is set.
/// - Warns when historical mode uses a likely non-archival endpoint.
pub async fn create_mainnet_provider(historical_mode: bool) -> Result<HistoricalStateProvider> {
    let configured_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut endpoint = configured_endpoint
        .clone()
        .unwrap_or_else(|| MAINNET_ARCHIVE_GRPC_ENDPOINT.to_string());

    if historical_mode && endpoint.contains("fullnode.mainnet.sui.io") {
        if configured_endpoint.is_some() {
            eprintln!(
                "  WARNING: historical mode detected non-archival endpoint ({}); switching to {}",
                endpoint, MAINNET_ARCHIVE_GRPC_ENDPOINT
            );
        }
        endpoint = MAINNET_ARCHIVE_GRPC_ENDPOINT.to_string();
    } else if historical_mode && configured_endpoint.is_none() {
        endpoint = MAINNET_ARCHIVE_GRPC_ENDPOINT.to_string();
    }

    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
    let grpc = GrpcClient::with_api_key(&endpoint, api_key).await?;
    let graphql = GraphQLClient::mainnet();

    Ok(HistoricalStateProvider::with_clients(grpc, graphql))
}

/// Resolve the effective gRPC endpoint used by examples, honoring env overrides.
pub fn effective_grpc_endpoint_for_examples() -> String {
    std::env::var("SUI_GRPC_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| MAINNET_ARCHIVE_GRPC_ENDPOINT.to_string())
}

/// Whether the endpoint appears to be Mysten's public archive endpoint.
pub fn is_mainnet_archive_endpoint(endpoint: &str) -> bool {
    let lower = endpoint.to_ascii_lowercase();
    lower.contains("archive.mainnet.sui.io") || lower.contains("fullnode.mainnet.sui.io")
}

/// Heuristic for failures that often come from missing runtime objects in archive replay.
pub fn is_likely_runtime_object_gap_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("contractabort") && lower.contains("abort_code: 1"))
        || lower.contains("unchanged_loaded_runtime_objects")
        || lower.contains("missing runtime object")
        || lower.contains("missing object input")
        || (lower.contains("dynamic_field")
            && lower.contains("major_status: aborted")
            && lower.contains("sub_status: some(2)"))
}

/// Build an actionable runtime-gap hint when replay likely failed due to archive data limits.
pub fn archive_runtime_gap_hint(error_message: &str, endpoint: Option<&str>) -> Option<String> {
    if !is_likely_runtime_object_gap_error(error_message) {
        return None;
    }

    let endpoint = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(effective_grpc_endpoint_for_examples);
    if !is_mainnet_archive_endpoint(&endpoint) {
        return None;
    }

    Some(format!(
        "Likely archive runtime-object gap; current endpoint: {}. Retry with `SUI_GRPC_ENDPOINT={}`.",
        endpoint, SURFLUX_MAINNET_GRPC_ENDPOINT
    ))
}

/// Print an actionable hint when archive replay likely failed due to runtime-object gaps.
pub fn maybe_print_archive_runtime_hint(error_message: &str) {
    if let Some(hint) = archive_runtime_gap_hint(error_message, None) {
        println!("\n  INFO: {}", hint);
    }
}

/// Configure an environment to fetch missing objects from a mainnet endpoint.
///
/// This wires both:
/// - a top-level object fetcher (`GrpcFetcher`) for missing object inputs, and
/// - a child fetcher for dynamic-field/child-object lookups inside VM execution.
pub async fn configure_environment_mainnet_fetchers(
    env: &mut SimulationEnvironment,
    grpc_endpoint: &str,
    historical_versions: HashMap<String, u64>,
    patcher: Option<GenericObjectPatcher>,
    use_archive: bool,
) -> Result<()> {
    let endpoint = grpc_endpoint.trim();
    if endpoint.is_empty() {
        return Err(anyhow!("grpc endpoint must not be empty"));
    }

    env.set_fetcher(Box::new(GrpcFetcher::custom(endpoint)));
    env.set_fetcher_config(FetcherConfig {
        enabled: true,
        network: Some("mainnet".to_string()),
        endpoint: Some(endpoint.to_string()),
        use_archive,
    });

    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
    let child_grpc = GrpcClient::with_api_key(endpoint, api_key).await?;
    env.set_child_fetcher(create_child_fetcher(
        child_grpc,
        historical_versions,
        patcher,
    ));
    Ok(())
}

/// Derived package-linkage metadata used to register packages in a simulation environment.
#[derive(Debug, Clone, Default)]
pub struct PackageRegistrationPlan {
    /// storage address -> fetched package version
    pub package_versions: HashMap<AccountAddress, u64>,
    /// original/runtime package id -> upgraded/storage package id
    pub upgrade_map: HashMap<AccountAddress, AccountAddress>,
    /// upgraded/storage package id -> original/runtime package id
    pub original_id_map: HashMap<AccountAddress, AccountAddress>,
}

/// Build package-linkage registration maps from fetched package data.
pub fn build_package_registration_plan(
    packages: &HashMap<AccountAddress, PackageData>,
) -> PackageRegistrationPlan {
    let package_versions: HashMap<AccountAddress, u64> = packages
        .iter()
        .map(|(addr, pkg)| (*addr, pkg.version))
        .collect();

    let mut upgrade_map: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    for pkg in packages.values() {
        for (original, upgraded) in &pkg.linkage {
            if original != upgraded {
                upgrade_map.insert(*original, *upgraded);
            }
        }
    }

    let original_id_map: HashMap<AccountAddress, AccountAddress> = upgrade_map
        .iter()
        .map(|(original, upgraded)| (*upgraded, *original))
        .collect();

    PackageRegistrationPlan {
        package_versions,
        upgrade_map,
        original_id_map,
    }
}

/// Result summary for package registration into the simulation environment.
#[derive(Debug, Clone, Default)]
pub struct PackageRegistrationResult {
    pub loaded: usize,
    pub skipped_upgraded: usize,
    pub failed: Vec<(AccountAddress, String)>,
}

/// Register fetched packages in the simulation environment with full linkage metadata.
pub fn register_packages_with_linkage_plan(
    env: &mut SimulationEnvironment,
    packages: &HashMap<AccountAddress, PackageData>,
    plan: &PackageRegistrationPlan,
) -> PackageRegistrationResult {
    let mut result = PackageRegistrationResult::default();

    for (addr, pkg) in packages {
        // Skip packages superseded by an upgraded storage address.
        if plan.upgrade_map.contains_key(addr) {
            result.skipped_upgraded += 1;
            continue;
        }

        let original_id = plan.original_id_map.get(addr).copied();
        let linkage: BTreeMap<AccountAddress, (AccountAddress, u64)> = pkg
            .linkage
            .iter()
            .map(|(original, upgraded)| {
                let linked_version = plan.package_versions.get(upgraded).copied().unwrap_or(1);
                (*original, (*upgraded, linked_version))
            })
            .collect();

        match env.register_package_with_linkage(
            *addr,
            pkg.version,
            original_id,
            pkg.modules.clone(),
            linkage,
        ) {
            Ok(()) => result.loaded += 1,
            Err(err) => result.failed.push((*addr, err.to_string())),
        }
    }

    result
}

/// Validate package registration and return a readable error on failure.
pub fn ensure_package_registration_success(registration: &PackageRegistrationResult) -> Result<()> {
    if registration.failed.is_empty() {
        return Ok(());
    }

    let mut details = String::new();
    for (addr, err) in &registration.failed {
        if !details.is_empty() {
            details.push_str("; ");
        }
        details.push_str(&format!("{}: {}", addr.to_hex_literal(), err));
    }
    Err(anyhow!(
        "package registration failed ({} errors): {}",
        registration.failed.len(),
        details
    ))
}

/// Deploy package modules from one package id at a target package address.
///
/// Returns `Ok(true)` when source package modules were found and deployed.
/// Returns `Ok(false)` when the source package is not present in `packages`.
pub fn deploy_package_alias_if_present(
    env: &mut SimulationEnvironment,
    packages: &HashMap<AccountAddress, PackageData>,
    source: AccountAddress,
    target: &str,
) -> Result<bool> {
    let Some(pkg) = packages.get(&source) else {
        return Ok(false);
    };
    env.deploy_package_at_address(target, pkg.modules.clone())?;
    Ok(true)
}

/// Build a replay config directly from a gRPC transaction, resolving epoch metadata
/// via the gRPC client if needed.
pub fn build_replay_config_from_grpc(
    rt: &Runtime,
    grpc: &GrpcClient,
    grpc_tx: &GrpcTransaction,
) -> Result<SimulationConfig> {
    let digest_str = &grpc_tx.digest;
    let tx_hash = SuiTransactionDigest::from_str(digest_str)
        .map_err(|e| anyhow!("Invalid transaction digest {}: {}", digest_str, e))?
        .into_inner();

    // Resolve epoch metadata if missing
    let mut epoch = grpc_tx.epoch.unwrap_or(0);
    if epoch == 0 {
        if let Some(checkpoint) = grpc_tx.checkpoint {
            let cp_result = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), grpc.get_checkpoint(checkpoint)).await
            });
            if let Ok(Ok(Some(cp))) = cp_result {
                epoch = cp.epoch;
            }
        }
    }

    let mut protocol_version = 0u64;
    let mut reference_gas_price: Option<u64> = None;
    if epoch > 0 {
        let ep_result = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), grpc.get_epoch(Some(epoch))).await
        });
        if let Ok(Ok(Some(ep))) = ep_result {
            if let Some(pv) = ep.protocol_version {
                protocol_version = pv;
            }
            reference_gas_price = ep.reference_gas_price;
        }
    }

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let protocol_version = if protocol_version > 0 {
        protocol_version
    } else {
        DEFAULT_PROTOCOL_VERSION
    }
    .min(DEFAULT_PROTOCOL_VERSION);

    let mut config = SimulationConfig::default()
        .with_sender_address(sender_address)
        .with_epoch(epoch)
        .with_protocol_version(protocol_version)
        .with_tx_hash(tx_hash);

    if let Some(ts) = grpc_tx.timestamp_ms {
        config = config.with_tx_timestamp(ts);
    }

    if let Some(budget) = grpc_tx.gas_budget {
        if budget > 0 {
            config = config.with_gas_budget(Some(budget));
        }
    }

    if let Some(price) = grpc_tx.gas_price {
        if price > 0 {
            config = config.with_gas_price(price);
        }
    }

    if let Some(rgp) = reference_gas_price.or_else(|| grpc_tx.gas_price.filter(|p| *p > 0)) {
        config = config.with_reference_gas_price(rgp);
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{
        archive_runtime_gap_hint, ensure_package_registration_success, PackageRegistrationResult,
    };
    use move_core_types::account_address::AccountAddress;

    #[test]
    fn archive_runtime_hint_triggers_on_archive_endpoint() {
        let hint = archive_runtime_gap_hint(
            "ContractAbort { location: Undefined, abort_code: 1 } missing runtime object",
            Some("https://archive.mainnet.sui.io:443"),
        )
        .expect("archive hint should exist");
        assert!(hint.contains("SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443"));
    }

    #[test]
    fn archive_runtime_hint_skips_non_archive_endpoint() {
        let hint = archive_runtime_gap_hint(
            "ContractAbort { location: Undefined, abort_code: 1 } missing runtime object",
            Some("https://grpc.custom-provider.example"),
        );
        assert!(hint.is_none());
    }

    #[test]
    fn archive_runtime_hint_detects_dynamic_field_abort_pattern() {
        let hint = archive_runtime_gap_hint(
            "execution failed: VMError { major_status: ABORTED, sub_status: Some(2), location: Module(ModuleId { name: Identifier(\"dynamic_field\") }) }",
            Some("https://archive.mainnet.sui.io:443"),
        )
        .expect("dynamic-field archive hint should exist");
        assert!(hint.contains("SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443"));
    }

    #[test]
    fn ensure_package_registration_success_reports_failures() {
        let mut registration = PackageRegistrationResult::default();
        registration.failed.push((
            AccountAddress::from_hex_literal("0x2").expect("valid"),
            "load error".to_string(),
        ));
        let err = ensure_package_registration_success(&registration)
            .expect_err("registration should fail");
        let message = err.to_string();
        assert!(message.contains("package registration failed"));
        assert!(message.contains("0x2"));
        assert!(message.contains("load error"));
    }
}
