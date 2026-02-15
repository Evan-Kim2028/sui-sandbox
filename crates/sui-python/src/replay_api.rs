use super::*;

/// Replay a transaction with opinionated defaults for a compact native API.
///
/// Args:
///     digest: Transaction digest (optional when state_file contains a single transaction)
///     checkpoint: Optional checkpoint (if provided and source is omitted, source defaults to walrus)
///     discover_latest: Auto-discover digest from latest N checkpoints (requires discover_package_id)
///     discover_package_id: Package filter used for discovery when discover_latest is set
///     source: "hybrid", "grpc", or "walrus" (default: inferred)
///     state_file: Optional replay-state JSON for deterministic local input data
///     context_path: Optional prepared package context JSON to pre-seed package bytecode
///     cache_dir: Optional local replay cache when source="local"
///     walrus_network: Walrus network for discovery ("mainnet" or "testnet")
///     walrus_caching_url: Optional custom Walrus caching endpoint (requires walrus_aggregator_url)
///     walrus_aggregator_url: Optional custom Walrus aggregator endpoint (requires walrus_caching_url)
///     rpc_url: Sui RPC endpoint
///     allow_fallback: Allow fallback hydration paths
///     profile: Runtime defaults profile ("safe"|"balanced"|"fast")
///     fetch_strategy: Dynamic-field fetch strategy ("eager"|"full")
///     vm_only: Disable fallback paths and force VM-only behavior
///     prefetch_depth: Dynamic field prefetch depth
///     prefetch_limit: Dynamic field prefetch limit
///     auto_system_objects: Auto inject Clock/Random if missing
///     no_prefetch: Disable prefetch
///     compare: Compare local execution with on-chain effects
///     analyze_only: Hydration-only mode
///     synthesize_missing: Retry with synthetic object bytes when inputs are missing
///     self_heal_dynamic_fields: Enable dynamic field self-healing during VM execution
///     analyze_mm2: Build MM2 diagnostics (analyze-only mode)
///     verbose: Verbose replay logging
///
/// Returns: Replay result dict
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn replay_transaction(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let (effective_digest, effective_checkpoint) = resolve_replay_target_from_discovery(
        digest,
        checkpoint,
        state_file,
        discover_latest,
        discover_package_id,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
    .map_err(to_py_err)?;

    let source_owned = source.map(|s| s.to_string()).unwrap_or_else(|| {
        if effective_checkpoint.is_some() {
            "walrus".to_string()
        } else {
            "hybrid".to_string()
        }
    });
    replay(
        py,
        effective_digest.as_deref(),
        rpc_url,
        &source_owned,
        effective_checkpoint,
        state_file,
        context_path,
        cache_dir,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        analyze_only,
        synthesize_missing,
        self_heal_dynamic_fields,
        analyze_mm2,
        verbose,
    )
}

/// Analyze replay hydration/readiness only (CLI parity for `analyze replay`).
///
/// This is equivalent to `replay_transaction(..., analyze_only=True)`.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn analyze_replay(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        discover_package_id,
        source,
        state_file,
        context_path,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        false,
        true,
        false,
        false,
        analyze_mm2,
        verbose,
    )
}

/// Compatibility alias for `analyze_replay`.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn replay_analyze(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    analyze_replay(
        py,
        digest,
        checkpoint,
        discover_latest,
        discover_package_id,
        source,
        state_file,
        context_path,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        analyze_mm2,
        verbose,
    )
}

/// Execute replay and return execution/effects-focused fields.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    verbose=false,
))]
pub(super) fn replay_effects(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let replay_result = replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        discover_package_id,
        source,
        state_file,
        context_path,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        false,
        synthesize_missing,
        self_heal_dynamic_fields,
        false,
        verbose,
    )?;
    let replay_value = py_json_value(py, replay_result.bind(py).as_any()).map_err(to_py_err)?;
    let out = serde_json::json!({
        "digest": replay_value.get("digest").cloned().unwrap_or(serde_json::Value::Null),
        "local_success": replay_value
            .get("local_success")
            .cloned()
            .unwrap_or(serde_json::json!(false)),
        "local_error": replay_value
            .get("local_error")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "effects": replay_value
            .get("effects")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "comparison": replay_value
            .get("comparison")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "diagnostics": replay_value
            .get("diagnostics")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "execution_path": replay_value
            .get("execution_path")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "commands_executed": replay_value
            .get("commands_executed")
            .cloned()
            .unwrap_or(serde_json::json!(0)),
        "classification": classify_replay_output(&replay_value),
    });
    json_value_to_py(py, &out)
}

