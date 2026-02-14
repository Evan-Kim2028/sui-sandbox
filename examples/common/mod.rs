//! Common utilities for PTB replay examples.
//!
//! This module provides glue code for examples that bridges `sui-transport and sui-prefetch`
//! and `sui-sandbox-core`. It re-exports utilities from both crates and provides
//! example-specific helper functions.
//!
//! ## Data Helpers (from `sui_prefetch` and `sui_transport`)
//!
//! - [`collect_historical_versions`]: Aggregate object versions from gRPC response (sui_prefetch)
//! - [`create_grpc_client`]: Initialize gRPC client (sui_transport)
//!
//! ## Infrastructure Workarounds (from `sui_sandbox_core::utilities`)
//!
//! - `GenericObjectPatcher`: Patch objects for version-lock workarounds
//! - `normalize_address`: Normalize address format (0x2 -> 0x000...002)
//! - `is_framework_package`: Check if package is framework (0x1/0x2/0x3)
//! - `parse_type_tag`: Parse Sui type strings to Move TypeTags
//! - `extract_package_ids_from_type`: Extract package addresses from type strings
//! - `extract_dependencies_from_bytecode`: Find package dependencies in bytecode
//!
//! ## Example-Specific Helpers (this module)
//!
//! Functions that bridge both crates for replay examples:
//! - [`create_child_fetcher`]: Build on-demand child object loader
//! - [`build_resolver_from_packages`]: Build resolver from cached packages
//! - [`build_generic_patcher`]: Configure patcher with resolver modules
//! - [`create_vm_harness`]: Create VM harness from transaction data
//! - [`register_input_objects`]: Register objects in VM harness

// Allow unused since these are public re-exports for examples to use
#![allow(dead_code)]
#![allow(unused_imports)]

use move_core_types::account_address::AccountAddress;
use sui_sandbox_core::utilities::normalize_address;

// Re-export from sui-transport and sui-prefetch
pub use sui_prefetch::collect_historical_versions;
pub use sui_transport::create_grpc_client;

// Re-export from sui-sandbox-core::utilities (type/bytecode utilities)
pub use sui_sandbox_core::utilities::{
    extract_dependencies_from_bytecode, extract_package_ids_from_type, parse_type_tag,
};

// Backwards compatibility alias
pub use parse_type_tag as parse_type_tag_simple;

// =============================================================================
// Example-Specific Helpers
// =============================================================================
//
// These functions bridge sui-transport and sui-prefetch and sui-sandbox-core for replay examples.
// They depend on types from both crates and are specific to the example use case.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

use anyhow::{anyhow, Result};
use base64::Engine;
use move_core_types::identifier::Identifier;
use std::collections::BTreeMap;
use std::str::FromStr;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectID, ObjectInput};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::sandbox_runtime::ChildFetcherFn;
use sui_sandbox_core::simulation::{ExecutionResult, SimulationEnvironment};
use sui_sandbox_core::tx_replay::CachedTransaction;
use sui_sandbox_core::utilities::GenericObjectPatcher;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness, DEFAULT_PROTOCOL_VERSION};
use sui_state_fetcher::types::PackageData;
use sui_state_fetcher::{HistoricalStateProvider, ReplayState};
use sui_transport::grpc::{GrpcClient, GrpcTransaction};
use sui_types::digests::TransactionDigest as SuiTransactionDigest;

/// Build a GenericObjectPatcher with modules from the resolver.
///
/// Sets up the patcher with:
/// - Modules loaded into the layout registry
/// - Default patching rules for timestamp and version fields
/// - Transaction timestamp for time-based patches
///
/// If verbose is true, prints configuration details.
pub fn build_generic_patcher(
    resolver: &LocalModuleResolver,
    tx_timestamp_ms: u64,
    verbose: bool,
) -> GenericObjectPatcher {
    let mut patcher = GenericObjectPatcher::new();

    // Add modules for struct layout extraction
    patcher.add_modules(resolver.compiled_modules());

    // Set timestamp for time-based patches
    patcher.set_timestamp(tx_timestamp_ms);

    // Add default rules (timestamp fields, version fields)
    patcher.add_default_rules();

    if verbose {
        println!(
            "   ✓ Generic patcher configured with {} modules",
            resolver.module_count()
        );

        // Report detected versions from bytecode constant pools
        if patcher.has_detected_versions() {
            println!("   Version constants detected from bytecode:");
            for (pkg_addr, version) in patcher.detected_versions() {
                println!(
                    "      {} -> v{}",
                    &pkg_addr[..20.min(pkg_addr.len())],
                    version
                );
            }
        }
    }

    patcher
}

