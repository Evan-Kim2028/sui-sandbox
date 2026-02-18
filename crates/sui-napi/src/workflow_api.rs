use super::*;

/// Validate a typed workflow spec (JSON or YAML) and return step counts.
#[napi]
pub fn workflow_validate(spec_path: String) -> napi::Result<serde_json::Value> {
    let path = PathBuf::from(&spec_path);
    let spec = WorkflowSpec::load_from_path(&path).map_err(to_napi_err)?;
    let mut replay_steps = 0usize;
    let mut analyze_replay_steps = 0usize;
    let mut command_steps = 0usize;
    for step in &spec.steps {
        match step.action {
            WorkflowStepAction::Replay(_) => replay_steps += 1,
            WorkflowStepAction::AnalyzeReplay(_) => analyze_replay_steps += 1,
            WorkflowStepAction::Command(_) => command_steps += 1,
        }
    }

    Ok(serde_json::json!({
        "spec_file": path.display().to_string(),
        "version": spec.version,
        "name": spec.name,
        "steps": spec.steps.len(),
        "replay_steps": replay_steps,
        "analyze_replay_steps": analyze_replay_steps,
        "command_steps": command_steps,
    }))
}

/// Generate a typed workflow spec from a built-in template.
#[napi]
pub fn workflow_init(
    template: Option<String>,
    output_path: Option<String>,
    format: Option<String>,
    digest: Option<String>,
    checkpoint: Option<u32>,
    include_analyze_step: Option<bool>,
    strict_replay: Option<bool>,
    name: Option<String>,
    package_id: Option<String>,
    view_objects: Option<Vec<String>>,
    force: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let template_str = template.as_deref().unwrap_or("generic");
    let include_analyze_step = include_analyze_step.unwrap_or(true);
    let strict_replay = strict_replay.unwrap_or(true);
    let view_objects = view_objects.unwrap_or_default();
    let force = force.unwrap_or(false);
    let checkpoint_u64 = checkpoint.map(|v| v as u64);

    let parsed_template =
        parse_workflow_template(template_str).map_err(to_napi_err)?;
    let resolved_digest = digest
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| parsed_template.default_digest().to_string());
    let resolved_checkpoint = checkpoint_u64.unwrap_or(parsed_template.default_checkpoint());

    let mut spec = build_builtin_workflow(
        parsed_template,
        &BuiltinWorkflowInput {
            digest: Some(resolved_digest.clone()),
            checkpoint: Some(resolved_checkpoint),
            include_analyze_step,
            include_replay_step: true,
            strict_replay,
            package_id: package_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            view_objects: view_objects.clone(),
        },
    )
    .map_err(to_napi_err)?;
    if let Some(n) = name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        spec.name = Some(n.to_string());
    }
    spec.validate().map_err(to_napi_err)?;

    let parsed_format =
        parse_workflow_output_format(format.as_deref()).map_err(to_napi_err)?;
    let format_hint = parsed_format.unwrap_or(WorkflowOutputFormat::Json);
    let resolved_output_path = output_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "workflow.{}.{}",
                parsed_template.key(),
                format_hint.extension()
            ))
        });
    let output_format = parsed_format
        .or_else(|| WorkflowOutputFormat::from_path(&resolved_output_path))
        .unwrap_or(format_hint);

    if resolved_output_path.exists() && !force {
        return Err(to_napi_err(anyhow!(
            "Refusing to overwrite existing workflow spec at {} (pass force=true)",
            resolved_output_path.display()
        )));
    }

    write_workflow_spec(&resolved_output_path, &spec, output_format).map_err(to_napi_err)?;

    Ok(serde_json::json!({
        "template": parsed_template.key(),
        "output_file": resolved_output_path.display().to_string(),
        "format": output_format.extension(),
        "digest": resolved_digest,
        "checkpoint": resolved_checkpoint,
        "include_analyze_step": include_analyze_step,
        "strict_replay": strict_replay,
        "package_id": package_id,
        "view_objects": view_objects.len(),
        "workflow_name": spec.name,
        "steps": spec.steps.len(),
    }))
}

