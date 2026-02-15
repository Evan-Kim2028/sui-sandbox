//! Shared orchestration helpers for replay-focused workflows.
//!
//! This module centralizes command planning for typed workflow replay steps so
//! CLI and Python bindings can share the same behavior.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::historical_view::{
    execute_historical_view_from_snapshot, execute_historical_view_from_versions,
    load_json_or_yaml_file, HistoricalVersionsSnapshot, HistoricalViewOutput,
    HistoricalViewRequest,
};
use crate::ptb::{Argument, Command, InputValue, ObjectChange, ObjectInput, PTBBuilder};
use crate::simulation::{ExecutionResult, SimulationEnvironment};
use crate::workflow::{WorkflowAnalyzeReplayStep, WorkflowDefaults, WorkflowReplayStep};

/// Replay-first orchestrator surface shared by CLI/Python adapters.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReplayOrchestrator;

/// Typed decode output for one command return value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecodedReturnValue {
    /// Zero-based value index inside a command return tuple.
    pub index: usize,
    /// Canonical type tag (when available).
    pub type_tag: Option<String>,
    /// Decoded value (best-effort typed JSON).
    pub value: serde_json::Value,
    /// Original bytes as base64.
    pub raw_base64: String,
    /// Original bytes as hex.
    pub raw_hex: String,
}

/// Named schema field for command return decoding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReturnDecodeField {
    /// Zero-based index in the command return tuple.
    pub index: usize,
    /// Output key name in the decoded object.
    pub name: String,
    /// Optional decode hint (e.g., `u64`, `address`, `vector<u8>`, `utf8`, `hex`).
    #[serde(default)]
    pub type_hint: Option<String>,
    /// Optional divisor applied to numeric values (e.g., `1e9`).
    #[serde(default)]
    pub scale: Option<f64>,
}

impl ReturnDecodeField {
    /// Create a schema field with no decode hints.
    pub fn new(index: usize, name: impl Into<String>) -> Self {
        Self {
            index,
            name: name.into(),
            type_hint: None,
            scale: None,
        }
    }

    /// Create a schema field with canonical `u64` decoding and scaling.
    pub fn scaled_u64(index: usize, name: impl Into<String>, scale: f64) -> Self {
        Self::new(index, name)
            .with_type_hint("u64")
            .with_scale(scale)
    }

    pub fn with_type_hint(mut self, hint: impl Into<String>) -> Self {
        self.type_hint = Some(hint.into());
        self
    }

    pub fn with_scale(mut self, scale: f64) -> Self {
        self.scale = Some(scale);
        self
    }
}

/// One labeled checkpoint snapshot for historical series execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoricalSeriesPoint {
    pub checkpoint: u64,
    pub versions: HashMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl HistoricalSeriesPoint {
    pub fn new(checkpoint: u64, versions: HashMap<String, u64>) -> Self {
        Self {
            checkpoint,
            versions,
            label: None,
            metadata: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Output row for one historical series point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalSeriesRun {
    pub checkpoint: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub output: HistoricalViewOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded: Option<serde_json::Map<String, serde_json::Value>>,
}

impl HistoricalSeriesRun {
    /// Read one metadata field as `u64`.
    pub fn metadata_u64(&self, key: &str) -> Option<u64> {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.get(key))
            .and_then(serde_json::Value::as_u64)
    }

    /// Read one metadata field as `&str`.
    pub fn metadata_str(&self, key: &str) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.get(key))
            .and_then(serde_json::Value::as_str)
    }
}

/// Execution options for historical series runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HistoricalSeriesExecutionOptions {
    /// Optional upper bound on worker threads for per-point execution.
    ///
    /// `None` or `Some(1)` executes sequentially.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
}

/// Aggregate summary over historical series runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoricalSeriesSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub success_rate: f64,
    pub total_gas_used: u64,
    pub average_gas_used: f64,
}

/// Full output payload for one historical-series execution run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalSeriesReport {
    pub request: HistoricalViewRequest,
    pub points: Vec<HistoricalSeriesPoint>,
    pub runs: Vec<HistoricalSeriesRun>,
    pub summary: HistoricalSeriesSummary,
}

/// Compact execution-effects summary for one PTB/function call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecutionEffectsSummary {
    pub gas_used: u64,
    pub created_objects: usize,
    pub mutated_objects: usize,
    pub deleted_objects: usize,
    pub wrapped_objects: usize,
    pub unwrapped_objects: usize,
    pub transferred_objects: usize,
}

/// Successful PTB execution payload with raw result + compact summary.
#[derive(Debug)]
pub struct SuccessfulExecution {
    pub result: ExecutionResult,
    pub summary: ExecutionEffectsSummary,
}

/// Builder for one Move call command.
#[derive(Debug, Clone)]
pub struct MoveCallBuilder {
    package: AccountAddress,
    module: String,
    function: String,
    type_args: Vec<TypeTag>,
    args: Vec<Argument>,
}

impl MoveCallBuilder {
    pub fn new(
        package: AccountAddress,
        module: impl Into<String>,
        function: impl Into<String>,
    ) -> Self {
        Self {
            package,
            module: module.into(),
            function: function.into(),
            type_args: Vec::new(),
            args: Vec::new(),
        }
    }

    pub fn with_type_args<I>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = TypeTag>,
    {
        self.type_args = values.into_iter().collect();
        self
    }

    pub fn with_args<I>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = Argument>,
    {
        self.args = values.into_iter().collect();
        self
    }

    pub fn push_arg(mut self, value: Argument) -> Self {
        self.args.push(value);
        self
    }

    pub fn build(self) -> Result<Command> {
        Ok(Command::MoveCall {
            package: self.package,
            module: Identifier::new(self.module.as_str())?,
            function: Identifier::new(self.function.as_str())?,
            type_args: self.type_args,
            args: self.args,
        })
    }
}

/// Replay-focused PTB builder that works directly with `SimulationEnvironment`.
#[derive(Default)]
pub struct ReplayPtbBuilder {
    inner: PTBBuilder,
}

impl ReplayPtbBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pure<T: serde::Serialize>(&mut self, value: T) -> Result<Argument> {
        self.inner.pure(&value)
    }

    pub fn input(&mut self, value: InputValue) -> Result<Argument> {
        Ok(self.inner.add_input_value(value))
    }

    pub fn owned_object_from_env(
        &mut self,
        env: &SimulationEnvironment,
        object_id: &str,
    ) -> Result<Argument> {
        self.input(ReplayOrchestrator::owned_object_input(env, object_id)?)
    }

    pub fn immutable_object_from_env(
        &mut self,
        env: &SimulationEnvironment,
        object_id: &str,
    ) -> Result<Argument> {
        self.input(ReplayOrchestrator::immutable_object_input(env, object_id)?)
    }

    pub fn shared_object_from_env(
        &mut self,
        env: &SimulationEnvironment,
        object_id: &str,
        mutable: bool,
    ) -> Result<Argument> {
        self.input(ReplayOrchestrator::shared_object_input(
            env, object_id, mutable,
        )?)
    }

    pub fn command(&mut self, command: Command) -> Result<Argument> {
        Ok(self.inner.add_command(command))
    }

    pub fn move_call(&mut self, call: MoveCallBuilder) -> Result<Argument> {
        self.command(call.build()?)
    }

    pub fn nested_result(command_result: Argument, value_index: u16) -> Result<Argument> {
        match command_result {
            Argument::Result(command_index) => {
                Ok(Argument::NestedResult(command_index, value_index))
            }
            other => Err(anyhow!(
                "expected Argument::Result for nested result reference, got {:?}",
                other
            )),
        }
    }

    pub fn inputs(&self) -> &[InputValue] {
        self.inner.inputs()
    }

    pub fn commands(&self) -> &[Command] {
        self.inner.commands()
    }

    pub fn into_parts(self) -> (Vec<InputValue>, Vec<Command>) {
        self.inner.into_parts()
    }

    pub fn execute(self, env: &mut SimulationEnvironment) -> ExecutionResult {
        let (inputs, commands) = self.inner.into_parts();
        env.execute_ptb(inputs, commands)
    }
}

fn run_historical_series_point(
    point: &HistoricalSeriesPoint,
    request: &HistoricalViewRequest,
    grpc_endpoint: Option<&str>,
    grpc_api_key: Option<&str>,
) -> Result<HistoricalSeriesRun> {
    let snapshot = HistoricalVersionsSnapshot {
        checkpoint: point.checkpoint,
        versions: point.versions.clone(),
    };
    let output = ReplayOrchestrator::execute_historical_view_from_snapshot(
        &snapshot,
        request,
        grpc_endpoint,
        grpc_api_key,
    )?;
    Ok(HistoricalSeriesRun {
        checkpoint: point.checkpoint,
        label: point.label.clone(),
        metadata: point.metadata.clone(),
        output,
        decoded: None,
    })
}

fn parse_historical_series_point_entry(value: &serde_json::Value) -> Result<HistoricalSeriesPoint> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("series point must be an object"))?;
    let checkpoint = obj
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("series point missing numeric `checkpoint`"))?;
    let versions = match obj.get("versions").or_else(|| obj.get("objects")) {
        Some(v) => parse_series_versions_map(v)?,
        None => {
            return Err(anyhow!(
                "series point missing `versions` (or `objects`) map"
            ))
        }
    };

    let mut point = HistoricalSeriesPoint::new(checkpoint, versions);
    if let Some(label) = obj.get("label").and_then(serde_json::Value::as_str) {
        point = point.with_label(label.to_string());
    }
    if let Some(metadata) = obj.get("metadata") {
        point = point.with_metadata(metadata.clone());
    }
    Ok(point)
}

