//! Execute historical view functions across checkpoint series snapshots.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use sui_sandbox_core::orchestrator::{HistoricalSeriesExecutionOptions, ReplayOrchestrator};

#[derive(Debug, Parser)]
#[command(
    name = "historical-series",
    about = "Execute a historical Move view function across a checkpoint/version series"
)]
pub struct HistoricalSeriesCmd {
    /// Request spec file (JSON or YAML) matching HistoricalViewRequest
    #[arg(long, value_name = "PATH")]
    request_file: PathBuf,

    /// Series points file (JSON or YAML). Supported formats:
    /// - [{checkpoint, versions, label?, metadata?}, ...]
    /// - { points: [...] }
    /// - { daily_snapshots: [{checkpoint, day?, datetime_utc?, description?, objects:{id:{version}|u64}}] }
    #[arg(long, value_name = "PATH")]
    series_file: PathBuf,

    /// Optional decode schema file (JSON/YAML list of ReturnDecodeField)
    #[arg(long, value_name = "PATH")]
    schema_file: Option<PathBuf>,

    /// Command index to decode when schema is supplied
    #[arg(long, value_name = "INDEX", default_value_t = 0)]
    command_index: usize,

    /// Optional gRPC endpoint override
    #[arg(long, value_name = "URL")]
    grpc_endpoint: Option<String>,

    /// Optional gRPC API key override
    #[arg(long, value_name = "KEY")]
    grpc_api_key: Option<String>,

    /// Maximum worker threads for per-point execution
    #[arg(long, value_name = "N", default_value_t = 1)]
    max_concurrency: usize,
}

impl HistoricalSeriesCmd {
    pub async fn execute(&self, json_output: bool) -> Result<()> {
        let _ = json_output;
        let options = HistoricalSeriesExecutionOptions {
            max_concurrency: Some(self.max_concurrency.max(1)),
        };
        let report = ReplayOrchestrator::execute_historical_series_from_files_with_options(
            &self.request_file,
            &self.series_file,
            self.schema_file.as_deref(),
            self.command_index,
            self.grpc_endpoint.as_deref(),
            self.grpc_api_key.as_deref(),
            &options,
        )
        .with_context(|| "execute historical-series file workflow")?;
        let output = serde_json::json!({
            "request": report.request,
            "points": report.points.len(),
            "summary": report.summary,
            "runs": report.runs,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }
}
