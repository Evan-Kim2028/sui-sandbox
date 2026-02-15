//! DeepBook Margin State Time Series Example (Position B)
//!
//! This example demonstrates historical time series tracking of DeepBook v3
//! margin positions on Sui, iterating through daily snapshots and decoding
//! manager_state outputs.
//!
//! ## Quick Start
//!
//! ```bash
//! cargo run --example deepbook_timeseries
//! ```

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

use sui_sandbox_core::historical_view::{
    HistoricalVersionsSnapshot, HistoricalViewOutput, HistoricalViewRequest,
};
use sui_sandbox_core::orchestrator::{ReplayOrchestrator, ReturnDecodeField};

// ============================================================================
// DeepBook Margin Constants (Mainnet)
// ============================================================================

const DEEPBOOK_PACKAGE: &str = "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497";
const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";

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

const SUI_TYPE: &str = "0x2::sui::SUI";
const USDC_TYPE: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

const DEFAULT_TIMESERIES_FILE: &str =
    "./examples/advanced/deepbook_margin_state/data/position_b_daily_timeseries.json";

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
    version: u64,
    #[allow(dead_code)]
    checkpoint_found: u64,
}

#[derive(Debug, Default)]
struct DayResult {
    day: u32,
    datetime_utc: Option<String>,
    success: bool,
    gas_used: u64,
    error: Option<String>,
    margin_state: Option<MarginState>,
}

#[derive(Debug, Clone, Default)]
struct MarginState {
    risk_ratio_pct: f64,
    base_asset_sui: f64,
    quote_asset_usdc: f64,
    base_debt_sui: f64,
    quote_debt_usdc: f64,
    base_pyth_price_usd: f64,
    quote_pyth_price_usd: f64,
    current_price_usdc: f64,
}

impl MarginState {
    fn from_historical_output(output: &HistoricalViewOutput) -> Result<Option<Self>> {
        let raw = &output.raw;
        let schema = vec![
            ReturnDecodeField {
                index: 2,
                name: "risk_ratio_pct".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(10_000_000.0), // u64 / 1e9 * 100
            },
            ReturnDecodeField {
                index: 3,
                name: "base_asset_sui".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(1_000_000_000.0),
            },
            ReturnDecodeField {
                index: 4,
                name: "quote_asset_usdc".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(1_000_000.0),
            },
            ReturnDecodeField {
                index: 5,
                name: "base_debt_sui".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(1_000_000_000.0),
            },
            ReturnDecodeField {
                index: 6,
                name: "quote_debt_usdc".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(1_000_000.0),
            },
            ReturnDecodeField {
                index: 7,
                name: "base_pyth_price_usd".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(100_000_000.0),
            },
            ReturnDecodeField {
                index: 9,
                name: "quote_pyth_price_usd".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(100_000_000.0),
            },
            ReturnDecodeField {
                index: 11,
                name: "current_price_usdc".to_string(),
                type_hint: Some("u64".to_string()),
                scale: Some(1_000_000.0),
            },
        ];

        let Some(decoded) = ReplayOrchestrator::decode_command_return_schema(raw, 0, &schema)?
        else {
            return Ok(None);
        };

        Ok(Some(Self {
            risk_ratio_pct: json_number(&decoded, "risk_ratio_pct")?,
            base_asset_sui: json_number(&decoded, "base_asset_sui")?,
            quote_asset_usdc: json_number(&decoded, "quote_asset_usdc")?,
            base_debt_sui: json_number(&decoded, "base_debt_sui")?,
            quote_debt_usdc: json_number(&decoded, "quote_debt_usdc")?,
            base_pyth_price_usd: json_number(&decoded, "base_pyth_price_usd")?,
            quote_pyth_price_usd: json_number(&decoded, "quote_pyth_price_usd")?,
            current_price_usdc: json_number(&decoded, "current_price_usdc")?,
        }))
    }

    fn risk_ratio_percent(&self) -> f64 {
        self.risk_ratio_pct
    }

    fn base_asset_sui(&self) -> f64 {
        self.base_asset_sui
    }

    fn quote_asset_usdc(&self) -> f64 {
        self.quote_asset_usdc
    }

    fn base_debt_sui(&self) -> f64 {
        self.base_debt_sui
    }

    fn quote_debt_usdc(&self) -> f64 {
        self.quote_debt_usdc
    }

    fn current_price_display(&self) -> f64 {
        self.current_price_usdc
    }

    fn base_price_usd(&self) -> f64 {
        self.base_pyth_price_usd
    }