/// Classify replay output into structured failure categories and retry hints.
#[pyfunction]
pub(super) fn classify_replay_result(
    py: Python<'_>,
    result: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let raw = py_json_value(py, result).map_err(to_py_err)?;
    let classified = classify_replay_output(&raw);
    json_value_to_py(py, &classified)
}

pub(super) fn parse_json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(super) fn parse_analysis_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get("analysis")
        .and_then(|v| v.get(key))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| value.get(key).and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

pub(super) fn parse_analysis_lists(
    value: &serde_json::Value,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let missing_inputs = parse_json_string_list(
        value
            .get("analysis")
            .and_then(|v| v.get("missing_inputs"))
            .or_else(|| value.get("missing_inputs")),
    );
    let missing_packages = parse_json_string_list(
        value
            .get("analysis")
            .and_then(|v| v.get("missing_packages"))
            .or_else(|| value.get("missing_packages")),
    );
    let suggestions = parse_json_string_list(
        value
            .get("analysis")
            .and_then(|v| v.get("suggestions"))
            .or_else(|| value.get("suggestions")),
    );
    (missing_inputs, missing_packages, suggestions)
}

/// Compare no-prefetch vs prefetch hydration to diagnose dynamic-field data gaps.
///
/// This API does not run VM execution; it uses hydration-only replay analysis.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn dynamic_field_diagnostics(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    // Baseline: no dynamic-field prefetch.
    let baseline = replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        discover_package_id,
        source,
        state_file,
        context_path,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        true,
        false,
        true,
        false,
        false,
        analyze_mm2,
        verbose,
    )?;
    let baseline_json = py_json_value(py, baseline.bind(py).as_any()).map_err(to_py_err)?;

    // Resolve target from baseline so both runs inspect the exact same transaction.
    let resolved_digest = baseline_json
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| digest.map(ToOwned::to_owned))
        .ok_or_else(|| {
            to_py_err(anyhow!(
                "dynamic_field_diagnostics could not resolve digest"
            ))
        })?;
    let resolved_checkpoint = baseline_json
        .get("analysis")
        .and_then(|v| v.get("checkpoint"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            baseline_json
                .get("checkpoint")
                .and_then(serde_json::Value::as_u64)
        })
        .or(checkpoint);

    // Prefetch pass: dynamic-field prefetch enabled.
    let prefetch = replay_transaction(
        py,
        Some(&resolved_digest),
        resolved_checkpoint,
        None,
        None,
        source,
        state_file,
        context_path,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        false,
        false,
        true,
        false,
        false,
        analyze_mm2,
        verbose,
    )?;
    let prefetch_json = py_json_value(py, prefetch.bind(py).as_any()).map_err(to_py_err)?;

    let baseline_objects = parse_analysis_u64(&baseline_json, "objects");
    let prefetch_objects = parse_analysis_u64(&prefetch_json, "objects");
    let baseline_packages = parse_analysis_u64(&baseline_json, "packages");
    let prefetch_packages = parse_analysis_u64(&prefetch_json, "packages");
    let baseline_commands = parse_analysis_u64(&baseline_json, "commands");
    let prefetch_commands = parse_analysis_u64(&prefetch_json, "commands");

    let (baseline_missing_inputs, baseline_missing_packages, baseline_suggestions) =
        parse_analysis_lists(&baseline_json);
    let (prefetch_missing_inputs, prefetch_missing_packages, prefetch_suggestions) =
        parse_analysis_lists(&prefetch_json);

    let baseline_class = classify_replay_output(&baseline_json);
    let prefetch_class = classify_replay_output(&prefetch_json);

    let objects_added_by_prefetch = prefetch_objects.saturating_sub(baseline_objects);
    let missing_inputs_resolved = baseline_missing_inputs
        .len()
        .saturating_sub(prefetch_missing_inputs.len());
    let missing_packages_resolved = baseline_missing_packages
        .len()
        .saturating_sub(prefetch_missing_packages.len());

    let likely_dynamic_field_dependency =
        objects_added_by_prefetch > 0 || missing_inputs_resolved > 0;
    let remaining_data_gaps =
        !prefetch_missing_inputs.is_empty() || !prefetch_missing_packages.is_empty();

    let mut recommendations: Vec<String> = Vec::new();
    if likely_dynamic_field_dependency {
        recommendations.push(
            "Dynamic-field hydration appears relevant: keep prefetch enabled (no_prefetch=False)."
                .to_string(),
        );
    }
    if objects_added_by_prefetch > 0 && prefetch_depth < 6 {
        recommendations.push(format!(
            "Prefetch discovered additional objects ({}). Consider increasing prefetch_depth (current {}).",
            objects_added_by_prefetch, prefetch_depth
        ));
    }
    if remaining_data_gaps {
        recommendations.push(
            "Data gaps remain after prefetch; try archive-grade endpoint, checkpoint-pinned source, or context/state enrichment."
                .to_string(),
        );
    }
    if !remaining_data_gaps && likely_dynamic_field_dependency {
        recommendations.push(
            "Prefetch resolved hydration gaps for this transaction; keep this configuration for similar workloads."
                .to_string(),
        );
    }
    if recommendations.is_empty() {
        recommendations
            .push("No clear dynamic-field hydration gap was detected for this target.".to_string());
    }

    let result = serde_json::json!({
        "digest": resolved_digest,
        "checkpoint": resolved_checkpoint,
        "prefetch_depth": prefetch_depth,
        "prefetch_limit": prefetch_limit,
        "likely_dynamic_field_dependency": likely_dynamic_field_dependency,
        "remaining_data_gaps": remaining_data_gaps,
        "delta": {
            "objects_added_by_prefetch": objects_added_by_prefetch,
            "packages_delta": prefetch_packages as i64 - baseline_packages as i64,
            "commands_delta": prefetch_commands as i64 - baseline_commands as i64,
            "missing_inputs_resolved": missing_inputs_resolved,
            "missing_packages_resolved": missing_packages_resolved,
        },
        "baseline_no_prefetch": {
            "local_success": baseline_json.get("local_success").cloned().unwrap_or(serde_json::json!(false)),
            "objects": baseline_objects,
            "packages": baseline_packages,
            "commands": baseline_commands,
            "missing_inputs": baseline_missing_inputs,
            "missing_packages": baseline_missing_packages,
            "suggestions": baseline_suggestions,
            "classification": baseline_class,
        },
        "prefetch_enabled": {
            "local_success": prefetch_json.get("local_success").cloned().unwrap_or(serde_json::json!(false)),
            "objects": prefetch_objects,
            "packages": prefetch_packages,
            "commands": prefetch_commands,
            "missing_inputs": prefetch_missing_inputs,
            "missing_packages": prefetch_missing_packages,
            "suggestions": prefetch_suggestions,
            "classification": prefetch_class,
        },
        "recommendations": recommendations,
    });

    json_value_to_py(py, &result)
}