fn parse_daily_snapshot_series_entry(value: &serde_json::Value) -> Result<HistoricalSeriesPoint> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("daily snapshot must be an object"))?;
    let checkpoint = obj
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("daily snapshot missing numeric `checkpoint`"))?;
    let versions = parse_series_versions_map(
        obj.get("objects")
            .ok_or_else(|| anyhow!("daily snapshot missing `objects` map"))?,
    )?;

    let mut point = HistoricalSeriesPoint::new(checkpoint, versions);
    if let Some(day) = obj.get("day").and_then(serde_json::Value::as_u64) {
        point = point.with_label(format!("day-{}", day));
    }
    point = point.with_metadata(serde_json::json!({
        "day": obj.get("day").cloned(),
        "datetime_utc": obj.get("datetime_utc").cloned(),
        "description": obj.get("description").cloned(),
    }));
    Ok(point)
}

fn parse_series_versions_map(value: &serde_json::Value) -> Result<HashMap<String, u64>> {
    let map = value
        .as_object()
        .ok_or_else(|| anyhow!("versions/objects field must be a map"))?;
    let mut out = HashMap::with_capacity(map.len());
    for (id, entry) in map {
        let version = entry.as_u64().or_else(|| {
            entry
                .as_object()
                .and_then(|inner| inner.get("version"))
                .and_then(serde_json::Value::as_u64)
        });
        let version = version.ok_or_else(|| anyhow!("object {} missing numeric `version`", id))?;
        out.insert(id.clone(), version);
    }
    Ok(out)
}

impl ReplayOrchestrator {
    /// Build a pure PTB input from a serializable value.
    pub fn pure_input<T: serde::Serialize>(value: T) -> Result<InputValue> {
        Ok(InputValue::Pure(bcs::to_bytes(&value)?))
    }

    /// Build an owned-object PTB input from an object id currently loaded in `env`.
    pub fn owned_object_input(env: &SimulationEnvironment, object_id: &str) -> Result<InputValue> {
        Ok(InputValue::Object(
            env.get_object_for_ptb_with_mode(object_id, Some("owned"))?,
        ))
    }

    /// Build an immutable-object PTB input from an object id currently loaded in `env`.
    pub fn immutable_object_input(
        env: &SimulationEnvironment,
        object_id: &str,
    ) -> Result<InputValue> {
        Ok(InputValue::Object(env.get_object_for_ptb_with_mode(
            object_id,
            Some("immutable"),
        )?))
    }

    /// Build a shared-object PTB input from an object id currently loaded in `env`.
    pub fn shared_object_input(
        env: &SimulationEnvironment,
        object_id: &str,
        mutable: bool,
    ) -> Result<InputValue> {
        let id = AccountAddress::from_hex_literal(object_id)
            .with_context(|| format!("invalid object id: {object_id}"))?;
        let obj = env
            .get_object(&id)
            .ok_or_else(|| anyhow!("object not found in environment: {object_id}"))?;
        Ok(InputValue::Object(ObjectInput::Shared {
            id,
            bytes: obj.bcs_bytes.clone(),
            type_tag: Some(obj.type_tag.clone()),
            version: Some(obj.version),
            mutable,
        }))
    }

    /// Execute a no-arg Move call against the provided environment.
    pub fn execute_noarg_move_call(
        env: &mut SimulationEnvironment,
        package: AccountAddress,
        module: &str,
        function: &str,
    ) -> Result<ExecutionResult> {
        let cmd = Command::MoveCall {
            package,
            module: Identifier::new(module)?,
            function: Identifier::new(function)?,
            type_args: vec![],
            args: vec![],
        };
        Ok(env.execute_ptb(vec![], vec![cmd]))
    }

    /// Execute a generic historical view request from a versions snapshot.
    pub fn execute_historical_view_from_versions(
        versions_file: &Path,
        request: &HistoricalViewRequest,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
    ) -> Result<HistoricalViewOutput> {
        execute_historical_view_from_versions(versions_file, request, grpc_endpoint, grpc_api_key)
    }

    /// Execute a generic historical view request from an in-memory snapshot.
    pub fn execute_historical_view_from_snapshot(
        snapshot: &HistoricalVersionsSnapshot,
        request: &HistoricalViewRequest,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
    ) -> Result<HistoricalViewOutput> {
        execute_historical_view_from_snapshot(snapshot, request, grpc_endpoint, grpc_api_key)
    }

