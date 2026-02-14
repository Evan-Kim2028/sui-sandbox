//! DeepBook Margin State Time Series Example (Position B)
//!
//! This example demonstrates **historical time series tracking** of DeepBook v3 margin
//! positions on Sui, iterating through 8 consecutive daily snapshots to show position
//! evolution over time.
//!
//! ## What This Demonstrates
//!
//! 1. **Daily time series reconstruction** - Query margin state at multiple historical points
//! 2. **Position evolution tracking** - See how margin health changes day-over-day
//! 3. **Batch historical queries** - Efficiently process multiple checkpoints
//!
//! ## Quick Start
//!
//! ```bash
//! # Run with default time series file (Position B - 8 days)
//! cargo run --example deepbook_timeseries
//!
//! # Run with Walrus mode (fully decentralized, no gRPC)
//! WALRUS_MODE=1 cargo run --example deepbook_timeseries
//! ```
//!
//! ## Time Series Data
//!
//! Uses pre-computed object versions from `data/position_b_daily_timeseries.json`:
//!
//! | Day | Checkpoint | Description |
//! |-----|------------|-------------|
//! | 1 | 235510810 | Position creation |
//! | 2 | 235859237 | First activity day |
//! | 3 | 236134228 | Continued trading |
//! | 4 | 236289445 | Position growth |
//! | 5 | 236527001 | Mid-week |
//! | 6 | 236790859 | Active trading |
//! | 7 | 237019020 | Approaching week end |
//! | 8 | 237335780 | Week 1 complete |

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

use sui_sandbox_core::fetcher::GrpcFetcher;
use sui_sandbox_core::simulation::{FetcherConfig, SimulationEnvironment};
use sui_sandbox_core::utilities::collect_required_package_roots_from_type_strings;
use sui_state_fetcher::types::PackageData;
use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::GrpcClient;
use sui_transport::walrus::{extract_object_bcs, get_object_from_checkpoint, WalrusClient};

#[path = "../../common/mod.rs"]
mod common;

// ============================================================================
// DeepBook Margin Constants (Mainnet) - from @mysten/deepbook-v3 SDK
// ============================================================================

const DEEPBOOK_PACKAGE: &str = "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497";
const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";

// Shared objects (same for all SUI/USDC positions)
const MARGIN_REGISTRY: &str = "0x0e40998b359a9ccbab22a98ed21bd4346abf19158bc7980c8291908086b3a742";
const CLOCK: &str = "0x6";
const DEEPBOOK_POOL: &str = "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407";
const BASE_MARGIN_POOL: &str = "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344";
const QUOTE_MARGIN_POOL: &str =
    "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f";
const SUI_PYTH_PRICE_INFO: &str =
    "0x801dbc2f0053d34734814b2d6df491ce7807a725fe9a01ad74a07e9c51396c37";
const USDC_PYTH_PRICE_INFO: &str =
    "0x5dec622733a204ca27f5a90d8c2fad453cc6665186fd5dff13a83d0b6c9027ab";

// Asset types
const SUI_TYPE: &str = "0x2::sui::SUI";
const USDC_TYPE: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

// Default time series file path
const DEFAULT_TIMESERIES_FILE: &str =
    "./examples/advanced/deepbook_margin_state/data/position_b_daily_timeseries.json";

// Type alias for fetched object data: (bcs_bytes, type_string, version, is_shared)
type FetchedObjectData = common::ExampleFetchedObjectData;

// ============================================================================
// JSON Schema for Time Series Data
// ============================================================================

#[derive(Debug, Deserialize)]
struct TimeSeriesData {
    description: String,
    margin_manager_id: String,
    pool_type: String,
    #[serde(default)]
    position_created_checkpoint: Option<u64>,
    daily_snapshots: Vec<DailySnapshot>,
}

#[derive(Debug, Deserialize)]
struct DailySnapshot {
    day: u32,
    checkpoint: u64,
    #[serde(default)]
    datetime_utc: Option<String>,
    description: String,
    objects: HashMap<String, ObjectVersionInfo>,
}

#[derive(Debug, Deserialize, Clone)]
struct ObjectVersionInfo {
    // Note: JSON has a `name` field but we don't need it - names come from objects_to_fetch
    version: u64,
    checkpoint_found: u64,
}

/// Results from a single day's manager_state call
#[derive(Debug, Default)]
struct DayResult {
    day: u32,
    datetime_utc: Option<String>,
    success: bool,
    gas_used: u64,
    error: Option<String>,
    /// Margin state from manager_state return values
    margin_state: Option<MarginState>,
}