/// Protocol-first run path: prepare context + replay in one call.
///
/// Non-generic protocols require `package_id`. Runtime inputs (objects, type
/// args, historical choices) stay explicit by design.
#[allow(clippy::too_many_arguments)]
pub(super) fn protocol_run_impl(
    py: Python<'_>,
    digest: Option<&str>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    source: Option<&str>,
    state_file: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    let protocol_owned = protocol.to_string();
    let resolved_package_id =
        resolve_protocol_package_id(&protocol_owned, package_id).map_err(to_py_err)?;
    let context_path_owned = context_path.map(ToOwned::to_owned);
    let prepared = py
        .allow_threads(move || {
            prepare_package_context_inner(
                &resolved_package_id,
                resolve_deps,
                context_path_owned.as_deref(),
            )
        })
        .map_err(to_py_err)?;

    let context_tmp = if context_path.is_some() {
        None
    } else {
        Some(write_temp_context_file(&prepared).map_err(to_py_err)?)
    };
    let effective_context = context_path
        .map(ToOwned::to_owned)
        .or_else(|| context_tmp.as_ref().map(|p| p.display().to_string()));

    let result = replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        if discover_latest.is_some() {
            prepared
                .get("package_id")
                .and_then(serde_json::Value::as_str)
        } else {
            None
        },
        source,
        state_file,
        effective_context.as_deref(),
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        analyze_only,
        synthesize_missing,
        self_heal_dynamic_fields,
        analyze_mm2,
        verbose,
    );
    if let Some(path) = context_tmp {
        let _ = std::fs::remove_file(path);
    }
    result
}

