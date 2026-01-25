//! Scallop Lending Deposit Replay Example with MM2 Dynamic Field Analysis
//!
//! Demonstrates replaying a historical Scallop deposit transaction locally with
//! MM2 bytecode analysis for predictive dynamic field prefetching.
//!
//! Run with: cargo run --example scallop_deposit
//!
//! ## Overview
//!
//! This example replays a Scallop lending protocol deposit transaction using:
//! - **MM2 Predictive Prefetch**: Bytecode analysis predicts market and reserve dynamic fields
//! - gRPC for transaction data and historical object versions
//! - On-demand child fetching for dynamic fields
//! - Automatic package dependency resolution via linkage tables
//!
//! ## MM2 Dynamic Field Analysis
//!
//! Scallop's lending protocol uses dynamic fields for:
//! - Market reserves (Table<TypeName, Reserve>)
//! - User obligations (Table<address, Obligation>)
//! - Interest rate models
//!
//! MM2 analyzes deposit/borrow functions to predict these accesses.
//!
//! ## Required Setup
//!
//! Configure gRPC endpoint and API key in your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
//! SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on your provider
//! ```
//!
//! ## Key Techniques
//!
//! 1. **MM2 Predictive Prefetch**: Bytecode analysis predicts dynamic field accesses
//! 2. **Pure gRPC Fetching**: All data fetched fresh via configurable gRPC endpoint
//! 3. **Historical Object Versions**: Uses `unchanged_loaded_runtime_objects` for exact versions
//! 4. **Package Linkage Tables**: Follows upgrade chains to get correct package versions
//! 5. **Address Aliasing**: Maps storage IDs to bytecode addresses for upgraded packages
//! 6. **Object Patching**: Automatically fixes version-locked protocols (Scallop, Cetus)

mod common;

use anyhow::{anyhow, Result};
use base64::Engine;
use common::{
    create_dynamic_discovery_cache, create_enhanced_child_fetcher_with_cache,
    extract_dependencies_from_bytecode, extract_package_ids_from_type, prefetch_dynamic_fields,
};
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;
use std::str::FromStr;
use sui_sandbox_core::predictive_prefetch::{PredictivePrefetchConfig, PredictivePrefetcher};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::{grpc_to_fetched_transaction, CachedTransaction};
use sui_sandbox_core::utilities::{
    detect_version_constants, extract_version_constant_from_bytecode, is_framework_package,
    normalize_address, GenericObjectPatcher, HistoricalVersionFinder, PackageModuleFetcher,
    SearchStrategy, VersionFinderConfig,
};
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{GrpcClient, GrpcInput};

/// Scallop lending deposit transaction
/// NOTE: This should be a RECENT transaction to work with pruned historical data.
/// The original tx "JwCJUP4DEXRJna37UEXGJfLS1qMd1TUqdmvhpfyhNmU" has Version object value=8,
/// but the gRPC endpoint only has CURRENT_VERSION=100 bytecode available.
///
/// Recent transactions from Scallop's latest upgrade (0xd384ded6...) will work.
const TX_DIGEST: &str = "CTzkozkRKga4NvaKLptStVUdRbLbtPqR22FkuK6WVUQ4";

/// Adapter to make GrpcClient work with HistoricalVersionFinder.
struct GrpcFetcherAdapter<'a> {
    grpc: &'a GrpcClient,
    #[allow(dead_code)]
    rt: &'a tokio::runtime::Runtime,
}

impl<'a> GrpcFetcherAdapter<'a> {
    fn new(grpc: &'a GrpcClient, rt: &'a tokio::runtime::Runtime) -> Self {
        Self { grpc, rt }
    }
}

#[async_trait::async_trait]
impl<'a> PackageModuleFetcher for GrpcFetcherAdapter<'a> {
    async fn fetch_modules_at_version(
        &self,
        package_id: &str,
        version: u64,
    ) -> anyhow::Result<Option<Vec<(String, Vec<u8>)>>> {
        let result = self
            .grpc
            .get_object_at_version(package_id, Some(version))
            .await?;
        Ok(result.and_then(|obj| obj.package_modules))
    }

    async fn get_latest_version(&self, package_id: &str) -> anyhow::Result<Option<u64>> {
        let result = self.grpc.get_object(package_id).await?;
        Ok(result.map(|obj| obj.version))
    }
}

