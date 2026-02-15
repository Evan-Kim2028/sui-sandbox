use super::*;

/// Validate a typed workflow spec (JSON or YAML) and return step counts.
#[pyfunction]
pub(super) fn workflow_validate(py: Python<'_>, spec_path: &str) -> PyResult<PyObject> {
    let spec_path_owned = spec_path.to_string();
    let value = py
        .allow_threads(move || {
            let path = PathBuf::from(&spec_path_owned);
            let spec = WorkflowSpec::load_from_path(&path)?;
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
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Generate a typed workflow spec from a built-in template.
#[pyfunction]
#[pyo3(signature = (
    *,
    template="generic",
    output_path=None,
    format=None,
    digest=None,
    checkpoint=None,
    include_analyze_step=true,
    strict_replay=true,
    name=None,
    package_id=None,
    view_objects=vec![],
    force=false,
))]
pub(super) fn workflow_init(
    py: Python<'_>,
    template: &str,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    include_analyze_step: bool,
    strict_replay: bool,
    name: Option<&str>,
    package_id: Option<&str>,
    view_objects: Vec<String>,
    force: bool,
) -> PyResult<PyObject> {
    let template_owned = template.to_string();
    let output_path_owned = output_path.map(ToOwned::to_owned);
    let format_owned = format.map(ToOwned::to_owned);
    let digest_owned = digest.map(ToOwned::to_owned);
    let name_owned = name.map(ToOwned::to_owned);
    let package_id_owned = package_id.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let template = parse_workflow_template(&template_owned)?;
            let digest = digest_owned
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| template.default_digest().to_string());
            let checkpoint = checkpoint.unwrap_or(template.default_checkpoint());

            let mut spec = build_builtin_workflow(
                template,
                &BuiltinWorkflowInput {
                    digest: Some(digest.clone()),
                    checkpoint: Some(checkpoint),
                    include_analyze_step,
                    include_replay_step: true,
                    strict_replay,
                    package_id: package_id_owned
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    view_objects: view_objects.clone(),
                },
            )?;
            if let Some(name) = name_owned
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                spec.name = Some(name.to_string());
            }
            spec.validate()?;

            let parsed_format = parse_workflow_output_format(format_owned.as_deref())?;
            let format_hint = parsed_format.unwrap_or(WorkflowOutputFormat::Json);
            let output_path = output_path_owned
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "workflow.{}.{}",
                        template.key(),
                        format_hint.extension()
                    ))
                });
            let output_format = parsed_format
                .or_else(|| WorkflowOutputFormat::from_path(&output_path))
                .unwrap_or(format_hint);

            if output_path.exists() && !force {
                return Err(anyhow!(
                    "Refusing to overwrite existing workflow spec at {} (pass force=True)",
                    output_path.display()
                ));
            }

            write_workflow_spec(&output_path, &spec, output_format)?;

            Ok(serde_json::json!({
                "template": template.key(),
                "output_file": output_path.display().to_string(),
                "format": output_format.as_str(),
                "digest": digest,
                "checkpoint": checkpoint,
                "include_analyze_step": include_analyze_step,
                "strict_replay": strict_replay,
                "package_id": package_id_owned,
                "view_objects": view_objects.len(),
                "workflow_name": spec.name,
                "steps": spec.steps.len(),
            }))
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Auto-generate a draft adapter workflow from a package id.
///
/// This mirrors CLI `workflow auto` behavior:
/// - dependency closure validation (fail closed unless `best_effort=True`)
/// - template inference from module names (or explicit override)
/// - scaffold-only output when replay seed is unavailable
/// - replay-capable output with `digest` or `discover_latest`
#[pyfunction]
#[pyo3(signature = (
    package_id,
    *,
    template=None,
    output_path=None,
    format=None,
    digest=None,
    discover_latest=None,
    checkpoint=None,
    name=None,
    best_effort=false,
    force=false,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