/// Auto-generate a draft adapter workflow from a package id.
///
/// This mirrors CLI `workflow auto` behavior:
/// - dependency closure validation (fail closed unless `best_effort=true`)
/// - template inference from module names (or explicit override)
/// - scaffold-only output when replay seed is unavailable
/// - replay-capable output with `digest` or `discover_latest`
#[napi]
pub fn workflow_auto(
    package_id: String,
    template: Option<String>,
    output_path: Option<String>,
    format: Option<String>,
    digest: Option<String>,
    discover_latest: Option<u32>,
    checkpoint: Option<u32>,
    name: Option<String>,
    best_effort: Option<bool>,
    force: Option<bool>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    let best_effort = best_effort.unwrap_or(false);
    let force = force.unwrap_or(false);
    let walrus_network_str = walrus_network.as_deref().unwrap_or("mainnet");
    let discover_latest_u64 = discover_latest.map(|v| v as u64);
    let checkpoint_u64 = checkpoint.map(|v| v as u64);

    let package_id_trimmed = package_id.trim();
    if package_id_trimmed.is_empty() {
        return Err(to_napi_err(anyhow!("package_id cannot be empty")));
    }

    let mut dependency_packages_fetched = None;
    let mut unresolved_dependencies = Vec::new();
    let mut dependency_probe_error = None;
    match probe_dependency_closure_for_workflow(package_id_trimmed) {
        Ok((fetched_packages, unresolved)) => {
            dependency_packages_fetched = Some(fetched_packages);
            unresolved_dependencies = unresolved;
        }
        Err(err) => {
            if best_effort {
                dependency_probe_error = Some(err.to_string());
            } else {
                return Err(to_napi_err(anyhow!(
                    "AUTO_CLOSURE_INCOMPLETE: dependency closure probe failed for package {}: {}\nHint: resolve package fetch issues, or rerun with best_effort=true to emit scaffold output.",
                    package_id_trimmed,
                    err
                )));
            }
        }
    }
    if !unresolved_dependencies.is_empty() && !best_effort {
        return Err(to_napi_err(anyhow!(
            "AUTO_CLOSURE_INCOMPLETE: unresolved package dependencies after closure fetch for package {}: {}\nHint: ensure transitive package bytecode is available, or rerun with best_effort=true to emit scaffold output.",
            package_id_trimmed,
            unresolved_dependencies.join(", ")
        )));
    }

    let mut package_module_count = None;
    let mut module_names = Vec::new();
    let mut package_module_probe_error = None;
    match probe_package_modules_for_workflow(package_id_trimmed) {
        Ok((count, names)) => {
            package_module_count = Some(count);
            module_names = names;
        }
        Err(err) => {
            package_module_probe_error = Some(err.to_string());
        }
    }

    let inference = if let Some(template_raw) = template.as_deref() {
        WorkflowTemplateInference {
            template: parse_workflow_template(template_raw).map_err(to_napi_err)?,
            confidence: "manual",
            source: "user",
            reason: None,
        }
    } else {
        infer_workflow_template_from_modules(&module_names)
    };
    let inferred_template = inference.template;

    let explicit_digest = digest
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let mut discovery_probe_error = None;
    let discovered_target = if let Some(latest) = discover_latest_u64 {
        match discover_latest_target_for_workflow(
            package_id_trimmed,
            latest,
            walrus_network_str,
            walrus_caching_url.as_deref(),
            walrus_aggregator_url.as_deref(),
        ) {
            Ok(target) => Some(target),
            Err(err) => {
                if best_effort {
                    discovery_probe_error = Some(err.to_string());
                    None
                } else {
                    return Err(to_napi_err(anyhow!(
                        "AUTO_DISCOVERY_EMPTY: failed to auto-discover replay target for package {}: {}\nHint: rerun with a larger discover_latest window, provide digest explicitly, or use best_effort=true for scaffold-only output.",
                        package_id_trimmed,
                        err
                    )));
                }
            }
        }
    } else {
        None
    };

    let resolved_digest = explicit_digest
        .clone()
        .or_else(|| discovered_target.as_ref().map(|target| target.digest.clone()));
    let include_replay = resolved_digest.is_some();
    let resolved_checkpoint = if include_replay {
        if let Some(target) = discovered_target.as_ref() {
            Some(target.checkpoint)
        } else {
            Some(checkpoint_u64.unwrap_or(inferred_template.default_checkpoint()))
        }
    } else {
        None
    };
    let replay_seed_source = if explicit_digest.is_some() {
        "digest"
    } else if discovered_target.is_some() {
        "discover_latest"
    } else {
        "none"
    };

    let mut missing_inputs = Vec::new();
    if !include_replay {
        if discover_latest_u64.is_some() {
            missing_inputs.push(
                "auto-discovery target (rerun with larger discover_latest window)".to_string(),
            );
        } else {
            missing_inputs.push("digest".to_string());
            missing_inputs
                .push("checkpoint (optional; default inferred per template)".to_string());
        }
    }

    let mut spec = build_builtin_workflow(
        inferred_template,
        &BuiltinWorkflowInput {
            digest: resolved_digest,
            checkpoint: resolved_checkpoint,
            include_analyze_step: include_replay,
            include_replay_step: include_replay,
            strict_replay: true,
            package_id: Some(package_id_trimmed.to_string()),
            view_objects: Vec::new(),
        },
    )
    .map_err(to_napi_err)?;

    let pkg_suffix = short_package_id(package_id_trimmed);
    spec.name = Some(
        name.unwrap_or_else(|| format!("auto_{}_{}", inferred_template.key(), pkg_suffix)),
    );
    spec.description = Some(format!(
        "Auto draft adapter generated from package {} (template: {}).",
        package_id_trimmed,
        inferred_template.key()
    ));
    spec.validate().map_err(to_napi_err)?;

    let parsed_format =
        parse_workflow_output_format(format.as_deref()).map_err(to_napi_err)?;
    let format_hint = parsed_format.unwrap_or(WorkflowOutputFormat::Json);
    let resolved_output_path = output_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "workflow.auto.{}.{}.{}",
                inferred_template.key(),
                pkg_suffix,
                format_hint.extension()
            ))
        });
    let output_format = parsed_format
        .or_else(|| WorkflowOutputFormat::from_path(&resolved_output_path))
        .unwrap_or(format_hint);

    if resolved_output_path.exists() && !force {
        return Err(to_napi_err(anyhow!(
            "Refusing to overwrite existing workflow spec at {} (pass force=true)",
            resolved_output_path.display()
        )));
    }
    write_workflow_spec(&resolved_output_path, &spec, output_format).map_err(to_napi_err)?;

    Ok(serde_json::json!({
        "package_id": package_id_trimmed,
        "template": inferred_template.key(),
        "inference_source": inference.source,
        "inference_confidence": inference.confidence,
        "inference_reason": inference.reason,
        "output_file": resolved_output_path.display().to_string(),
        "format": output_format.extension(),
        "replay_steps_included": include_replay,
        "replay_seed_source": replay_seed_source,
        "discover_latest": discover_latest_u64,
        "discovered_checkpoint": discovered_target.as_ref().map(|target| target.checkpoint),
        "discovery_probe_error": discovery_probe_error,
        "missing_inputs": missing_inputs,
        "package_module_count": package_module_count,
        "package_module_probe_error": package_module_probe_error,
        "dependency_packages_fetched": dependency_packages_fetched,
        "unresolved_dependencies": unresolved_dependencies,
        "dependency_probe_error": dependency_probe_error,
        "steps": spec.steps.len(),
    }))
}