/// Build a LocalModuleResolver from cached packages with linkage support.
///
/// Handles:
/// - Skipping packages superseded by upgraded versions (via linkage)
/// - Address aliasing for upgraded packages
/// - Loading the Sui framework
///
/// Returns (resolver, module_count, alias_count, skipped_count).
pub fn build_resolver_from_packages(
    cached: &CachedTransaction,
    linkage_upgrades: &HashMap<String, String>,
    verbose: bool,
) -> Result<(LocalModuleResolver, usize, usize, usize)> {
    let mut resolver = LocalModuleResolver::new();
    let mut module_load_count = 0;
    let mut alias_count = 0;
    let mut skipped_count = 0;

    for (pkg_id, modules) in &cached.packages {
        let pkg_id_normalized = normalize_address(pkg_id);

        // Skip packages superseded by upgraded versions
        if let Some(upgraded_id) = linkage_upgrades.get(&pkg_id_normalized) {
            if cached.packages.contains_key(upgraded_id) {
                skipped_count += 1;
                if verbose {
                    println!(
                        "      Skipping {} (superseded by {})",
                        &pkg_id[..16.min(pkg_id.len())],
                        &upgraded_id[..16.min(upgraded_id.len())]
                    );
                }
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(pkg_id).ok();

        // Decode modules from base64
        let decoded_modules: Vec<(String, Vec<u8>)> = modules
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        match resolver.add_package_modules_at(decoded_modules, target_addr) {
            Ok((count, source_addr)) => {
                module_load_count += count;
                if let (Some(target), Some(source)) = (target_addr, source_addr) {
                    if target != source {
                        alias_count += 1;
                    }
                }
            }
            Err(e) => {
                if verbose {
                    println!(
                        "   ! Failed to load package {}: {}",
                        &pkg_id[..16.min(pkg_id.len())],
                        e
                    );
                }
            }
        }
    }

    // Load Sui framework
    match resolver.load_sui_framework() {
        Ok(n) => {
            if verbose {
                println!("   ✓ Loaded {} framework modules", n);
            }
        }
        Err(e) => {
            if verbose {
                println!("   ! Framework load failed: {}", e);
            }
        }
    }

    Ok((resolver, module_load_count, alias_count, skipped_count))
}

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
                    // Apply patching if patcher is available
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

/// Common fetched object tuple used by replay examples:
/// `(bcs_bytes, type_string, version, is_shared)`.
pub type ExampleFetchedObjectData = (Vec<u8>, Option<String>, u64, bool);

/// Fetch object BCS data at an optional historical version with optional latest fallback.
pub fn fetch_object_data(
    rt: &Runtime,
    grpc: &GrpcClient,
    object_id: &str,
    historical_version: Option<u64>,
    allow_latest_fallback: bool,
) -> Option<ExampleFetchedObjectData> {
    let at_version = historical_version.and_then(|version| {
        rt.block_on(async { grpc.get_object_at_version(object_id, Some(version)).await })
            .ok()
            .flatten()
            .and_then(|obj| {
                let is_shared = matches!(obj.owner, sui_transport::grpc::GrpcOwner::Shared { .. });
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
            let is_shared = matches!(obj.owner, sui_transport::grpc::GrpcOwner::Shared { .. });
            let bcs = obj.bcs?;
            Some((bcs, obj.type_string, obj.version, is_shared))
        })
}

/// Preload dynamic field wrapper objects for a list of parent object ids.
///
/// Returns a map of dynamic-field object id to fetched object tuple.
pub fn preload_dynamic_field_objects(
    rt: &Runtime,
    graphql: &sui_transport::graphql::GraphQLClient,
    grpc: &GrpcClient,
    parent_ids: &[&str],
    limit: usize,
) -> HashMap<String, ExampleFetchedObjectData> {
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
    fetched: &HashMap<String, ExampleFetchedObjectData>,
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

/// Build a shared-object PTB input from cached fetched object data.
pub fn make_shared_object_input(
    obj_id: &str,
    fetched: &HashMap<String, ExampleFetchedObjectData>,
) -> Result<InputValue> {
    let (bcs, type_str, version, _) = fetched
        .get(obj_id)
        .ok_or_else(|| anyhow!("Object {} not found", obj_id))?;
    let type_tag = type_str.as_ref().and_then(|s| TypeTag::from_str(s).ok());
    Ok(InputValue::Object(ObjectInput::Shared {
        id: ObjectID::from_hex_literal(obj_id)?,
        bytes: bcs.clone(),
        type_tag,
        version: Some(*version),
        mutable: false,
    }))
}

/// Execute a single MoveCall with shared-object inputs sourced from fetched object data.
pub fn execute_shared_move_call(
    env: &mut SimulationEnvironment,
    package: AccountAddress,
    module: &str,
    function: &str,
    type_args: Vec<TypeTag>,
    object_arg_ids: &[&str],
    fetched: &HashMap<String, ExampleFetchedObjectData>,
) -> Result<ExecutionResult> {
    let inputs = object_arg_ids
        .iter()
        .map(|obj_id| make_shared_object_input(obj_id, fetched))
        .collect::<Result<Vec<_>>>()?;
    let args = object_arg_ids
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            let input_idx =
                u16::try_from(idx).map_err(|_| anyhow!("too many PTB inputs: {}", idx + 1))?;
            Ok(Argument::Input(input_idx))
        })
        .collect::<Result<Vec<_>>>()?;

    let commands = vec![Command::MoveCall {
        package,
        module: Identifier::new(module)?,
        function: Identifier::new(function)?,
        type_args,
        args,
    }];

    Ok(env.execute_ptb(inputs, commands))
}

/// Create a VM harness configured for transaction replay.
///
/// Sets up the harness with:
/// - Clock timestamp from transaction
/// - Sender address from transaction
///
/// Returns (harness, sender_address).
pub fn create_vm_harness<'a>(
    grpc_tx: &GrpcTransaction,
    resolver: &'a LocalModuleResolver,
    tx_timestamp_ms: u64,
) -> Result<(VMHarness<'a>, AccountAddress)> {
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let harness = VMHarness::with_config(resolver, false, config)?;

    Ok((harness, sender_address))
}

/// Build a replay-accurate SimulationConfig from fetched state metadata.
///
/// Populates:
/// - tx hash (for object ID derivation)
/// - epoch / protocol version
/// - reference gas price and gas price
/// - gas budget (if present)
/// - tx timestamp (if present)
/// - sender address
pub fn build_replay_config(state: &ReplayState) -> Result<SimulationConfig> {
    let digest_str = &state.transaction.digest.0;
    let tx_hash = SuiTransactionDigest::from_str(digest_str)
        .map_err(|e| anyhow::anyhow!("Invalid transaction digest {}: {}", digest_str, e))?
        .into_inner();

    let protocol_version = if state.protocol_version > 0 {
        state.protocol_version
    } else {
        DEFAULT_PROTOCOL_VERSION
    }
    .min(DEFAULT_PROTOCOL_VERSION);

    let mut config = SimulationConfig::default()
        .with_sender_address(state.transaction.sender)
        .with_epoch(state.epoch)
        .with_protocol_version(protocol_version)
        .with_tx_hash(tx_hash);

    if let Some(ts) = state.transaction.timestamp_ms {
        config = config.with_tx_timestamp(ts);
    }

    if state.transaction.gas_budget > 0 {
        config = config.with_gas_budget(Some(state.transaction.gas_budget));
    }

    if state.transaction.gas_price > 0 {
        config = config.with_gas_price(state.transaction.gas_price);
    }

    if let Some(rgp) = state
        .reference_gas_price
        .or(if state.transaction.gas_price > 0 {
            Some(state.transaction.gas_price)
        } else {
            None
        })
    {
        config = config.with_reference_gas_price(rgp);
    }

    Ok(config)
}

/// Create a mainnet HistoricalStateProvider with practical endpoint defaults.
///
/// Behavior:
/// - Uses `SUI_GRPC_ENDPOINT` when explicitly set.
/// - Uses `https://archive.mainnet.sui.io:443` by default when no explicit endpoint is set.
/// - Warns when historical mode uses a likely non-archival endpoint.
pub async fn create_mainnet_provider(historical_mode: bool) -> Result<HistoricalStateProvider> {
    const MAINNET_PUBLIC_GRPC: &str = "https://fullnode.mainnet.sui.io:443";
    const MAINNET_ARCHIVE_GRPC: &str = "https://archive.mainnet.sui.io:443";

    let configured_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut endpoint = configured_endpoint
        .clone()
        .unwrap_or_else(|| MAINNET_ARCHIVE_GRPC.to_string());

    if historical_mode && endpoint.contains("fullnode.mainnet.sui.io") {
        if configured_endpoint.is_some() {
            eprintln!(
                "  ⚠ Historical mode detected non-archival endpoint ({}); switching to {}",
                endpoint, MAINNET_ARCHIVE_GRPC
            );
        }
        endpoint = MAINNET_ARCHIVE_GRPC.to_string();
    } else if historical_mode && configured_endpoint.is_none() {
        endpoint = MAINNET_ARCHIVE_GRPC.to_string();
    }

    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
    let grpc = GrpcClient::with_api_key(&endpoint, api_key).await?;
    let graphql = GraphQLClient::mainnet();

    Ok(HistoricalStateProvider::with_clients(grpc, graphql))
}

/// Resolve the effective gRPC endpoint used by examples, honoring env overrides.
pub fn effective_grpc_endpoint_for_examples() -> String {
    const MAINNET_ARCHIVE_GRPC: &str = "https://archive.mainnet.sui.io:443";
    std::env::var("SUI_GRPC_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| MAINNET_ARCHIVE_GRPC.to_string())
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
}

/// Print an actionable hint when archive replay likely failed due to runtime-object gaps.
pub fn maybe_print_archive_runtime_hint(error_message: &str) {
    if !is_likely_runtime_object_gap_error(error_message) {
        return;
    }

    let endpoint = effective_grpc_endpoint_for_examples();
    if is_mainnet_archive_endpoint(&endpoint) {
        println!("\n  ℹ️  Likely archive runtime-object gap detected.");
        println!("     Current endpoint: {}", endpoint);
        println!("     Suggestion: retry with `SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443`.");
    }
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

/// Build a replay config directly from a gRPC transaction, resolving epoch metadata
/// via the gRPC client if needed.
pub fn build_replay_config_from_grpc(
    rt: &Runtime,
    grpc: &GrpcClient,
    grpc_tx: &GrpcTransaction,
) -> Result<SimulationConfig> {
    let digest_str = &grpc_tx.digest;
    let tx_hash = SuiTransactionDigest::from_str(digest_str)
        .map_err(|e| anyhow::anyhow!("Invalid transaction digest {}: {}", digest_str, e))?
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

/// Register input objects in the VM harness.
///
/// Marks all historical objects as available inputs for the transaction.
/// Returns the count of registered objects.
pub fn register_input_objects(
    harness: &mut VMHarness,
    historical_versions: &HashMap<String, u64>,
) -> usize {
    let mut count = 0;
    for (obj_id, version) in historical_versions {
        if let Ok(addr) = AccountAddress::from_hex_literal(obj_id) {
            harness.add_sui_input_object(
                addr,
                *version,
                sui_types::object::Owner::Shared {
                    initial_shared_version: sui_types::base_types::SequenceNumber::from_u64(
                        *version,
                    ),
                },
            );
            count += 1;
        }
    }
    count
}

/// Build a decoded object cache (id -> (type_str, bcs_bytes)) from cached base64 objects.
pub fn build_cached_object_index(
    objects_b64: &HashMap<String, String>,
    object_types: &HashMap<String, String>,
) -> HashMap<String, (String, Vec<u8>)> {
    let mut result = HashMap::new();
    for (obj_id, b64) in objects_b64 {
        let type_str = match object_types.get(obj_id) {
            Some(t) => t.clone(),
            None => continue,
        };
        if let Ok(bcs) = base64::engine::general_purpose::STANDARD.decode(b64) {
            result.insert(obj_id.clone(), (type_str, bcs));
        }
    }
    result
}

// Re-export prefetch utilities
pub use sui_prefetch::{
    prefetch_dynamic_fields, prefetch_dynamic_fields_at_checkpoint, PrefetchedDynamicFields,
};
pub use sui_transport::graphql::GraphQLClient;

use move_core_types::language_storage::TypeTag;
use sui_prefetch::{DynamicFieldKey, PrefetchedChild};
use sui_sandbox_core::sandbox_runtime::KeyBasedChildFetcherFn;

type CachedObjectIndex = Arc<HashMap<String, (String, Vec<u8>)>>;
/// Dynamic discovery cache for child objects discovered during execution.
/// This is populated when we enumerate parent's dynamic fields and caches
/// all children for that parent, not just the one we're looking for.
#[derive(Debug, Default)]
pub struct DynamicDiscoveryCacheState {
    pub by_id: HashMap<String, (String, Vec<u8>)>,
    pub by_key: HashMap<DynamicFieldKey, PrefetchedChild>,
}

pub type DynamicDiscoveryCache = Arc<parking_lot::Mutex<DynamicDiscoveryCacheState>>;

/// Create a dynamic discovery cache for child fetching.
pub fn create_dynamic_discovery_cache() -> DynamicDiscoveryCache {
    Arc::new(parking_lot::Mutex::new(
        DynamicDiscoveryCacheState::default(),
    ))
}

/// Create an enhanced child fetcher with GraphQL fallback and prefetch cache.
///
/// This fetcher tries multiple strategies:
/// 1. Check prefetched cache first
/// 2. Check dynamic discovery cache (populated during execution)
/// 3. Try gRPC with historical version
/// 4. Fall back to GraphQL for current version
/// 5. Try dynamic field enumeration on parent (and cache ALL children)
///
/// The discovery_cache parameter is optional. If provided, newly discovered
/// children will be cached for future lookups.
pub fn create_enhanced_child_fetcher(
    grpc: GrpcClient,
    graphql: GraphQLClient,
    historical_versions: HashMap<String, u64>,
    prefetched: PrefetchedDynamicFields,
    patcher: Option<GenericObjectPatcher>,
    checkpoint: Option<u64>,
) -> ChildFetcherFn {
    create_enhanced_child_fetcher_with_cache(
        grpc,
        graphql,
        historical_versions,
        prefetched,
        patcher,
        checkpoint,
        None,
    )
}

/// Create an enhanced child fetcher with a dynamic discovery cache.
///
/// Same as `create_enhanced_child_fetcher` but with a shared cache that gets
/// populated during execution. This is useful when the transaction accesses
/// objects that weren't in the original transaction effects.
///
/// **NEW**: Computes `max_lamport_version` from historical_versions to validate
/// objects not in the transaction effects. If an object's current version is
/// <= max_lamport_version, it's safe to use (hasn't been modified since tx time).
pub fn create_enhanced_child_fetcher_with_cache(
    grpc: GrpcClient,
    graphql: GraphQLClient,
    historical_versions: HashMap<String, u64>,
    prefetched: PrefetchedDynamicFields,
    patcher: Option<GenericObjectPatcher>,
    checkpoint: Option<u64>,
    discovery_cache: Option<DynamicDiscoveryCache>,
) -> ChildFetcherFn {
    // Compute max lamport version for validation
    let max_lamport_version = historical_versions.values().copied().max().unwrap_or(0);

    let grpc_arc = Arc::new(grpc);
    let graphql_arc = Arc::new(graphql);
    let historical_arc = Arc::new(historical_versions);
    let prefetched_arc = Arc::new(prefetched.children.clone());
    let patcher_arc = Arc::new(parking_lot::Mutex::new(patcher));
    let discovery_cache = discovery_cache.unwrap_or_else(create_dynamic_discovery_cache);

    Box::new(move |parent_id: AccountAddress, child_id: AccountAddress| {
        let child_id_str = child_id.to_hex_literal();
        let parent_id_str = parent_id.to_hex_literal();
        let known_version = historical_arc.get(&child_id_str).copied();
        let allow_stale = known_version.is_none() && checkpoint.is_some();

        // Strategy 0: Check prefetched cache first
        if let Some((version, type_str, bcs)) = prefetched_arc.get(&child_id_str) {
            let version_ok = match known_version {
                Some(expected) => *version == expected,
                None => *version <= max_lamport_version,
            };

            if version_ok {
                // Apply patching if available
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

        // Strategy 0.5: Check dynamic discovery cache
        {
            let cache = discovery_cache.lock();
            if let Some((type_str, bcs)) = cache.by_id.get(&child_id_str) {
                if known_version.is_none() {
                    // Apply patching if available
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
        }

        // Try to fetch at historical version if known
        let rt = tokio::runtime::Runtime::new().ok()?;

        // Strategy 1: Try gRPC with historical version (if known) or current
        // If no historical version is known, we'll fetch current and validate
        let result = rt.block_on(async {
            grpc_arc
                .get_object_at_version(&child_id_str, known_version)
                .await
        });

        if let Ok(Some(obj)) = &result {
            // Validate version if we don't have a known historical version
            if known_version.is_none() && obj.version > max_lamport_version && !allow_stale {
                // Object has been modified since the transaction - skip!
                // Continue to try GraphQL or other strategies
            } else if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                // Version is valid (either known historical or current <= max_lamport)
                // Apply patching if available
                let final_bcs = {
                    let mut guard = patcher_arc.lock();
                    if let Some(ref mut p) = *guard {
                        p.patch_object(type_str, bcs)
                    } else {
                        bcs.clone()
                    }
                };
                match parse_type_tag(type_str) {
                    Some(type_tag) => {
                        return Some((type_tag, final_bcs));
                    }
                    None => {
                        // eprintln!("[child_fetcher] FAILED to parse type_tag for: {}", type_str);
                    }
                }
            }
        }

        // Strategy 2: Try GraphQL direct object fetch
        // eprintln!("[child_fetcher] Trying GraphQL direct fetch...");
        let mut gql_obj_opt = None;
        let mut gql_snapshot_used = false;
        if let Some(expected_version) = known_version {
            gql_obj_opt = graphql_arc
                .fetch_object_at_version(&child_id_str, expected_version)
                .ok();
        } else if let Some(cp) = checkpoint {
            if let Ok(obj) = graphql_arc.fetch_object_at_checkpoint(&child_id_str, cp) {
                gql_obj_opt = Some(obj);
                gql_snapshot_used = true;
            }
        }

        if gql_obj_opt.is_none() {
            gql_obj_opt = graphql_arc.fetch_object(&child_id_str).ok();
        }

        if let Some(obj) = gql_obj_opt {
            let version_ok = match known_version {
                Some(expected) => obj.version == expected,
                None => gql_snapshot_used || obj.version <= max_lamport_version || allow_stale,
            };

            if version_ok {
                if let Some(bcs_b64) = &obj.bcs_base64 {
                    if let Some(type_str) = &obj.type_string {
                        if let Ok(bcs) = base64::engine::general_purpose::STANDARD.decode(bcs_b64) {
                            // Apply patching if available
                            let final_bcs = {
                                let mut guard = patcher_arc.lock();
                                if let Some(ref mut p) = *guard {
                                    p.patch_object(type_str, &bcs)
                                } else {
                                    bcs.clone()
                                }
                            };
                            if let Some(type_tag) = parse_type_tag(type_str) {
                                return Some((type_tag, final_bcs));
                            }
                        }
                    }
                }
            }
        }

        // Strategy 2.5: If version is unknown and direct fetches failed, try a bounded
        // backscan over recent versions (helps when historical versions are pruned
        // or dynamic fields were deleted after the tx).
        if known_version.is_none() && max_lamport_version > 0 {
            let backscan_limit = std::env::var("SUI_CHILD_FETCH_BACKSCAN")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(200);
            let mut offset = 0u64;
            while offset <= backscan_limit {
                if offset > max_lamport_version {
                    break;
                }
                let ver = max_lamport_version - offset;
                let result = rt.block_on(async {
                    grpc_arc
                        .get_object_at_version(&child_id_str, Some(ver))
                        .await
                });

                if let Ok(Some(obj)) = &result {
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

                offset += 1;
            }
        }

        // Strategy 3: Try enumerating parent's dynamic fields to find the child
        // AND cache all discovered children for future lookups
        let (dfs, snapshot_used) = match checkpoint {
            Some(cp) => {
                match graphql_arc.fetch_dynamic_fields_at_checkpoint(&parent_id_str, 200, cp) {
                    Ok(fields) => (fields, true),
                    Err(_) => match graphql_arc.fetch_dynamic_fields(&parent_id_str, 200) {
                        Ok(fields) => (fields, false),
                        Err(_) => (Vec::new(), false),
                    },
                }
            }
            None => match graphql_arc.fetch_dynamic_fields(&parent_id_str, 200) {
                Ok(fields) => (fields, false),
                Err(_) => (Vec::new(), false),
            },
        };

        if !dfs.is_empty() {
            let df_count = dfs.len();
            let mut found_result: Option<(TypeTag, Vec<u8>)> = None;

            for df in dfs {
                // Validate version bound when possible
                if let Some(expected) = known_version {
                    if let Some(v) = df.version {
                        if v != expected {
                            continue;
                        }
                    }
                } else if let Some(v) = df.version {
                    if !snapshot_used && v > max_lamport_version && !allow_stale {
                        continue;
                    }
                }

                if let Some(obj_id) = &df.object_id {
                    // Try to fetch this child's BCS data via GraphQL or gRPC
                    let child_obj_id = normalize_address(obj_id);

                    // Try to get BCS for this child
                    let child_data: Option<(String, Vec<u8>)> =
                        if let (Some(vt), Some(vb)) = (&df.value_type, &df.value_bcs) {
                            // Dynamic field info has the value directly
                            base64::engine::general_purpose::STANDARD
                                .decode(vb)
                                .ok()
                                .map(|bcs| (vt.clone(), bcs))
                        } else {
                            // Fetch the object directly (checkpoint/known version preferred)
                            let mut obj_opt = None;
                            let mut obj_snapshot_used = false;
                            if let Some(expected) = known_version {
                                obj_opt = graphql_arc
                                    .fetch_object_at_version(&child_obj_id, expected)
                                    .ok();
                            } else if let Some(cp) = checkpoint {
                                if let Ok(obj) =
                                    graphql_arc.fetch_object_at_checkpoint(&child_obj_id, cp)
                                {
                                    obj_opt = Some(obj);
                                    obj_snapshot_used = true;
                                }
                            }

                            if obj_opt.is_none() {
                                obj_opt = graphql_arc.fetch_object(&child_obj_id).ok();
                            }

                            obj_opt.and_then(|o| {
                                if let Some(expected) = known_version {
                                    if o.version != expected {
                                        return None;
                                    }
                                } else if !obj_snapshot_used
                                    && o.version > max_lamport_version
                                    && !allow_stale
                                {
                                    return None;
                                }

                                if let (Some(ts), Some(b64)) = (o.type_string, o.bcs_base64) {
                                    base64::engine::general_purpose::STANDARD
                                        .decode(&b64)
                                        .ok()
                                        .map(|bcs| (ts, bcs))
                                } else {
                                    None
                                }
                            })
                        };

                    if let Some((type_str, bcs)) = child_data {
                        // Cache this child for future lookups
                        {
                            let mut cache = discovery_cache.lock();
                            cache
                                .by_id
                                .insert(child_obj_id.clone(), (type_str.clone(), bcs.clone()));

                            if let Some(name_bcs) = df.decode_name_bcs() {
                                let normalized_parent = {
                                    let hex =
                                        parent_id_str.strip_prefix("0x").unwrap_or(&parent_id_str);
                                    format!("0x{}", hex.to_lowercase())
                                };
                                let key = DynamicFieldKey {
                                    parent_id: normalized_parent,
                                    name_type: df.name_type.clone(),
                                    name_bcs,
                                };
                                cache.by_key.insert(
                                    key,
                                    PrefetchedChild {
                                        object_id: child_obj_id.clone(),
                                        version: df.version.unwrap_or(0),
                                        type_string: type_str.clone(),
                                        bcs: bcs.clone(),
                                    },
                                );
                            }
                        }

                        // Is this the child we're looking for?
                        if normalize_address(obj_id) == normalize_address(&child_id_str) {
                            // Apply patching if available
                            let final_bcs = {
                                let mut guard = patcher_arc.lock();
                                if let Some(ref mut p) = *guard {
                                    p.patch_object(&type_str, &bcs)
                                } else {
                                    bcs.clone()
                                }
                            };
                            if let Some(type_tag) = parse_type_tag(&type_str) {
                                found_result = Some((type_tag, final_bcs));
                            }
                        }
                    }
                }
            }

            if let Some(result) = found_result {
                return Some(result);
            }
            let _ = df_count; // silence unused warning
        }

        if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
            eprintln!(
                "[child_fetcher] FAILED parent={} child={} known_version={:?}",
                parent_id_str, child_id_str, known_version
            );
        }
        None
    })
}

/// Create a key-based child fetcher for fuzzy matching on package upgrades.
///
/// This fetcher is called when the computed child ID doesn't match any known object,
/// which can happen when package upgrades change type addresses. It tries to match
/// by key bytes alone, ignoring the type address component.
pub fn create_key_based_child_fetcher(
    prefetched: PrefetchedDynamicFields,
    discovery_cache: Option<DynamicDiscoveryCache>,
    graphql: Option<GraphQLClient>,
    cached_objects: Option<CachedObjectIndex>,
) -> KeyBasedChildFetcherFn {
    let prefetched_arc = Arc::new(prefetched);
    let graphql = graphql.map(Arc::new);

    Box::new(
        move |parent_id: AccountAddress,
              _child_id: AccountAddress,
              key_type: &TypeTag,
              key_bytes: &[u8]| {
            let parent_str = parent_id.to_hex_literal();
            let key_type_str = format!("{}", key_type);

            // Use fuzzy matching: try exact match first, then fallback to bytes-only match
            if let Some(child) =
                prefetched_arc.get_by_key_fuzzy(&parent_str, &key_type_str, key_bytes)
            {
                if let Some(type_tag) = parse_type_tag(&child.type_string) {
                    if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
                        eprintln!(
                            "[key_fetcher] HIT prefetched parent={} key_type={} key_len={}",
                            parent_str,
                            key_type_str,
                            key_bytes.len()
                        );
                    }
                    return Some((type_tag, child.bcs.clone()));
                }
            }

            if let Some(ref dc) = discovery_cache {
                let cache = dc.lock();
                let normalized_parent = {
                    let hex = parent_str.strip_prefix("0x").unwrap_or(&parent_str);
                    format!("0x{}", hex.to_lowercase())
                };
                let key = DynamicFieldKey {
                    parent_id: normalized_parent.clone(),
                    name_type: key_type_str.clone(),
                    name_bcs: key_bytes.to_vec(),
                };

                if let Some(child) = cache.by_key.get(&key) {
                    if let Some(type_tag) = parse_type_tag(&child.type_string) {
                        if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
                            eprintln!(
                                "[key_fetcher] HIT cache parent={} key_type={} key_len={}",
                                parent_str,
                                key_type_str,
                                key_bytes.len()
                            );
                        }
                        return Some((type_tag, child.bcs.clone()));
                    }
                }

                if let Some((_, child)) = cache
                    .by_key
                    .iter()
                    .find(|(k, _)| k.parent_id == normalized_parent && k.name_bcs == key_bytes)
                {
                    if let Some(type_tag) = parse_type_tag(&child.type_string) {
                        if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
                            eprintln!(
                                "[key_fetcher] HIT cache_fuzzy parent={} key_type={} key_len={}",
                                parent_str,
                                key_type_str,
                                key_bytes.len()
                            );
                        }
                        return Some((type_tag, child.bcs.clone()));
                    }
                }
            }

            if let Some(cache) = cached_objects.as_ref() {
                if let Some((type_tag, bcs)) =
                    lookup_cached_dynamic_field(cache, &parent_str, key_type, key_bytes)
                {
                    if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
                        eprintln!(
                            "[key_fetcher] HIT cached_objects parent={} key_type={} key_len={}",
                            parent_str,
                            key_type_str,
                            key_bytes.len()
                        );
                    }
                    return Some((type_tag, bcs));
                }
            }

            if let Some(gql) = graphql.as_ref() {
                if let Ok(Some(df)) =
                    gql.fetch_dynamic_field_by_name(&parent_str, &key_type_str, key_bytes)
                {
                    if let (Some(type_str), Some(bcs_b64)) = (df.value_type, df.value_bcs) {
                        if let Ok(bcs) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                        {
                            if let Some(type_tag) = parse_type_tag(&type_str) {
                                if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref()
                                    == Some("1")
                                {
                                    eprintln!(
                                        "[key_fetcher] HIT graphql parent={} key_type={} key_len={}",
                                        parent_str,
                                        key_type_str,
                                        key_bytes.len()
                                    );
                                }
                                return Some((type_tag, bcs));
                            }
                        }
                    }
                }
            }

            if std::env::var("SUI_CHILD_FETCH_DEBUG").ok().as_deref() == Some("1") {
                let key_hex = key_bytes
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join("");
                eprintln!(
                    "[key_fetcher] FAILED parent={} key_type={} key_len={} key_bytes=0x{}",
                    parent_str,
                    key_type_str,
                    key_bytes.len(),
                    key_hex
                );
            }

            None
        },
    )
}

fn lookup_cached_dynamic_field(
    cache: &HashMap<String, (String, Vec<u8>)>,
    parent_id: &str,
    key_type: &TypeTag,
    key_bytes: &[u8],
) -> Option<(TypeTag, Vec<u8>)> {
    for (obj_id, (type_str, bcs)) in cache {
        if !type_str.contains("dynamic_field::Field<") {
            continue;
        }

        let name_type_str = extract_dynamic_field_name_type(type_str)?;
        if !type_tag_matches_outer_address_agnostic(key_type, &name_type_str) {
            continue;
        }

        let type_bcs = sui_prefetch::type_string_to_bcs(&name_type_str)?;
        let computed_id = sui_prefetch::compute_dynamic_field_id(parent_id, key_bytes, &type_bcs)?;
        if computed_id != *obj_id {
            continue;
        }

        let type_tag = parse_type_tag(type_str)?;
        return Some((type_tag, bcs.clone()));
    }

    None
}

fn extract_dynamic_field_name_type(type_str: &str) -> Option<String> {
    let start = type_str.find('<')?;
    let end = type_str.rfind('>')?;
    if end <= start + 1 {
        return None;
    }
    let inner = &type_str[start + 1..end];
    let params = sui_sandbox_core::utilities::split_type_params(inner);
    params.first().map(|s| s.trim().to_string())
}

fn type_tag_matches_outer_address_agnostic(key_type: &TypeTag, other_type: &str) -> bool {
    let other_tag = match parse_type_tag(other_type) {
        Some(tag) => tag,
        None => return false,
    };

    match (key_type, other_tag) {
        (TypeTag::Struct(a), TypeTag::Struct(b)) => {
            a.module == b.module && a.name == b.name && a.type_params == b.type_params
        }
        (a, b) => *a == b,
    }
}