    /// Execute one view request across multiple historical snapshots.
    pub fn execute_historical_view_batch(
        snapshots: &[HistoricalVersionsSnapshot],
        request: &HistoricalViewRequest,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
    ) -> Result<Vec<HistoricalViewOutput>> {
        let mut outputs = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            outputs.push(Self::execute_historical_view_from_snapshot(
                snapshot,
                request,
                grpc_endpoint,
                grpc_api_key,
            )?);
        }
        Ok(outputs)
    }

    /// Convenience constructor for one historical-series point.
    pub fn series_point_from_checkpoint_versions(
        checkpoint: u64,
        versions: HashMap<String, u64>,
    ) -> HistoricalSeriesPoint {
        HistoricalSeriesPoint::new(checkpoint, versions)
    }

    /// Load historical-series points from a JSON/YAML document.
    ///
    /// Supported formats:
    /// - `[{checkpoint, versions|objects, label?, metadata?}, ...]`
    /// - `{ points: [...] }`
    /// - `{ daily_snapshots: [{checkpoint, day?, datetime_utc?, description?, objects:{id:{version}|u64}}] }`
    pub fn load_historical_series_points(path: &Path) -> Result<Vec<HistoricalSeriesPoint>> {
        let value: serde_json::Value = load_json_or_yaml_file(path)?;
        Self::parse_historical_series_points_value(&value)
    }

    /// Parse historical-series points from an in-memory JSON value.
    pub fn parse_historical_series_points_value(
        value: &serde_json::Value,
    ) -> Result<Vec<HistoricalSeriesPoint>> {
        if let Some(array) = value.as_array() {
            return array
                .iter()
                .map(parse_historical_series_point_entry)
                .collect::<Result<Vec<_>>>();
        }

        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("series payload must be an array or object"))?;

        if let Some(points_value) = object.get("points") {
            let points = points_value
                .as_array()
                .ok_or_else(|| anyhow!("`points` must be an array"))?;
            return points
                .iter()
                .map(parse_historical_series_point_entry)
                .collect::<Result<Vec<_>>>();
        }

        if let Some(snapshots_value) = object.get("daily_snapshots") {
            let snapshots = snapshots_value
                .as_array()
                .ok_or_else(|| anyhow!("`daily_snapshots` must be an array"))?;
            return snapshots
                .iter()
                .map(parse_daily_snapshot_series_entry)
                .collect::<Result<Vec<_>>>();
        }

        Err(anyhow!(
            "series payload missing supported keys (`points` or `daily_snapshots`)"
        ))
    }

    /// Execute historical-series directly from request/series file inputs.
    pub fn execute_historical_series_from_files(
        request_file: &Path,
        series_file: &Path,
        schema_file: Option<&Path>,
        command_index: usize,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
    ) -> Result<HistoricalSeriesReport> {
        Self::execute_historical_series_from_files_with_options(
            request_file,
            series_file,
            schema_file,
            command_index,
            grpc_endpoint,
            grpc_api_key,
            &HistoricalSeriesExecutionOptions::default(),
        )
    }

    /// Execute historical-series directly from request/series file inputs.
    pub fn execute_historical_series_from_files_with_options(
        request_file: &Path,
        series_file: &Path,
        schema_file: Option<&Path>,
        command_index: usize,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
        options: &HistoricalSeriesExecutionOptions,
    ) -> Result<HistoricalSeriesReport> {
        let request: HistoricalViewRequest = load_json_or_yaml_file(request_file)
            .with_context(|| format!("parse request file {}", request_file.display()))?;
        let points = Self::load_historical_series_points(series_file)
            .with_context(|| format!("parse series file {}", series_file.display()))?;

        let runs = if let Some(schema_path) = schema_file {
            let schema: Vec<ReturnDecodeField> = load_json_or_yaml_file(schema_path)
                .with_context(|| format!("parse schema file {}", schema_path.display()))?;
            Self::execute_historical_series_with_schema_and_options(
                &points,
                &request,
                command_index,
                &schema,
                grpc_endpoint,
                grpc_api_key,
                options,
            )?
        } else {
            Self::execute_historical_series_with_options(
                &points,
                &request,
                grpc_endpoint,
                grpc_api_key,
                options,
            )?
        };
        let summary = Self::summarize_historical_series_runs(&runs);

        Ok(HistoricalSeriesReport {
            request,
            points,
            runs,
            summary,
        })
    }

    /// Execute one view request across labeled historical series points.
    pub fn execute_historical_series(
        points: &[HistoricalSeriesPoint],
        request: &HistoricalViewRequest,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
    ) -> Result<Vec<HistoricalSeriesRun>> {
        Self::execute_historical_series_with_options(
            points,
            request,
            grpc_endpoint,
            grpc_api_key,
            &HistoricalSeriesExecutionOptions::default(),
        )
    }

    /// Execute one view request across labeled historical series points with execution options.
    pub fn execute_historical_series_with_options(
        points: &[HistoricalSeriesPoint],
        request: &HistoricalViewRequest,
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
        options: &HistoricalSeriesExecutionOptions,
    ) -> Result<Vec<HistoricalSeriesRun>> {
        let workers = options.max_concurrency.unwrap_or(1).max(1);
        if workers <= 1 || points.len() <= 1 {
            return points
                .iter()
                .map(|point| {
                    run_historical_series_point(point, request, grpc_endpoint, grpc_api_key)
                })
                .collect();
        }

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .context("failed to build historical-series worker pool")?;
        let runs = pool.install(|| {
            points
                .par_iter()
                .map(|point| {
                    run_historical_series_point(point, request, grpc_endpoint, grpc_api_key)
                })
                .collect::<Vec<_>>()
        });
        runs.into_iter().collect()
    }

    /// Execute historical-series points and decode one command return schema per point.
    pub fn execute_historical_series_with_schema(
        points: &[HistoricalSeriesPoint],
        request: &HistoricalViewRequest,
        command_index: usize,
        schema: &[ReturnDecodeField],
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
    ) -> Result<Vec<HistoricalSeriesRun>> {
        Self::execute_historical_series_with_schema_and_options(
            points,
            request,
            command_index,
            schema,
            grpc_endpoint,
            grpc_api_key,
            &HistoricalSeriesExecutionOptions::default(),
        )
    }

    /// Execute historical-series points and decode one command return schema per point.
    pub fn execute_historical_series_with_schema_and_options(
        points: &[HistoricalSeriesPoint],
        request: &HistoricalViewRequest,
        command_index: usize,
        schema: &[ReturnDecodeField],
        grpc_endpoint: Option<&str>,
        grpc_api_key: Option<&str>,
        options: &HistoricalSeriesExecutionOptions,
    ) -> Result<Vec<HistoricalSeriesRun>> {
        let mut runs = Self::execute_historical_series_with_options(
            points,
            request,
            grpc_endpoint,
            grpc_api_key,
            options,
        )?;
        for run in &mut runs {
            if run.output.success {
                run.decoded =
                    Self::decode_command_return_schema(&run.output.raw, command_index, schema)?;
            }
        }
        Ok(runs)
    }

    /// Summarize historical-series outcomes.
    pub fn summarize_historical_series_runs(
        runs: &[HistoricalSeriesRun],
    ) -> HistoricalSeriesSummary {
        let total = runs.len();
        let success = runs.iter().filter(|run| run.output.success).count();
        let failed = total.saturating_sub(success);
        let total_gas_used = runs
            .iter()
            .filter_map(|run| run.output.gas_used)
            .sum::<u64>();
        let average_gas_used = if total > 0 {
            total_gas_used as f64 / total as f64
        } else {
            0.0
        };
        let success_rate = if total > 0 {
            success as f64 / total as f64
        } else {
            0.0
        };

        HistoricalSeriesSummary {
            total,
            success,
            failed,
            success_rate,
            total_gas_used,
            average_gas_used,
        }
    }

    /// Convenience constructor for batch snapshots.
    pub fn snapshot_from_checkpoint_versions(
        checkpoint: u64,
        versions: HashMap<String, u64>,
    ) -> HistoricalVersionsSnapshot {
        HistoricalVersionsSnapshot {
            checkpoint,
            versions,
        }
    }

    /// Decode base64 return values for a command into raw byte vectors.
    ///
    /// Returns `Ok(None)` when execution failed or the command has no return values.
    pub fn decode_command_return_values(
        raw: &serde_json::Value,
        command_index: usize,
    ) -> Result<Option<Vec<Vec<u8>>>> {
        if !raw
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(None);
        }

        let Some(commands) = raw
            .get("return_values")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(None);
        };
        let Some(command_values) = commands
            .get(command_index)
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(None);
        };

        let mut decoded = Vec::with_capacity(command_values.len());
        for (idx, value) in command_values.iter().enumerate() {
            let encoded = value.as_str().ok_or_else(|| {
                anyhow!(
                    "command {} return value {} is not a base64 string",
                    command_index,
                    idx
                )
            })?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|e| {
                    anyhow!(
                        "invalid base64 for command {} return value {}: {}",
                        command_index,
                        idx,
                        e
                    )
                })?;
            decoded.push(bytes);
        }
        Ok(Some(decoded))
    }

    /// Decode canonical return type tags for a command (when available).
    ///
    /// Returns `Ok(None)` when execution failed, tags are missing, or command index is missing.
    pub fn decode_command_return_type_tags(
        raw: &serde_json::Value,
        command_index: usize,
    ) -> Result<Option<Vec<Option<String>>>> {
        if !raw
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(None);
        }

        let Some(commands) = raw
            .get("return_type_tags")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(None);
        };
        let Some(command_tags) = commands
            .get(command_index)
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(None);
        };

        let mut decoded = Vec::with_capacity(command_tags.len());
        for (idx, value) in command_tags.iter().enumerate() {
            if value.is_null() {
                decoded.push(None);
                continue;
            }
            let tag = value.as_str().ok_or_else(|| {
                anyhow!(
                    "command {} return type tag {} is not a string/null",
                    command_index,
                    idx
                )
            })?;
            decoded.push(Some(tag.to_string()));
        }
        Ok(Some(decoded))
    }

    /// Decode command return values into typed JSON using available return type tags.
    ///
    /// This is best-effort: unsupported or undecodable values are returned as raw bytes.
    pub fn decode_command_return_values_typed(
        raw: &serde_json::Value,
        command_index: usize,
    ) -> Result<Option<Vec<DecodedReturnValue>>> {
        let Some(values) = Self::decode_command_return_values(raw, command_index)? else {
            return Ok(None);
        };
        let tags = Self::decode_command_return_type_tags(raw, command_index)?.unwrap_or_default();
        let typed = values
            .iter()
            .enumerate()
            .map(|(idx, bytes)| {
                let type_tag = tags.get(idx).cloned().unwrap_or(None);
                let value = decode_bytes_with_optional_type_tag(bytes, type_tag.as_deref(), false)
                    .unwrap_or_else(|err| raw_decode_value(bytes, Some(err.to_string())));
                DecodedReturnValue {
                    index: idx,
                    type_tag,
                    value,
                    raw_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    raw_hex: format!("0x{}", hex::encode(bytes)),
                }
            })
            .collect();
        Ok(Some(typed))
    }

    /// Decode command return values into a named object according to a schema.
    ///
    /// Schema fields can override type decoding with `type_hint` and optionally apply
    /// scaling (`decoded / scale`) for numeric values.
    pub fn decode_command_return_schema(
        raw: &serde_json::Value,
        command_index: usize,
        schema: &[ReturnDecodeField],
    ) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
        let Some(values) = Self::decode_command_return_values(raw, command_index)? else {
            return Ok(None);
        };
        let tags = Self::decode_command_return_type_tags(raw, command_index)?.unwrap_or_default();

        let mut out = serde_json::Map::with_capacity(schema.len());
        for field in schema {
            let Some(bytes) = values.get(field.index) else {
                out.insert(field.name.clone(), serde_json::Value::Null);
                continue;
            };
            let inferred = tags.get(field.index).and_then(|v| v.as_deref());
            let hint = field.type_hint.as_deref().or(inferred);
            let mut decoded =
                decode_bytes_with_optional_type_tag(bytes, hint, true).map_err(|e| {
                    anyhow!(
                        "failed to decode schema field '{}' at index {}: {}",
                        field.name,
                        field.index,
                        e
                    )
                })?;
            if let Some(scale) = field.scale {
                decoded = apply_scale(decoded, scale).map_err(|e| {
                    anyhow!(
                        "failed to apply scale for schema field '{}' at index {}: {}",
                        field.name,
                        field.index,
                        e
                    )
                })?;
            }
            out.insert(field.name.clone(), decoded);
        }
        Ok(Some(out))
    }

    /// Parse one decoded schema field as f64.
    ///
    /// Accepts numeric JSON values and numeric strings; `null` maps to `0.0`.
    pub fn decoded_number_field(
        decoded: &serde_json::Map<String, serde_json::Value>,
        key: &str,
    ) -> Result<f64> {
        let value = decoded
            .get(key)
            .ok_or_else(|| anyhow!("decoded field '{}' missing", key))?;
        match value {
            serde_json::Value::Number(num) => num
                .as_f64()
                .ok_or_else(|| anyhow!("decoded field '{}' is not f64-representable", key)),
            serde_json::Value::String(s) => s
                .parse::<f64>()
                .map_err(|e| anyhow!("decoded field '{}' parse error: {}", key, e)),
            serde_json::Value::Null => Ok(0.0),
            other => Err(anyhow!(
                "decoded field '{}' has non-numeric type: {}",
                key,
                other
            )),
        }
    }

    /// Build a replay PTB helper for `SimulationEnvironment`.
    pub fn ptb_builder() -> ReplayPtbBuilder {
        ReplayPtbBuilder::new()
    }

    /// Build one Move call command helper.
    pub fn move_call_builder(
        package: AccountAddress,
        module: impl Into<String>,
        function: impl Into<String>,
    ) -> MoveCallBuilder {
        MoveCallBuilder::new(package, module, function)
    }

    /// Reference one value in a multi-return command output.
    pub fn nested_result(command_result: Argument, value_index: u16) -> Result<Argument> {
        ReplayPtbBuilder::nested_result(command_result, value_index)
    }

    /// Decode one command return value as little-endian `u64`.
    ///
    /// Returns `Ok(None)` when execution failed, command is missing, or value is missing.
    pub fn decode_command_return_u64(
        raw: &serde_json::Value,
        command_index: usize,
        value_index: usize,
    ) -> Result<Option<u64>> {
        let Some(decoded) = Self::decode_command_return_values(raw, command_index)? else {
            return Ok(None);
        };
        let Some(bytes) = decoded.get(value_index) else {
            return Ok(None);
        };
        if bytes.len() < 8 {
            return Err(anyhow!(
                "command {} return value {} has {} bytes; expected >= 8 for u64",
                command_index,
                value_index,
                bytes.len()
            ));
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes[..8]);
        Ok(Some(u64::from_le_bytes(buf)))
    }

    /// Ensure PTB execution succeeded, returning a contextual error when not.
    pub fn ensure_execution_success(step: &str, result: &ExecutionResult) -> Result<()> {
        if result.success {
            return Ok(());
        }
        let mut msg = format!("{step} failed");
        if let Some(err) = &result.error {
            msg.push_str(&format!("; error={err}"));
        }
        if let Some(raw) = &result.raw_error {
            msg.push_str(&format!("; raw={raw}"));
        }
        Err(anyhow!(msg))
    }

    /// Execute one PTB, require success, and return raw result + compact summary.
    pub fn execute_ptb_with_summary(
        env: &mut SimulationEnvironment,
        step: &str,
        ptb: ReplayPtbBuilder,
    ) -> Result<SuccessfulExecution> {
        let result = ptb.execute(env);
        Self::ensure_execution_success(step, &result)?;
        let summary = Self::summarize_execution_effects(&result);
        Ok(SuccessfulExecution { result, summary })
    }

    /// Summarize object/gas effects for one execution result.
    pub fn summarize_execution_effects(result: &ExecutionResult) -> ExecutionEffectsSummary {
        let Some(effects) = result.effects.as_ref() else {
            return ExecutionEffectsSummary::default();
        };
        ExecutionEffectsSummary {
            gas_used: effects.gas_used,
            created_objects: effects.created.len(),
            mutated_objects: effects.mutated.len(),
            deleted_objects: effects.deleted.len(),
            wrapped_objects: effects.wrapped.len(),
            unwrapped_objects: effects.unwrapped.len(),
            transferred_objects: effects.transferred.len(),
        }
    }

    /// Execute a no-arg Move call, require success, and return compact effects summary.
    pub fn execute_noarg_move_call_with_summary(
        env: &mut SimulationEnvironment,
        package: AccountAddress,
        module: &str,
        function: &str,
        step: &str,
    ) -> Result<ExecutionEffectsSummary> {
        let result = Self::execute_noarg_move_call(env, package, module, function)?;
        Self::ensure_execution_success(step, &result)?;
        Ok(Self::summarize_execution_effects(&result))
    }

    /// Find the parent object id for one dynamic-field entry by type-tag substring matching.
    ///
    /// Useful when a protocol writes wrapper objects into dynamic fields and only emits a logical
    /// object id in return values.
    pub fn find_dynamic_field_parent_by_type_substrings(
        result: &ExecutionResult,
        required_substrings: &[&str],
    ) -> Result<AccountAddress> {
        if required_substrings.is_empty() {
            return Err(anyhow!("required_substrings cannot be empty"));
        }
        let effects = result
            .effects
            .as_ref()
            .ok_or_else(|| anyhow!("missing effects"))?;
        for ((parent, _child), (type_tag, _bytes)) in &effects.dynamic_field_entries {
            let type_str = type_tag.to_canonical_string(true);
            if required_substrings
                .iter()
                .all(|needle| type_str.contains(needle))
            {
                return Ok(*parent);
            }
        }
        Err(anyhow!(
            "no dynamic-field entry matched required type substrings: {}",
            required_substrings.join(", ")
        ))
    }

    /// Find a created object id from PTB effects by matching its struct module/name.
    pub fn find_created_object_id_by_struct_tag(
        env: &SimulationEnvironment,
        result: &ExecutionResult,
        module: &str,
        struct_name: &str,
    ) -> Result<AccountAddress> {
        let effects = result
            .effects
            .as_ref()
            .ok_or_else(|| anyhow!("missing effects"))?;
        for object_id in &effects.created {
            if let Some(obj) = env.get_object(object_id) {
                if let TypeTag::Struct(s) = &obj.type_tag {
                    if s.module.as_ident_str().as_str() == module
                        && s.name.as_ident_str().as_str() == struct_name
                    {
                        return Ok(*object_id);
                    }
                }
            }
        }
        Err(anyhow!(
            "could not find created {}::{} object in PTB effects",
            module,
            struct_name
        ))
    }

    /// Recover a created object from effects bytes and load it back into the environment.
    ///
    /// This is useful when a protocol returns logical IDs while the concrete object bytes
    /// are only present in effect change sets.
    pub fn recover_created_object_into_env(
        env: &mut SimulationEnvironment,
        result: &ExecutionResult,
        module: &str,
        struct_name: &str,
        shared: bool,
        mutable: bool,
        version: u64,
    ) -> Result<AccountAddress> {
        let effects = result
            .effects
            .as_ref()
            .ok_or_else(|| anyhow!("missing effects"))?;
        for change in &effects.object_changes {
            let (id, object_type) = match change {
                ObjectChange::Created {
                    id,
                    object_type: Some(object_type),
                    ..
                } => (id, object_type),
                _ => continue,
            };
            let is_match = matches!(object_type, TypeTag::Struct(s)
                if s.module.as_ident_str().as_str() == module
                    && s.name.as_ident_str().as_str() == struct_name);
            if !is_match {
                continue;
            }
            let bytes = effects.created_object_bytes.get(id).ok_or_else(|| {
                anyhow!(
                    "created {}::{} object bytes missing from effects map",
                    module,
                    struct_name
                )
            })?;
            let type_str = object_type.to_canonical_string(true);
            env.load_object_from_data(
                &id.to_hex_literal(),
                bytes.clone(),
                Some(&type_str),
                shared,
                mutable,
                version,
            )?;
            return Ok(*id);
        }
        Err(anyhow!(
            "no created {}::{} entry found in object_changes",
            module,
            struct_name
        ))
    }

    /// Resolve a created object id from PTB return values/effects and ensure it exists in env.
    ///
    /// Resolution order:
    /// 1) decode return value as object id, if already loaded in env
    /// 2) find created object in effects by struct tag
    /// 3) recover created object bytes from effects into env by struct tag
    pub fn resolve_created_object_id_from_return_or_effects(
        env: &mut SimulationEnvironment,
        result: &ExecutionResult,
        command_index: usize,
        value_index: usize,
        module: &str,
        struct_name: &str,
        shared: bool,
        mutable: bool,
        version: u64,
    ) -> Result<AccountAddress> {
        let returned_id =
            Self::decode_execution_command_return_object_id(result, command_index, value_index)
                .ok();

        if let Some(id) = returned_id {
            if env.get_object(&id).is_some() {
                return Ok(id);
            }
        }

        if let Ok(id) = Self::find_created_object_id_by_struct_tag(env, result, module, struct_name)
        {
            return Ok(id);
        }

        if let Ok(id) = Self::recover_created_object_into_env(
            env,
            result,
            module,
            struct_name,
            shared,
            mutable,
            version,
        ) {
            return Ok(id);
        }

        if let Some(id) = returned_id {
            return Err(anyhow!(
                "return value decoded to {}, but {}::{} object was not found/recovered from effects",
                id.to_hex_literal(),
                module,
                struct_name
            ));
        }

        Err(anyhow!(
            "failed to resolve {}::{} object from command {} return {} or effects",
            module,
            struct_name,
            command_index,
            value_index
        ))
    }

    /// Decode return values from a command in an `ExecutionResult`.
    pub fn decode_execution_command_return_values(
        result: &ExecutionResult,
        command_index: usize,
    ) -> Result<Vec<Vec<u8>>> {
        let effects = result
            .effects
            .as_ref()
            .ok_or_else(|| anyhow!("missing effects"))?;
        let returns = effects
            .return_values
            .get(command_index)
            .ok_or_else(|| anyhow!("missing return values for command {}", command_index))?;
        Ok(returns.clone())
    }

    /// Decode return values from an `ExecutionResult` command into typed JSON.
    ///
    /// This is best-effort: unsupported or undecodable values are returned as raw bytes.
    pub fn decode_execution_command_return_values_typed(
        result: &ExecutionResult,
        command_index: usize,
    ) -> Result<Vec<DecodedReturnValue>> {
        let effects = result
            .effects
            .as_ref()
            .ok_or_else(|| anyhow!("missing effects"))?;
        let values = effects
            .return_values
            .get(command_index)
            .ok_or_else(|| anyhow!("missing return values for command {}", command_index))?;
        let tags = effects.return_type_tags.get(command_index).cloned();

        let typed = values
            .iter()
            .enumerate()
            .map(|(idx, bytes)| {
                let tag = tags
                    .as_ref()
                    .and_then(|cmd| cmd.get(idx))
                    .and_then(|tt| tt.as_ref())
                    .map(|tt| tt.to_canonical_string(true));
                let value = decode_bytes_with_optional_type_tag(bytes, tag.as_deref(), false)
                    .unwrap_or_else(|err| raw_decode_value(bytes, Some(err.to_string())));
                DecodedReturnValue {
                    index: idx,
                    type_tag: tag,
                    value,
                    raw_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    raw_hex: format!("0x{}", hex::encode(bytes)),
                }
            })
            .collect();
        Ok(typed)
    }

    /// Decode `ExecutionResult` command return values into a named object according to a schema.
    pub fn decode_execution_command_return_schema(
        result: &ExecutionResult,
        command_index: usize,
        schema: &[ReturnDecodeField],
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let effects = result
            .effects
            .as_ref()
            .ok_or_else(|| anyhow!("missing effects"))?;
        let values = effects
            .return_values
            .get(command_index)
            .ok_or_else(|| anyhow!("missing return values for command {}", command_index))?;
        let tags = effects.return_type_tags.get(command_index).cloned();

        let mut out = serde_json::Map::with_capacity(schema.len());
        for field in schema {
            let Some(bytes) = values.get(field.index) else {
                out.insert(field.name.clone(), serde_json::Value::Null);
                continue;
            };
            let inferred = tags
                .as_ref()
                .and_then(|cmd| cmd.get(field.index))
                .and_then(|tt| tt.as_ref())
                .map(|tt| tt.to_canonical_string(true));
            let hint = field.type_hint.as_deref().or(inferred.as_deref());
            let mut decoded =
                decode_bytes_with_optional_type_tag(bytes, hint, true).map_err(|e| {
                    anyhow!(
                        "failed to decode schema field '{}' at index {}: {}",
                        field.name,
                        field.index,
                        e
                    )
                })?;
            if let Some(scale) = field.scale {
                decoded = apply_scale(decoded, scale).map_err(|e| {
                    anyhow!(
                        "failed to apply scale for schema field '{}' at index {}: {}",
                        field.name,
                        field.index,
                        e
                    )
                })?;
            }
            out.insert(field.name.clone(), decoded);
        }
        Ok(out)
    }

    /// Decode one return value from a command payload as little-endian `u64`.
    pub fn decode_execution_return_u64_at(returns: &[Vec<u8>], value_index: usize) -> Result<u64> {
        let bytes = returns
            .get(value_index)
            .ok_or_else(|| anyhow!("missing return value {}", value_index))?;
        decode_u64_le(bytes)
            .map_err(|e| anyhow!("failed to decode return {} as u64: {}", value_index, e))
    }

    /// Decode one command return value as little-endian `u64` from an `ExecutionResult`.
    pub fn decode_execution_command_return_u64(
        result: &ExecutionResult,
        command_index: usize,
        value_index: usize,
    ) -> Result<u64> {
        let returns = Self::decode_execution_command_return_values(result, command_index)?;
        Self::decode_execution_return_u64_at(&returns, value_index).map_err(|e| {
            anyhow!(
                "failed decoding command {} return {} as u64: {}",
                command_index,
                value_index,
                e
            )
        })
    }

    /// Decode one command return value as an object id from an `ExecutionResult`.
    pub fn decode_execution_command_return_object_id(
        result: &ExecutionResult,
        command_index: usize,
        value_index: usize,
    ) -> Result<AccountAddress> {
        let returns = Self::decode_execution_command_return_values(result, command_index)?;
        let bytes = returns.get(value_index).ok_or_else(|| {
            anyhow!(
                "missing return value {} for command {}",
                value_index,
                command_index
            )
        })?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "expected 32-byte object id at command {} return {}, got {} bytes",
                command_index,
                value_index,
                bytes.len()
            ));
        }
        let mut raw = [0u8; 32];
        raw.copy_from_slice(bytes);
        Ok(AccountAddress::new(raw))
    }

    /// Decode a `(u64, u64, u64)` return payload from a command in an `ExecutionResult`.
    ///
    /// Supports either:
    /// 1) three separate return values, or
    /// 2) one tuple payload packed into 24+ bytes.
    pub fn decode_execution_command_return_u64_triplet(
        result: &ExecutionResult,
        command_index: usize,
    ) -> Result<(u64, u64, u64)> {
        let returns = Self::decode_execution_command_return_values(result, command_index)?;
        if returns.len() >= 3 {
            return Ok((
                Self::decode_execution_return_u64_at(&returns, 0)?,
                Self::decode_execution_return_u64_at(&returns, 1)?,
                Self::decode_execution_return_u64_at(&returns, 2)?,
            ));
        }

        let bytes = returns
            .first()
            .ok_or_else(|| anyhow!("missing return payload for command {}", command_index))?;
        if bytes.len() < 24 {
            return Err(anyhow!(
                "expected tuple payload >=24 bytes for command {}, got {}",
                command_index,
                bytes.len()
            ));
        }
        Ok((
            decode_u64_le(&bytes[0..8])?,
            decode_u64_le(&bytes[8..16])?,
            decode_u64_le(&bytes[16..24])?,
        ))
    }

    /// Build a CLI argument vector for a `workflow` replay step.
    pub fn build_replay_command(
        defaults: &WorkflowDefaults,
        replay: &WorkflowReplayStep,
    ) -> Vec<String> {
        let mut args = vec!["replay".to_string()];
        let digest = replay
            .digest
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if let Some(digest) = digest {
            args.push(digest);
        } else if replay.latest.is_some() || replay.checkpoint.is_some() {
            args.push("*".to_string());
        }

        if let Some(path) = replay.state_json.as_ref() {
            args.push("--state-json".to_string());
            args.push(path.display().to_string());
        }
        if let Some(checkpoint) = replay.checkpoint.as_deref() {
            args.push("--checkpoint".to_string());
            args.push(checkpoint.to_string());
        }
        if let Some(latest) = replay.latest {
            args.push("--latest".to_string());
            args.push(latest.to_string());
        }
        if let Some(source) = replay.source.or(defaults.source) {
            args.push("--source".to_string());
            args.push(source.as_cli_value().to_string());
        }
        if let Some(profile) = replay.profile.or(defaults.profile) {
            args.push("--profile".to_string());
            args.push(profile.as_cli_value().to_string());
        }
        if let Some(fetch_strategy) = replay.fetch_strategy.or(defaults.fetch_strategy) {
            args.push("--fetch-strategy".to_string());
            args.push(fetch_strategy.as_cli_value().to_string());
        }
        if let Some(allow_fallback) = replay.allow_fallback.or(defaults.allow_fallback) {
            args.push("--allow-fallback".to_string());
            args.push(allow_fallback.to_string());
        }
        if let Some(auto_system_objects) =
            replay.auto_system_objects.or(defaults.auto_system_objects)
        {
            args.push("--auto-system-objects".to_string());
            args.push(auto_system_objects.to_string());
        }
        if let Some(prefetch_depth) = replay.prefetch_depth.or(defaults.prefetch_depth) {
            args.push("--prefetch-depth".to_string());
            args.push(prefetch_depth.to_string());
        }
        if let Some(prefetch_limit) = replay.prefetch_limit.or(defaults.prefetch_limit) {
            args.push("--prefetch-limit".to_string());
            args.push(prefetch_limit.to_string());
        }

        if replay.no_prefetch.or(defaults.no_prefetch).unwrap_or(false) {
            args.push("--no-prefetch".to_string());
        }
        if replay.compare.or(defaults.compare).unwrap_or(false) {
            args.push("--compare".to_string());
        }
        if replay.strict.or(defaults.strict).unwrap_or(false) {
            args.push("--strict".to_string());
        }
        if replay.vm_only.or(defaults.vm_only).unwrap_or(false) {
            args.push("--vm-only".to_string());
        }
        if replay
            .synthesize_missing
            .or(defaults.synthesize_missing)
            .unwrap_or(false)
        {
            args.push("--synthesize-missing".to_string());
        }
        if replay
            .self_heal_dynamic_fields
            .or(defaults.self_heal_dynamic_fields)
            .unwrap_or(false)
        {
            args.push("--self-heal-dynamic-fields".to_string());
        }

        args
    }

    /// Build a CLI argument vector for a `workflow` analyze replay step.
    pub fn build_analyze_replay_command(
        defaults: &WorkflowDefaults,
        analyze: &WorkflowAnalyzeReplayStep,
    ) -> Vec<String> {
        let mut args = vec![
            "analyze".to_string(),
            "replay".to_string(),
            analyze.digest.clone(),
        ];

        if let Some(checkpoint) = analyze.checkpoint {
            args.push("--checkpoint".to_string());
            args.push(checkpoint.to_string());
        }
        if let Some(source) = analyze.source.or(defaults.source) {
            args.push("--source".to_string());
            args.push(source.as_cli_value().to_string());
        }
        if let Some(allow_fallback) = analyze.allow_fallback.or(defaults.allow_fallback) {
            args.push("--allow-fallback".to_string());
            args.push(allow_fallback.to_string());
        }
        if let Some(auto_system_objects) =
            analyze.auto_system_objects.or(defaults.auto_system_objects)
        {
            args.push("--auto-system-objects".to_string());
            args.push(auto_system_objects.to_string());
        }
        if let Some(prefetch_depth) = analyze.prefetch_depth.or(defaults.prefetch_depth) {
            args.push("--prefetch-depth".to_string());
            args.push(prefetch_depth.to_string());
        }
        if let Some(prefetch_limit) = analyze.prefetch_limit.or(defaults.prefetch_limit) {
            args.push("--prefetch-limit".to_string());
            args.push(prefetch_limit.to_string());
        }

        if analyze
            .no_prefetch
            .or(defaults.no_prefetch)
            .unwrap_or(false)
        {
            args.push("--no-prefetch".to_string());
        }
        if analyze.mm2.or(defaults.mm2).unwrap_or(false) {
            args.push("--mm2".to_string());
        }

        args
    }
}

