//! Gas Accuracy Test - Compare local gas calculations with on-chain results.
//!
//! This example fetches recent transactions from Sui mainnet and compares
//! the on-chain gas costs with what our local VM execution calculates using
//! the full AccurateGasMeter infrastructure.
//!
//! Uses:
//! - gRPC for historical data (via SUI_GRPC_ENDPOINT/.env)
//! - CachedTransaction for complete transaction context
//! - AccurateGasMeter for instruction-level gas tracking
//! - Version tracking for accurate replay
//!
//! Run with:
//!   cargo run --example gas_accuracy_test
//!
//! Required environment variables:
//!   SUI_GRPC_ENDPOINT - gRPC endpoint (default: https://fullnode.mainnet.sui.io:443)
//!   SUI_GRPC_API_KEY  - API key for gRPC (optional)

use anyhow::{anyhow, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use sui_sandbox_core::gas::{
    calculate_min_tx_cost, finalize_computation_cost, load_protocol_config, GasParameters,
};
use sui_sandbox_core::object_runtime::ChildFetcherFn;
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::{
    build_address_aliases_for_test, grpc_to_fetched_transaction, replay_with_version_tracking,
    CachedTransaction,
};
use sui_sandbox_core::utilities::{
    grpc_object_to_package_data, CallbackPackageFetcher, FetchedPackageData,
    HistoricalPackageResolver, HistoricalStateReconstructor,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_transport::grpc::{GrpcClient, GrpcInput, GrpcTransaction};

type PackageObjects = (HashMap<String, Vec<u8>>, HashMap<String, String>);
type PackagesWithSource = Vec<(String, Vec<(String, String)>, Option<String>, bool)>;

// ============================================================================
// JSON-RPC Types for fetching transaction digests
// ============================================================================

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: T,
}

#[derive(Debug, Deserialize)]
struct QueryResult {
    data: Vec<TransactionBlock>,
    #[serde(rename = "nextCursor")]
    #[allow(dead_code)]
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TransactionBlock {
    digest: String,
    effects: Effects,
}

#[derive(Debug, Clone, Deserialize)]
struct Effects {
    status: Status,
    #[serde(rename = "gasUsed")]
    gas_used: GasUsed,
}

#[derive(Debug, Clone, Deserialize)]
struct Status {
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GasUsed {
    #[serde(rename = "computationCost")]
    computation_cost: String,
    #[serde(rename = "storageCost")]
    storage_cost: String,
    #[serde(rename = "storageRebate")]
    storage_rebate: String,
    #[serde(rename = "nonRefundableStorageFee")]
    non_refundable_storage_fee: String,
}

// ============================================================================
// Gas Comparison Types
// ============================================================================

#[derive(Debug)]
struct GasComparison {
    #[allow(dead_code)]
    onchain_status: String,
    local_success: bool,

    // On-chain gas
    onchain_computation: u64,
    onchain_storage: u64,
    onchain_rebate: u64,
    onchain_non_refundable: u64,

    // Local gas (from AccurateGasMeter)
    local_computation: Option<u64>,
    local_storage: Option<u64>,
    local_rebate: Option<u64>,
    local_non_refundable: Option<u64>,

    // Execution details
    commands_count: usize,
    execution_error: Option<String>,
}

impl GasComparison {
    fn computation_accuracy(&self) -> Option<f64> {
        self.local_computation.map(|local| {
            if self.onchain_computation == 0 {
                100.0
            } else {
                let diff = (local as i64 - self.onchain_computation as i64).abs();
                (100.0 - (diff as f64 / self.onchain_computation as f64 * 100.0)).max(0.0)
            }
        })
    }

    fn storage_accuracy(&self) -> Option<f64> {
        self.local_storage.map(|local| {
            if self.onchain_storage == 0 {
                100.0
            } else {
                let diff = (local as i64 - self.onchain_storage as i64).abs();
                (100.0 - (diff as f64 / self.onchain_storage as f64 * 100.0)).max(0.0)
            }
        })
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() -> Result<()> {
    // Load .env file if it exists
    load_env_file();

    println!("╔═══════════════════════════════════════════════════════════════════╗");
    println!("║     Sui Gas Accuracy Test - Full VM Execution with AccurateGas    ║");
    println!("╚═══════════════════════════════════════════════════════════════════╝");
    println!();

    // Get gRPC endpoint from environment
    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    println!("gRPC Endpoint: {}", endpoint);
    if api_key.is_some() {
        println!("API Key: configured");
    }
    println!();

    // Fetch recent transaction digests
    println!("Fetching recent transaction digests...");
    let digests = fetch_recent_digests(10)?;
    println!("Found {} transactions\n", digests.len());

    // Create tokio runtime for async operations
    let rt = Arc::new(tokio::runtime::Runtime::new()?);

    // Connect to gRPC
    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    let grpc = Arc::new(grpc);

    // Process each transaction
    let mut comparisons: Vec<GasComparison> = Vec::new();

    for (i, (digest, onchain_gas)) in digests.iter().enumerate() {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Transaction {}/{}: {}", i + 1, digests.len(), digest);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

        match process_transaction(&rt, &grpc, digest, onchain_gas) {
            Ok(comparison) => {
                print_comparison(&comparison);
                comparisons.push(comparison);
            }
            Err(e) => {
                println!("  Error processing transaction: {}\n", e);
                comparisons.push(GasComparison {
                    onchain_status: onchain_gas.status.clone(),
                    local_success: false,
                    onchain_computation: onchain_gas.computation.parse().unwrap_or(0),
                    onchain_storage: onchain_gas.storage.parse().unwrap_or(0),
                    onchain_rebate: onchain_gas.rebate.parse().unwrap_or(0),
                    onchain_non_refundable: onchain_gas.non_refundable.parse().unwrap_or(0),
                    local_computation: None,
                    local_storage: None,
                    local_rebate: None,
                    local_non_refundable: None,
                    commands_count: 0,
                    execution_error: Some(e.to_string()),
                });
            }
        }
    }

    // Print summary
    print_summary(&comparisons);

    Ok(())
}

fn load_env_file() {
    if let Ok(env_path) = std::env::current_dir() {
        let env_file = env_path.join(".env");
        if env_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&env_file) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((key, value)) = line.split_once('=') {
                        std::env::set_var(key.trim(), value.trim());
                    }
                }
            }
        }
    }
}

struct OnchainGas {
    status: String,
    computation: String,
    storage: String,
    rebate: String,
    non_refundable: String,
}

fn fetch_recent_digests(limit: usize) -> Result<Vec<(String, OnchainGas)>> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "suix_queryTransactionBlocks",
        "params": [{
            "options": {
                "showInput": false,
                "showEffects": true,
                "showEvents": false,
                "showObjectChanges": false
            }
        }, null, limit * 3, true]
    });

    let response = ureq::post("https://fullnode.mainnet.sui.io:443")
        .set("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| anyhow!("Request failed: {}", e))?;

    let result: RpcResponse<QueryResult> = response
        .into_json()
        .map_err(|e| anyhow!("Failed to parse response: {}", e))?;

    // Filter for successful transactions
    let digests: Vec<(String, OnchainGas)> = result
        .result
        .data
        .into_iter()
        .filter(|tx| tx.effects.status.status == "success")
        .take(limit)
        .map(|tx| {
            (
                tx.digest,
                OnchainGas {
                    status: tx.effects.status.status,
                    computation: tx.effects.gas_used.computation_cost,
                    storage: tx.effects.gas_used.storage_cost,
                    rebate: tx.effects.gas_used.storage_rebate,
                    non_refundable: tx.effects.gas_used.non_refundable_storage_fee,
                },
            )
        })
        .collect();

    Ok(digests)
}

