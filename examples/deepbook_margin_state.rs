//! DeepBook Margin Manager State Query Example
//!
//! This example demonstrates calling the `manager_state` function on a real
//! margin position from DeepBook v3 to get health/liquidation data.
//!
//! ## What This Demonstrates
//!
//! 1. **Historical state reconstruction** at specific checkpoints using gRPC
//! 2. Using HistoricalStateProvider for proper package dependency resolution
//! 3. Handling upgraded packages with correct original_id and linkage
//! 4. Fetching all required shared objects (margin manager, pools, oracles)
//! 5. Building a PTB to call `manager_state`
//! 6. On-demand dynamic field fetching via `child_fetcher`
//!
//! ## Key Features
//!
//! - **Pre-computed versions**: Use Snowflake to efficiently find object versions
//!   at a target checkpoint, then use gRPC to fetch the actual BCS data
//! - **On-demand fetching**: Dynamic fields (used by `sui::versioned`) are
//!   fetched automatically during VM execution via the `child_fetcher` callback
//! - **Fully standalone**: Only requires gRPC access and a pre-generated versions file
//!
//! ## Quick Start (Recommended)
//!
//! ```bash
//! # Run with pre-computed versions from Snowflake (fast!)
//! VERSIONS_FILE=./data/deepbook_versions_240733000.json cargo run --example deepbook_margin_state
//! ```
//!
//! ## All Run Modes
//!
//! ```bash
//! # Mode 1: Historical state with pre-computed versions (RECOMMENDED)
//! VERSIONS_FILE=./data/deepbook_versions_240733000.json cargo run --example deepbook_margin_state
//!
//! # Mode 2: Current/latest state (no versions file)
//! cargo run --example deepbook_margin_state
//!
//! # Mode 3: Historical state via Walrus checkpoint scanning (slow fallback)
//! CHECKPOINT=240733000 cargo run --example deepbook_margin_state
//! ```
//!
//! ## Required Setup
//!
//! Configure your `.env` file:
//! ```
//! SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443
//! SUI_GRPC_API_KEY=your-api-key-here  # optional but recommended
//! ```
//!
//! ## Generating New Versions Files (requires Snowflake access)
//!
//! To create a versions file for a different checkpoint, use this Snowflake query pattern:
//!
//! ```sql
//! -- For each object, find its version at or before the target checkpoint
//! SELECT 'ObjectName' as name, object_id, version, checkpoint FROM (
//!     SELECT object_id, version, checkpoint
//!     FROM ANALYTICS_DB_V2.CHAINDATA_MAINNET.OBJECT
//!     WHERE object_id = '0x...'
//!       AND checkpoint <= TARGET_CHECKPOINT
//!     ORDER BY checkpoint DESC LIMIT 1
//! )
//! UNION ALL
//! -- repeat for each object...
//! ```
//!
//! Then save results to `data/deepbook_versions_CHECKPOINT.json` in the format:
//! ```json
//! {
//!   "checkpoint": 240733000,
//!   "objects": {
//!     "0x...": { "name": "Clock", "version": 714666359 }
//!   }
//! }
//! ```
//!
//! ## Architecture
//!
//! ```
//! ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
//! │   Snowflake     │────▶│  versions.json  │────▶│  Rust Example   │
//! │  (version       │     │  (checkpoint +  │     │  (load versions │
//! │   lookup)       │     │   object→ver)   │     │   from JSON)    │
//! └─────────────────┘     └─────────────────┘     └────────┬────────┘
//!                                                          │
//!                         ┌─────────────────┐              │
//!                         │   gRPC Endpoint │◀─────────────┘
//!                         │  (fetch BCS at  │
//!                         │   version)      │
//!                         └────────┬────────┘
//!                                  │
//!                         ┌────────▼────────┐
//!                         │   Local Move VM │
//!                         │  (execute PTB)  │
//!                         └─────────────────┘
//! ```

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use sui_sandbox_core::fetcher::GrpcFetcher;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput, ObjectID};
use sui_sandbox_core::simulation::{FetcherConfig, SimulationEnvironment};
use sui_state_fetcher::HistoricalStateProvider;
use sui_transport::grpc::GrpcOwner;
use sui_transport::walrus::{extract_object_versions_from_checkpoint, WalrusClient};