fn raw_decode_value(bytes: &[u8], decode_error: Option<String>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "raw_base64".to_string(),
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
    );
    map.insert(
        "raw_hex".to_string(),
        serde_json::Value::String(format!("0x{}", hex::encode(bytes))),
    );
    if let Some(err) = decode_error {
        map.insert("decode_error".to_string(), serde_json::Value::String(err));
    }
    serde_json::Value::Object(map)
}

fn decode_bytes_with_optional_type_tag(
    bytes: &[u8],
    hint: Option<&str>,
    strict: bool,
) -> Result<serde_json::Value> {
    let Some(hint) = hint else {
        return Ok(raw_decode_value(bytes, None));
    };
    let trimmed = hint.trim();
    if trimmed.is_empty() {
        return Ok(raw_decode_value(bytes, None));
    }

    match decode_bytes_with_hint(bytes, trimmed) {
        Ok(value) => Ok(value),
        Err(err) if strict => Err(err),
        Err(err) => Ok(raw_decode_value(bytes, Some(err.to_string()))),
    }
}

fn decode_bytes_with_hint(bytes: &[u8], hint: &str) -> Result<serde_json::Value> {
    let normalized = hint.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "hex" => {
            return Ok(serde_json::Value::String(format!(
                "0x{}",
                hex::encode(bytes)
            )));
        }
        "base64" | "bytes" | "raw" => {
            return Ok(serde_json::Value::String(
                base64::engine::general_purpose::STANDARD.encode(bytes),
            ));
        }
        "utf8" | "string" => {
            let value: String = bcs::from_bytes(bytes).context("bcs decode utf8 string")?;
            return Ok(serde_json::Value::String(value));
        }
        _ => {}
    }

    let type_tag = crate::types::parse_type_tag(hint)
        .with_context(|| format!("invalid type hint/type tag '{}'", hint))?;
    decode_bytes_with_type_tag(bytes, &type_tag)
}