fn process_transaction(
    rt: &Arc<tokio::runtime::Runtime>,
    grpc: &Arc<GrpcClient>,
    digest: &str,
    onchain_gas: &OnchainGas,
) -> Result<GasComparison> {
    // Step 1: Fetch full transaction from gRPC
    println!("  Fetching transaction data via gRPC...");
    let grpc_tx = rt
        .block_on(async { grpc.get_transaction(digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found"))?;

    println!("    Commands: {}", grpc_tx.commands.len());
    for (i, cmd) in grpc_tx.commands.iter().enumerate() {
        let cmd_str = format!("{:?}", cmd);
        let truncated = if cmd_str.len() > 80 {
            &cmd_str[..80]
        } else {
            &cmd_str
        };
        println!("      [{}] {}...", i, truncated);
    }

    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);
    let gas_price = grpc_tx.gas_price.unwrap_or(1000);
    let gas_budget = grpc_tx.gas_budget.unwrap_or(50_000_000_000);

    // Step 2: Collect historical versions
    let historical_versions = collect_historical_versions(&grpc_tx);
    println!("    Objects with versions: {}", historical_versions.len());

    // Step 3: Fetch objects
    println!("  Fetching objects...");
    let (raw_objects, object_types) = fetch_objects(rt, grpc, &historical_versions)?;
    println!("    Fetched {} objects", raw_objects.len());

    // Step 4: Collect package IDs
    let mut package_ids: Vec<String> = Vec::new();
    for cmd in &grpc_tx.commands {
        if let sui_transport::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            if !package_ids.contains(package) {
                package_ids.push(package.clone());
            }
        }
    }
    for type_str in object_types.values() {
        for pkg_id in extract_package_ids_from_type(type_str) {
            if !package_ids.contains(&pkg_id) {
                package_ids.push(pkg_id);
            }
        }
    }

    // Step 5: Resolve packages
    println!("  Resolving packages...");
    let grpc_for_fetcher = grpc.clone();
    let rt_for_fetcher = rt.clone();
    let historical_for_fetcher = historical_versions.clone();

    let fetcher = CallbackPackageFetcher::new(move |pkg_id: &str, version: Option<u64>| {
        let actual_version = version.or_else(|| historical_for_fetcher.get(pkg_id).copied());
        let result = rt_for_fetcher.as_ref().block_on(async {
            grpc_for_fetcher
                .get_object_at_version(pkg_id, actual_version)
                .await
        })?;

        Ok(result
            .as_ref()
            .and_then(|obj| grpc_object_to_package_data(pkg_id, obj)))
    });

    let mut pkg_resolver = HistoricalPackageResolver::new(fetcher);
    pkg_resolver.set_historical_versions(historical_versions.clone());
    pkg_resolver.resolve_packages(&package_ids)?;
    println!("    Resolved {} packages", pkg_resolver.package_count());

    // Step 6: Build module resolver
    println!("  Building module resolver...");
    let resolver = build_resolver(&pkg_resolver)?;
    println!("    Loaded {} modules", resolver.module_count());

    // Step 7: Patch objects
    let mut reconstructor = {
        let mut r = HistoricalStateReconstructor::new();
        r.set_timestamp(tx_timestamp_ms);
        r.configure_from_modules(resolver.compiled_modules());
        r
    };
    let reconstructed = reconstructor.reconstruct(&raw_objects, &object_types);

    let patched_objects_b64: HashMap<String, String> = reconstructed
        .objects
        .iter()
        .map(|(id, bcs)| {
            (
                id.clone(),
                base64::engine::general_purpose::STANDARD.encode(bcs),
            )
        })
        .collect();

    // Step 8: Create VM harness with accurate gas metering
    println!("  Creating VM harness with AccurateGasMeter...");
    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;

    let protocol_version = 68u64;

    let config = SimulationConfig::default()
        .with_accurate_gas(true)
        // Note: use_sui_natives is disabled for now as it requires more complex
        // child fetcher setup. Bytecode instruction gas is still accurately metered.
        // Native function gas will use our custom implementations.
        .with_gas_budget(Some(gas_budget))
        .with_gas_price(gas_price)
        .with_protocol_version(protocol_version)
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;

    // Step 9: Set up child fetcher
    let child_fetcher = create_child_fetcher(
        rt.clone(),
        grpc.clone(),
        Arc::new(historical_versions.clone()),
        Arc::new(patched_objects_b64.clone()),
        Arc::new(object_types.clone()),
    );
    harness.set_child_fetcher(child_fetcher);

    // Step 10: Register input objects
    for (obj_id, version) in &historical_versions {
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
        }
    }

    // Step 11: Build transaction and execute with version tracking
    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;

    // Count inputs and their total bytes for debugging
    let input_count = fetched_tx.inputs.len();
    let total_input_bytes: usize = patched_objects_b64
        .values()
        .filter_map(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .map(|bytes| bytes.len())
        .sum();
    println!(
        "    Inputs: {} objects, ~{} bytes total",
        input_count, total_input_bytes
    );

    println!("  Executing transaction with AccurateGasMeter...");
    let mut cached = CachedTransaction::new(fetched_tx.clone());
    cached.packages = pkg_resolver.packages_as_base64();
    cached.objects = patched_objects_b64.clone();
    cached.object_types = object_types.clone();
    cached.object_versions = historical_versions.clone();

    let address_aliases = build_address_aliases_for_test(&cached);
    harness.set_address_aliases(address_aliases.clone());

    // Execute with version tracking
    let result = replay_with_version_tracking(
        &fetched_tx,
        &mut harness,
        &patched_objects_b64,
        &address_aliases,
        Some(&historical_versions),
    )?;

    // Step 12: Get gas summary
    // Computation gas comes from the PTB execution (in gas_used field)
    // Storage costs come from the harness's storage tracker
    let storage_summary = harness.storage_summary();

    // Get the protocol config for min tx cost calculation
    let protocol_config = load_protocol_config(protocol_version);
    let min_tx_cost = calculate_min_tx_cost(&protocol_config, gas_price);

    let (local_computation, local_storage, local_rebate, local_non_refundable) = {
        // Computation gas is in result.gas_used (accumulated from PTB execution)
        // Apply gas rounding (round UP to nearest 1000 gas units since protocol v14)
        // Then convert to MIST using gas_price
        let computation_gas_units = result.gas_used;

        // Apply Sui's finalization: rounding + min tx cost
        let computation_cost =
            finalize_computation_cost(computation_gas_units, gas_price, min_tx_cost);

        // Storage costs come from the tracker
        if let Some(storage) = storage_summary {
            let storage_cost = storage.total_cost();
            let storage_rebate = storage.storage_rebate;
            let non_refundable = storage_cost.saturating_sub(storage_rebate);

            (
                Some(computation_cost),
                Some(storage_cost),
                Some(storage_rebate),
                Some(non_refundable),
            )
        } else {
            // If no storage tracking, still report computation
            (Some(computation_cost), None, None, None)
        }
    };

    Ok(GasComparison {
        onchain_status: onchain_gas.status.clone(),
        local_success: result.local_success,
        onchain_computation: onchain_gas.computation.parse().unwrap_or(0),
        onchain_storage: onchain_gas.storage.parse().unwrap_or(0),
        onchain_rebate: onchain_gas.rebate.parse().unwrap_or(0),
        onchain_non_refundable: onchain_gas.non_refundable.parse().unwrap_or(0),
        local_computation,
        local_storage,
        local_rebate,
        local_non_refundable,
        commands_count: grpc_tx.commands.len(),
        execution_error: result.local_error,
    })
}

fn collect_historical_versions(grpc_tx: &GrpcTransaction) -> HashMap<String, u64> {
    let mut versions: HashMap<String, u64> = HashMap::new();

    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        versions.insert(id.clone(), *ver);
    }
    for (id, ver) in &grpc_tx.changed_objects {
        versions.insert(id.clone(), *ver);
    }
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        versions.insert(id.clone(), *ver);
    }
    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                versions.entry(object_id.clone()).or_insert(*version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                versions
                    .entry(object_id.clone())
                    .or_insert(*initial_version);
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                versions.entry(object_id.clone()).or_insert(*version);
            }
            GrpcInput::Pure { .. } => {}
        }
    }

    versions
}