mod common;
use common::create_child_fetcher;

// ============================================================================
// DeepBook Margin Constants (Mainnet) - from @mysten/deepbook-v3 SDK
// ============================================================================

// DeepBook V3 Package
const DEEPBOOK_PACKAGE: &str = "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497";

// Margin Package
const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";

// USDC Coin Package (needed for type parameters - not in linkage tables)
const USDC_PACKAGE: &str = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7";

// Margin Registry (shared object that tracks all margin managers)
const MARGIN_REGISTRY: &str = "0x0e40998b359a9ccbab22a98ed21bd4346abf19158bc7980c8291908086b3a742";

// Clock object (system shared object)
const CLOCK: &str = "0x6";

// ============================================================================
// Target Margin Position (SUI/USDC pool with active loan)
// From Snowflake query: most recent position with loan
// ============================================================================

// Margin manager we want to query
const TARGET_MARGIN_MANAGER: &str =
    "0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80";

// Associated DeepBook pool (SUI/USDC)
const DEEPBOOK_POOL: &str = "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407";

// Margin pools
const BASE_MARGIN_POOL: &str = "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344"; // SUI pool
const QUOTE_MARGIN_POOL: &str = "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f"; // USDC pool

// Pyth Price Info Objects (from SDK mainnetCoins)
const SUI_PYTH_PRICE_INFO: &str = "0x801dbc2f0053d34734814b2d6df491ce7807a725fe9a01ad74a07e9c51396c37";
const USDC_PYTH_PRICE_INFO: &str = "0x5dec622733a204ca27f5a90d8c2fad453cc6665186fd5dff13a83d0b6c9027ab";