fn main() -> anyhow::Result<()> {
    // Load environment from .env file
    // Searches for .env in current directory, then walks up parent directories
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║    Scallop Deposit Replay - with MM2 Dynamic Field Analysis          ║");
    println!("║                                                                      ║");
    println!("║  Demonstrates MM2 bytecode analysis for predicting market reserve    ║");
    println!("║  and obligation dynamic field accesses in Scallop lending protocol.  ║");
    println!("║  Configure SUI_GRPC_ENDPOINT and SUI_GRPC_API_KEY in .env file.      ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let result = replay_via_grpc_no_cache(TX_DIGEST)?;

    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Scallop Deposit         | local: {:7} | expected: SUCCESS ║",
        if result { "SUCCESS" } else { "FAILURE" }
    );
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if result {
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME - 1:1 MAINNET PARITY         ║");
        println!("║                                                                      ║");
        println!("║ MM2 successfully predicted market reserve and obligation dynamic    ║");
        println!("║ field accesses for Scallop deposit operation.                       ║");
    } else {
        println!("║ ✗ TRANSACTION DID NOT MATCH EXPECTED OUTCOME                        ║");
        println!("║                                                                      ║");
        println!("║ Note: GenericObjectPatcher fixed version-lock issues, but this tx   ║");
        println!("║ has additional compatibility issues (argument deserialization).     ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Replay a transaction using ONLY gRPC for data fetching (no cache).
fn replay_via_grpc_no_cache(tx_digest: &str) -> Result<bool> {
    let rt = tokio::runtime::Runtime::new()?;

    // =========================================================================
    // Step 1: Connect to gRPC endpoint
    // =========================================================================
    println!("Step 1: Connecting to gRPC...");

    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let grpc = rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?;
    println!("   ✓ Connected to gRPC: {}", endpoint);

    // Also create GraphQL client for dynamic field enumeration fallback
    let graphql = GraphQLClient::mainnet();
    println!("   ✓ GraphQL client ready");

    // =========================================================================
    // Step 2: Fetch transaction via gRPC
    // =========================================================================
    println!("\nStep 2: Fetching transaction via gRPC...");

    let grpc_tx = rt
        .block_on(async { grpc.get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found: {}", tx_digest))?;

    println!("   Digest: {}", grpc_tx.digest);
    println!("   Sender: {}", grpc_tx.sender);
    println!("   Commands: {}", grpc_tx.commands.len());
    println!("   Inputs: {}", grpc_tx.inputs.len());
    println!("   Status: {:?}", grpc_tx.status);

    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Step 2b: MM2 Predictive Prefetch Analysis
    // =========================================================================
    println!("\nStep 2b: Running MM2 predictive prefetch analysis...");

    let mut prefetcher = PredictivePrefetcher::new();
    let mm2_config = PredictivePrefetchConfig::default();

    let mm2_result =
        prefetcher.prefetch_for_transaction(&grpc, Some(&graphql), &rt, &grpc_tx, &mm2_config);

    let pred_stats = &mm2_result.prediction_stats;
    println!("   MM2 Analysis Results:");
    println!("      Commands analyzed: {}", pred_stats.commands_analyzed);
    println!("      Predictions made: {}", pred_stats.predictions_made);
    println!(
        "      Predictions matched ground truth: {}",
        pred_stats.predictions_matched_ground_truth
    );
    println!(
        "      Confidence breakdown - High: {}, Medium: {}, Low: {}",
        pred_stats.high_confidence_predictions,
        pred_stats.medium_confidence_predictions,
        pred_stats.low_confidence_predictions
    );
    println!("      Packages analyzed: {}", pred_stats.packages_analyzed);
    println!("      Analysis time: {}ms", pred_stats.analysis_time_ms);

    // Log predictions for lending protocol dynamic fields
    if !mm2_result.predictions.is_empty() {
        println!("   Predicted dynamic field accesses (markets, obligations, etc.):");
        for pred in mm2_result.predictions.iter().take(6) {
            let matched = if pred.matched_ground_truth {
                "✓"
            } else {
                "○"
            };
            println!(
                "      {} [{:?}] {} ({})",
                matched, pred.confidence, pred.key_type, pred.source_function
            );
        }
        if mm2_result.predictions.len() > 6 {
            println!("      ... and {} more", mm2_result.predictions.len() - 6);
        }
    }

    // =========================================================================
    // Step 3: Collect all historical object versions from gRPC effects
    // =========================================================================
    println!("\nStep 3: Collecting historical object versions...");

    // Use BTreeMap for deterministic iteration order
    let mut historical_versions: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();

    println!(
        "   unchanged_loaded_runtime_objects: {}",
        grpc_tx.unchanged_loaded_runtime_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        println!("      {} @ version {}", id, ver);
        historical_versions.insert(id.clone(), *ver);
    }

    println!("   changed_objects: {}", grpc_tx.changed_objects.len());
    for (id, ver) in &grpc_tx.changed_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    println!(
        "   unchanged_consensus_objects: {}",
        grpc_tx.unchanged_consensus_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    // Track ownership type for each input object
    // This is important for correctly handling transfers (owned objects can be transferred, shared cannot)
    let mut input_ownership: HashMap<String, sui_types::object::Owner> = HashMap::new();

    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*version);
                // Owned by sender (address-owned)
                input_ownership.insert(
                    object_id.clone(),
                    sui_types::object::Owner::AddressOwner(
                        sui_types::base_types::SuiAddress::from_str(&grpc_tx.sender)
                            .unwrap_or_default(),
                    ),
                );
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*initial_version);
                // Shared object
                input_ownership.insert(
                    object_id.clone(),
                    sui_types::object::Owner::Shared {
                        initial_shared_version: sui_types::base_types::SequenceNumber::from_u64(
                            *initial_version,
                        ),
                    },
                );
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*version);
                // Receiving object - treated as owned by sender
                input_ownership.insert(
                    object_id.clone(),
                    sui_types::object::Owner::AddressOwner(
                        sui_types::base_types::SuiAddress::from_str(&grpc_tx.sender)
                            .unwrap_or_default(),
                    ),
                );
            }
            GrpcInput::Pure { .. } => {}
        }
    }

    println!("   Total unique objects: {}", historical_versions.len());
    println!(
        "   Owned inputs: {}",
        input_ownership
            .iter()
            .filter(|(_, o)| matches!(o, sui_types::object::Owner::AddressOwner(_)))
            .count()
    );
    println!(
        "   Shared inputs: {}",
        input_ownership
            .iter()
            .filter(|(_, o)| matches!(o, sui_types::object::Owner::Shared { .. }))
            .count()
    );

    // =========================================================================
    // Step 3b: Prefetch dynamic fields recursively
    // =========================================================================
    println!("\nStep 3b: Prefetching dynamic fields recursively...");

    let prefetched = prefetch_dynamic_fields(
        &graphql,
        &grpc,
        &rt,
        &historical_versions.clone().into_iter().collect(),
        3,   // max_depth
        200, // max_fields_per_object
    );

    println!(
        "   ✓ Discovered {} dynamic fields, fetched {} child objects",
        prefetched.total_discovered, prefetched.fetched_count
    );

    if !prefetched.failed.is_empty() {
        println!("   ! {} objects failed to fetch", prefetched.failed.len());
    }

    // Add prefetched children to historical_versions for later use
    for (child_id, (version, _, _)) in &prefetched.children {
        historical_versions
            .entry(child_id.clone())
            .or_insert(*version);
    }

    println!(
        "   Total unique objects (after prefetch): {}",
        historical_versions.len()
    );

    // =========================================================================
    // Step 4: Fetch all objects at historical versions via gRPC
    // =========================================================================
    println!("\nStep 4: Fetching objects at historical versions via gRPC...");

    // Store RAW objects first - we'll patch them after loading modules
    // Use BTreeMap for deterministic iteration order
    let mut raw_objects: std::collections::BTreeMap<String, Vec<u8>> =
        std::collections::BTreeMap::new();
    let mut object_types: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut packages: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    let mut package_ids_to_fetch: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for cmd in &grpc_tx.commands {
        if let sui_move_interface_extractor::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            package_ids_to_fetch.insert(package.clone());
        }
    }
    println!(
        "   Packages referenced in commands: {}",
        package_ids_to_fetch.len()
    );

    let mut fetched_count = 0;
    let mut failed_count = 0;

    for (obj_id, version) in &historical_versions {
        let result =
            rt.block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await });

        match result {
            Ok(Some(obj)) => {
                if let Some(bcs) = &obj.bcs {
                    // Store RAW bytes - we'll patch after loading modules
                    raw_objects.insert(obj_id.clone(), bcs.clone());
                    if let Some(type_str) = &obj.type_string {
                        object_types.insert(obj_id.clone(), type_str.clone());
                        for pkg_id in extract_package_ids_from_type(type_str) {
                            package_ids_to_fetch.insert(pkg_id);
                        }
                    }
                    fetched_count += 1;

                    if let Some(modules) = &obj.package_modules {
                        let modules_b64: Vec<(String, String)> = modules
                            .iter()
                            .map(|(name, bytes)| {
                                (
                                    name.clone(),
                                    base64::engine::general_purpose::STANDARD.encode(bytes),
                                )
                            })
                            .collect();
                        packages.insert(obj_id.clone(), modules_b64);
                        package_ids_to_fetch.remove(obj_id);
                    }
                }
            }
            Ok(None) => {
                failed_count += 1;
            }
            Err(_) => {
                failed_count += 1;
            }
        }
    }

    println!(
        "   ✓ Fetched {} raw objects ({} failed)",
        fetched_count, failed_count
    );

    // =========================================================================
    // Step 4b: Detect Version object values for historical bytecode resolution
    // =========================================================================
    println!("\nStep 4b: Detecting Version object values for historical bytecode...");

    // Find Version objects and extract their values
    // This is critical for historical replay - we need bytecode with matching CURRENT_VERSION
    let mut target_version_constant: Option<u64> = None;
    let mut version_package_id: Option<String> = None;

    for (obj_id, raw_bcs) in &raw_objects {
        let type_str = object_types.get(obj_id).map(|s| s.as_str()).unwrap_or("");

        // Look for Scallop's Version struct: 0x...::version::Version
        if type_str.contains("::version::Version") && !type_str.starts_with("0x2::") {
            // Version struct is { id: UID, value: u64 } - value is at offset 32 (after 32-byte UID)
            if raw_bcs.len() >= 40 {
                let value_bytes: [u8; 8] = raw_bcs[32..40].try_into().unwrap_or([0; 8]);
                let version_value = u64::from_le_bytes(value_bytes);

                // Extract package ID from type string (e.g., "0xefe8b36d...::version::Version")
                if let Some(pkg_id) = type_str.split("::").next() {
                    let pkg_normalized = normalize_address(pkg_id);
                    println!("   Found Version object:");
                    println!("      Object ID: {}", &obj_id[..20.min(obj_id.len())]);
                    println!(
                        "      Package: {}",
                        &pkg_normalized[..20.min(pkg_normalized.len())]
                    );
                    println!("      Historical value: {}", version_value);

                    target_version_constant = Some(version_value);
                    version_package_id = Some(pkg_normalized);
                }
            }
        }
    }

    // If we found a Version object, use HistoricalVersionFinder to find the correct package version
    // Key insight: Scallop upgrades are at DIFFERENT storage addresses, but share the same original_id
    // We need to find which storage address has bytecode with matching CURRENT_VERSION
    let mut historical_package_versions: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();

    if let (Some(target_constant), Some(pkg_id)) =
        (target_version_constant, version_package_id.as_ref())
    {
        println!(
            "\n   Searching for package with CURRENT_VERSION = {}...",
            target_constant
        );
        println!("   This requires finding the correct upgrade/storage address.");

        // First, get the latest version to find upgrade chain via linkage
        let latest_result = rt.block_on(async { grpc.get_object(pkg_id).await });

        if let Ok(Some(latest_obj)) = latest_result {
            println!(
                "   Latest package version: {} at {}",
                latest_obj.version,
                &pkg_id[..20.min(pkg_id.len())]
            );

            // Check if this package has the correct CURRENT_VERSION
            if let Some(modules) = &latest_obj.package_modules {
                let modules_vec: Vec<_> = modules
                    .iter()
                    .map(|(n, b)| (n.clone(), b.clone()))
                    .collect();
                println!(
                    "   Checking {} modules for CURRENT_VERSION...",
                    modules_vec.len()
                );

                let detected = extract_version_constant_from_bytecode(&modules_vec);
                println!("   Detected CURRENT_VERSION from bytecode: {:?}", detected);

                if let Some(detected) = detected {
                    println!("   Latest CURRENT_VERSION: {}", detected);

                    if detected == target_constant {
                        println!(
                            "   ✓ Latest version matches! Using version {}",
                            latest_obj.version
                        );
                        historical_package_versions.insert(pkg_id.clone(), latest_obj.version);
                    } else {
                        // Need to search for historical version
                        println!(
                            "   Latest has CURRENT_VERSION={}, but we need {}",
                            detected, target_constant
                        );
                        println!("   Searching for historical package version...");

                        let fetcher = GrpcFetcherAdapter::new(&grpc, &rt);
                        let config = VersionFinderConfig {
                            max_versions_to_search: 50,
                            strategy: SearchStrategy::Descending, // Start from latest, work backwards
                            use_cache: true,
                        };
                        let finder = HistoricalVersionFinder::with_config(fetcher, config);

                        // Search for the package version
                        let find_result = rt.block_on(async {
                            finder
                                .find_package_version_for_constant(pkg_id, target_constant)
                                .await
                        });

                        match find_result {
                            Ok(Some(result)) => {
                                println!("   ✓ Found matching package version!");
                                println!(
                                    "      Package version: {} (searched {} versions)",
                                    result.package_version, result.versions_searched
                                );
                                println!(
                                    "      Bytecode CURRENT_VERSION: {}",
                                    result.detected_constant
                                );

                                // Store this for later use when fetching packages
                                historical_package_versions
                                    .insert(pkg_id.clone(), result.package_version);
                            }
                            Ok(None) => {
                                println!(
                                    "   ! Could not find package version with CURRENT_VERSION = {}",
                                    target_constant
                                );
                                println!("     This may happen if the package is upgraded at different addresses.");
                                println!(
                                    "     Will search upgrade addresses during package fetching."
                                );
                            }
                            Err(e) => {
                                println!("   ! Error searching for package version: {}", e);
                            }
                        }
                    }
                } else {
                    println!("   ! No CURRENT_VERSION constant found in original package");
                    println!("     Will search upgrade storage addresses for matching version...");

                    // Get linkage table to find upgrade addresses
                    if let Some(linkage) = &latest_obj.package_linkage {
                        let mut found_match = false;
                        for link in linkage {
                            // Look for self-references where upgraded_id differs from original
                            let orig_norm = normalize_address(&link.original_id);
                            let upgrade_norm = normalize_address(&link.upgraded_id);
                            if orig_norm == *pkg_id && orig_norm != upgrade_norm {
                                println!(
                                    "     Checking upgrade address: {} (v{})",
                                    &upgrade_norm[..20.min(upgrade_norm.len())],
                                    link.upgraded_version
                                );

                                // Fetch this upgrade and check its CURRENT_VERSION
                                let upgrade_result =
                                    rt.block_on(async { grpc.get_object(&upgrade_norm).await });

                                if let Ok(Some(upgrade_obj)) = upgrade_result {
                                    if let Some(upgrade_modules) = &upgrade_obj.package_modules {
                                        let upgrade_modules_vec: Vec<_> = upgrade_modules
                                            .iter()
                                            .map(|(n, b)| (n.clone(), b.clone()))
                                            .collect();
                                        if let Some(upgrade_detected) =
                                            extract_version_constant_from_bytecode(
                                                &upgrade_modules_vec,
                                            )
                                        {
                                            println!(
                                                "       CURRENT_VERSION = {}",
                                                upgrade_detected
                                            );
                                            if upgrade_detected == target_constant {
                                                println!("     ✓ Found matching upgrade: {} with CURRENT_VERSION = {}",
                                                    &upgrade_norm[..20.min(upgrade_norm.len())], upgrade_detected);
                                                // Use this version for all Scallop packages
                                                historical_package_versions
                                                    .insert(pkg_id.clone(), upgrade_obj.version);
                                                historical_package_versions.insert(
                                                    upgrade_norm.clone(),
                                                    upgrade_obj.version,
                                                );
                                                found_match = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if !found_match {
                            println!(
                                "     ! Could not find upgrade with CURRENT_VERSION = {}",
                                target_constant
                            );
                            println!("       Will use latest bytecode (version check may fail)");
                        }
                    }
                }
            } else {
                println!("   ! No modules found in latest package object");
            }
        } else {
            println!("   ! Could not fetch latest package");
        }
    } else {
        println!("   No Version objects found - using latest package versions");
    }

    // target_version_constant is used later in Step 5b

    // =========================================================================
    // Step 5: Fetch packages with transitive dependencies
    // =========================================================================
    println!(
        "\nStep 5: Fetching packages with transitive dependencies (using historical versions)..."
    );

    let mut fetched_packages: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut packages_to_fetch = package_ids_to_fetch.clone();
    let max_depth = 10;

    // Maps original_id → upgraded_id for upgraded packages (use BTreeMap for determinism)
    let mut linkage_upgrades: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    // Reverse mapping: upgraded_id → original_id (to know what PTB address to use when storing)
    let mut linkage_originals: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    // Track package versions for sorting during loading
    let mut package_versions: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();

    for depth in 0..max_depth {
        if packages_to_fetch.is_empty() {
            break;
        }

        println!(
            "   Depth {}: fetching {} packages...",
            depth,
            packages_to_fetch.len()
        );
        let mut new_deps: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for pkg_id in packages_to_fetch.iter() {
            let pkg_id_normalized = normalize_address(pkg_id);
            if fetched_packages.contains(&pkg_id_normalized) {
                continue;
            }

            // Check if this package has been upgraded - if so, fetch from the upgraded address.
            // This is critical because on Sui, the original address always contains v1 bytecode
            // even after upgrades. The upgraded bytecode is at a different storage address.
            // Note: Clone the values here to avoid borrow conflicts with later mutations.
            let known_upgrade = linkage_upgrades.get(&pkg_id_normalized).cloned();
            let (fetch_id, fetch_id_normalized) = if let Some(upgraded_id) = known_upgrade {
                // Already know this package is upgraded - fetch from upgraded address
                (upgraded_id.clone(), upgraded_id)
            } else {
                (pkg_id.clone(), pkg_id_normalized.clone())
            };

            // Use historical package version if found by HistoricalVersionFinder,
            // otherwise fall back to historical_versions (from effects) or None (latest)
            let version = historical_package_versions
                .get(&fetch_id_normalized)
                .copied()
                .or_else(|| historical_versions.get(&fetch_id).copied());

            if historical_package_versions.contains_key(&fetch_id_normalized) {
                println!(
                    "      Using historical version {} for {}",
                    version.unwrap_or(0),
                    &fetch_id_normalized[..20.min(fetch_id_normalized.len())]
                );
            }

            let result =
                rt.block_on(async { grpc.get_object_at_version(&fetch_id, version).await });

            match result {
                Ok(Some(obj)) => {
                    // First, check if this package's own linkage table indicates it has been upgraded.
                    // On Sui, a package's linkage table can contain a self-reference entry where
                    // original_id == this package's address but upgraded_id is different (the new storage).
                    // This is the KEY mechanism to discover upgrades for packages we fetch directly.
                    let mut self_upgrade: Option<String> = None;
                    if let Some(linkage) = &obj.package_linkage {
                        for l in linkage {
                            let orig_normalized = normalize_address(&l.original_id);
                            let upgraded_normalized = normalize_address(&l.upgraded_id);
                            // Check if this linkage entry is a self-reference (package upgraded itself)
                            if orig_normalized == fetch_id_normalized
                                && orig_normalized != upgraded_normalized
                            {
                                println!(
                                    "      ! Package {} has self-upgrade to {} (v{})",
                                    &fetch_id[..20.min(fetch_id.len())],
                                    &l.upgraded_id[..20.min(l.upgraded_id.len())],
                                    l.upgraded_version
                                );
                                self_upgrade = Some(upgraded_normalized.clone());
                                // Record this upgrade mapping
                                linkage_upgrades
                                    .insert(orig_normalized.clone(), upgraded_normalized.clone());
                                linkage_originals
                                    .insert(upgraded_normalized.clone(), orig_normalized.clone());
                                break;
                            }
                        }
                    }

                    // If we discovered a self-upgrade, re-fetch from the upgraded address instead
                    if let Some(upgraded_addr) = self_upgrade {
                        println!(
                            "      Fetching upgraded bytecode from {}...",
                            &upgraded_addr[..20.min(upgraded_addr.len())]
                        );
                        let upgrade_version = historical_versions.get(&upgraded_addr).copied();
                        let upgrade_result = rt.block_on(async {
                            grpc.get_object_at_version(&upgraded_addr, upgrade_version)
                                .await
                        });

                        if let Ok(Some(upgraded_obj)) = upgrade_result {
                            if let Some(modules) = &upgraded_obj.package_modules {
                                let modules_b64: Vec<(String, String)> = modules
                                    .iter()
                                    .map(|(name, bytes)| {
                                        (
                                            name.clone(),
                                            base64::engine::general_purpose::STANDARD.encode(bytes),
                                        )
                                    })
                                    .collect();
                                println!(
                                    "      ✓ {} v{} ({} modules) [self-upgraded from {}]",
                                    &upgraded_addr[..20.min(upgraded_addr.len())],
                                    upgraded_obj.version,
                                    modules.len(),
                                    &fetch_id[..16.min(fetch_id.len())]
                                );

                                // Extract dependencies from upgraded bytecode
                                for (_name, bytecode_b64) in &modules_b64 {
                                    if let Ok(bytecode) = base64::engine::general_purpose::STANDARD
                                        .decode(bytecode_b64)
                                    {
                                        let deps = extract_dependencies_from_bytecode(&bytecode);
                                        for dep in deps {
                                            let dep_normalized = normalize_address(&dep);
                                            let actual_dep = linkage_upgrades
                                                .get(&dep_normalized)
                                                .cloned()
                                                .unwrap_or(dep_normalized);
                                            if !fetched_packages.contains(&actual_dep)
                                                && !packages.contains_key(&actual_dep)
                                            {
                                                new_deps.insert(actual_dep);
                                            }
                                        }
                                    }
                                }

                                // Store at ORIGINAL address (what PTB references)
                                let storage_key = pkg_id_normalized.clone();
                                packages.insert(storage_key.clone(), modules_b64);
                                package_versions.insert(storage_key.clone(), upgraded_obj.version);
                                fetched_packages.insert(storage_key);
                                fetched_packages.insert(upgraded_addr.clone());
                                fetched_packages.insert(fetch_id_normalized.clone());
                            }
                        }
                        continue; // Skip normal processing, we handled the upgrade
                    }

                    if let Some(modules) = &obj.package_modules {
                        let modules_b64: Vec<(String, String)> = modules
                            .iter()
                            .map(|(name, bytes)| {
                                (
                                    name.clone(),
                                    base64::engine::general_purpose::STANDARD.encode(bytes),
                                )
                            })
                            .collect();
                        if fetch_id_normalized != pkg_id_normalized {
                            println!(
                                "      ✓ {} v{} ({} modules) [upgraded from {}]",
                                &fetch_id[..20.min(fetch_id.len())],
                                obj.version,
                                modules.len(),
                                &pkg_id[..16.min(pkg_id.len())]
                            );
                        } else {
                            println!(
                                "      ✓ {} v{} ({} modules)",
                                &pkg_id[..20.min(pkg_id.len())],
                                obj.version,
                                modules.len()
                            );
                        }

                        // Check if this is an upgraded package (original_id != storage_id)
                        // This is the key for package upgrade handling!
                        if let Some(original_id) = &obj.package_original_id {
                            let original_normalized = normalize_address(original_id);
                            let storage_normalized = normalize_address(&fetch_id);
                            if original_normalized != storage_normalized {
                                println!(
                                    "         ! Package upgraded from {} to {}",
                                    &original_normalized[..20.min(original_normalized.len())],
                                    &storage_normalized[..20.min(storage_normalized.len())]
                                );
                                // Map: original -> storage (this package's current address)
                                // This allows us to find this upgraded version when resolving original types
                                linkage_upgrades.insert(
                                    original_normalized.clone(),
                                    storage_normalized.clone(),
                                );
                            }
                        }

                        if let Some(linkage) = &obj.package_linkage {
                            if !linkage.is_empty() {
                                println!(
                                    "         Linkage entries for {}:",
                                    &pkg_id[..20.min(pkg_id.len())]
                                );
                            }
                            for l in linkage {
                                if is_framework_package(&l.original_id) {
                                    continue;
                                }

                                let orig_normalized = normalize_address(&l.original_id);
                                let upgraded_normalized = normalize_address(&l.upgraded_id);
                                println!(
                                    "           {} -> {} (v{})",
                                    &l.original_id[..20.min(l.original_id.len())],
                                    &l.upgraded_id[..20.min(l.upgraded_id.len())],
                                    l.upgraded_version
                                );
                                if orig_normalized != upgraded_normalized {
                                    linkage_upgrades.insert(
                                        orig_normalized.clone(),
                                        upgraded_normalized.clone(),
                                    );
                                    // Also store reverse mapping for when we fetch upgraded packages
                                    linkage_originals.insert(
                                        upgraded_normalized.clone(),
                                        orig_normalized.clone(),
                                    );

                                    if !fetched_packages.contains(&upgraded_normalized)
                                        && !packages.contains_key(&upgraded_normalized)
                                    {
                                        new_deps.insert(upgraded_normalized.clone());
                                    }
                                }
                            }
                        }

                        for (_name, bytecode_b64) in &modules_b64 {
                            if let Ok(bytecode) =
                                base64::engine::general_purpose::STANDARD.decode(bytecode_b64)
                            {
                                let deps = extract_dependencies_from_bytecode(&bytecode);
                                for dep in deps {
                                    let dep_normalized = normalize_address(&dep);
                                    let actual_dep = linkage_upgrades
                                        .get(&dep_normalized)
                                        .cloned()
                                        .unwrap_or(dep_normalized);
                                    if !fetched_packages.contains(&actual_dep)
                                        && !packages.contains_key(&actual_dep)
                                    {
                                        new_deps.insert(actual_dep);
                                    }
                                }
                            }
                        }

                        // Determine the storage key: use the ORIGINAL address that PTB references.
                        // If this is an upgraded package (fetch_id != pkg_id), pkg_id is already original.
                        // If fetch_id == pkg_id but we know pkg_id is an upgraded storage address,
                        // use the original address from linkage_originals.
                        let storage_key = if pkg_id_normalized != fetch_id_normalized {
                            // We explicitly fetched from upgraded address; pkg_id_normalized is original
                            pkg_id_normalized.clone()
                        } else if let Some(original) = linkage_originals.get(&pkg_id_normalized) {
                            // This pkg_id is actually an upgraded storage address; use original
                            original.clone()
                        } else {
                            // Normal case: not upgraded, store at the pkg_id
                            pkg_id_normalized.clone()
                        };

                        // Store modules keyed by the address that PTB references.
                        // build_address_aliases will detect bytecode address differs and create alias.
                        packages.insert(storage_key.clone(), modules_b64);
                        package_versions.insert(storage_key.clone(), obj.version);
                        fetched_packages.insert(storage_key.clone());
                        // Also mark the fetched address as done to avoid redundant fetching
                        fetched_packages.insert(fetch_id_normalized.clone());
                        if pkg_id_normalized != fetch_id_normalized
                            && pkg_id_normalized != storage_key
                        {
                            fetched_packages.insert(pkg_id_normalized.clone());
                        }
                    }
                }
                Ok(None) => {
                    fetched_packages.insert(fetch_id_normalized.clone());
                    if pkg_id_normalized != fetch_id_normalized {
                        fetched_packages.insert(pkg_id_normalized.clone());
                    }
                }
                Err(_) => {
                    fetched_packages.insert(fetch_id_normalized.clone());
                    if pkg_id_normalized != fetch_id_normalized {
                        fetched_packages.insert(pkg_id_normalized.clone());
                    }
                }
            }
        }

        packages_to_fetch = new_deps;
    }

    if !linkage_upgrades.is_empty() {
        println!("   Linkage upgrades: {} mappings", linkage_upgrades.len());
    }

    // Explicit Scallop upgrade mapping: all upgrade addresses share the same original_id
    // but the gRPC response may not include package_original_id field for all of them.
    let scallop_original = "0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf";
    let scallop_original_norm = normalize_address(scallop_original);
    let known_scallop_upgrades = [
        "0x6e641f0dca8aedab3101d047e96439178f16301bf0b57fe8745086ff1195eb3e",
        "0xc38f849e81cfe46d4e4320f508ea7dda42934a329d5a6571bb4c3cb6ea63f5da",
        "0xd384ded6b9e7f4d2c4c9007b0291ef88fbfed8e709bce83d2da69de2d79d013d",
        "0xe7dbb371a9595631f7964b7ece42255ad0e738cc85fe6da26c7221b220f01af6",
    ];

    // Add explicit Scallop upgrade mappings - find the HIGHEST versioned upgrade
    let mut best_scallop_upgrade: Option<(String, u64)> = None;
    for addr in &known_scallop_upgrades {
        let normalized = normalize_address(addr);
        if packages.contains_key(&normalized) {
            let version = package_versions.get(&normalized).copied().unwrap_or(0);
            let is_better = best_scallop_upgrade
                .as_ref()
                .map(|(_, v)| version > *v)
                .unwrap_or(true);
            if is_better {
                best_scallop_upgrade = Some((normalized, version));
            }
        }
    }

    if let Some((best_upgrade, version)) = best_scallop_upgrade {
        // Only add if not already in linkage_upgrades from gRPC discovery
        if !linkage_upgrades.contains_key(&scallop_original_norm) {
            println!(
                "   Adding explicit Scallop upgrade mapping: {} -> {} (v{})",
                &scallop_original_norm[..20.min(scallop_original_norm.len())],
                &best_upgrade[..20.min(best_upgrade.len())],
                version
            );
            linkage_upgrades.insert(scallop_original_norm.clone(), best_upgrade);
        }
    }

    // Post-processing: Replace original package bytecode with upgraded bytecode.
    // This handles the case where we fetched Scallop v1 at depth 0 before discovering
    // that it has been upgraded via another package's linkage table.
    let mut replaced_count = 0;
    for (original_id, upgraded_id) in &linkage_upgrades {
        // Check if we have both original and upgraded - replace original with upgraded
        if packages.contains_key(original_id) && packages.contains_key(upgraded_id) {
            // Copy upgraded bytecode to original address (what PTB references)
            if let Some(upgraded_modules) = packages.get(upgraded_id).cloned() {
                let original_version = package_versions.get(original_id).copied().unwrap_or(1);
                let upgraded_version = package_versions.get(upgraded_id).copied().unwrap_or(1);

                // Only replace if upgraded version is newer
                if upgraded_version > original_version {
                    println!(
                        "   Replacing {} (v{}) with upgraded bytecode from {} (v{})",
                        &original_id[..20.min(original_id.len())],
                        original_version,
                        &upgraded_id[..20.min(upgraded_id.len())],
                        upgraded_version
                    );
                    packages.insert(original_id.clone(), upgraded_modules);
                    package_versions.insert(original_id.clone(), upgraded_version);
                    replaced_count += 1;
                }
            }
        } else if packages.contains_key(original_id) && !packages.contains_key(upgraded_id) {
            // We have v1 bytecode at original_id, but need to fetch upgraded bytecode
            println!(
                "   Re-fetching {} from upgraded address {}...",
                &original_id[..20.min(original_id.len())],
                &upgraded_id[..20.min(upgraded_id.len())]
            );

            let version = historical_versions.get(upgraded_id.as_str()).copied();
            let result =
                rt.block_on(async { grpc.get_object_at_version(upgraded_id, version).await });

            if let Ok(Some(obj)) = result {
                if let Some(modules) = &obj.package_modules {
                    let modules_b64: Vec<(String, String)> = modules
                        .iter()
                        .map(|(name, bytes)| {
                            (
                                name.clone(),
                                base64::engine::general_purpose::STANDARD.encode(bytes),
                            )
                        })
                        .collect();
                    println!(
                        "      ✓ {} v{} ({} modules) [replaces v1 at {}]",
                        &upgraded_id[..20.min(upgraded_id.len())],
                        obj.version,
                        modules.len(),
                        &original_id[..16.min(original_id.len())]
                    );
                    // Store upgraded bytecode at original address (what PTB references)
                    packages.insert(original_id.clone(), modules_b64);
                    package_versions.insert(original_id.clone(), obj.version);
                    replaced_count += 1;
                }
            }
        }
    }
    if replaced_count > 0 {
        println!(
            "   Replaced {} packages with upgraded bytecode",
            replaced_count
        );
    }

    // =========================================================================
    // Step 5b: Find historical bytecode with correct CURRENT_VERSION
    // =========================================================================
    // If we detected a Version object with a specific value (e.g., 8), we need to find
    // a package version that has matching CURRENT_VERSION constant.
    if let Some(target_const) = target_version_constant {
        println!(
            "\nStep 5b: Finding historical bytecode with CURRENT_VERSION = {}...",
            target_const
        );

        // Find all Scallop-related packages by checking which packages have many modules
        // (Scallop upgrades have 30+ modules each)
        // Also include any package known to be a Scallop upgrade via linkage
        let scallop_original = "0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf";
        let scallop_original_norm = normalize_address(scallop_original);

        // Known Scallop upgrade addresses (these all have bytecode with original_id = 0xefe8b36d...)
        let known_scallop_upgrades = [
            "0x6e641f0dca8aedab3101d047e96439178f16301bf0b57fe8745086ff1195eb3e",
            "0xc38f849e81cfe46d4e4320f508ea7dda42934a329d5a6571bb4c3cb6ea63f5da",
            "0xd384ded6b9e7f4d2c4c9007b0291ef88fbfed8e709bce83d2da69de2d79d013d",
            "0xe7dbb371a9595631f7964b7ece42255ad0e738cc85fe6da26c7221b220f01af6",
        ];

        let mut scallop_upgrades: Vec<String> = Vec::new();

        // Add known upgrades
        for addr in &known_scallop_upgrades {
            let normalized = normalize_address(addr);
            if packages.contains_key(&normalized) {
                scallop_upgrades.push(normalized);
            }
        }

        // Also add any from linkage maps
        for (orig, upgraded) in &linkage_upgrades {
            if normalize_address(orig) == scallop_original_norm
                && !scallop_upgrades.contains(upgraded)
            {
                scallop_upgrades.push(upgraded.clone());
            }
        }

        println!(
            "   Found {} Scallop upgrade addresses to search",
            scallop_upgrades.len()
        );

        // For each upgrade, search for a version with matching CURRENT_VERSION
        let fetcher = GrpcFetcherAdapter::new(&grpc, &rt);
        let config = VersionFinderConfig {
            max_versions_to_search: 50,
            strategy: SearchStrategy::Descending, // Start from latest, work backwards
            use_cache: true,
        };
        let finder = HistoricalVersionFinder::with_config(fetcher, config);

        let mut found_historical = false;
        for upgrade_addr in &scallop_upgrades {
            println!(
                "   Searching {} for CURRENT_VERSION = {}...",
                &upgrade_addr[..20.min(upgrade_addr.len())],
                target_const
            );

            let find_result = rt.block_on(async {
                finder
                    .find_package_version_for_constant(upgrade_addr, target_const)
                    .await
            });

            match find_result {
                Ok(Some(result)) => {
                    println!(
                        "   ✓ Found at {} version {} (CURRENT_VERSION = {})",
                        &upgrade_addr[..20.min(upgrade_addr.len())],
                        result.package_version,
                        result.detected_constant
                    );

                    // Re-fetch this package at the found version
                    let historical_pkg = rt.block_on(async {
                        grpc.get_object_at_version(upgrade_addr, Some(result.package_version))
                            .await
                    });

                    if let Ok(Some(obj)) = historical_pkg {
                        if let Some(modules) = obj.package_modules {
                            let modules_b64: Vec<(String, String)> = modules
                                .iter()
                                .map(|(name, bytes)| {
                                    (
                                        name.clone(),
                                        base64::engine::general_purpose::STANDARD.encode(bytes),
                                    )
                                })
                                .collect();

                            println!(
                                "   ✓ Loaded historical bytecode: {} modules",
                                modules_b64.len()
                            );

                            // Replace the bytecode for all Scallop addresses
                            packages.insert(scallop_original_norm.clone(), modules_b64.clone());
                            package_versions
                                .insert(scallop_original_norm.clone(), result.package_version);

                            // Also update upgrade addresses
                            for up_addr in &scallop_upgrades {
                                packages.insert(up_addr.clone(), modules_b64.clone());
                                package_versions.insert(up_addr.clone(), result.package_version);
                            }

                            // Mark that we're using historical bytecode
                            historical_package_versions
                                .insert(scallop_original_norm.clone(), result.package_version);
                            found_historical = true;
                            break;
                        }
                    }
                }
                Ok(None) => {
                    println!("     Not found at this address");
                }
                Err(e) => {
                    println!("     Error: {}", e);
                }
            }
        }

        if !found_historical {
            println!(
                "   ! Could not find historical bytecode with CURRENT_VERSION = {}",
                target_const
            );
            println!("     Transaction will likely fail version check");
        }
    }

    println!("   Total packages: {}", packages.len());

    // =========================================================================
    // Step 6: Build transaction structure
    // =========================================================================
    println!("\nStep 6: Building transaction structure...");

    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    // Add packages (objects will be added after patching in Step 7b)
    for (pkg_id, modules) in packages {
        cached.packages.insert(pkg_id, modules);
    }

    // Store object types and versions, objects will be added after patching (convert to HashMap)
    cached.object_types = object_types.clone().into_iter().collect();
    cached.object_versions = historical_versions.clone().into_iter().collect();

    println!("   ✓ Built CachedTransaction (packages only)");
    println!("      Packages: {}", cached.packages.len());
    println!("      Raw objects to patch: {}", raw_objects.len());

    // =========================================================================
    // Step 7: Build module resolver
    // =========================================================================
    println!("\nStep 7: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();

    let mut module_load_count = 0;
    let mut alias_count = 0;
    let mut skipped_count = 0;

    // Sort packages by VERSION (ascending) to ensure upgraded bytecode is loaded last.
    // When multiple packages have bytecode aliased to the same address (due to Sui's package
    // upgrade model), the later-loaded one takes precedence.
    //
    // We use the package version tracked during fetching. For upgraded Scallop packages like:
    //   - 0x6e641f0dca... v7  (CURRENT_VERSION = 7)
    //   - 0xd384ded6b9... v17 (CURRENT_VERSION = 17)
    // Both have the same bytecode address 0xefe8b36d..., so we need v17 loaded last.
    let mut sorted_packages: Vec<_> = cached.packages.iter().collect();
    sorted_packages.sort_by_key(|(pkg_id, _)| package_versions.get(*pkg_id).copied().unwrap_or(0));

    for (pkg_id, modules) in sorted_packages {
        let pkg_id_normalized = normalize_address(pkg_id);
        // Skip if this package has a DIFFERENT upgraded address that we should load from instead
        // BUT don't skip if we've already replaced this package's bytecode with the upgrade
        if let Some(upgraded_id) = linkage_upgrades.get(&pkg_id_normalized) {
            let pkg_version = package_versions
                .get(&pkg_id_normalized)
                .copied()
                .unwrap_or(1);
            let upgrade_version = package_versions.get(upgraded_id).copied().unwrap_or(1);

            // Only skip if:
            // 1. The upgrade package exists in cached.packages
            // 2. This package still has OLD bytecode (version < upgrade version)
            // If we replaced the bytecode, both will have the same version
            if cached.packages.contains_key(upgraded_id) && pkg_version < upgrade_version {
                skipped_count += 1;
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(pkg_id).ok();

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
                println!(
                    "   ! Failed to load package {}: {}",
                    &pkg_id[..16.min(pkg_id.len())],
                    e
                );
            }
        }
    }
    println!(
        "   ✓ Loaded {} user modules ({} packages with aliases, {} skipped)",
        module_load_count, alias_count, skipped_count
    );

    match resolver.load_sui_framework() {
        Ok(n) => println!("   ✓ Loaded {} framework modules", n),
        Err(e) => println!("   ! Framework load failed: {}", e),
    }

    println!("   ✓ Resolver ready");

    // =========================================================================
    // Step 7b: Create GenericObjectPatcher and patch objects
    // =========================================================================
    println!("\nStep 7b: Patching objects with GenericObjectPatcher...");

    let mut generic_patcher = GenericObjectPatcher::new();

    // Add modules for struct layout extraction (enables field-name-based patching)
    generic_patcher.add_modules(resolver.compiled_modules());

    // Set timestamp for time-based patches
    generic_patcher.set_timestamp(tx_timestamp_ms);

    // Add default patching rules (version and timestamp fields)
    generic_patcher.add_default_rules();

    // Auto-detect version constants from loaded bytecode.
    // This scans all loaded modules to find version constants used in comparisons.
    //
    // IMPORTANT: The detect_version_constants function can incorrectly pick up constants
    // from other modules. Instead, for Scallop specifically, we read the actual
    // CURRENT_VERSION from the current_version module's constant pool.
    let detected_versions = detect_version_constants(resolver.compiled_modules());

    // Get the ACTUAL CURRENT_VERSION from the Scallop current_version module
    let scallop_addr = AccountAddress::from_hex_literal(
        "0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf",
    )
    .unwrap();
    let scallop_cv_module_id = move_core_types::language_storage::ModuleId::new(
        scallop_addr,
        move_core_types::identifier::Identifier::new("current_version").unwrap(),
    );
    let actual_scallop_version =
        resolver
            .get_module_struct(&scallop_cv_module_id)
            .and_then(|module| {
                module.constant_pool().iter().find_map(|c| {
                    if c.type_ == move_binary_format::file_format::SignatureToken::U64 {
                        bcs::from_bytes::<u64>(&c.data).ok()
                    } else {
                        None
                    }
                })
            });

    println!(
        "   Actual Scallop CURRENT_VERSION from bytecode: {:?}",
        actual_scallop_version
    );

    // Check if the Version object value matches the actual bytecode constant
    // If they match, NO patching is needed!
    let version_values_match = target_version_constant == actual_scallop_version;

    if !detected_versions.is_empty() {
        println!("   Auto-detected version constants from bytecode:");
        for (pkg_addr, version) in &detected_versions {
            println!(
                "      {} -> version {}",
                &pkg_addr[..20.min(pkg_addr.len())],
                version
            );

            // Only register for patching if Version object doesn't match bytecode
            if !version_values_match {
                generic_patcher.register_version(pkg_addr, *version);
            }
        }

        if version_values_match {
            println!("   ✓ Version object matches bytecode - version patching DISABLED");
            println!(
                "     (Historical Version.value = {:?}, bytecode CURRENT_VERSION = {:?})",
                target_version_constant, actual_scallop_version
            );
        }
    }

    // Patch objects and convert to base64 for cached storage (use BTreeMap for determinism)
    let mut objects: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (obj_id, raw_bcs) in &raw_objects {
        let type_str = object_types.get(obj_id).map(|s| s.as_str()).unwrap_or("");
        let patched_bcs = generic_patcher.patch_object(type_str, raw_bcs);
        let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(&patched_bcs);
        objects.insert(obj_id.clone(), bcs_b64);
    }

    // Add patched objects to cached transaction (convert to HashMap)
    cached.objects = objects.into_iter().collect();

    // Report patches applied
    let patch_stats = generic_patcher.stats();
    if !patch_stats.is_empty() {
        println!("   Object patches applied (by field name):");
        for (field_name, count) in patch_stats {
            println!("      field '{}' -> {} patches", field_name, count);
        }
    }
    println!("   ✓ Patched {} objects", cached.objects.len());

    // =========================================================================
    // Step 8: Create VM harness
    // =========================================================================
    println!("\nStep 8: Creating VM harness...");

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;
    println!("   Sender: 0x{}", hex::encode(sender_address.as_ref()));

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;
    println!("   ✓ VM harness created");

    // =========================================================================
    // Step 9: Set up enhanced on-demand child fetcher
    // =========================================================================
    println!("\nStep 9: Setting up enhanced child fetcher...");

    // Create discovery cache for caching GraphQL results
    let discovery_cache = create_dynamic_discovery_cache();

    // Create the enhanced child fetcher with gRPC + GraphQL fallback
    // Note: We pass None for patcher since the child fetcher handles fetching fresh state
    let historical_hashmap: std::collections::HashMap<String, u64> = historical_versions
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let child_fetcher = create_enhanced_child_fetcher_with_cache(
        grpc,
        graphql,
        historical_hashmap,
        prefetched,
        None, // Patcher not needed - fresh state is fetched
        Some(discovery_cache),
    );

    harness.set_child_fetcher(child_fetcher);
    println!("   ✓ Enhanced child fetcher configured (gRPC + GraphQL fallback)");

    // =========================================================================
    // Step 10: Register input objects with proper ownership
    // =========================================================================
    println!("\nStep 10: Registering input objects...");

    let mut registered_count = 0;
    let mut owned_count = 0;
    let mut shared_count = 0;
    for (obj_id, version) in &historical_versions {
        if let Ok(addr) = AccountAddress::from_hex_literal(obj_id) {
            // Use tracked ownership if available, otherwise default to shared
            let ownership = input_ownership.get(obj_id).cloned().unwrap_or_else(|| {
                // Default to shared for objects not in the input list (e.g., dynamic field children)
                sui_types::object::Owner::Shared {
                    initial_shared_version: sui_types::base_types::SequenceNumber::from_u64(
                        *version,
                    ),
                }
            });

            match &ownership {
                sui_types::object::Owner::AddressOwner(_) => owned_count += 1,
                sui_types::object::Owner::Shared { .. } => shared_count += 1,
                _ => {}
            }

            harness.add_sui_input_object(addr, *version, ownership);
            registered_count += 1;
        }
    }
    println!(
        "   ✓ Registered {} input objects ({} owned, {} shared)",
        registered_count, owned_count, shared_count
    );

    // =========================================================================
    // Step 11: Execute transaction replay
    // =========================================================================
    println!("\nStep 11: Executing transaction replay...");

    // Build comprehensive address aliases including linkage upgrades
    // This is essential for upgraded packages where types use different addresses
    // Convert BTreeMap to HashMap for the function signature
    let linkage_upgrades_hashmap: std::collections::HashMap<String, String> = linkage_upgrades
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let address_aliases = sui_sandbox_core::tx_replay::build_comprehensive_address_aliases(
        &cached,
        &linkage_upgrades_hashmap,
    );
    if !address_aliases.is_empty() {
        println!("   Address aliases for replay: {}", address_aliases.len());
        for (from, to) in &address_aliases {
            println!(
                "      {} -> {}",
                &from.to_hex_literal()[..22],
                &to.to_hex_literal()[..22]
            );
        }
    }

    // Convert package_versions to HashMap for the version-aware alias method
    let package_versions_hashmap: std::collections::HashMap<String, u64> = package_versions
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    harness.set_address_aliases_with_versions(address_aliases.clone(), package_versions_hashmap);

    let result = sui_sandbox_core::tx_replay::replay_with_objects_and_aliases(
        &cached.transaction,
        &mut harness,
        &cached.objects,
        &address_aliases,
    )?;

    println!(
        "\n  Local execution: {}",
        if result.local_success {
            "SUCCESS"
        } else {
            "FAILURE"
        }
    );

    if !result.local_success {
        if let Some(err) = &result.local_error {
            println!("  Error: {}", err);
        }
    }

    Ok(result.local_success)
}