fn fetch_objects(
    rt: &Arc<tokio::runtime::Runtime>,
    grpc: &Arc<GrpcClient>,
    historical_versions: &HashMap<String, u64>,
) -> Result<PackageObjects> {
    let mut raw_objects: HashMap<String, Vec<u8>> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();

    for (obj_id, version) in historical_versions {
        if let Ok(Some(obj)) =
            rt.block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await })
        {
            if let Some(bcs) = &obj.bcs {
                raw_objects.insert(obj_id.clone(), bcs.clone());
                if let Some(type_str) = &obj.type_string {
                    object_types.insert(obj_id.clone(), type_str.clone());
                }
            }
        }
    }

    Ok((raw_objects, object_types))
}

fn build_resolver<F>(
    pkg_resolver: &HistoricalPackageResolver<CallbackPackageFetcher<F>>,
) -> Result<LocalModuleResolver>
where
    F: Fn(&str, Option<u64>) -> anyhow::Result<Option<FetchedPackageData>> + Send + Sync,
{
    let mut resolver = LocalModuleResolver::new();
    let linkage_upgrades = pkg_resolver.linkage_upgrades();

    let all_packages: Vec<(String, Vec<(String, String)>)> =
        pkg_resolver.packages_as_base64().into_iter().collect();

    // Build packages with source for sorting
    let mut packages_with_source: PackagesWithSource = Vec::new();

    for (pkg_id, modules_b64) in all_packages {
        if let Some(upgraded) = linkage_upgrades.get(&pkg_id as &str) {
            if pkg_resolver.get_package(upgraded).is_some() {
                continue;
            }
        }

        let source_addr_opt: Option<String> =
            modules_b64.first().and_then(|pair: &(String, String)| {
                let (_, b64) = pair;
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .and_then(|bytes| {
                        CompiledModule::deserialize_with_defaults(&bytes)
                            .ok()
                            .map(|m| m.self_id().address().to_hex_literal())
                    })
            });

        let is_original = source_addr_opt
            .as_ref()
            .map(|src: &String| {
                pkg_id.contains(&src[..src.len().min(20)])
                    || src.contains(&pkg_id[..pkg_id.len().min(20)])
            })
            .unwrap_or(false);

        packages_with_source.push((pkg_id, modules_b64, source_addr_opt, is_original));
    }

    // Sort: originals first
    packages_with_source.sort_by(|a, b| {
        if a.3 != b.3 {
            return b.3.cmp(&a.3);
        }
        a.0.cmp(&b.0)
    });

    let mut loaded_source_addrs: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for (pkg_id, modules_b64, source_addr_opt, _is_original) in packages_with_source {
        if let Some(ref source_addr) = source_addr_opt {
            if loaded_source_addrs.contains(source_addr) {
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(&pkg_id).ok();
        let decoded_modules: Vec<(String, Vec<u8>)> = modules_b64
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        if let Ok((_, Some(source_addr))) =
            resolver.add_package_modules_at(decoded_modules, target_addr)
        {
            loaded_source_addrs.insert(source_addr.to_hex_literal());
        }
    }

    resolver.load_sui_framework()?;

    Ok(resolver)
}

fn create_child_fetcher(
    rt: Arc<tokio::runtime::Runtime>,
    grpc: Arc<GrpcClient>,
    historical: Arc<HashMap<String, u64>>,
    patched: Arc<HashMap<String, String>>,
    types: Arc<HashMap<String, String>>,
) -> ChildFetcherFn {
    Box::new(
        move |_parent_id: AccountAddress, child_id: AccountAddress| {
            let child_id_str = child_id.to_hex_literal();

            // Try patched objects first
            if let Some(b64) = patched.get(&child_id_str) {
                if let Ok(bcs) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    if let Some(type_str) = types.get(&child_id_str) {
                        if let Some(type_tag) = parse_type_tag_simple(type_str) {
                            return Some((type_tag, bcs));
                        }
                    }
                }
            }

            // Fall back to gRPC
            let version = historical.get(&child_id_str).copied();
            let result =
                rt.block_on(async { grpc.get_object_at_version(&child_id_str, version).await });

            if let Ok(Some(obj)) = result {
                if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                    if let Some(type_tag) = parse_type_tag_simple(type_str) {
                        return Some((type_tag, bcs.clone()));
                    }
                }
            }

            None
        },
    )
}