/// Decoded margin state from manager_state return values
/// The function returns 14 values in this order (matching main.rs):
/// (manager_id, deepbook_pool_id, risk_ratio, base_asset, quote_asset,
///  base_debt, quote_debt, base_pyth_price, base_pyth_decimals,
///  quote_pyth_price, quote_pyth_decimals, current_price,
///  lowest_trigger_above, highest_trigger_below)
#[derive(Debug, Clone, Default)]
struct MarginState {
    // Note: Return values 0-1 are manager_id and pool_id (object IDs) - skipped
    /// Risk ratio / health factor (scaled by 1e9)
    risk_ratio: u64,
    /// Base asset (SUI) balance including locked (in MIST, 1e9 = 1 SUI)
    base_asset: u64,
    /// Quote asset (USDC) balance (scaled by 1e6)
    quote_asset: u64,
    /// Base asset debt (in MIST)
    base_debt: u64,
    /// Quote asset debt (scaled by 1e6)
    quote_debt: u64,
    /// Base Pyth oracle price (scaled)
    base_pyth_price: u64,
    /// Quote Pyth oracle price (scaled)
    quote_pyth_price: u64,
    /// Current calculated price (base/quote)
    current_price: u64,
}

impl MarginState {
    /// Decode margin state from BCS-encoded return values
    fn from_return_values(return_values: &[Vec<u8>]) -> Option<Self> {
        if return_values.len() < 14 {
            return None;
        }

        fn decode_u64(bytes: &[u8]) -> u64 {
            if bytes.len() >= 8 {
                u64::from_le_bytes(bytes[0..8].try_into().unwrap_or([0; 8]))
            } else {
                0
            }
        }

        // Skip return_values[0] (manager_id) and [1] (pool_id) - they're object IDs
        Some(MarginState {
            risk_ratio: decode_u64(&return_values[2]),
            base_asset: decode_u64(&return_values[3]),
            quote_asset: decode_u64(&return_values[4]),
            base_debt: decode_u64(&return_values[5]),
            quote_debt: decode_u64(&return_values[6]),
            base_pyth_price: decode_u64(&return_values[7]),
            quote_pyth_price: decode_u64(&return_values[9]),
            current_price: decode_u64(&return_values[11]),
        })
    }

    /// Get risk ratio as a percentage (divide by 1e9 then * 100)
    fn risk_ratio_percent(&self) -> f64 {
        self.risk_ratio as f64 / 1e9 * 100.0
    }

    /// Get base asset balance in SUI
    fn base_asset_sui(&self) -> f64 {
        self.base_asset as f64 / 1e9
    }

    /// Get quote asset balance in USDC
    fn quote_asset_usdc(&self) -> f64 {
        self.quote_asset as f64 / 1e6
    }

    /// Get base debt in SUI
    fn base_debt_sui(&self) -> f64 {
        self.base_debt as f64 / 1e9
    }

    /// Get quote debt in USDC
    fn quote_debt_usdc(&self) -> f64 {
        self.quote_debt as f64 / 1e6
    }

    /// Get current price (base/quote) - scaled by 1e6
    fn current_price_display(&self) -> f64 {
        self.current_price as f64 / 1e6
    }

    /// Get base Pyth price in USD (scaled by 1e8)
    fn base_price_usd(&self) -> f64 {
        self.base_pyth_price as f64 / 1e8
    }