// Asset types
const SUI_TYPE: &str = "0x2::sui::SUI";
const USDC_TYPE: &str = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    print_header();

    let rt = tokio::runtime::Runtime::new()?;

    // Check for historical mode via VERSIONS_FILE (Snowflake-generated) or CHECKPOINT (Walrus scan)
    let versions_file = std::env::var("VERSIONS_FILE").ok();
    let checkpoint: Option<u64> = if versions_file.is_some() {
        None // VERSIONS_FILE takes precedence, checkpoint will be read from file
    } else {
        std::env::var("CHECKPOINT")
            .ok()
            .and_then(|s| s.parse().ok())
    };

    // For historical mode, fetch checkpoint data from Walrus to get object versions
    // Also check SCAN_CHECKPOINTS env var for how many checkpoints to scan backwards
    let scan_checkpoints: u64 = std::env::var("SCAN_CHECKPOINTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100); // Default to scanning 100 checkpoints

    // Load historical versions either from Snowflake JSON file or Walrus checkpoint scanning
    let (historical_versions, effective_checkpoint): (HashMap<String, u64>, Option<u64>) =
        if let Some(ref path) = versions_file {
            println!("  📂 SNOWFLAKE MODE: Loading versions from {}", path);

            match load_versions_from_json(path) {
                Ok((versions, cp)) => {
                    println!("     ✓ Loaded {} object versions at checkpoint {}", versions.len(), cp);
                    for (obj_id, ver) in &versions {
                        let short_id = &obj_id[..std::cmp::min(20, obj_id.len())];
                        println!("       - {}... = v{}", short_id, ver);
                    }
                    println!();
                    (versions, Some(cp))
                }
                Err(e) => {
                    println!("     ⚠ Failed to load versions file: {}", e);
                    println!("     Falling back to current state\n");
                    (HashMap::new(), None)
                }
            }
        } else if let Some(cp) = checkpoint {
            println!("  ⏱️  WALRUS MODE: Checkpoint {}", cp);
            println!("     Scanning up to {} checkpoints for object versions...", scan_checkpoints);

            let walrus = WalrusClient::mainnet();

            // First, get versions from the target checkpoint
            let mut versions: HashMap<String, u64> = match walrus.get_checkpoint(cp) {
                Ok(cp_data) => {
                    let v = extract_object_versions_from_checkpoint(&cp_data);
                    v.into_iter().map(|(k, (ver, _))| (k, ver)).collect()
                }
                Err(e) => {
                    println!("     ⚠ Failed to fetch target checkpoint: {}", e);
                    HashMap::new()
                }
            };

            println!("     Found {} versions in target checkpoint", versions.len());

            // Build list of objects we need to find versions for
            let objects_to_find: Vec<&str> = vec![
                TARGET_MARGIN_MANAGER,
                MARGIN_REGISTRY,
                DEEPBOOK_POOL,
                BASE_MARGIN_POOL,
                QUOTE_MARGIN_POOL,
                SUI_PYTH_PRICE_INFO,
                USDC_PYTH_PRICE_INFO,
                CLOCK,
            ];

            // Check which objects are missing from target checkpoint
            let missing: Vec<&str> = objects_to_find
                .iter()
                .filter(|obj_id| !versions.contains_key(**obj_id))
                .copied()
                .collect();

            if !missing.is_empty() && scan_checkpoints > 1 {
                println!("     {} objects not in target checkpoint, scanning backwards...", missing.len());

                // Build version index for missing objects
                match walrus.build_version_index_with_progress(
                    cp.saturating_sub(1), // Start from checkpoint before target
                    &missing,
                    scan_checkpoints - 1,
                    |scanned, found, remaining| {
                        if scanned % 10 == 0 || remaining == 0 {
                            print!("\r     Scanned {} checkpoints, found {}/{} objects...",
                                scanned, found, missing.len());
                            use std::io::Write;
                            std::io::stdout().flush().ok();
                        }
                    },
                ) {
                    Ok(index) => {
                        println!("\n     ✓ Version index built: scanned {} checkpoints", index.checkpoints_scanned);

                        // Merge found versions into our map
                        for (obj_id, version) in index.versions {
                            versions.insert(obj_id, version);
                        }

                        if !index.not_found.is_empty() {
                            println!("     ⚠ {} objects not found (will use latest):", index.not_found.len());
                            for obj_id in &index.not_found {
                                let short_id = if obj_id.len() > 20 { &obj_id[..20] } else { obj_id };
                                println!("        - {}...", short_id);
                            }
                        }
                    }
                    Err(e) => {
                        println!("\n     ⚠ Failed to build version index: {}", e);
                    }
                }
            }

            println!("     ✓ Total {} historical versions found\n", versions.len());
            (versions, Some(cp))
        } else {
            println!("  📍 CURRENT MODE: Latest state\n");
            (HashMap::new(), None)
        };

    // Use effective_checkpoint for the rest of the code
    let checkpoint = effective_checkpoint;

    // =========================================================================
    // STEP 1: Create HistoricalStateProvider for proper package resolution
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 1: Initializing HistoricalStateProvider");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let provider = rt.block_on(async { HistoricalStateProvider::mainnet().await })?;
    println!("  ✓ Connected to mainnet via HistoricalStateProvider\n");

    // =========================================================================
    // STEP 2: Fetch packages with FULL dependency resolution
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 2: Fetching packages with dependency resolution");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Key packages we need - the provider will fetch ALL transitive dependencies
    // Note: USDC_PACKAGE must be explicitly included because it's referenced via
    // type parameters (e.g., USDC type in MarginManager<SUI, USDC>) but not in
    // the package linkage tables
    let package_ids: Vec<AccountAddress> = vec![
        AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE)?,
        AccountAddress::from_hex_literal(MARGIN_PACKAGE)?,
        AccountAddress::from_hex_literal(USDC_PACKAGE)?,
    ];

    println!("  Fetching packages with transitive dependencies...");
    if checkpoint.is_some() {
        println!("  (at checkpoint {})", checkpoint.unwrap());
    }
    let packages = rt.block_on(async {
        provider
            .fetch_packages_with_deps(&package_ids, None, checkpoint)
            .await
    })?;

    println!("  ✓ Fetched {} packages total:", packages.len());
    for (addr, pkg) in &packages {
        let original = pkg
            .original_id
            .map(|o| format!("0x{}...", hex::encode(&o.as_ref()[..4])))
            .unwrap_or_else(|| "None".to_string());
        println!(
            "    - 0x{}... (v{}, orig={}, {} modules, {} linkages)",
            hex::encode(&addr.as_ref()[..4]),
            pkg.version,
            original,
            pkg.modules.len(),
            pkg.linkage.len()
        );
    }

    // Debug: Show all packages' linkage tables and check for missing deps
    println!("\n  Package linkage tables:");
    let mut all_linked_packages: std::collections::HashSet<AccountAddress> = std::collections::HashSet::new();
    for (addr, pkg) in &packages {
        if !pkg.linkage.is_empty() {
            println!("    0x{}... links to:", hex::encode(&addr.as_ref()[..4]));
            for (original, upgraded) in &pkg.linkage {
                let in_fetched = packages.contains_key(upgraded);
                let status = if in_fetched { "✓" } else { "✗ MISSING" };
                println!(
                    "      {} {} -> {}",
                    status,
                    original.to_hex_literal(),
                    upgraded.to_hex_literal()
                );
                all_linked_packages.insert(*upgraded);
            }
        }
    }

    // Check for any missing packages
    let missing: Vec<_> = all_linked_packages
        .iter()
        .filter(|addr| !packages.contains_key(*addr))
        .collect();
    if !missing.is_empty() {
        println!("\n  ⚠️  Missing packages in linkage tables:");
        for addr in &missing {
            println!("    - {}", addr.to_hex_literal());
        }
    } else {
        println!("\n  ✓ All linked packages are present");
    }

    // =========================================================================
    // STEP 3: Fetch required shared objects
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 3: Fetching shared objects");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let grpc = provider.grpc();

    let objects_to_fetch = [
        (TARGET_MARGIN_MANAGER, "Target Margin Manager"),
        (MARGIN_REGISTRY, "Margin Registry"),
        (DEEPBOOK_POOL, "DeepBook Pool (SUI/USDC)"),
        (BASE_MARGIN_POOL, "Base Margin Pool (SUI)"),
        (QUOTE_MARGIN_POOL, "Quote Margin Pool (USDC)"),
        (SUI_PYTH_PRICE_INFO, "SUI Pyth Oracle"),
        (USDC_PYTH_PRICE_INFO, "USDC Pyth Oracle"),
        (CLOCK, "Clock"),
    ];

    let mut fetched_objects: HashMap<String, (Vec<u8>, Option<String>, u64, bool)> = HashMap::new();

    // For historical mode, get the checkpoint timestamp to find matching object versions
    let checkpoint_timestamp_ms: Option<u64> = if let Some(cp) = checkpoint {
        rt.block_on(async { grpc.get_checkpoint(cp).await })
            .ok()
            .flatten()
            .and_then(|c| c.timestamp_ms)
    } else {
        None
    };

    if let Some(ts) = checkpoint_timestamp_ms {
        println!("  Checkpoint {} timestamp: {} ms", checkpoint.unwrap(), ts);
    }

    for (obj_id, name) in &objects_to_fetch {
        // Check if we have a historical version from Walrus checkpoint data
        let historical_version = historical_versions.get(*obj_id).copied();

        let result = if let Some(version) = historical_version {
            // Fetch at specific historical version
            rt.block_on(async { grpc.get_object_at_version(obj_id, Some(version)).await })
                .ok()
                .flatten()
                .and_then(|obj| {
                    let is_shared = matches!(obj.owner, GrpcOwner::Shared { .. });
                    let bcs = obj.bcs?;
                    Some((bcs, obj.type_string, obj.version, is_shared))
                })
        } else {
            // Fetch latest version
            rt.block_on(async { grpc.get_object(obj_id).await })
                .ok()
                .flatten()
                .and_then(|obj| {
                    let is_shared = matches!(obj.owner, GrpcOwner::Shared { .. });
                    let bcs = obj.bcs?;
                    Some((bcs, obj.type_string, obj.version, is_shared))
                })
        };

        match result {
            Some((bcs, type_str, version, is_shared)) => {
                let mode = if historical_version.is_some() {
                    " [historical]"
                } else if checkpoint.is_some() {
                    " [latest-fallback]"
                } else {
                    ""
                };
                println!("  ✓ {} (v{}){}", name, version, mode);
                fetched_objects.insert(obj_id.to_string(), (bcs, type_str, version, is_shared));
            }
            None => println!("  ✗ {} - not found or no BCS data", name),
        }
    }

    // =========================================================================
    // STEP 4: Create SimulationEnvironment and load packages with linkage
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 4: Creating SimulationEnvironment with package linkage");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut env = SimulationEnvironment::new()?;

    // Enable on-demand fetching for dynamic fields and missing objects
    // This uses the surflux historical gRPC endpoint
    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .unwrap_or_else(|_| "grpc.surflux.dev:443".to_string());
    println!("  Setting up on-demand fetcher: {}", grpc_endpoint);
    let fetcher = Box::new(GrpcFetcher::custom(&grpc_endpoint));
    let fetcher_config = FetcherConfig {
        enabled: true,
        network: Some("mainnet".to_string()),
        endpoint: Some(grpc_endpoint.clone()),
        use_archive: true,
        ..Default::default()
    };
    env = env.with_fetcher(fetcher, fetcher_config);
    println!("  ✓ On-demand fetching enabled");

    // Set up child fetcher for dynamic field resolution during VM execution
    // This is critical for DeepBook pools which use sui::versioned (dynamic fields)
    if let Some(_cp) = checkpoint {
        // Historical mode: gRPC-only child fetcher with version lookup from Walrus
        let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
        let grpc_endpoint_clone = grpc_endpoint.clone();
        let historical_versions_clone = historical_versions.clone();
        let checkpoint_fetcher: sui_sandbox_core::sandbox_runtime::ChildFetcherFn =
            Box::new(move |_parent_id, child_id| {
                let child_id_str = child_id.to_hex_literal();

                // Check if we have a historical version from the checkpoint
                let version = historical_versions_clone.get(&child_id_str).copied();

                // Fetch via gRPC at historical version if known, otherwise latest
                let rt = tokio::runtime::Runtime::new().ok()?;
                let grpc_result = rt.block_on(async {
                    let client = sui_transport::grpc::GrpcClient::with_api_key(
                        &grpc_endpoint_clone,
                        api_key.clone()
                    ).await.ok()?;
                    client.get_object_at_version(&child_id_str, version).await.ok()?
                })?;

                let type_str = grpc_result.type_string.as_ref()?;
                let bcs = grpc_result.bcs?;
                let type_tag = common::parse_type_tag(type_str)?;
                Some((type_tag, bcs))
            });
        env.set_child_fetcher(checkpoint_fetcher);
        println!(
            "  ✓ Child fetcher enabled (historical mode, {} known versions)",
            historical_versions.len()
        );
    } else {
        // Current mode: use gRPC-based child fetcher for latest state
        let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
        let child_grpc = rt.block_on(async {
            sui_transport::grpc::GrpcClient::with_api_key(&grpc_endpoint, api_key).await
        })?;
        let child_fetcher = create_child_fetcher(child_grpc, Default::default(), None);
        env.set_child_fetcher(child_fetcher);
        println!("  ✓ Child fetcher enabled for dynamic fields");
    }

    // Set a sender address
    let sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    env.set_sender(sender);

    // Build a map of package storage address -> version for proper linkage resolution
    let package_versions: HashMap<AccountAddress, u64> = packages
        .iter()
        .map(|(addr, pkg)| (*addr, pkg.version))
        .collect();

    // Build a mapping of original -> upgraded from all linkage tables
    // This tells us which packages have been upgraded
    let mut upgrade_map: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    for (_addr, pkg) in &packages {
        for (original, upgraded) in &pkg.linkage {
            // If original != upgraded, this is an upgrade relationship
            if original != upgraded {
                upgrade_map.insert(*original, *upgraded);
            }
        }
    }

    println!("  Detected {} package upgrades:", upgrade_map.len());
    for (original, upgraded) in &upgrade_map {
        println!(
            "    {} -> {}",
            original.to_hex_literal(),
            upgraded.to_hex_literal()
        );
    }

    // Build reverse mapping: upgraded -> original (for setting original_id)
    let original_id_map: HashMap<AccountAddress, AccountAddress> = upgrade_map
        .iter()
        .map(|(original, upgraded)| (*upgraded, *original))
        .collect();

    // Load packages with full linkage support
    // Skip packages that have been upgraded (we'll load the upgraded version instead)
    let mut loaded_count = 0;
    for (addr, pkg) in &packages {
        // Skip if this package has been upgraded to a different address
        if upgrade_map.contains_key(addr) {
            println!(
                "    Skipping 0x{}... (upgraded to 0x{}...)",
                hex::encode(&addr.as_ref()[..4]),
                hex::encode(&upgrade_map.get(addr).unwrap().as_ref()[..4])
            );
            continue;
        }

        // Determine original_id - if this package is an upgrade, set its original address
        let original_id = original_id_map.get(addr).copied();

        // Convert linkage from HashMap<AccountAddress, AccountAddress> to BTreeMap<AccountAddress, (AccountAddress, u64)>
        // IMPORTANT: Use the actual version of each linked package, not the caller's version
        let linkage: BTreeMap<AccountAddress, (AccountAddress, u64)> = pkg
            .linkage
            .iter()
            .map(|(original, upgraded)| {
                // Look up the version of the linked package, default to 1 if not found
                let linked_version = package_versions.get(upgraded).copied().unwrap_or(1);
                (*original, (*upgraded, linked_version))
            })
            .collect();

        match env.register_package_with_linkage(*addr, pkg.version, original_id, pkg.modules.clone(), linkage) {
            Ok(()) => {
                if original_id.is_some() {
                    println!(
                        "    Loaded 0x{}... (v{}, upgrade of 0x{}...)",
                        hex::encode(&addr.as_ref()[..4]),
                        pkg.version,
                        hex::encode(&original_id.unwrap().as_ref()[..4])
                    );
                }
                loaded_count += 1;
            }
            Err(e) => println!(
                "    Warning: Failed to load package 0x{}...: {}",
                hex::encode(&addr.as_ref()[..4]),
                e
            ),
        }
    }
    println!("  ✓ Loaded {} packages with linkage into environment", loaded_count);

    // Load objects
    for (obj_id, (bcs, type_str, version, is_shared)) in &fetched_objects {
        env.load_object_from_data(
            obj_id,
            bcs.clone(),
            type_str.as_deref(),
            *is_shared,
            false,
            *version,
        )?;
    }
    println!("  ✓ Loaded {} objects into environment", fetched_objects.len());

    // Fetch dynamic fields for versioned objects (Pool uses Versioned which stores inner data as DF)
    println!("\n  Fetching dynamic fields for versioned objects...");
    let versioned_objects = [
        (DEEPBOOK_POOL, "DeepBook Pool"),
        (BASE_MARGIN_POOL, "Base Margin Pool"),
        (QUOTE_MARGIN_POOL, "Quote Margin Pool"),
    ];

    let graphql = provider.graphql();
    let mut df_count = 0;
    for (parent_id, name) in &versioned_objects {
        match graphql.fetch_dynamic_fields(parent_id, 10) {
            Ok(fields) => {
                println!("    {} has {} dynamic fields", name, fields.len());
                for field in fields {
                    // The wrapper object ID is in object_id
                    if let Some(obj_id) = &field.object_id {
                        let _version = field.version.unwrap_or(1);
                        // Fetch the actual dynamic field object
                        match rt.block_on(async { grpc.get_object(obj_id).await }) {
                            Ok(Some(obj)) => {
                                if let Some(bcs) = obj.bcs {
                                    env.load_object_from_data(
                                        obj_id,
                                        bcs,
                                        obj.type_string.as_deref(),
                                        false, // dynamic fields are not shared
                                        false,
                                        obj.version,
                                    )?;
                                    df_count += 1;
                                    println!(
                                        "      ✓ DF {} (v{}, key_type={})",
                                        &obj_id[..16],
                                        obj.version,
                                        &field.name_type
                                    );
                                }
                            }
                            Ok(None) => {
                                println!("      ⚠ DF {} not found", obj_id);
                            }
                            Err(e) => {
                                println!("      ⚠ DF {} fetch error: {}", obj_id, e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("    ⚠ {} - failed to fetch DFs: {}", name, e);
            }
        }
    }
    println!("  ✓ Loaded {} dynamic field objects", df_count);

    // =========================================================================
    // STEP 5: Call manager_state function
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 5: Calling manager_state on margin position");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Target margin manager: {}", TARGET_MARGIN_MANAGER);
    println!("  Pool: SUI/USDC");
    println!();

    // Parse type arguments for the generic function
    let base_type = TypeTag::from_str(SUI_TYPE)?;
    let quote_type = TypeTag::from_str(USDC_TYPE)?;

    let margin_pkg = AccountAddress::from_hex_literal(MARGIN_PACKAGE)?;

    // Helper to create shared object input
    fn make_shared_input(
        obj_id: &str,
        fetched: &HashMap<String, (Vec<u8>, Option<String>, u64, bool)>,
    ) -> Result<InputValue> {
        let (bcs, type_str, version, _) = fetched
            .get(obj_id)
            .ok_or_else(|| anyhow::anyhow!("Object {} not found", obj_id))?;

        let type_tag = type_str.as_ref().and_then(|s| TypeTag::from_str(s).ok());

        Ok(InputValue::Object(ObjectInput::Shared {
            id: ObjectID::from_hex_literal(obj_id)?,
            bytes: bcs.clone(),
            type_tag,
            version: Some(*version),
            mutable: false, // read-only call
        }))
    }

    // Build inputs from fetched objects
    let inputs = vec![
        make_shared_input(TARGET_MARGIN_MANAGER, &fetched_objects)?,
        make_shared_input(MARGIN_REGISTRY, &fetched_objects)?,
        make_shared_input(SUI_PYTH_PRICE_INFO, &fetched_objects)?,
        make_shared_input(USDC_PYTH_PRICE_INFO, &fetched_objects)?,
        make_shared_input(DEEPBOOK_POOL, &fetched_objects)?,
        make_shared_input(BASE_MARGIN_POOL, &fetched_objects)?,
        make_shared_input(QUOTE_MARGIN_POOL, &fetched_objects)?,
        make_shared_input(CLOCK, &fetched_objects)?,
    ];

    // Build the MoveCall command
    let commands = vec![Command::MoveCall {
        package: margin_pkg,
        module: Identifier::new("margin_manager")?,
        function: Identifier::new("manager_state")?,
        type_args: vec![base_type, quote_type],
        args: vec![
            Argument::Input(0), // self (margin manager)
            Argument::Input(1), // registry
            Argument::Input(2), // base_oracle
            Argument::Input(3), // quote_oracle
            Argument::Input(4), // pool
            Argument::Input(5), // base_margin_pool
            Argument::Input(6), // quote_margin_pool
            Argument::Input(7), // clock
        ],
    }];

    // Execute the PTB
    let result = env.execute_ptb(inputs, commands);

    if result.success {
        println!("  ✓ manager_state call SUCCEEDED");
        if let Some(effects) = &result.effects {
            println!("    Gas used: {} MIST", effects.gas_used);
            println!("    Events emitted: {}", effects.events.len());
        }

        println!("\n  Note: Return values from pure functions require VM state inspection.");
        println!("  The successful execution confirms the position health is calculable.");
    } else {
        println!("  ✗ manager_state call FAILED");
        if let Some(err) = &result.error {
            println!("    Error: {:?}", err);
        }

        // Provide context for the expected error
        if result.raw_error.as_ref().map_or(false, |e| e.contains("dynamic_field")) {
            println!("\n  ℹ️  This error is expected for DeepBook pools which use Versioned objects.");
            println!("     Versioned objects store their inner data in dynamic fields that need");
            println!("     to be fetched separately. Solutions:");
            println!("     1. Use fullnode's simulate_transaction API (recommended)");
            println!("     2. Implement on-demand dynamic field fetching");
            println!("     3. Pre-fetch dynamic field objects using graphql.fetch_dynamic_fields()");
        }
    }

    // =========================================================================
    // Summary
    // =========================================================================
    print_summary();

    Ok(())
}

/// Load object versions from a JSON file generated by Snowflake queries.
///
/// Expected format:
/// ```json
/// {
///   "checkpoint": 240733000,
///   "objects": {
///     "0x...": { "name": "...", "version": 123456 }
///   }
/// }
/// ```
fn load_versions_from_json(path: &str) -> Result<(HashMap<String, u64>, u64)> {
    let content = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    let checkpoint = json["checkpoint"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing 'checkpoint' field in versions file"))?;

    let objects = json["objects"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Missing 'objects' field in versions file"))?;

    let mut versions = HashMap::new();
    for (obj_id, obj_data) in objects {
        if let Some(version) = obj_data["version"].as_u64() {
            versions.insert(obj_id.clone(), version);
        }
    }

    Ok((versions, checkpoint))
}

fn print_header() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║        DeepBook Margin Manager State Query Example                   ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║  Demonstrates historical state reconstruction using:                 ║");
    println!("║    1. Snowflake → object versions at a checkpoint (pre-computed)     ║");
    println!("║    2. gRPC → fetch BCS data at historical versions                   ║");
    println!("║    3. Local Move VM → execute manager_state function                 ║");
    println!("║                                                                      ║");
    println!("║  Run with: VERSIONS_FILE=./data/deepbook_versions_240733000.json     ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
}

fn print_summary() {
    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         MANAGER_STATE RETURNS                        ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║  Index | Field                    | Description                      ║");
    println!("║  ------+-------------------------+----------------------------------║");
    println!("║    0   | manager_id              | Margin manager object ID          ║");
    println!("║    1   | deepbook_pool_id        | Associated DeepBook pool          ║");
    println!("║    2   | risk_ratio              | Health factor (assets/debt)       ║");
    println!("║    3   | base_asset              | Base asset balance (w/ locked)    ║");
    println!("║    4   | quote_asset             | Quote asset balance               ║");
    println!("║    5   | base_debt               | Borrowed base amount              ║");
    println!("║    6   | quote_debt              | Borrowed quote amount             ║");
    println!("║    7   | base_pyth_price         | Pyth oracle price for base        ║");
    println!("║    8   | base_pyth_decimals      | Base price decimals               ║");
    println!("║    9   | quote_pyth_price        | Pyth oracle price for quote       ║");
    println!("║   10   | quote_pyth_decimals     | Quote price decimals              ║");
    println!("║   11   | current_price           | Calculated base/quote price       ║");
    println!("║   12   | lowest_trigger_above    | TP/SL trigger (longs)             ║");
    println!("║   13   | highest_trigger_below   | TP/SL trigger (shorts)            ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
}