fn extract_package_ids_from_type(type_str: &str) -> Vec<String> {
    let mut packages = Vec::new();
    for part in type_str.split(|c| ['<', '>', ','].contains(&c)) {
        let trimmed = part.trim();
        if let Some(addr) = trimmed.split("::").next() {
            if addr.starts_with("0x") && addr.len() >= 10 && !packages.contains(&addr.to_string()) {
                packages.push(addr.to_string());
            }
        }
    }
    packages
}

fn parse_type_tag_simple(type_str: &str) -> Option<move_core_types::language_storage::TypeTag> {
    sui_sandbox_core::types::parse_type_tag(type_str).ok()
}

fn print_comparison(comp: &GasComparison) {
    println!("  On-chain Gas:");
    println!(
        "    Computation: {:>15} MIST",
        format_number(comp.onchain_computation)
    );
    println!(
        "    Storage:     {:>15} MIST",
        format_number(comp.onchain_storage)
    );
    println!(
        "    Rebate:      {:>15} MIST",
        format_number(comp.onchain_rebate)
    );
    println!(
        "    Non-refund:  {:>15} MIST",
        format_number(comp.onchain_non_refundable)
    );
    println!();

    if comp.local_computation.is_some() {
        println!("  Local Gas (AccurateGasMeter):");
        if let Some(comp_gas) = comp.local_computation {
            println!(
                "    Computation: {:>15} MIST (accuracy: {:.1}%)",
                format_number(comp_gas),
                comp.computation_accuracy().unwrap_or(0.0)
            );
        }
        if let Some(storage) = comp.local_storage {
            println!(
                "    Storage:     {:>15} MIST (accuracy: {:.1}%)",
                format_number(storage),
                comp.storage_accuracy().unwrap_or(0.0)
            );
        }
        if let Some(rebate) = comp.local_rebate {
            println!("    Rebate:      {:>15} MIST", format_number(rebate));
        }
        if let Some(non_ref) = comp.local_non_refundable {
            println!("    Non-refund:  {:>15} MIST", format_number(non_ref));
        }
    } else {
        println!("  Local Gas: NOT AVAILABLE");
        if let Some(err) = &comp.execution_error {
            println!("    Error: {}", err);
        }
    }

    println!();
    println!(
        "  Execution: {} (commands: {})",
        if comp.local_success {
            "SUCCESS"
        } else {
            "FAILED"
        },
        comp.commands_count
    );
    println!();
}

