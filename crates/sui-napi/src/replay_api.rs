use super::*;

// ---------------------------------------------------------------------------
// Internal helpers (not exported via #[napi])
// ---------------------------------------------------------------------------

fn parse_json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
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

fn parse_analysis_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get("analysis")
        .and_then(|v| v.get(key))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| value.get(key).and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

fn parse_analysis_lists(
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

/// Unified replay helper that combines the Python `replay()` logic from lib.rs.
///
/// Handles profile/fetch_strategy settings, local cache, state_file, context_path,
/// and delegates to the appropriate core replay function.
#[allow(clippy::too_many_arguments)]
fn replay_impl(
    digest: Option<&str>,
    rpc_url: &str,
    source: &str,
    checkpoint: Option<u64>,
    state_file: Option<&str>,
    context_path: Option<&str>,
    cache_dir: Option<&str>,
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
) -> Result<serde_json::Value> {
    let profile_parsed = parse_replay_profile(profile)?;
    let _profile_env = workflow_apply_profile_env(profile_parsed);
    let fetch_strategy_parsed = parse_replay_fetch_strategy(fetch_strategy)?;
    let allow_fallback = if vm_only { false } else { allow_fallback };
    let no_prefetch = no_prefetch || fetch_strategy_parsed == WorkflowFetchStrategy::Eager;

    let source_is_local = source.eq_ignore_ascii_case("local");
    let use_local_cache = source_is_local || cache_dir.is_some();
    let context_packages = if let Some(path) = context_path {
        Some(load_context_packages_from_file(Path::new(path))?)
    } else {
        None
    };

    if state_file.is_some() && use_local_cache {
        return Err(anyhow!(
            "state_file cannot be combined with cache_dir/source='local'"
        ));
    }

    if let Some(state_path) = state_file {
        let replay_state = load_replay_state_from_file(Path::new(state_path), digest)?;
        return replay_loaded_state_inner(
            replay_state,
            "state_file",
            "state_json",
            context_packages.as_ref(),
            allow_fallback,
            auto_system_objects,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            analyze_only,
            synthesize_missing,
            analyze_mm2,
            rpc_url,
            verbose,
        );
    }

    if use_local_cache {
        let digest = digest.ok_or_else(|| {
            anyhow!("digest is required when replaying from cache_dir/source='local'")
        })?;
        let cache_path = cache_dir
            .map(PathBuf::from)
            .unwrap_or_else(default_local_cache_dir);
        let provider = FileStateProvider::new(&cache_path).with_context(|| {
            format!(
                "Failed to open local replay cache {}",
                cache_path.display()
            )
        })?;
        let replay_state = provider.get_state(digest)?;
        return replay_loaded_state_inner(
            replay_state,
            source,
            "local_cache",
            context_packages.as_ref(),
            allow_fallback,
            auto_system_objects,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            analyze_only,
            synthesize_missing,
            analyze_mm2,
            rpc_url,
            verbose,
        );
    }

    let digest = digest.ok_or_else(|| anyhow!("digest is required"))?;
    replay_inner(
        digest,
        rpc_url,
        source,
        checkpoint,
        context_packages.as_ref(),
        allow_fallback,
        prefetch_depth,
        prefetch_limit,
        auto_system_objects,
        no_prefetch,
        synthesize_missing,
        self_heal_dynamic_fields,
        vm_only,
        compare,
        analyze_only,
        analyze_mm2,
        verbose,
    )
}

/// Shared implementation for `protocol_run`, `adapter_run`, and `context_run`.
#[allow(clippy::too_many_arguments)]
fn protocol_run_impl(
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
) -> napi::Result<serde_json::Value> {
    let resolved_package_id =
        resolve_protocol_package_id(protocol, package_id).map_err(to_napi_err)?;
    let prepared = prepare_package_context_inner(
        &resolved_package_id,
        resolve_deps,
        context_path,
    )
    .map_err(to_napi_err)?;

    let context_tmp = if context_path.is_some() {
        None
    } else {
        Some(write_temp_context_file(&prepared).map_err(to_napi_err)?)
    };
    let effective_context = context_path
        .map(ToOwned::to_owned)
        .or_else(|| context_tmp.as_ref().map(|p| p.display().to_string()));

    let discover_package_id_str = if discover_latest.is_some() {
        prepared
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    } else {
        None
    };

    let result = replay_transaction_inner(
        digest,
        checkpoint,
        discover_latest,
        discover_package_id_str.as_deref(),
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

/// Core implementation for `replay_transaction` that returns `napi::Result`.
///
/// Resolves discovery targets, infers source from checkpoint, then delegates
/// to the unified `replay_impl` helper.
#[allow(clippy::too_many_arguments)]
fn replay_transaction_inner(
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
) -> napi::Result<serde_json::Value> {
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
    .map_err(to_napi_err)?;

    let source_owned = source.map(|s| s.to_string()).unwrap_or_else(|| {
        if effective_checkpoint.is_some() {
            "walrus".to_string()
        } else {
            "hybrid".to_string()
        }
    });

    replay_impl(
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
    .map_err(to_napi_err)
}

// ---------------------------------------------------------------------------
// Exported NAPI functions
// ---------------------------------------------------------------------------

/// Replay a transaction with opinionated defaults for a compact native API.
///
/// Parameters mirror the Python `replay_transaction` API. All optional fields
/// use sensible defaults when omitted.
///
/// - `digest`: Transaction digest (optional when `state_file` contains a single transaction)
/// - `checkpoint`: Optional checkpoint (if provided and source is omitted, source defaults to walrus)
/// - `discover_latest`: Auto-discover digest from latest N checkpoints (requires `discover_package_id`)
/// - `discover_package_id`: Package filter used for discovery when `discover_latest` is set
/// - `source`: "hybrid", "grpc", or "walrus" (default: inferred)
/// - `state_file`: Optional replay-state JSON for deterministic local input data
/// - `context_path`: Optional prepared package context JSON to pre-seed package bytecode
/// - `cache_dir`: Optional local replay cache when source="local"
/// - `walrus_network`: Walrus network for discovery ("mainnet" or "testnet", default: "mainnet")
/// - `walrus_caching_url`: Optional custom Walrus caching endpoint
/// - `walrus_aggregator_url`: Optional custom Walrus aggregator endpoint
/// - `rpc_url`: Sui RPC endpoint
/// - `allow_fallback`: Allow fallback hydration paths (default: true)
/// - `profile`: Runtime defaults profile ("safe"|"balanced"|"fast")
/// - `fetch_strategy`: Dynamic-field fetch strategy ("eager"|"full")
/// - `vm_only`: Disable fallback paths and force VM-only behavior (default: false)
/// - `prefetch_depth`: Dynamic field prefetch depth (default: 3)
/// - `prefetch_limit`: Dynamic field prefetch limit (default: 200)
/// - `auto_system_objects`: Auto inject Clock/Random if missing (default: true)
/// - `no_prefetch`: Disable prefetch (default: false)
/// - `compare`: Compare local execution with on-chain effects (default: false)
/// - `analyze_only`: Hydration-only mode (default: false)
/// - `synthesize_missing`: Retry with synthetic object bytes when inputs are missing (default: false)
/// - `self_heal_dynamic_fields`: Enable dynamic field self-healing during VM execution (default: false)
/// - `analyze_mm2`: Build MM2 diagnostics in analyze-only mode (default: false)
/// - `verbose`: Verbose replay logging (default: false)
#[napi]
pub fn replay_transaction(
    digest: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    discover_package_id: Option<String>,
    source: Option<String>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    analyze_only: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let walrus_network = walrus_network.as_deref().unwrap_or("mainnet");
    let rpc_url = rpc_url
        .as_deref()
        .unwrap_or("https://fullnode.mainnet.sui.io:443");
    let vm_only = vm_only.unwrap_or(false);
    let allow_fallback = allow_fallback.unwrap_or(true);
    let prefetch_depth = prefetch_depth.unwrap_or(3) as usize;
    let prefetch_limit = prefetch_limit.unwrap_or(200) as usize;
    let auto_system_objects = auto_system_objects.unwrap_or(true);
    let no_prefetch = no_prefetch.unwrap_or(false);
    let compare = compare.unwrap_or(false);
    let analyze_only = analyze_only.unwrap_or(false);
    let synthesize_missing = synthesize_missing.unwrap_or(false);
    let self_heal_dynamic_fields = self_heal_dynamic_fields.unwrap_or(false);
    let analyze_mm2 = analyze_mm2.unwrap_or(false);
    let verbose = verbose.unwrap_or(false);

    replay_transaction_inner(
        digest.as_deref(),
        checkpoint.map(|v| v as u64),
        discover_latest.map(|v| v as u64),
        discover_package_id.as_deref(),
        source.as_deref(),
        state_file.as_deref(),
        context_path.as_deref(),
        cache_dir.as_deref(),
        walrus_network,
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
        rpc_url,
        profile.as_deref(),
        fetch_strategy.as_deref(),
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
/// Equivalent to `replay_transaction({ ..., analyze_only: true })`.
#[napi]
pub fn analyze_replay(
    digest: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    discover_package_id: Option<String>,
    source: Option<String>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    replay_transaction(
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
        Some(false),  // compare
        Some(true),   // analyze_only
        Some(false),  // synthesize_missing
        Some(false),  // self_heal_dynamic_fields
        analyze_mm2,
        verbose,
    )
}

/// Compatibility alias for `analyze_replay`.
#[napi]
pub fn replay_analyze(
    digest: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    discover_package_id: Option<String>,
    source: Option<String>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    analyze_replay(
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
///
/// Runs a full `replay_transaction` then extracts the effects-relevant subset
/// and appends a classification.
#[napi]
pub fn replay_effects(
    digest: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    discover_package_id: Option<String>,
    source: Option<String>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let replay_value = replay_transaction(
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
        Some(false),  // analyze_only
        synthesize_missing,
        self_heal_dynamic_fields,
        Some(false),  // analyze_mm2
        verbose,
    )?;

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
    Ok(out)
}

/// Classify replay output into structured failure categories and retry hints.
///
/// Accepts a replay result JSON value and returns a classification object.
#[napi]
pub fn classify_replay_result(
    result: serde_json::Value,
) -> napi::Result<serde_json::Value> {
    let classified = classify_replay_output(&result);
    Ok(classified)
}

/// Compare no-prefetch vs prefetch hydration to diagnose dynamic-field data gaps.
///
/// This API does not run VM execution; it uses hydration-only replay analysis
/// to detect whether dynamic-field prefetch resolves missing input objects.
#[napi]
pub fn dynamic_field_diagnostics(
    digest: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    discover_package_id: Option<String>,
    source: Option<String>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let pd = prefetch_depth.unwrap_or(3);
    let pl = prefetch_limit.unwrap_or(200);

    // Baseline: no dynamic-field prefetch.
    let baseline_json = replay_transaction(
        digest.clone(),
        checkpoint,
        discover_latest,
        discover_package_id.clone(),
        source.clone(),
        state_file.clone(),
        context_path.clone(),
        cache_dir.clone(),
        walrus_network.clone(),
        walrus_caching_url.clone(),
        walrus_aggregator_url.clone(),
        rpc_url.clone(),
        profile.clone(),
        fetch_strategy.clone(),
        vm_only,
        allow_fallback,
        Some(pd),
        Some(pl),
        auto_system_objects,
        Some(true),   // no_prefetch = true (baseline)
        Some(false),  // compare
        Some(true),   // analyze_only
        Some(false),  // synthesize_missing
        Some(false),  // self_heal_dynamic_fields
        analyze_mm2,
        verbose,
    )?;

    // Resolve target from baseline so both runs inspect the exact same transaction.
    let resolved_digest = baseline_json
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| digest.clone())
        .ok_or_else(|| {
            to_napi_err(anyhow!(
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
        .or(checkpoint.map(|v| v as u64));

    // Prefetch pass: dynamic-field prefetch enabled.
    let prefetch_json = replay_transaction(
        Some(resolved_digest.clone()),
        resolved_checkpoint.map(|v| v as u32),
        None,
        None,
        source.clone(),
        state_file.clone(),
        context_path.clone(),
        cache_dir.clone(),
        walrus_network.clone(),
        walrus_caching_url.clone(),
        walrus_aggregator_url.clone(),
        rpc_url.clone(),
        profile.clone(),
        fetch_strategy.clone(),
        vm_only,
        allow_fallback,
        Some(pd),
        Some(pl),
        auto_system_objects,
        Some(false),  // no_prefetch = false (prefetch enabled)
        Some(false),  // compare
        Some(true),   // analyze_only
        Some(false),  // synthesize_missing
        Some(false),  // self_heal_dynamic_fields
        analyze_mm2,
        verbose,
    )?;

    let prefetch_depth_val = pd as usize;

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
            "Dynamic-field hydration appears relevant: keep prefetch enabled (no_prefetch=false)."
                .to_string(),
        );
    }
    if objects_added_by_prefetch > 0 && prefetch_depth_val < 6 {
        recommendations.push(format!(
            "Prefetch discovered additional objects ({}). Consider increasing prefetch_depth (current {}).",
            objects_added_by_prefetch, prefetch_depth_val
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
        "prefetch_depth": prefetch_depth_val,
        "prefetch_limit": pl as usize,
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

    Ok(result)
}

/// Protocol-first run path: prepare context + replay in one call.
///
/// Non-generic protocols require `package_id`. Runtime inputs (objects, type
/// args, historical choices) stay explicit by design.
#[napi]
pub fn protocol_run(
    digest: Option<String>,
    protocol: Option<String>,
    package_id: Option<String>,
    resolve_deps: Option<bool>,
    context_path: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    source: Option<String>,
    state_file: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    analyze_only: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let protocol = protocol.as_deref().unwrap_or("generic");
    let resolve_deps = resolve_deps.unwrap_or(true);
    let walrus_network = walrus_network.as_deref().unwrap_or("mainnet");
    let rpc_url_str = rpc_url
        .as_deref()
        .unwrap_or("https://fullnode.mainnet.sui.io:443");

    protocol_run_impl(
        digest.as_deref(),
        protocol,
        package_id.as_deref(),
        resolve_deps,
        context_path.as_deref(),
        checkpoint.map(|v| v as u64),
        discover_latest.map(|v| v as u64),
        source.as_deref(),
        state_file.as_deref(),
        cache_dir.as_deref(),
        walrus_network,
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
        rpc_url_str,
        profile.as_deref(),
        fetch_strategy.as_deref(),
        vm_only.unwrap_or(false),
        allow_fallback.unwrap_or(true),
        prefetch_depth.unwrap_or(3) as usize,
        prefetch_limit.unwrap_or(200) as usize,
        auto_system_objects.unwrap_or(true),
        no_prefetch.unwrap_or(false),
        compare.unwrap_or(false),
        analyze_only.unwrap_or(false),
        synthesize_missing.unwrap_or(false),
        self_heal_dynamic_fields.unwrap_or(false),
        analyze_mm2.unwrap_or(false),
        verbose.unwrap_or(false),
    )
}

/// Canonical alias for `protocol_run`.
#[napi]
pub fn adapter_run(
    digest: Option<String>,
    protocol: Option<String>,
    package_id: Option<String>,
    resolve_deps: Option<bool>,
    context_path: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    source: Option<String>,
    state_file: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    analyze_only: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    protocol_run(
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
///
/// Identical to `replay_transaction` with context support.
#[napi]
pub fn context_replay(
    digest: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    discover_package_id: Option<String>,
    source: Option<String>,
    state_file: Option<String>,
    context_path: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    analyze_only: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    replay_transaction(
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

/// Canonical context run wrapper: prepare context + replay in one call.
///
/// Takes a `package_id` and delegates to `protocol_run` with `protocol="generic"`.
#[napi]
pub fn context_run(
    package_id: String,
    digest: Option<String>,
    resolve_deps: Option<bool>,
    context_path: Option<String>,
    checkpoint: Option<u32>,
    discover_latest: Option<u32>,
    source: Option<String>,
    state_file: Option<String>,
    cache_dir: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    rpc_url: Option<String>,
    profile: Option<String>,
    fetch_strategy: Option<String>,
    vm_only: Option<bool>,
    allow_fallback: Option<bool>,
    prefetch_depth: Option<u32>,
    prefetch_limit: Option<u32>,
    auto_system_objects: Option<bool>,
    no_prefetch: Option<bool>,
    compare: Option<bool>,
    analyze_only: Option<bool>,
    synthesize_missing: Option<bool>,
    self_heal_dynamic_fields: Option<bool>,
    analyze_mm2: Option<bool>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let walrus_network_str = walrus_network.as_deref().unwrap_or("mainnet");
    let rpc_url_str = rpc_url
        .as_deref()
        .unwrap_or("https://fullnode.mainnet.sui.io:443");

    protocol_run_impl(
        digest.as_deref(),
        "generic",
        Some(&package_id),
        resolve_deps.unwrap_or(true),
        context_path.as_deref(),
        checkpoint.map(|v| v as u64),
        discover_latest.map(|v| v as u64),
        source.as_deref(),
        state_file.as_deref(),
        cache_dir.as_deref(),
        walrus_network_str,
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
        rpc_url_str,
        profile.as_deref(),
        fetch_strategy.as_deref(),
        vm_only.unwrap_or(false),
        allow_fallback.unwrap_or(true),
        prefetch_depth.unwrap_or(3) as usize,
        prefetch_limit.unwrap_or(200) as usize,
        auto_system_objects.unwrap_or(true),
        no_prefetch.unwrap_or(false),
        compare.unwrap_or(false),
        analyze_only.unwrap_or(false),
        synthesize_missing.unwrap_or(false),
        self_heal_dynamic_fields.unwrap_or(false),
        analyze_mm2.unwrap_or(false),
        verbose.unwrap_or(false),
    )
}
