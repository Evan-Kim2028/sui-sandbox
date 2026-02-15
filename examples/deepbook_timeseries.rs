//! DeepBook Margin State Time Series (data-driven)
//!
//! This example runs one historical view request across a daily checkpoint series
//! using JSON/YAML request/schema files and shared orchestrator utilities.
//!
//! Run:
//! ```bash
//! cargo run --example deepbook_timeseries
//! ```

use anyhow::{Context, Result};
use std::path::Path;

use sui_sandbox_core::orchestrator::{HistoricalSeriesExecutionOptions, ReplayOrchestrator};

const DEFAULT_REQUEST_FILE: &str = "examples/data/deepbook_margin_state/manager_state_request.json";
const DEFAULT_SERIES_FILE: &str =
    "examples/data/deepbook_margin_state/position_b_daily_timeseries.json";
const DEFAULT_SCHEMA_FILE: &str = "examples/data/deepbook_margin_state/manager_state_schema.json";

#[derive(Debug, Clone, Default)]
struct MarginState {
    risk_ratio_pct: f64,
    base_asset_sui: f64,
    quote_asset_usdc: f64,
    base_debt_sui: f64,
    quote_debt_usdc: f64,
    current_price_usdc: f64,
}

impl MarginState {
    fn from_decoded(decoded: &serde_json::Map<String, serde_json::Value>) -> Result<Self> {
        Ok(Self {
            risk_ratio_pct: ReplayOrchestrator::decoded_number_field(decoded, "risk_ratio_pct")?,
            base_asset_sui: ReplayOrchestrator::decoded_number_field(decoded, "base_asset_sui")?,
            quote_asset_usdc: ReplayOrchestrator::decoded_number_field(
                decoded,
                "quote_asset_usdc",
            )?,
            base_debt_sui: ReplayOrchestrator::decoded_number_field(decoded, "base_debt_sui")?,
            quote_debt_usdc: ReplayOrchestrator::decoded_number_field(decoded, "quote_debt_usdc")?,
            current_price_usdc: ReplayOrchestrator::decoded_number_field(
                decoded,
                "current_price_usdc",
            )?,
        })
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let request_file =
        std::env::var("REQUEST_FILE").unwrap_or_else(|_| DEFAULT_REQUEST_FILE.to_string());
    let series_file =
        std::env::var("TIMESERIES_FILE").unwrap_or_else(|_| DEFAULT_SERIES_FILE.to_string());
    let schema_file =
        std::env::var("SCHEMA_FILE").unwrap_or_else(|_| DEFAULT_SCHEMA_FILE.to_string());
    let max_concurrency = env_usize("MAX_CONCURRENCY", 4).max(1);

    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT").ok();
    let grpc_api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    println!("DeepBook historical manager_state series");
    println!("  request: {}", request_file);
    println!("  series:  {}", series_file);
    println!("  schema:  {}", schema_file);
    let options = HistoricalSeriesExecutionOptions {
        max_concurrency: Some(max_concurrency),
    };
    let report = ReplayOrchestrator::execute_historical_series_from_files_with_options(
        Path::new(&request_file),
        Path::new(&series_file),
        Some(Path::new(&schema_file)),
        0,
        grpc_endpoint.as_deref(),
        grpc_api_key.as_deref(),
        &options,
    )
    .with_context(|| "execute historical series from request/series/schema files")?;
    println!("  points:  {}", report.points.len());
    println!("  workers: {}\n", max_concurrency);
    let runs = report.runs;
    let summary = report.summary;

    println!(
        "summary: success={}/{} failed={} success_rate={:.1}% gas_total={}\n",
        summary.success,
        summary.total,
        summary.failed,
        summary.success_rate * 100.0,
        summary.total_gas_used
    );
    println!(
        "{:<6} {:<21} {:<12} {:>7} {:>10} {:>11} {:>10} {:>10} {:>10} {:>10}",
        "day",
        "datetime_utc",
        "checkpoint",
        "gas",
        "risk%",
        "price_usdc",
        "base_sui",
        "quote_usdc",
        "base_debt",
        "quote_debt"
    );

    for run in &runs {
        let day = run
            .metadata_u64("day")
            .map(|v| v.to_string())
            .unwrap_or_else(|| run.label.clone().unwrap_or_else(|| "n/a".to_string()));
        let datetime = run
            .metadata_str("datetime_utc")
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "n/a".to_string());

        if run.output.success {
            let decoded = run
                .decoded
                .as_ref()
                .context("missing decoded output for successful run")?;
            let state = MarginState::from_decoded(decoded)?;
            println!(
                "{:<6} {:<21} {:<12} {:>7} {:>10.2} {:>11.4} {:>10.4} {:>10.2} {:>10.4} {:>10.2}",
                day,
                datetime,
                run.checkpoint,
                run.output.gas_used.unwrap_or(0),
                state.risk_ratio_pct,
                state.current_price_usdc,
                state.base_asset_sui,
                state.quote_asset_usdc,
                state.base_debt_sui,
                state.quote_debt_usdc
            );
        } else {
            println!(
                "{:<6} {:<21} {:<12} {:>7} {:>10} {:>11} {:>10} {:>10} {:>10} {:>10}",
                day,
                datetime,
                run.checkpoint,
                run.output.gas_used.unwrap_or(0),
                "fail",
                "-",
                "-",
                "-",
                "-",
                "-"
            );
            if let Some(err) = run.output.error.as_deref() {
                println!("  error: {}", err);
            }
            if let Some(hint) = run.output.hint.as_deref() {
                println!("  hint:  {}", hint);
            }
        }
    }

    Ok(())
}