    /// Get quote Pyth price in USD (scaled by 1e8)
    fn quote_price_usd(&self) -> f64 {
        self.quote_pyth_price as f64 / 1e8
    }
}

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    print_header();

    let rt = tokio::runtime::Runtime::new()?;

    // Load time series data
    let timeseries_file =
        std::env::var("TIMESERIES_FILE").unwrap_or_else(|_| DEFAULT_TIMESERIES_FILE.to_string());

    let walrus_mode = std::env::var("WALRUS_MODE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    println!("  📂 Loading time series from: {}", timeseries_file);
    let timeseries = load_timeseries(&timeseries_file)?;

    println!("  ✓ {}", timeseries.description);
    println!(
        "  📊 Pool: {} | Snapshots: {}",
        timeseries.pool_type,
        timeseries.daily_snapshots.len()
    );
    println!(
        "  📍 Margin Manager: {}...",
        &timeseries.margin_manager_id[..20]
    );
    if let Some(created_cp) = timeseries.position_created_checkpoint {
        println!("  🕐 Position created at checkpoint: {}", created_cp);
    }
    println!();

    // =========================================================================
    // Initialize HistoricalStateProvider (once for all days)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("INITIALIZING: HistoricalStateProvider");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let provider = rt.block_on(common::create_mainnet_provider(true))?;
    let grpc_endpoint = provider.grpc_endpoint().to_string();
    println!(
        "  ✓ Connected to mainnet via HistoricalStateProvider ({})\n",
        grpc_endpoint
    );

    // Fetch packages once (they don't change across checkpoints for this example)
    println!("  Fetching packages with transitive dependencies...");
    let explicit_roots = vec![
        AccountAddress::from_hex_literal(DEEPBOOK_PACKAGE)?,
        AccountAddress::from_hex_literal(MARGIN_PACKAGE)?,
    ];
    let type_roots = vec![SUI_TYPE.to_string(), USDC_TYPE.to_string()];
    let package_ids: Vec<AccountAddress> =
        collect_required_package_roots_from_type_strings(&explicit_roots, &type_roots)?
            .into_iter()
            .collect();

    let packages = rt.block_on(async {
        provider
            .fetch_packages_with_deps(&package_ids, None, None)
            .await
    })?;
    println!("  ✓ Fetched {} packages total\n", packages.len());

    let package_registration_plan = common::build_package_registration_plan(&packages);
    println!(
        "  ✓ Package linkage plan prepared ({} upgrades)\n",
        package_registration_plan.upgrade_map.len()
    );

    // =========================================================================
    // Process each day's snapshot
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "PROCESSING: {} Daily Snapshots",
        timeseries.daily_snapshots.len()
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut results: Vec<DayResult> = Vec::new();
    let grpc = provider.grpc();
    let graphql = provider.graphql();

    for snapshot in &timeseries.daily_snapshots {
        println!(
            "┌─────────────────────────────────────────────────────────────────────────────────┐"
        );
        let datetime_str = snapshot.datetime_utc.as_deref().unwrap_or("N/A");
        println!(
            "│  Day {}: {} | {} (Checkpoint {})",
            snapshot.day, datetime_str, snapshot.description, snapshot.checkpoint
        );
        println!(
            "└─────────────────────────────────────────────────────────────────────────────────┘"
        );
        let day_result = process_snapshot(
            &rt,
            snapshot,
            &timeseries.margin_manager_id,
            walrus_mode,
            &grpc_endpoint,
            grpc,
            graphql,
            &packages,
            &package_registration_plan,
        )?;

        println!();
        results.push(day_result);
    }

    // =========================================================================
    // Summary Table
    // =========================================================================
    print_summary_table(&results, &timeseries);

    Ok(())
}

fn load_timeseries(path: &str) -> Result<TimeSeriesData> {
    let content = std::fs::read_to_string(path)?;
    let data: TimeSeriesData = serde_json::from_str(&content)?;
    Ok(data)
}