/// Run a typed workflow spec natively via NAPI bindings.
///
/// Supports replay, analyze_replay, and command steps without shelling out to
/// `sui-sandbox pipeline run` (compatibility alias: `workflow run`).
#[napi]
pub fn workflow_run(
    spec_path: String,
    dry_run: Option<bool>,
    continue_on_error: Option<bool>,
    report_path: Option<String>,
    rpc_url: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let dry_run = dry_run.unwrap_or(false);
    let continue_on_error = continue_on_error.unwrap_or(false);
    let rpc_url_str = rpc_url
        .as_deref()
        .unwrap_or("https://archive.mainnet.sui.io:443");
    let walrus_network_str = walrus_network.as_deref().unwrap_or("mainnet");
    let verbose = verbose.unwrap_or(false);

    let path = PathBuf::from(&spec_path);
    let spec = WorkflowSpec::load_from_path(&path).map_err(to_napi_err)?;
    workflow_run_spec_inner(
        spec,
        path.display().to_string(),
        dry_run,
        continue_on_error,
        report_path,
        rpc_url_str,
        walrus_network_str,
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
        verbose,
    )
    .map_err(to_napi_err)
}

/// Run a typed workflow spec directly from an in-memory JSON value.
///
/// This avoids writing temporary spec files for ad-hoc or programmatic workflows.
#[napi]
pub fn workflow_run_inline(
    spec: serde_json::Value,
    dry_run: Option<bool>,
    continue_on_error: Option<bool>,
    report_path: Option<String>,
    rpc_url: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let dry_run = dry_run.unwrap_or(false);
    let continue_on_error = continue_on_error.unwrap_or(false);
    let rpc_url_str = rpc_url
        .as_deref()
        .unwrap_or("https://archive.mainnet.sui.io:443");
    let walrus_network_str = walrus_network.as_deref().unwrap_or("mainnet");
    let verbose = verbose.unwrap_or(false);

    let inline_spec = parse_inline_workflow_spec_from_value(&spec).map_err(to_napi_err)?;
    workflow_run_spec_inner(
        inline_spec,
        "<inline>".to_string(),
        dry_run,
        continue_on_error,
        report_path,
        rpc_url_str,
        walrus_network_str,
        walrus_caching_url.as_deref(),
        walrus_aggregator_url.as_deref(),
        verbose,
    )
    .map_err(to_napi_err)
}