fn decode_bytes_with_type_tag(bytes: &[u8], type_tag: &TypeTag) -> Result<serde_json::Value> {
    match type_tag {
        TypeTag::Bool => {
            let value: bool = bcs::from_bytes(bytes).context("decode bool")?;
            Ok(serde_json::Value::Bool(value))
        }
        TypeTag::U8 => {
            let value: u8 = bcs::from_bytes(bytes).context("decode u8")?;
            Ok(serde_json::json!(value))
        }
        TypeTag::U16 => {
            let value: u16 = bcs::from_bytes(bytes).context("decode u16")?;
            Ok(serde_json::json!(value))
        }
        TypeTag::U32 => {
            let value: u32 = bcs::from_bytes(bytes).context("decode u32")?;
            Ok(serde_json::json!(value))
        }
        TypeTag::U64 => {
            let value: u64 = bcs::from_bytes(bytes).context("decode u64")?;
            Ok(serde_json::json!(value))
        }
        TypeTag::U128 => {
            let value: u128 = bcs::from_bytes(bytes).context("decode u128")?;
            Ok(serde_json::Value::String(value.to_string()))
        }
        TypeTag::U256 => {
            let value: move_core_types::u256::U256 =
                bcs::from_bytes(bytes).context("decode u256")?;
            Ok(serde_json::Value::String(value.to_string()))
        }
        TypeTag::Address | TypeTag::Signer => {
            let value: AccountAddress = bcs::from_bytes(bytes).context("decode address")?;
            Ok(serde_json::Value::String(value.to_hex_literal()))
        }
        TypeTag::Vector(inner) => decode_vector_with_type_tag(bytes, inner.as_ref()),
        TypeTag::Struct(_) => Ok(raw_decode_value(
            bytes,
            Some("struct decode requires layout-aware decoder".to_string()),
        )),
    }
}