#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    protocol="generic",
    package_id=None,
    resolve_deps=true,
    context_path=None,
    checkpoint=None,
    discover_latest=None,
    source=None,
    state_file=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn protocol_run(
    py: Python<'_>,
    digest: Option<&str>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    source: Option<&str>,
    state_file: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    protocol_run_impl(
        py,
        digest,
        protocol,
        package_id,
        resolve_deps,
        context_path,
        checkpoint,
        discover_latest,
        source,
        state_file,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        analyze_only,
        synthesize_missing,
        self_heal_dynamic_fields,
        analyze_mm2,
        verbose,
    )
}

/// Canonical alias for replaying against a prepared context.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    checkpoint=None,
    discover_latest=None,
    discover_package_id=None,
    source=None,
    state_file=None,
    context_path=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn context_replay(
    py: Python<'_>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    source: Option<&str>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    replay_transaction(
        py,
        digest,
        checkpoint,
        discover_latest,
        discover_package_id,
        source,
        state_file,
        context_path,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        analyze_only,
        synthesize_missing,
        self_heal_dynamic_fields,
        analyze_mm2,
        verbose,
    )
}

/// Canonical alias for `protocol_run`.
#[pyfunction]
#[pyo3(signature = (
    digest=None,
    *,
    protocol="generic",
    package_id=None,
    resolve_deps=true,
    context_path=None,
    checkpoint=None,
    discover_latest=None,
    source=None,
    state_file=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn adapter_run(
    py: Python<'_>,
    digest: Option<&str>,
    protocol: &str,
    package_id: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    source: Option<&str>,
    state_file: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    protocol_run_impl(
        py,
        digest,
        protocol,
        package_id,
        resolve_deps,
        context_path,
        checkpoint,
        discover_latest,
        source,
        state_file,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        analyze_only,
        synthesize_missing,
        self_heal_dynamic_fields,
        analyze_mm2,
        verbose,
    )
}

/// Canonical context run wrapper: prepare context + replay in one call.
#[pyfunction]
#[pyo3(signature = (
    package_id,
    digest=None,
    *,
    resolve_deps=true,
    context_path=None,
    checkpoint=None,
    discover_latest=None,
    source=None,
    state_file=None,
    cache_dir=None,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    rpc_url="https://fullnode.mainnet.sui.io:443",
    profile=None,
    fetch_strategy=None,
    vm_only=false,
    allow_fallback=true,
    prefetch_depth=3,
    prefetch_limit=200,
    auto_system_objects=true,
    no_prefetch=false,
    compare=false,
    analyze_only=false,
    synthesize_missing=false,
    self_heal_dynamic_fields=false,
    analyze_mm2=false,
    verbose=false,
))]
pub(super) fn context_run(
    py: Python<'_>,
    package_id: &str,
    digest: Option<&str>,
    resolve_deps: bool,
    context_path: Option<&str>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    source: Option<&str>,
    state_file: Option<&str>,
    cache_dir: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    rpc_url: &str,
    profile: Option<&str>,
    fetch_strategy: Option<&str>,
    vm_only: bool,
    allow_fallback: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    auto_system_objects: bool,
    no_prefetch: bool,
    compare: bool,
    analyze_only: bool,
    synthesize_missing: bool,
    self_heal_dynamic_fields: bool,
    analyze_mm2: bool,
    verbose: bool,
) -> PyResult<PyObject> {
    protocol_run_impl(
        py,
        digest,
        "generic",
        Some(package_id),
        resolve_deps,
        context_path,
        checkpoint,
        discover_latest,
        source,
        state_file,
        cache_dir,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        rpc_url,
        profile,
        fetch_strategy,
        vm_only,
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        compare,
        analyze_only,
        synthesize_missing,
        self_heal_dynamic_fields,
        analyze_mm2,
        verbose,
    )
}