fn print_summary(comparisons: &[GasComparison]) {
    println!("═══════════════════════════════════════════════════════════════════════");
    println!("                           SUMMARY STATISTICS");
    println!("═══════════════════════════════════════════════════════════════════════\n");

    let total = comparisons.len();
    let successful = comparisons.iter().filter(|c| c.local_success).count();
    let with_gas_data = comparisons
        .iter()
        .filter(|c| c.local_computation.is_some())
        .count();

    println!("Transactions analyzed: {}", total);
    println!(
        "Successful executions: {} ({:.0}%)",
        successful,
        (successful as f64 / total as f64) * 100.0
    );
    println!(
        "With gas data: {} ({:.0}%)",
        with_gas_data,
        (with_gas_data as f64 / total as f64) * 100.0
    );
    println!();

    // Calculate accuracy stats
    let comp_accuracies: Vec<f64> = comparisons
        .iter()
        .filter_map(|c| c.computation_accuracy())
        .collect();
    let storage_accuracies: Vec<f64> = comparisons
        .iter()
        .filter_map(|c| c.storage_accuracy())
        .collect();

    if !comp_accuracies.is_empty() {
        let avg_comp = comp_accuracies.iter().sum::<f64>() / comp_accuracies.len() as f64;
        let avg_storage = storage_accuracies.iter().sum::<f64>() / storage_accuracies.len() as f64;

        println!("Gas Accuracy (AccurateGasMeter vs On-chain):");
        println!("  Computation: {:.1}% average", avg_comp);
        println!("  Storage:     {:.1}% average", avg_storage);
        println!();
    }

    // Total gas comparison
    let total_onchain_comp: u64 = comparisons.iter().map(|c| c.onchain_computation).sum();
    let total_local_comp: u64 = comparisons.iter().filter_map(|c| c.local_computation).sum();
    let total_onchain_storage: u64 = comparisons.iter().map(|c| c.onchain_storage).sum();
    let total_local_storage: u64 = comparisons.iter().filter_map(|c| c.local_storage).sum();

    println!("Total On-chain Gas:");
    println!(
        "  Computation: {:>15} MIST ({:.4} SUI)",
        format_number(total_onchain_comp),
        total_onchain_comp as f64 / 1e9
    );
    println!(
        "  Storage:     {:>15} MIST ({:.4} SUI)",
        format_number(total_onchain_storage),
        total_onchain_storage as f64 / 1e9
    );
    println!();

    if with_gas_data > 0 {
        println!("Total Local Gas ({} transactions):", with_gas_data);
        println!(
            "  Computation: {:>15} MIST ({:.4} SUI)",
            format_number(total_local_comp),
            total_local_comp as f64 / 1e9
        );
        println!(
            "  Storage:     {:>15} MIST ({:.4} SUI)",
            format_number(total_local_storage),
            total_local_storage as f64 / 1e9
        );
        println!();
    }

    // Protocol parameters
    println!("Protocol Parameters (v68):");
    let config = load_protocol_config(68);
    let params = GasParameters::from_protocol_config(&config);
    println!(
        "  obj_data_cost_refundable:     {:>8}",
        params.obj_data_cost_refundable
    );
    println!(
        "  obj_metadata_cost_non_refund: {:>8}",
        params.obj_metadata_cost_non_refundable
    );
    println!(
        "  storage_rebate_rate:          {:>8} ({}%)",
        params.storage_rebate_rate,
        params.storage_rebate_rate as f64 / 100.0
    );
    println!(
        "  gas_model_version:            {:>8}",
        params.gas_model_version
    );
    println!();

    println!("═══════════════════════════════════════════════════════════════════════");
    println!("                              KEY INSIGHTS");
    println!("═══════════════════════════════════════════════════════════════════════\n");

    println!("This test uses the full AccurateGasMeter infrastructure:");
    println!("  • gRPC historical data fetching for accurate object versions");
    println!("  • CachedTransaction for complete transaction context");
    println!("  • HistoricalPackageResolver for package bytecode");
    println!("  • HistoricalStateReconstructor for object patching");
    println!("  • AccurateGasMeter for instruction-level gas tracking");
    println!("  • Version tracking for accurate replay comparison");
    println!();
    println!("Accuracy depends on:");
    println!("  • Complete object prefetching (dynamic fields may be missed)");
    println!("  • Correct package resolution (upgrade chains)");
    println!("  • Protocol version matching");
}

fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