fn decode_vector_with_type_tag(bytes: &[u8], inner: &TypeTag) -> Result<serde_json::Value> {
    match inner {
        TypeTag::Bool => {
            let value: Vec<bool> = bcs::from_bytes(bytes).context("decode vector<bool>")?;
            serde_json::to_value(value).context("serialize vector<bool>")
        }
        TypeTag::U8 => {
            let value: Vec<u8> = bcs::from_bytes(bytes).context("decode vector<u8>")?;
            serde_json::to_value(value).context("serialize vector<u8>")
        }
        TypeTag::U16 => {
            let value: Vec<u16> = bcs::from_bytes(bytes).context("decode vector<u16>")?;
            serde_json::to_value(value).context("serialize vector<u16>")
        }
        TypeTag::U32 => {
            let value: Vec<u32> = bcs::from_bytes(bytes).context("decode vector<u32>")?;
            serde_json::to_value(value).context("serialize vector<u32>")
        }
        TypeTag::U64 => {
            let value: Vec<u64> = bcs::from_bytes(bytes).context("decode vector<u64>")?;
            serde_json::to_value(value).context("serialize vector<u64>")
        }
        TypeTag::U128 => {
            let value: Vec<u128> = bcs::from_bytes(bytes).context("decode vector<u128>")?;
            Ok(serde_json::Value::Array(
                value
                    .into_iter()
                    .map(|v| serde_json::Value::String(v.to_string()))
                    .collect(),
            ))
        }
        TypeTag::U256 => {
            let value: Vec<move_core_types::u256::U256> =
                bcs::from_bytes(bytes).context("decode vector<u256>")?;
            Ok(serde_json::Value::Array(
                value
                    .into_iter()
                    .map(|v| serde_json::Value::String(v.to_string()))
                    .collect(),
            ))
        }
        TypeTag::Address | TypeTag::Signer => {
            let value: Vec<AccountAddress> =
                bcs::from_bytes(bytes).context("decode vector<address>")?;
            Ok(serde_json::Value::Array(
                value
                    .into_iter()
                    .map(|v| serde_json::Value::String(v.to_hex_literal()))
                    .collect(),
            ))
        }
        TypeTag::Vector(_) | TypeTag::Struct(_) => Ok(raw_decode_value(
            bytes,
            Some("nested vector/struct decode requires layout-aware decoder".to_string()),
        )),
    }
}

fn apply_scale(value: serde_json::Value, scale: f64) -> Result<serde_json::Value> {
    if scale == 0.0 {
        return Err(anyhow!("scale must be non-zero"));
    }
    let raw = match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| anyhow!("numeric value is not representable as f64"))?,
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|e| anyhow!("string numeric parse error: {}", e))?,
        other => {
            return Err(anyhow!(
                "scale can only be applied to numeric/string values, got {}",
                other
            ))
        }
    };
    let scaled = raw / scale;
    let number = serde_json::Number::from_f64(scaled)
        .ok_or_else(|| anyhow!("scaled value is not a finite JSON number"))?;
    Ok(serde_json::Value::Number(number))
}