    fn quote_price_usd(&self) -> f64 {
        self.quote_pyth_price_usd
    }
}

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    print_header();

    let timeseries_file =
        std::env::var("TIMESERIES_FILE").unwrap_or_else(|_| DEFAULT_TIMESERIES_FILE.to_string());
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

    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT").ok();
    let grpc_api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let request = HistoricalViewRequest {
        package_id: MARGIN_PACKAGE.to_string(),
        module: "margin_manager".to_string(),
        function: "manager_state".to_string(),
        type_args: vec![SUI_TYPE.to_string(), USDC_TYPE.to_string()],
        required_objects: vec![
            timeseries.margin_manager_id.clone(),
            MARGIN_REGISTRY.to_string(),
            SUI_PYTH_PRICE_INFO.to_string(),
            USDC_PYTH_PRICE_INFO.to_string(),
            DEEPBOOK_POOL.to_string(),
            BASE_MARGIN_POOL.to_string(),
            QUOTE_MARGIN_POOL.to_string(),
            CLOCK.to_string(),
        ],
        package_roots: vec![MARGIN_PACKAGE.to_string(), DEEPBOOK_PACKAGE.to_string()],
        type_refs: vec![SUI_TYPE.to_string(), USDC_TYPE.to_string()],
        fetch_child_objects: true,
    };

    let snapshots: Vec<HistoricalVersionsSnapshot> = timeseries
        .daily_snapshots
        .iter()
        .map(|snapshot| {
            let versions = snapshot
                .objects
                .iter()
                .map(|(id, info)| (id.clone(), info.version))
                .collect();
            ReplayOrchestrator::snapshot_from_checkpoint_versions(snapshot.checkpoint, versions)
        })
        .collect();

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "PROCESSING: {} Daily Snapshots",
        timeseries.daily_snapshots.len()
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let outputs = ReplayOrchestrator::execute_historical_view_batch(
        &snapshots,
        &request,
        grpc_endpoint.as_deref(),
        grpc_api_key.as_deref(),
    )?;

    let mut results = Vec::with_capacity(outputs.len());
    for (snapshot, output) in timeseries.daily_snapshots.iter().zip(outputs.iter()) {
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

        let mut day = DayResult {
            day: snapshot.day,
            datetime_utc: snapshot.datetime_utc.clone(),
            success: output.success,
            gas_used: output.gas_used.unwrap_or(0),
            error: output.error.clone().or_else(|| output.hint.clone()),
            margin_state: None,
        };

        if output.success {
            if let Some(state) = MarginState::from_historical_output(output)? {
                print_margin_state(&state, snapshot.day);
                day.margin_state = Some(state);
            }
            println!("  ✅ manager_state SUCCEEDED (gas: {} MIST)", day.gas_used);
        } else {
            println!("  ❌ manager_state FAILED");
            if let Some(error) = output.error.as_deref() {
                println!("     Error: {}", error);
            }
            if let Some(hint) = output.hint.as_deref() {
                println!("     Hint: {}", hint);
            }
        }
        println!();
        results.push(day);
    }

    print_summary_table(&results, &timeseries);
    Ok(())
}

fn load_timeseries(path: &str) -> Result<TimeSeriesData> {
    let content = std::fs::read_to_string(path)?;
    let data: TimeSeriesData = serde_json::from_str(&content)?;
    Ok(data)
}

fn json_number(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Result<f64> {
    let value = map
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("decoded field '{}' missing", key))?;
    match value {
        serde_json::Value::Number(num) => num
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("decoded field '{}' is not f64-representable", key)),
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("decoded field '{}' parse error: {}", key, e)),
        serde_json::Value::Null => Ok(0.0),
        other => Err(anyhow::anyhow!(
            "decoded field '{}' has non-numeric type: {}",
            key,
            other
        )),
    }
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

fn print_margin_state(state: &MarginState, day: u32) {
    println!("  ┌───────────────────────────────────────────────────────────────────────┐");
    println!(
        "  │  📊 MARGIN STATE - Day {}                                              │",
        day
    );
    println!("  ├───────────────────────────────────────────────────────────────────────┤");

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

    println!(
        "  │  SUI Balance:          {:>15.4} SUI                           │",
        state.base_asset_sui()
    );
    println!(
        "  │  SUI Debt:             {:>15.4} SUI                           │",
        state.base_debt_sui()
    );
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
            .map(|d| &d[..19])
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
    let failed_with_error = results
        .iter()
        .filter(|r| !r.success && r.error.as_ref().is_some())
        .count();
    if failed_with_error > 0 {
        println!(
            "║  Failed snapshots with errors: {}                                                                                         ║",
            failed_with_error
        );
    }
    println!("╚══════════════════════════════════════════════════════════════════════════════════════════════════════════════════════════╝");
}