fn process_snapshot(
    rt: &tokio::runtime::Runtime,
    snapshot: &DailySnapshot,
    margin_manager_id: &str,
    walrus_mode: bool,
    grpc_endpoint: &str,
    grpc: &GrpcClient,
    graphql: &GraphQLClient,
    packages: &HashMap<AccountAddress, PackageData>,
    package_registration_plan: &common::PackageRegistrationPlan,
) -> Result<DayResult> {
    let mut day_result = DayResult {
        day: snapshot.day,
        datetime_utc: snapshot.datetime_utc.clone(),
        ..Default::default()
    };

    let historical_versions: HashMap<String, u64> = snapshot
        .objects
        .iter()
        .map(|(id, info)| (id.clone(), info.version))
        .collect();

    let version_info: HashMap<String, ObjectVersionInfo> = snapshot
        .objects
        .iter()
        .map(|(id, info)| (id.clone(), info.clone()))
        .collect();

    let objects_to_fetch = [
        (margin_manager_id, "Margin Manager"),
        (MARGIN_REGISTRY, "Margin Registry"),
        (DEEPBOOK_POOL, "DeepBook Pool"),
        (BASE_MARGIN_POOL, "Base Margin Pool"),
        (QUOTE_MARGIN_POOL, "Quote Margin Pool"),
        (SUI_PYTH_PRICE_INFO, "SUI Oracle"),
        (USDC_PYTH_PRICE_INFO, "USDC Oracle"),
        (CLOCK, "Clock"),
    ];

    let fetched_objects = fetch_snapshot_objects(
        rt,
        walrus_mode,
        grpc,
        &objects_to_fetch,
        &historical_versions,
        &version_info,
    );

    let mut env = SimulationEnvironment::new()?;
    let fetcher = Box::new(GrpcFetcher::custom(grpc_endpoint));
    let fetcher_config = FetcherConfig {
        enabled: true,
        network: Some("mainnet".to_string()),
        endpoint: Some(grpc_endpoint.to_string()),
        use_archive: true,
    };
    env = env.with_fetcher(fetcher, fetcher_config);

    let api_key = std::env::var("SUI_GRPC_API_KEY").ok();
    let child_grpc = rt.block_on(async {
        sui_transport::grpc::GrpcClient::with_api_key(grpc_endpoint, api_key).await
    })?;
    let child_fetcher = common::create_child_fetcher(child_grpc, historical_versions.clone(), None);
    env.set_child_fetcher(child_fetcher);

    let sender = AccountAddress::from_hex_literal(
        "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    )?;
    env.set_sender(sender);

    let registration =
        common::register_packages_with_linkage_plan(&mut env, packages, package_registration_plan);
    if !registration.failed.is_empty() {
        println!(
            "  ⚠ {} package(s) failed to register for day {}",
            registration.failed.len(),
            snapshot.day
        );
        for (addr, err) in &registration.failed {
            println!("     - {}: {}", addr.to_hex_literal(), err);
        }
    }

    let _ = common::load_fetched_objects_into_env(&mut env, &fetched_objects, false)?;

    let dynamic_field_objects = common::preload_dynamic_field_objects(
        rt,
        graphql,
        grpc,
        &[DEEPBOOK_POOL, BASE_MARGIN_POOL, QUOTE_MARGIN_POOL],
        10,
    );
    let _ = common::load_fetched_objects_into_env(&mut env, &dynamic_field_objects, false)?;

    let base_type = TypeTag::from_str(SUI_TYPE)?;
    let quote_type = TypeTag::from_str(USDC_TYPE)?;
    let margin_pkg = AccountAddress::from_hex_literal(MARGIN_PACKAGE)?;
    let result = common::execute_shared_move_call(
        &mut env,
        margin_pkg,
        "margin_manager",
        "manager_state",
        vec![base_type, quote_type],
        &[
            margin_manager_id,
            MARGIN_REGISTRY,
            SUI_PYTH_PRICE_INFO,
            USDC_PYTH_PRICE_INFO,
            DEEPBOOK_POOL,
            BASE_MARGIN_POOL,
            QUOTE_MARGIN_POOL,
            CLOCK,
        ],
        &fetched_objects,
    )?;

    if result.success {
        day_result.success = true;
        if let Some(effects) = &result.effects {
            day_result.gas_used = effects.gas_used;
            if !effects.return_values.is_empty() && !effects.return_values[0].is_empty() {
                let return_vals = &effects.return_values[0];
                if let Some(margin_state) = MarginState::from_return_values(return_vals) {
                    day_result.margin_state = Some(margin_state.clone());
                    print_margin_state(&margin_state, snapshot.day);
                }
            }
        }
        println!(
            "  ✅ manager_state SUCCEEDED (gas: {} MIST)",
            day_result.gas_used
        );
    } else {
        day_result.success = false;
        let mut combined_error = String::new();
        if let Some(err) = &result.error {
            day_result.error = Some(format!("{:?}", err));
            combined_error.push_str(&format!("{err:?}"));
        }
        println!("  ❌ manager_state FAILED");
        if let Some(raw_err) = &result.raw_error {
            println!("     Raw error: {}", raw_err);
            if !combined_error.is_empty() {
                combined_error.push('\n');
            }
            combined_error.push_str(raw_err);
        }
        if let Some(e) = &day_result.error {
            println!("     Error: {}", e);
        }
        common::maybe_print_archive_runtime_hint(&combined_error);
    }

    Ok(day_result)
}

fn fetch_snapshot_objects(
    rt: &tokio::runtime::Runtime,
    walrus_mode: bool,
    grpc: &GrpcClient,
    objects_to_fetch: &[(&str, &str)],
    historical_versions: &HashMap<String, u64>,
    version_info: &HashMap<String, ObjectVersionInfo>,
) -> HashMap<String, FetchedObjectData> {
    if walrus_mode {
        println!("  🌊 Fetching objects from Walrus...");
        let walrus = WalrusClient::mainnet();
        let mut fetched = fetch_objects_from_walrus(&walrus, objects_to_fetch, version_info);

        let missing_count = objects_to_fetch.len() - fetched.len();
        if missing_count > 0 {
            println!(
                "  ⚠ {} objects not in Walrus, falling back to gRPC...",
                missing_count
            );
            for (obj_id, name) in objects_to_fetch {
                if fetched.contains_key(*obj_id) {
                    continue;
                }
                let historical_version = historical_versions.get(*obj_id).copied();
                let result = common::fetch_object_data(rt, grpc, obj_id, historical_version, true);
                if let Some((bcs, type_str, version, is_shared)) = result {
                    println!("    ✓ {} (v{}) [gRPC]", name, version);
                    fetched.insert(obj_id.to_string(), (bcs, type_str, version, is_shared));
                }
            }
        }
        return fetched;
    }

    let mut fetched: HashMap<String, FetchedObjectData> = HashMap::new();
    for (obj_id, _name) in objects_to_fetch {
        let historical_version = historical_versions.get(*obj_id).copied();
        let result = common::fetch_object_data(rt, grpc, obj_id, historical_version, true);
        if let Some((bcs, type_str, version, is_shared)) = result {
            fetched.insert(obj_id.to_string(), (bcs, type_str, version, is_shared));
        }
    }
    println!("  ✓ Fetched {} objects via gRPC", fetched.len());
    fetched
}

fn fetch_objects_from_walrus(
    walrus: &WalrusClient,
    objects_to_fetch: &[(&str, &str)],
    version_info: &HashMap<String, ObjectVersionInfo>,
) -> HashMap<String, FetchedObjectData> {
    use std::str::FromStr;
    use sui_types::base_types::ObjectID as SuiObjectID;

    let mut fetched_objects: HashMap<String, FetchedObjectData> = HashMap::new();

    // Group objects by checkpoint_found
    let mut by_checkpoint: HashMap<u64, Vec<(&str, &str)>> = HashMap::new();
    for (obj_id, name) in objects_to_fetch {
        if let Some(info) = version_info.get(*obj_id) {
            by_checkpoint
                .entry(info.checkpoint_found)
                .or_default()
                .push((*obj_id, *name));
        }
    }

    // Fetch each checkpoint and extract objects
    for (checkpoint_num, objects) in &by_checkpoint {
        match walrus.get_checkpoint(*checkpoint_num) {
            Ok(checkpoint_data) => {
                for (obj_id, name) in objects {
                    let sui_obj_id = match SuiObjectID::from_str(obj_id) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    if let Some(obj) = get_object_from_checkpoint(&checkpoint_data, &sui_obj_id) {
                        if let Some((type_str, bcs, version, is_shared)) = extract_object_bcs(&obj)
                        {
                            println!("    ✓ {} (v{}) from cp {}", name, version, checkpoint_num);
                            fetched_objects.insert(
                                obj_id.to_string(),
                                (bcs, Some(type_str), version, is_shared),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                println!("    ⚠ Failed to fetch checkpoint {}: {}", checkpoint_num, e);
            }
        }
    }

    fetched_objects
}

fn print_header() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║     DeepBook Margin State Time Series Example (Position B)          ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!("║  Demonstrates historical time series tracking:                       ║");
    println!("║    • 8 consecutive daily snapshots                                   ║");
    println!("║    • Position evolution tracking                                     ║");
    println!("║    • Day-over-day margin health analysis                             ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
}

/// Print detailed margin state for a single day
fn print_margin_state(state: &MarginState, day: u32) {
    println!("  ┌───────────────────────────────────────────────────────────────────────┐");
    println!(
        "  │  📊 MARGIN STATE - Day {}                                              │",
        day
    );
    println!("  ├───────────────────────────────────────────────────────────────────────┤");

    // Risk metrics
    let risk_pct = state.risk_ratio_percent();
    let risk_status = if risk_pct >= 1000.0 {
        "🟢 NO DEBT"
    } else if risk_pct > 50.0 {
        "🟢 HEALTHY"
    } else if risk_pct > 20.0 {
        "🟡 MODERATE"
    } else if risk_pct > 10.0 {
        "🟠 HIGH RISK"
    } else {
        "🔴 CRITICAL"
    };

    println!(
        "  │  Risk Ratio:           {:>10.2}%  {}                         │",
        risk_pct, risk_status
    );
    println!("  ├───────────────────────────────────────────────────────────────────────┤");

    // Prices
    println!(
        "  │  SUI/USDC Price:       ${:>10.4}                                     │",
        state.current_price_display()
    );
    println!(
        "  │  SUI Oracle (USD):     ${:>10.4}                                     │",
        state.base_price_usd()
    );
    println!(
        "  │  USDC Oracle (USD):    ${:>10.4}                                     │",
        state.quote_price_usd()
    );
    println!("  ├───────────────────────────────────────────────────────────────────────┤");

    // Base asset (SUI)
    println!(
        "  │  SUI Balance:          {:>15.4} SUI                           │",
        state.base_asset_sui()
    );
    println!(
        "  │  SUI Debt:             {:>15.4} SUI                           │",
        state.base_debt_sui()
    );

    // Quote asset (USDC)
    println!(
        "  │  USDC Balance:         {:>15.2} USDC                          │",
        state.quote_asset_usdc()
    );
    println!(
        "  │  USDC Debt:            {:>15.2} USDC                          │",
        state.quote_debt_usdc()
    );
    println!("  └───────────────────────────────────────────────────────────────────────┘");
}

fn print_summary_table(results: &[DayResult], timeseries: &TimeSeriesData) {
    println!("╔══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╗");
    println!("║                                       TIME SERIES RESULTS SUMMARY                                                        ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    println!(
        "║  Position: {}...                                                               ║",
        &timeseries.margin_manager_id[..42]
    );
    println!("║  Pool: {:109}║", timeseries.pool_type);
    println!("╠══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    println!("║  Day │ Date (UTC)          │ SUI Price │ SUI Balance │ SUI Debt   │ USDC Balance │ USDC Debt │ Risk %     │ Status     ║");
    println!("║──────┼─────────────────────┼───────────┼─────────────┼────────────┼──────────────┼───────────┼────────────┼────────────║");

    for result in results {
        let status = if result.success { "✅" } else { "❌" };
        let date_display = result
            .datetime_utc
            .as_deref()
            .map(|d| &d[..19]) // Take "YYYY-MM-DDTHH:MM:SS" part
            .unwrap_or("N/A                ");
        if let Some(ref state) = result.margin_state {
            let risk_display = if state.risk_ratio_percent() >= 1000.0 {
                "NO DEBT".to_string()
            } else {
                format!("{:.2}%", state.risk_ratio_percent())
            };
            println!(
                "║  {:3} │ {} │ ${:>7.4} │ {:>10.4} │ {:>9.4} │ {:>11.2} │ {:>8.2} │ {:>10} │     {}     ║",
                result.day,
                date_display,
                state.current_price_display(),
                state.base_asset_sui(),
                state.base_debt_sui(),
                state.quote_asset_usdc(),
                state.quote_debt_usdc(),
                risk_display,
                status
            );
        } else {
            println!(
                "║  {:3} │ {} │       N/A │         N/A │        N/A │          N/A │       N/A │        N/A │     {}     ║",
                result.day, date_display, status
            );
        }
    }

    let success_count = results.iter().filter(|r| r.success).count();
    let total = results.len();

    println!("╠══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╣");

    // Calculate and display trends if we have data
    let states: Vec<&MarginState> = results
        .iter()
        .filter_map(|r| r.margin_state.as_ref())
        .collect();
    if states.len() >= 2 {
        let first = states.first().unwrap();
        let last = states.last().unwrap();

        let price_change = last.current_price_display() - first.current_price_display();
        let sui_debt_change = last.base_debt_sui() - first.base_debt_sui();

        let price_arrow = if price_change > 0.0 {
            "↑"
        } else if price_change < 0.0 {
            "↓"
        } else {
            "→"
        };
        let debt_arrow = if sui_debt_change > 0.0 {
            "↑"
        } else if sui_debt_change < 0.0 {
            "↓"
        } else {
            "→"
        };

        println!(
            "║  📈 TRENDS: SUI Price: {} ${:+.4}  |  SUI Debt: {} {:+.4}                                                               ║",
            price_arrow, price_change, debt_arrow, sui_debt_change
        );
        println!("╠══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    }

    println!(
        "║  Success Rate: {}/{} ({:.0}%)                                                                                              ║",
        success_count,
        total,
        (success_count as f64 / total as f64) * 100.0
    );
    println!("╚══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╝");
}