fn decode_u64_le(bytes: &[u8]) -> Result<u64> {
    if bytes.len() < 8 {
        return Err(anyhow!(
            "expected at least 8 bytes for u64, got {}",
            bytes.len()
        ));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    Ok(u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptb::{InputValue, TransactionEffects};
    use crate::simulation::{ExecutionResult, SimulationEnvironment};
    use crate::workflow::{
        WorkflowAnalyzeReplayStep, WorkflowDefaults, WorkflowFetchStrategy, WorkflowReplayProfile,
        WorkflowReplayStep, WorkflowSource,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn has_flag(args: &[String], flag: &str) -> bool {
        args.iter().any(|arg| arg == flag)
    }

    #[test]
    fn replay_command_honors_defaults_and_flags() {
        let defaults = WorkflowDefaults {
            source: Some(WorkflowSource::Hybrid),
            profile: Some(WorkflowReplayProfile::Fast),
            fetch_strategy: Some(WorkflowFetchStrategy::Eager),
            vm_only: Some(true),
            synthesize_missing: Some(true),
            self_heal_dynamic_fields: Some(true),
            ..WorkflowDefaults::default()
        };
        let replay: WorkflowReplayStep = serde_json::from_value(json!({
            "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "checkpoint": "239615926"
        }))
        .expect("valid replay step");

        let args = ReplayOrchestrator::build_replay_command(&defaults, &replay);
        assert!(has_flag(&args, "--profile"));
        assert!(has_flag(&args, "--fetch-strategy"));
        assert!(has_flag(&args, "--vm-only"));
        assert!(has_flag(&args, "--synthesize-missing"));
        assert!(has_flag(&args, "--self-heal-dynamic-fields"));
    }

    #[test]
    fn analyze_command_honors_mm2_override() {
        let defaults = WorkflowDefaults {
            mm2: Some(true),
            ..WorkflowDefaults::default()
        };
        let analyze_default: WorkflowAnalyzeReplayStep = serde_json::from_value(
            json!({ "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2" }),
        )
        .expect("valid analyze step");
        let args_default =
            ReplayOrchestrator::build_analyze_replay_command(&defaults, &analyze_default);
        assert!(has_flag(&args_default, "--mm2"));

        let analyze_override: WorkflowAnalyzeReplayStep = serde_json::from_value(json!({
            "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "mm2": false
        }))
        .expect("valid analyze step override");
        let args_override =
            ReplayOrchestrator::build_analyze_replay_command(&defaults, &analyze_override);
        assert!(!has_flag(&args_override, "--mm2"));
    }

    #[test]
    fn decodes_u64_return_value_from_base64() {
        let mut bytes = vec![0u8; 8];
        bytes.copy_from_slice(&1234u64.to_le_bytes());
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let raw = serde_json::json!({
            "success": true,
            "return_values": [[encoded]]
        });

        let value = ReplayOrchestrator::decode_command_return_u64(&raw, 0, 0)
            .expect("decode should succeed")
            .expect("value should exist");
        assert_eq!(value, 1234);
    }

    #[test]
    fn returns_none_for_failed_execution() {
        let raw = serde_json::json!({
            "success": false,
            "return_values": []
        });
        let value =
            ReplayOrchestrator::decode_command_return_u64(&raw, 0, 0).expect("decode should work");
        assert!(value.is_none());
    }

    #[test]
    fn decodes_command_return_values_typed_from_type_tags() {
        let encoded_u64 =
            base64::engine::general_purpose::STANDARD.encode(1234u64.to_le_bytes().to_vec());
        let encoded_bool = base64::engine::general_purpose::STANDARD.encode(vec![1u8]);
        let raw = serde_json::json!({
            "success": true,
            "return_values": [[encoded_u64, encoded_bool]],
            "return_type_tags": [["u64", "bool"]],
        });

        let decoded = ReplayOrchestrator::decode_command_return_values_typed(&raw, 0)
            .expect("typed decode should succeed")
            .expect("typed values should exist");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].value, serde_json::json!(1234u64));
        assert_eq!(decoded[1].value, serde_json::json!(true));
    }

    #[test]
    fn decodes_command_return_schema_with_scaling() {
        let encoded_u64 =
            base64::engine::general_purpose::STANDARD.encode(2_500_000u64.to_le_bytes().to_vec());
        let raw = serde_json::json!({
            "success": true,
            "return_values": [[encoded_u64]],
            "return_type_tags": [["u64"]],
        });
        let schema = vec![ReturnDecodeField {
            index: 0,
            name: "price".to_string(),
            type_hint: None,
            scale: Some(1_000_000.0),
        }];

        let decoded = ReplayOrchestrator::decode_command_return_schema(&raw, 0, &schema)
            .expect("schema decode should succeed")
            .expect("schema values should exist");
        assert_eq!(decoded.get("price"), Some(&serde_json::json!(2.5)));
        let price =
            ReplayOrchestrator::decoded_number_field(&decoded, "price").expect("numeric field");
        assert_eq!(price, 2.5);
    }

    #[test]
    fn return_decode_field_scaled_u64_builder_sets_hint_and_scale() {
        let field = ReturnDecodeField::scaled_u64(7, "value", 1_000_000.0);
        assert_eq!(field.index, 7);
        assert_eq!(field.name, "value");
        assert_eq!(field.type_hint.as_deref(), Some("u64"));
        assert_eq!(field.scale, Some(1_000_000.0));
    }

    #[test]
    fn schema_decode_supports_named_non_type_hints() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("hello".as_bytes());
        let raw = serde_json::json!({
            "success": true,
            "return_values": [[encoded]]
        });
        let schema = vec![ReturnDecodeField {
            index: 0,
            name: "blob".to_string(),
            type_hint: Some("base64".to_string()),
            scale: None,
        }];
        let decoded = ReplayOrchestrator::decode_command_return_schema(&raw, 0, &schema)
            .expect("schema decode")
            .expect("values");
        assert_eq!(decoded.get("blob"), Some(&serde_json::json!("aGVsbG8=")));
    }

    #[test]
    fn pure_input_encodes_bcs_value() {
        let input = ReplayOrchestrator::pure_input(7u64).expect("pure input");
        match input {
            InputValue::Pure(bytes) => assert_eq!(bytes, bcs::to_bytes(&7u64).expect("bcs")),
            _ => panic!("expected pure input"),
        }
    }

    fn build_success_result(return_values: Vec<Vec<Vec<u8>>>) -> ExecutionResult {
        let mut effects = TransactionEffects::success();
        effects.return_values = return_values;
        ExecutionResult {
            success: true,
            effects: Some(effects),
            error: None,
            raw_error: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: 0,
            error_context: None,
            state_at_failure: None,
        }
    }

    #[test]
    fn decodes_execution_result_object_id() {
        let id = AccountAddress::from_hex_literal("0x6").expect("id");
        let result = build_success_result(vec![vec![id.to_vec()]]);
        let decoded = ReplayOrchestrator::decode_execution_command_return_object_id(&result, 0, 0)
            .expect("decode object id");
        assert_eq!(decoded, id);
    }

    #[test]
    fn decodes_execution_result_u64_triplet_from_three_values() {
        let result = build_success_result(vec![vec![
            1u64.to_le_bytes().to_vec(),
            2u64.to_le_bytes().to_vec(),
            3u64.to_le_bytes().to_vec(),
        ]]);
        let decoded = ReplayOrchestrator::decode_execution_command_return_u64_triplet(&result, 0)
            .expect("decode u64 triplet");
        assert_eq!(decoded, (1, 2, 3));
    }

    #[test]
    fn decodes_execution_schema_with_type_hints() {
        let mut result = build_success_result(vec![vec![42u64.to_le_bytes().to_vec()]]);
        if let Some(effects) = result.effects.as_mut() {
            effects.return_type_tags = vec![vec![Some(
                crate::types::parse_type_tag("u64").expect("valid type"),
            )]];
        }
        let schema = vec![ReturnDecodeField {
            index: 0,
            name: "answer".to_string(),
            type_hint: None,
            scale: None,
        }];
        let decoded =
            ReplayOrchestrator::decode_execution_command_return_schema(&result, 0, &schema)
                .expect("execution schema decode");
        assert_eq!(decoded.get("answer"), Some(&serde_json::json!(42u64)));
    }

    #[test]
    fn decodes_execution_return_u64_at() {
        let returns = vec![10u64.to_le_bytes().to_vec(), 20u64.to_le_bytes().to_vec()];
        let value =
            ReplayOrchestrator::decode_execution_return_u64_at(&returns, 1).expect("decode u64");
        assert_eq!(value, 20);
    }

    #[test]
    fn batch_empty_returns_empty() {
        let request = HistoricalViewRequest {
            package_id: "0x2".to_string(),
            module: "sui".to_string(),
            function: "dummy".to_string(),
            type_args: Vec::new(),
            required_objects: vec!["0x6".to_string()],
            package_roots: Vec::new(),
            type_refs: Vec::new(),
            fetch_child_objects: false,
        };
        let outputs = ReplayOrchestrator::execute_historical_view_batch(&[], &request, None, None)
            .expect("empty batch should not fail");
        assert!(outputs.is_empty());
    }

    #[test]
    fn snapshot_constructor_keeps_values() {
        let mut versions = HashMap::new();
        versions.insert("0x6".to_string(), 42);
        let snapshot = ReplayOrchestrator::snapshot_from_checkpoint_versions(123, versions.clone());
        assert_eq!(snapshot.checkpoint, 123);
        assert_eq!(snapshot.versions.get("0x6"), Some(&42));
    }

    #[test]
    fn series_point_constructor_keeps_values() {
        let mut versions = HashMap::new();
        versions.insert("0x6".to_string(), 42);
        let point =
            ReplayOrchestrator::series_point_from_checkpoint_versions(123, versions.clone())
                .with_label("day-1")
                .with_metadata(serde_json::json!({ "day": 1 }));
        assert_eq!(point.checkpoint, 123);
        assert_eq!(point.versions.get("0x6"), Some(&42));
        assert_eq!(point.label.as_deref(), Some("day-1"));
        assert_eq!(point.metadata, Some(serde_json::json!({ "day": 1 })));
    }

    #[test]
    fn parse_historical_series_points_supports_daily_snapshots_payload() {
        let payload = serde_json::json!({
            "daily_snapshots": [{
                "day": 1,
                "checkpoint": 123,
                "datetime_utc": "2026-01-01T00:00:00Z",
                "description": "snapshot",
                "objects": {
                    "0x6": { "version": 42 },
                    "0x7": 9
                }
            }]
        });

        let points = ReplayOrchestrator::parse_historical_series_points_value(&payload)
            .expect("daily snapshot payload should parse");
        assert_eq!(points.len(), 1);
        let point = &points[0];
        assert_eq!(point.checkpoint, 123);
        assert_eq!(point.versions.get("0x6"), Some(&42));
        assert_eq!(point.versions.get("0x7"), Some(&9));
        assert_eq!(point.label.as_deref(), Some("day-1"));
        assert_eq!(
            point.metadata,
            Some(serde_json::json!({
                "day": 1,
                "datetime_utc": "2026-01-01T00:00:00Z",
                "description": "snapshot",
            }))
        );
    }

    #[test]
    fn load_historical_series_points_supports_points_wrapper() {
        let tmp = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(
            tmp.path(),
            serde_json::json!({
                "points": [{
                    "checkpoint": 55,
                    "versions": {"0x6": 77},
                    "label": "sample"
                }]
            })
            .to_string(),
        )
        .expect("write fixture");

        let points = ReplayOrchestrator::load_historical_series_points(tmp.path())
            .expect("points wrapper should parse");
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].checkpoint, 55);
        assert_eq!(points[0].versions.get("0x6"), Some(&77));
        assert_eq!(points[0].label.as_deref(), Some("sample"));
    }

    #[test]
    fn historical_series_run_metadata_accessors_work() {
        let run = HistoricalSeriesRun {
            checkpoint: 1,
            label: Some("day-1".to_string()),
            metadata: Some(serde_json::json!({
                "day": 1,
                "datetime_utc": "2026-01-01T00:00:00Z",
            })),
            output: HistoricalViewOutput {
                versions_file: "fixture".to_string(),
                checkpoint: 1,
                grpc_endpoint: "endpoint".to_string(),
                success: true,
                gas_used: Some(10),
                error: None,
                hint: None,
                raw: serde_json::json!({"success": true}),
            },
            decoded: None,
        };
        assert_eq!(run.metadata_u64("day"), Some(1));
        assert_eq!(
            run.metadata_str("datetime_utc"),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(run.metadata_u64("missing"), None);
    }

    #[test]
    fn historical_series_empty_returns_empty() {
        let request = HistoricalViewRequest {
            package_id: "0x2".to_string(),
            module: "sui".to_string(),
            function: "dummy".to_string(),
            type_args: Vec::new(),
            required_objects: vec!["0x6".to_string()],
            package_roots: Vec::new(),
            type_refs: Vec::new(),
            fetch_child_objects: false,
        };
        let runs = ReplayOrchestrator::execute_historical_series(&[], &request, None, None)
            .expect("empty series should not fail");
        assert!(runs.is_empty());
    }

    #[test]
    fn historical_series_with_schema_empty_returns_empty() {
        let request = HistoricalViewRequest {
            package_id: "0x2".to_string(),
            module: "sui".to_string(),
            function: "dummy".to_string(),
            type_args: Vec::new(),
            required_objects: vec!["0x6".to_string()],
            package_roots: Vec::new(),
            type_refs: Vec::new(),
            fetch_child_objects: false,
        };
        let runs = ReplayOrchestrator::execute_historical_series_with_schema(
            &[],
            &request,
            0,
            &[],
            None,
            None,
        )
        .expect("empty series should not fail");
        assert!(runs.is_empty());
    }

    #[test]
    fn historical_series_with_options_empty_returns_empty() {
        let request = HistoricalViewRequest {
            package_id: "0x2".to_string(),
            module: "sui".to_string(),
            function: "dummy".to_string(),
            type_args: Vec::new(),
            required_objects: vec!["0x6".to_string()],
            package_roots: Vec::new(),
            type_refs: Vec::new(),
            fetch_child_objects: false,
        };
        let options = HistoricalSeriesExecutionOptions {
            max_concurrency: Some(4),
        };
        let runs = ReplayOrchestrator::execute_historical_series_with_options(
            &[],
            &request,
            None,
            None,
            &options,
        )
        .expect("empty series should not fail");
        assert!(runs.is_empty());
    }

    #[test]
    fn historical_series_from_files_supports_empty_points() {
        let request_file = tempfile::NamedTempFile::new().expect("request temp file");
        let series_file = tempfile::NamedTempFile::new().expect("series temp file");
        std::fs::write(
            request_file.path(),
            serde_json::json!({
                "package_id": "0x2",
                "module": "sui",
                "function": "dummy",
                "required_objects": ["0x6"]
            })
            .to_string(),
        )
        .expect("write request fixture");
        std::fs::write(
            series_file.path(),
            serde_json::json!({ "points": [] }).to_string(),
        )
        .expect("write series fixture");

        let report = ReplayOrchestrator::execute_historical_series_from_files(
            request_file.path(),
            series_file.path(),
            None,
            0,
            None,
            None,
        )
        .expect("empty points should still parse");
        assert!(report.points.is_empty());
        assert!(report.runs.is_empty());
        assert_eq!(report.summary.total, 0);
        assert_eq!(report.summary.success, 0);
        assert_eq!(report.summary.failed, 0);
    }

    #[test]
    fn summarize_historical_series_runs_reports_counts() {
        let run_ok = HistoricalSeriesRun {
            checkpoint: 1,
            label: Some("ok".to_string()),
            metadata: None,
            output: HistoricalViewOutput {
                versions_file: "a".to_string(),
                checkpoint: 1,
                grpc_endpoint: "ep".to_string(),
                success: true,
                gas_used: Some(100),
                error: None,
                hint: None,
                raw: serde_json::json!({"success": true}),
            },
            decoded: None,
        };
        let run_fail = HistoricalSeriesRun {
            checkpoint: 2,
            label: Some("fail".to_string()),
            metadata: None,
            output: HistoricalViewOutput {
                versions_file: "b".to_string(),
                checkpoint: 2,
                grpc_endpoint: "ep".to_string(),
                success: false,
                gas_used: None,
                error: Some("boom".to_string()),
                hint: None,
                raw: serde_json::json!({"success": false}),
            },
            decoded: None,
        };
        let summary = ReplayOrchestrator::summarize_historical_series_runs(&[run_ok, run_fail]);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.success, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.total_gas_used, 100);
        assert_eq!(summary.average_gas_used, 50.0);
        assert_eq!(summary.success_rate, 0.5);
    }

    #[test]
    fn move_call_builder_builds_command() {
        let package = AccountAddress::from_hex_literal("0x2").expect("package");
        let type_arg = crate::types::parse_type_tag("0x2::sui::SUI").expect("type arg");
        let command = ReplayOrchestrator::move_call_builder(package, "coin", "supply")
            .with_type_args([type_arg.clone()])
            .with_args([Argument::Input(0)])
            .build()
            .expect("command");
        match command {
            Command::MoveCall {
                package: pkg,
                module,
                function,
                type_args,
                args,
            } => {
                assert_eq!(pkg, package);
                assert_eq!(module.as_ident_str().as_str(), "coin");
                assert_eq!(function.as_ident_str().as_str(), "supply");
                assert_eq!(type_args, vec![type_arg]);
                assert_eq!(args, vec![Argument::Input(0)]);
            }
            _ => panic!("expected move call"),
        }
    }

    #[test]
    fn replay_ptb_builder_tracks_indices() {
        let mut builder = ReplayOrchestrator::ptb_builder();
        let first = builder.pure(1u64).expect("first input");
        let second = builder.pure(2u64).expect("second input");
        assert_eq!(first, Argument::Input(0));
        assert_eq!(second, Argument::Input(1));

        let command = builder
            .command(Command::MakeMoveVec {
                type_tag: None,
                elements: vec![first, second],
            })
            .expect("command result");
        assert_eq!(command, Argument::Result(0));
        assert_eq!(builder.inputs().len(), 2);
        assert_eq!(builder.commands().len(), 1);
    }

    #[test]
    fn nested_result_requires_result_argument() {
        let err = ReplayPtbBuilder::nested_result(Argument::Input(0), 0)
            .expect_err("input cannot be nested-result base");
        assert!(err.to_string().contains("expected Argument::Result"));
    }

    #[test]
    fn created_object_lookup_requires_effects() {
        let env = SimulationEnvironment::new().expect("environment");
        let result = ExecutionResult {
            success: true,
            effects: None,
            error: None,
            raw_error: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: 0,
            error_context: None,
            state_at_failure: None,
        };
        let err =
            ReplayOrchestrator::find_created_object_id_by_struct_tag(&env, &result, "pool", "Pool")
                .expect_err("missing effects should fail");
        assert!(err.to_string().contains("missing effects"));
    }

    #[test]
    fn dynamic_field_parent_lookup_requires_non_empty_needles() {
        let result = ExecutionResult {
            success: true,
            effects: None,
            error: None,
            raw_error: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: 0,
            error_context: None,
            state_at_failure: None,
        };
        let err = ReplayOrchestrator::find_dynamic_field_parent_by_type_substrings(&result, &[])
            .expect_err("empty needle list should fail");
        assert!(err
            .to_string()
            .contains("required_substrings cannot be empty"));
    }

    #[test]
    fn summarize_execution_effects_defaults_without_effects() {
        let result = ExecutionResult {
            success: true,
            effects: None,
            error: None,
            raw_error: None,
            failed_command_index: None,
            failed_command_description: None,
            commands_succeeded: 0,
            error_context: None,
            state_at_failure: None,
        };
        let summary = ReplayOrchestrator::summarize_execution_effects(&result);
        assert_eq!(summary, ExecutionEffectsSummary::default());
    }

    #[test]
    fn resolve_created_object_from_return_or_effects_fails_without_effect_match() {
        let mut env = SimulationEnvironment::new().expect("environment");
        let id = AccountAddress::from_hex_literal("0x7").expect("id");
        let result = build_success_result(vec![vec![id.to_vec()]]);

        let err = ReplayOrchestrator::resolve_created_object_id_from_return_or_effects(
            &mut env, &result, 0, 0, "pool", "Pool", true, false, 1,
        )
        .expect_err("resolution should fail");
        assert!(err.to_string().contains("not found/recovered"));
    }
}