/// Canonical alias for `workflow_validate`.
#[napi]
pub fn pipeline_validate(spec_path: String) -> napi::Result<serde_json::Value> {
    workflow_validate(spec_path)
}

/// Canonical alias for `workflow_init`.
#[napi]
pub fn pipeline_init(
    template: Option<String>,
    output_path: Option<String>,
    format: Option<String>,
    digest: Option<String>,
    checkpoint: Option<u32>,
    include_analyze_step: Option<bool>,
    strict_replay: Option<bool>,
    name: Option<String>,
    package_id: Option<String>,
    view_objects: Option<Vec<String>>,
    force: Option<bool>,
) -> napi::Result<serde_json::Value> {
    workflow_init(
        template,
        output_path,
        format,
        digest,
        checkpoint,
        include_analyze_step,
        strict_replay,
        name,
        package_id,
        view_objects,
        force,
    )
}

/// Canonical alias for `workflow_auto`.
#[napi]
pub fn pipeline_auto(
    package_id: String,
    template: Option<String>,
    output_path: Option<String>,
    format: Option<String>,
    digest: Option<String>,
    discover_latest: Option<u32>,
    checkpoint: Option<u32>,
    name: Option<String>,
    best_effort: Option<bool>,
    force: Option<bool>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    workflow_auto(
        package_id,
        template,
        output_path,
        format,
        digest,
        discover_latest,
        checkpoint,
        name,
        best_effort,
        force,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )
}

/// Canonical alias for `workflow_run`.
#[napi]
pub fn pipeline_run(
    spec_path: String,
    dry_run: Option<bool>,
    continue_on_error: Option<bool>,
    report_path: Option<String>,
    rpc_url: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    workflow_run(
        spec_path,
        dry_run,
        continue_on_error,
        report_path,
        rpc_url,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        verbose,
    )
}

/// Canonical alias for `workflow_run_inline`.
#[napi]
pub fn pipeline_run_inline(
    spec: serde_json::Value,
    dry_run: Option<bool>,
    continue_on_error: Option<bool>,
    report_path: Option<String>,
    rpc_url: Option<String>,
    walrus_network: Option<String>,
    walrus_caching_url: Option<String>,
    walrus_aggregator_url: Option<String>,
    verbose: Option<bool>,
) -> napi::Result<serde_json::Value> {
    workflow_run_inline(
        spec,
        dry_run,
        continue_on_error,
        report_path,
        rpc_url,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
        verbose,
    )
}