pub(super) fn workflow_auto(
    py: Python<'_>,
    package_id: &str,
    template: Option<&str>,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    discover_latest: Option<u64>,
    checkpoint: Option<u64>,
    name: Option<&str>,
    best_effort: bool,
    force: bool,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    let package_id_owned = package_id.to_string();
    let template_owned = template.map(ToOwned::to_owned);
    let output_path_owned = output_path.map(ToOwned::to_owned);
    let format_owned = format.map(ToOwned::to_owned);
    let digest_owned = digest.map(ToOwned::to_owned);
    let name_owned = name.map(ToOwned::to_owned);
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_owned = walrus_aggregator_url.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let package_id = package_id_owned.trim();
            if package_id.is_empty() {
                return Err(anyhow!("package_id cannot be empty"));
            }

            let mut dependency_packages_fetched = None;
            let mut unresolved_dependencies = Vec::new();
            let mut dependency_probe_error = None;
            match probe_dependency_closure_for_workflow(package_id) {
                Ok((fetched_packages, unresolved)) => {
                    dependency_packages_fetched = Some(fetched_packages);
                    unresolved_dependencies = unresolved;
                }
                Err(err) => {
                    if best_effort {
                        dependency_probe_error = Some(err.to_string());
                    } else {
                        return Err(anyhow!(
                            "AUTO_CLOSURE_INCOMPLETE: dependency closure probe failed for package {}: {}\nHint: resolve package fetch issues, or rerun with best_effort=True to emit scaffold output.",
                            package_id,
                            err
                        ));
                    }
                }
            }
            if !unresolved_dependencies.is_empty() && !best_effort {
                return Err(anyhow!(
                    "AUTO_CLOSURE_INCOMPLETE: unresolved package dependencies after closure fetch for package {}: {}\nHint: ensure transitive package bytecode is available, or rerun with best_effort=True to emit scaffold output.",
                    package_id,
                    unresolved_dependencies.join(", ")
                ));
            }

            let mut package_module_count = None;
            let mut module_names = Vec::new();
            let mut package_module_probe_error = None;
            match probe_package_modules_for_workflow(package_id) {
                Ok((count, names)) => {
                    package_module_count = Some(count);
                    module_names = names;
                }
                Err(err) => {
                    package_module_probe_error = Some(err.to_string());
                }
            }

            let inference = if let Some(template_raw) = template_owned.as_deref() {
                WorkflowTemplateInference {
                    template: parse_workflow_template(template_raw)?,
                    confidence: "manual",
                    source: "user",
                    reason: None,
                }
            } else {
                infer_workflow_template_from_modules(&module_names)
            };
            let template = inference.template;

            let explicit_digest = digest_owned
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let mut discovery_probe_error = None;
            let discovered_target = if let Some(latest) = discover_latest {
                match discover_latest_target_for_workflow(
                    package_id,
                    latest,
                    &walrus_network_owned,
                    walrus_caching_owned.as_deref(),
                    walrus_aggregator_owned.as_deref(),
                ) {
                    Ok(target) => Some(target),
                    Err(err) => {
                        if best_effort {
                            discovery_probe_error = Some(err.to_string());
                            None
                        } else {
                            return Err(anyhow!(
                                "AUTO_DISCOVERY_EMPTY: failed to auto-discover replay target for package {}: {}\nHint: rerun with a larger discover_latest window, provide digest explicitly, or use best_effort=True for scaffold-only output.",
                                package_id,
                                err
                            ));
                        }
                    }
                }
            } else {
                None
            };

            let digest = explicit_digest
                .clone()
                .or_else(|| discovered_target.as_ref().map(|target| target.digest.clone()));
            let include_replay = digest.is_some();
            let checkpoint = if include_replay {
                if let Some(target) = discovered_target.as_ref() {
                    Some(target.checkpoint)
                } else {
                    Some(checkpoint.unwrap_or(template.default_checkpoint()))
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
                if discover_latest.is_some() {
                    missing_inputs.push(
                        "auto-discovery target (rerun with larger discover_latest window)"
                            .to_string(),
                    );
                } else {
                    missing_inputs.push("digest".to_string());
                    missing_inputs
                        .push("checkpoint (optional; default inferred per template)".to_string());
                }
            }

            let mut spec = build_builtin_workflow(
                template,
                &BuiltinWorkflowInput {
                    digest,
                    checkpoint,
                    include_analyze_step: include_replay,
                    include_replay_step: include_replay,
                    strict_replay: true,
                    package_id: Some(package_id.to_string()),
                    view_objects: Vec::new(),
                },
            )?;

            let pkg_suffix = short_package_id(package_id);
            spec.name = Some(name_owned.unwrap_or_else(|| {
                format!("auto_{}_{}", template.key(), pkg_suffix)
            }));
            spec.description = Some(format!(
                "Auto draft adapter generated from package {} (template: {}).",
                package_id,
                template.key()
            ));
            spec.validate()?;

            let parsed_format = parse_workflow_output_format(format_owned.as_deref())?;
            let format_hint = parsed_format.unwrap_or(WorkflowOutputFormat::Json);
            let output_path = output_path_owned
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "workflow.auto.{}.{}.{}",
                        template.key(),
                        pkg_suffix,
                        format_hint.extension()
                    ))
                });
            let output_format = parsed_format
                .or_else(|| WorkflowOutputFormat::from_path(&output_path))
                .unwrap_or(format_hint);

            if output_path.exists() && !force {
                return Err(anyhow!(
                    "Refusing to overwrite existing workflow spec at {} (pass force=True)",
                    output_path.display()
                ));
            }
            write_workflow_spec(&output_path, &spec, output_format)?;

            Ok(serde_json::json!({
                "package_id": package_id,
                "template": template.key(),
                "inference_source": inference.source,
                "inference_confidence": inference.confidence,
                "inference_reason": inference.reason,
                "output_file": output_path.display().to_string(),
                "format": output_format.as_str(),
                "replay_steps_included": include_replay,
                "replay_seed_source": replay_seed_source,
                "discover_latest": discover_latest,
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
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Run a typed workflow spec natively via Python bindings.
///
/// Supports replay, analyze_replay, and command steps without shelling out to
/// `sui-sandbox pipeline run` (compatibility alias: `workflow run`).
#[pyfunction]
#[pyo3(signature = (
    spec_path,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://archive.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
pub(super) fn workflow_run(
    py: Python<'_>,
    spec_path: &str,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    let spec_path_owned = spec_path.to_string();
    let report_path_owned = report_path.map(ToOwned::to_owned);
    let rpc_url_owned = rpc_url.to_string();
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_owned = walrus_aggregator_url.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            let spec_path = PathBuf::from(&spec_path_owned);
            let spec = WorkflowSpec::load_from_path(&spec_path)?;
            workflow_run_spec_inner(
                spec,
                spec_path.display().to_string(),
                dry_run,
                continue_on_error,
                report_path_owned,
                &rpc_url_owned,
                &walrus_network_owned,
                walrus_caching_owned.as_deref(),
                walrus_aggregator_owned.as_deref(),
                verbose,
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Run a typed workflow spec directly from an in-memory Python object (dict/list).
///
/// This avoids writing temporary spec files for ad-hoc or notebook workflows.
#[pyfunction]
#[pyo3(signature = (
    spec,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://archive.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
pub(super) fn workflow_run_inline(
    py: Python<'_>,
    spec: &Bound<'_, PyAny>,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    let inline_spec = parse_inline_workflow_spec(py, spec).map_err(to_py_err)?;
    let report_path_owned = report_path.map(ToOwned::to_owned);
    let rpc_url_owned = rpc_url.to_string();
    let walrus_network_owned = walrus_network.to_string();
    let walrus_caching_owned = walrus_caching_url.map(ToOwned::to_owned);
    let walrus_aggregator_owned = walrus_aggregator_url.map(ToOwned::to_owned);

    let value = py
        .allow_threads(move || {
            workflow_run_spec_inner(
                inline_spec,
                "<inline>".to_string(),
                dry_run,
                continue_on_error,
                report_path_owned,
                &rpc_url_owned,
                &walrus_network_owned,
                walrus_caching_owned.as_deref(),
                walrus_aggregator_owned.as_deref(),
                verbose,
            )
        })
        .map_err(to_py_err)?;
    json_value_to_py(py, &value)
}

/// Canonical alias for `workflow_validate`.
#[pyfunction]
pub(super) fn pipeline_validate(py: Python<'_>, spec_path: &str) -> PyResult<PyObject> {
    workflow_validate(py, spec_path)
}

/// Canonical alias for `workflow_init`.
#[pyfunction]
#[pyo3(signature = (
    *,
    template="generic",
    output_path=None,
    format=None,
    digest=None,
    checkpoint=None,
    include_analyze_step=true,
    strict_replay=true,
    name=None,
    package_id=None,
    view_objects=vec![],
    force=false,
))]
pub(super) fn pipeline_init(
    py: Python<'_>,
    template: &str,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    checkpoint: Option<u64>,
    include_analyze_step: bool,
    strict_replay: bool,
    name: Option<&str>,
    package_id: Option<&str>,
    view_objects: Vec<String>,
    force: bool,
) -> PyResult<PyObject> {
    workflow_init(
        py,
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
#[pyfunction]
#[pyo3(signature = (
    package_id,
    *,
    template=None,
    output_path=None,
    format=None,
    digest=None,
    discover_latest=None,
    checkpoint=None,
    name=None,
    best_effort=false,
    force=false,
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
))]
pub(super) fn pipeline_auto(
    py: Python<'_>,
    package_id: &str,
    template: Option<&str>,
    output_path: Option<&str>,
    format: Option<&str>,
    digest: Option<&str>,
    discover_latest: Option<u64>,
    checkpoint: Option<u64>,
    name: Option<&str>,
    best_effort: bool,
    force: bool,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> PyResult<PyObject> {
    workflow_auto(
        py,
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
#[pyfunction]
#[pyo3(signature = (
    spec_path,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://archive.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
pub(super) fn pipeline_run(
    py: Python<'_>,
    spec_path: &str,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    workflow_run(
        py,
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
#[pyfunction]
#[pyo3(signature = (
    spec,
    *,
    dry_run=false,
    continue_on_error=false,
    report_path=None,
    rpc_url="https://archive.mainnet.sui.io:443",
    walrus_network="mainnet",
    walrus_caching_url=None,
    walrus_aggregator_url=None,
    verbose=false,
))]
pub(super) fn pipeline_run_inline(
    py: Python<'_>,
    spec: &Bound<'_, PyAny>,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<&str>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> PyResult<PyObject> {
    workflow_run_inline(
        py,
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
