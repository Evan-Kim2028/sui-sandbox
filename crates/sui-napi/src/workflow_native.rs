use super::*;
use sui_sandbox_core::workflow_planner::{
    apply_workflow_profile_env as core_apply_workflow_profile_env,
    infer_workflow_template_from_modules as core_infer_workflow_template_from_modules,
    parse_builtin_workflow_template as core_parse_builtin_workflow_template,
    parse_workflow_fetch_strategy as core_parse_workflow_fetch_strategy,
    parse_workflow_profile as core_parse_workflow_profile,
    short_package_id as core_short_package_id,
    summarize_failure_output as core_summarize_failure_output,
    workflow_build_step_command as core_workflow_build_step_command,
    workflow_step_kind as core_workflow_step_kind, workflow_step_label as core_workflow_step_label,
    WorkflowEnvGuard as CoreWorkflowEnvGuard,
    WorkflowTemplateInference as CoreWorkflowTemplateInference,
};
#[cfg(test)]
use sui_sandbox_core::workflow_planner::{
    workflow_build_analyze_replay_command as core_workflow_build_analyze_replay_command,
    workflow_build_replay_command as core_workflow_build_replay_command,
};

pub(crate) fn parse_workflow_template(template: &str) -> Result<BuiltinWorkflowTemplate> {
    core_parse_builtin_workflow_template(template)
}

pub(crate) fn parse_workflow_output_format(
    format: Option<&str>,
) -> Result<Option<WorkflowOutputFormat>> {
    let Some(format) = format else {
        return Ok(None);
    };
    match format.trim().to_ascii_lowercase().as_str() {
        "json" => Ok(Some(WorkflowOutputFormat::Json)),
        "yaml" | "yml" => Ok(Some(WorkflowOutputFormat::Yaml)),
        other => Err(anyhow!(
            "invalid format '{}': expected 'json' or 'yaml'",
            other
        )),
    }
}

pub(crate) fn short_package_id(package_id: &str) -> String {
    core_short_package_id(package_id)
}

pub(crate) fn write_workflow_spec(
    path: &Path,
    spec: &WorkflowSpec,
    format: WorkflowOutputFormat,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create workflow output directory {}",
                    parent.display()
                )
            })?;
        }
    }
    let serialized = match format {
        WorkflowOutputFormat::Json => serde_json::to_string_pretty(spec)?,
        WorkflowOutputFormat::Yaml => serde_yaml::to_string(spec)?,
    };
    std::fs::write(path, serialized)
        .with_context(|| format!("Failed to write workflow spec {}", path.display()))?;
    Ok(())
}

pub(crate) fn probe_package_modules_for_workflow(package_id: &str) -> Result<(usize, Vec<String>)> {
    let graphql_endpoint = resolve_graphql_endpoint("https://fullnode.mainnet.sui.io:443");
    let graphql = GraphQLClient::new(&graphql_endpoint);
    let modules = fetch_package_modules(&graphql, package_id)?;
    let names = modules
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    Ok((names.len(), names))
}

pub(crate) fn probe_dependency_closure_for_workflow(
    package_id: &str,
) -> Result<(usize, Vec<String>)> {
    let fetched = fetch_package_bytecodes_inner(package_id, true)?;
    let packages_value = fetched
        .get("packages")
        .ok_or_else(|| anyhow!("fetch package probe output missing `packages` field"))?;
    let decoded = decode_context_packages_value(packages_value)?;
    let unresolved = unresolved_package_dependencies_for_modules(
        decoded
            .iter()
            .map(|(id, pkg)| (*id, pkg.modules.clone()))
            .collect(),
    )?;
    Ok((
        decoded.len(),
        unresolved
            .into_iter()
            .map(|address| address.to_hex_literal())
            .collect(),
    ))
}

pub(crate) type WorkflowTemplateInference = CoreWorkflowTemplateInference;

pub(crate) fn infer_workflow_template_from_modules(
    module_names: &[String],
) -> WorkflowTemplateInference {
    core_infer_workflow_template_from_modules(module_names)
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowDiscoveryTarget {
    pub(crate) digest: String,
    pub(crate) checkpoint: u64,
}

pub(crate) fn discover_latest_target_for_workflow(
    package_id: &str,
    latest: u64,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<WorkflowDiscoveryTarget> {
    if latest == 0 {
        return Err(anyhow!("discover_latest must be greater than zero"));
    }
    let discovered = discover_checkpoint_targets_inner(
        None,
        Some(latest),
        Some(package_id),
        false,
        1,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )?;
    let target = discovered
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .and_then(|targets| targets.first())
        .ok_or_else(|| {
            anyhow!(
                "no candidate transactions discovered for package {} in latest {} checkpoint(s)",
                package_id,
                latest
            )
        })?;
    let digest = target
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("discovery target missing digest"))?
        .to_string();
    let checkpoint = target
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("discovery target missing checkpoint"))?;
    Ok(WorkflowDiscoveryTarget { digest, checkpoint })
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowRunStepExecution {
    pub(crate) exit_code: i32,
    pub(crate) output: serde_json::Value,
}

pub(crate) fn workflow_step_kind(action: &WorkflowStepAction) -> &'static str {
    core_workflow_step_kind(action)
}

pub(crate) fn workflow_step_label(step: &WorkflowStep, index: usize) -> String {
    core_workflow_step_label(step, index)
}

pub(crate) fn workflow_summarize_failure_output(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    core_summarize_failure_output(stdout, stderr)
}

pub(crate) fn workflow_build_step_command(
    defaults: &WorkflowDefaults,
    step: &WorkflowStep,
) -> Result<Vec<String>> {
    core_workflow_build_step_command(defaults, step)
}

#[cfg(test)]
pub(crate) fn workflow_build_replay_command(
    defaults: &WorkflowDefaults,
    replay: &WorkflowReplayStep,
) -> Vec<String> {
    core_workflow_build_replay_command(defaults, replay)
}

#[cfg(test)]
pub(crate) fn workflow_build_analyze_replay_command(
    defaults: &WorkflowDefaults,
    analyze: &WorkflowAnalyzeReplayStep,
) -> Vec<String> {
    core_workflow_build_analyze_replay_command(defaults, analyze)
}

pub(crate) fn workflow_discover_target_for_replay(
    checkpoint: Option<&str>,
    latest: Option<u64>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<WorkflowDiscoveryTarget> {
    let discovered = discover_checkpoint_targets_inner(
        checkpoint,
        latest,
        None,
        false,
        1,
        walrus_network,
        walrus_caching_url,
        walrus_aggregator_url,
    )?;
    let target = discovered
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .and_then(|targets| targets.first())
        .ok_or_else(|| anyhow!("workflow replay step discovery returned no targets"))?;
    let digest = target
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("workflow replay step discovery target missing digest"))?
        .to_string();
    let checkpoint = target
        .get("checkpoint")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("workflow replay step discovery target missing checkpoint"))?;
    Ok(WorkflowDiscoveryTarget { digest, checkpoint })
}

/// Parse an inline workflow spec from a `serde_json::Value`.
///
/// In the NAPI layer we receive JSON values directly (no Python round-trip),
/// so we deserialize straight from `serde_json::Value`.
pub(crate) fn parse_inline_workflow_spec_from_value(
    spec: &serde_json::Value,
) -> Result<WorkflowSpec> {
    let spec: WorkflowSpec = serde_json::from_value(spec.clone())
        .context("invalid inline workflow spec JSON payload")?;
    spec.validate()?;
    Ok(spec)
}

pub(crate) fn workflow_parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    for (idx, arg) in args.iter().enumerate() {
        if arg == flag {
            return args.get(idx + 1).cloned();
        }
        let prefix = format!("{flag}=");
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

pub(crate) fn workflow_extract_interface_module_names(
    interface: &serde_json::Value,
) -> Vec<String> {
    let mut names = interface
        .get("modules")
        .and_then(serde_json::Value::as_object)
        .map(|modules| modules.keys().cloned().collect::<Vec<_>>())
        .or_else(|| {
            interface
                .as_object()
                .map(|modules| modules.keys().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_default();
    names.sort();
    names
}

pub(crate) fn workflow_has_comparison_mismatch(replay_output: &serde_json::Value) -> bool {
    let Some(comparison) = replay_output.get("comparison") else {
        return false;
    };
    let read = |key: &str| comparison.get(key).and_then(serde_json::Value::as_bool);
    [
        read("status_match"),
        read("created_match"),
        read("mutated_match"),
        read("deleted_match"),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value)
}

pub(crate) type WorkflowEnvGuard = CoreWorkflowEnvGuard;

pub(crate) fn workflow_apply_profile_env(profile: WorkflowReplayProfile) -> WorkflowEnvGuard {
    core_apply_workflow_profile_env(profile)
}

pub(crate) fn parse_replay_profile(value: Option<&str>) -> Result<WorkflowReplayProfile> {
    core_parse_workflow_profile(value)
}

pub(crate) fn parse_replay_fetch_strategy(value: Option<&str>) -> Result<WorkflowFetchStrategy> {
    core_parse_workflow_fetch_strategy(value)
}

pub(crate) fn workflow_execute_command_step(
    command: &WorkflowCommandStep,
    rpc_url: &str,
) -> Result<WorkflowRunStepExecution> {
    let normalized = normalize_command_args(&command.args)?;
    let Some(program) = normalized.first() else {
        return Err(anyhow!("command step args cannot be empty"));
    };

    if program == "status" {
        return Ok(WorkflowRunStepExecution {
            exit_code: 0,
            output: serde_json::json!({
                "success": true,
                "mode": "napi_native",
                "status": "ready",
            }),
        });
    }

    if program == "analyze" && normalized.get(1).is_some_and(|value| value == "package") {
        let package_id = workflow_parse_flag_value(&normalized, "--package-id")
            .ok_or_else(|| anyhow!("`analyze package` requires --package-id"))?;
        let interface = extract_interface_inner(Some(&package_id), None, rpc_url)?;
        let module_names = workflow_extract_interface_module_names(&interface);
        let list_modules = normalized.iter().any(|value| value == "--list-modules");
        return Ok(WorkflowRunStepExecution {
            exit_code: 0,
            output: serde_json::json!({
                "success": true,
                "package_id": package_id,
                "modules": module_names.len(),
                "module_names": if list_modules { Some(module_names) } else { None },
            }),
        });
    }

    if program == "view" && normalized.get(1).is_some_and(|value| value == "object") {
        let object_id = normalized
            .get(2)
            .cloned()
            .ok_or_else(|| anyhow!("`view object` requires an object id argument"))?;
        let version = workflow_parse_flag_value(&normalized, "--version")
            .map(|raw| raw.parse::<u64>())
            .transpose()
            .map_err(|_| anyhow!("`view object --version` must be a u64"))?;
        let object = fetch_object_bcs_inner(&object_id, version, None, None)?;
        let bcs_bytes = object
            .get("bcs_base64")
            .and_then(serde_json::Value::as_str)
            .map(|value| value.len())
            .unwrap_or(0);
        return Ok(WorkflowRunStepExecution {
            exit_code: 0,
            output: serde_json::json!({
                "success": true,
                "object_id": object_id,
                "version": object.get("version").cloned().unwrap_or(serde_json::Value::Null),
                "type_tag": object.get("type_tag").cloned().unwrap_or(serde_json::Value::Null),
                "bcs_base64_len": bcs_bytes,
            }),
        });
    }

    let output = Command::new(program)
        .args(&normalized[1..])
        .output()
        .with_context(|| format!("failed to execute command step program `{}`", program))?;
    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();
    let mut payload = serde_json::json!({
        "success": success,
        "program": program,
        "stdout": stdout,
        "stderr": stderr,
    });
    if !success {
        let summary = workflow_summarize_failure_output(&output.stdout, &output.stderr)
            .unwrap_or_else(|| {
                format!("command `{}` failed with exit code {}", program, exit_code)
            });
        if let Some(object) = payload.as_object_mut() {
            object.insert("error".to_string(), serde_json::json!(summary));
        }
    }

    Ok(WorkflowRunStepExecution {
        exit_code,
        output: payload,
    })
}

pub(crate) fn workflow_execute_replay_step(
    defaults: &WorkflowDefaults,
    replay: &WorkflowReplayStep,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> Result<WorkflowRunStepExecution> {
    let profile = replay
        .profile
        .or(defaults.profile)
        .unwrap_or(WorkflowReplayProfile::Balanced);
    let _profile_env = workflow_apply_profile_env(profile);
    let fetch_strategy = replay
        .fetch_strategy
        .or(defaults.fetch_strategy)
        .unwrap_or(WorkflowFetchStrategy::Full);
    let vm_only = replay.vm_only.or(defaults.vm_only).unwrap_or(false);
    let synthesize_missing = replay
        .synthesize_missing
        .or(defaults.synthesize_missing)
        .unwrap_or(false);
    let self_heal_dynamic_fields = replay
        .self_heal_dynamic_fields
        .or(defaults.self_heal_dynamic_fields)
        .unwrap_or(false);

    let source = replay
        .source
        .or(defaults.source)
        .unwrap_or(WorkflowSource::Hybrid);

    let mut allow_fallback = replay
        .allow_fallback
        .or(defaults.allow_fallback)
        .unwrap_or(true);
    if vm_only {
        allow_fallback = false;
    }
    let auto_system_objects = replay
        .auto_system_objects
        .or(defaults.auto_system_objects)
        .unwrap_or(true);
    let no_prefetch_requested = replay.no_prefetch.or(defaults.no_prefetch).unwrap_or(false);
    let no_prefetch = no_prefetch_requested || fetch_strategy == WorkflowFetchStrategy::Eager;
    let prefetch_depth = replay
        .prefetch_depth
        .or(defaults.prefetch_depth)
        .unwrap_or(3);
    let prefetch_limit = replay
        .prefetch_limit
        .or(defaults.prefetch_limit)
        .unwrap_or(200);
    let compare = replay.compare.or(defaults.compare).unwrap_or(false);
    let strict = replay.strict.or(defaults.strict).unwrap_or(false);

    let mut digest = replay
        .digest
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if replay.latest.is_some() && digest.is_some() {
        return Err(anyhow!(
            "workflow replay cannot combine `digest` and `latest` in napi native mode"
        ));
    }

    let mut checkpoint = None;
    if let Some(checkpoint_raw) = replay
        .checkpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(parsed) = checkpoint_raw.parse::<u64>() {
            checkpoint = Some(parsed);
        } else if digest.is_some() {
            return Err(anyhow!(
                "workflow replay checkpoint `{}` must be numeric when digest is provided in napi native mode",
                checkpoint_raw
            ));
        } else {
            let discovered = workflow_discover_target_for_replay(
                Some(checkpoint_raw),
                None,
                walrus_network,
                walrus_caching_url,
                walrus_aggregator_url,
            )?;
            digest = Some(discovered.digest);
            checkpoint = Some(discovered.checkpoint);
        }
    }
    if let Some(latest) = replay.latest {
        if latest == 0 {
            return Err(anyhow!("workflow replay latest must be >= 1"));
        }
        let discovered = workflow_discover_target_for_replay(
            None,
            Some(latest),
            walrus_network,
            walrus_caching_url,
            walrus_aggregator_url,
        )?;
        digest = Some(discovered.digest);
        checkpoint = Some(discovered.checkpoint);
    }
    if digest.is_none() && replay.state_json.is_none() {
        return Err(anyhow!(
            "workflow replay requires digest or state_json in napi native mode"
        ));
    }

    let source_str = source.as_cli_value();
    let mut output = if let Some(state_json) = replay.state_json.as_ref() {
        let replay_state = load_replay_state_from_file(state_json, digest.as_deref())?;
        replay_loaded_state_inner(
            replay_state,
            source_str,
            "state_json",
            None,
            allow_fallback,
            auto_system_objects,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            false,
            synthesize_missing,
            false,
            rpc_url,
            verbose,
        )?
    } else if source == WorkflowSource::Local {
        let digest = digest
            .as_deref()
            .ok_or_else(|| anyhow!("workflow replay missing digest for local source"))?;
        let cache_dir = default_local_cache_dir();
        let provider = FileStateProvider::new(&cache_dir).with_context(|| {
            format!(
                "failed to open workflow local replay cache {}",
                cache_dir.display()
            )
        })?;
        let replay_state = provider.get_state(digest)?;
        replay_loaded_state_inner(
            replay_state,
            source_str,
            "local_cache",
            None,
            allow_fallback,
            auto_system_objects,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            false,
            synthesize_missing,
            false,
            rpc_url,
            verbose,
        )?
    } else {
        replay_inner(
            digest
                .as_deref()
                .ok_or_else(|| anyhow!("workflow replay missing digest"))?,
            rpc_url,
            source_str,
            checkpoint,
            None,
            allow_fallback,
            prefetch_depth,
            prefetch_limit,
            auto_system_objects,
            no_prefetch,
            synthesize_missing,
            self_heal_dynamic_fields,
            vm_only,
            compare,
            false,
            false,
            verbose,
        )?
    };

    let local_success = output
        .get("local_success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut exit_code = if local_success { 0 } else { 1 };
    if strict && local_success && compare && workflow_has_comparison_mismatch(&output) {
        exit_code = 1;
        if let Some(object) = output.as_object_mut() {
            object.insert(
                "strict_error".to_string(),
                serde_json::json!("comparison mismatch under strict replay"),
            );
        }
    }
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "workflow_source".to_string(),
            serde_json::json!(source.as_cli_value()),
        );
        object.insert(
            "workflow_profile".to_string(),
            serde_json::json!(profile.as_cli_value()),
        );
        object.insert(
            "workflow_fetch_strategy".to_string(),
            serde_json::json!(fetch_strategy.as_cli_value()),
        );
    }
    if let Some(execution_path) = output
        .get_mut("execution_path")
        .and_then(serde_json::Value::as_object_mut)
    {
        execution_path.insert("vm_only".to_string(), serde_json::json!(vm_only));
        execution_path.insert(
            "allow_fallback".to_string(),
            serde_json::json!(allow_fallback),
        );
        execution_path.insert(
            "dynamic_field_prefetch".to_string(),
            serde_json::json!(!no_prefetch),
        );
        execution_path.insert(
            "self_heal_dynamic_fields".to_string(),
            serde_json::json!(self_heal_dynamic_fields),
        );
    }

    Ok(WorkflowRunStepExecution { exit_code, output })
}

pub(crate) fn workflow_execute_analyze_replay_step(
    defaults: &WorkflowDefaults,
    analyze: &WorkflowAnalyzeReplayStep,
    rpc_url: &str,
    verbose: bool,
) -> Result<WorkflowRunStepExecution> {
    let mm2_enabled = analyze.mm2.or(defaults.mm2).unwrap_or(false);
    let digest = analyze.digest.trim();
    if digest.is_empty() {
        return Err(anyhow!("workflow analyze_replay digest cannot be empty"));
    }
    let profile = defaults.profile.unwrap_or(WorkflowReplayProfile::Balanced);
    let _profile_env = workflow_apply_profile_env(profile);
    let source = analyze
        .source
        .or(defaults.source)
        .unwrap_or(WorkflowSource::Hybrid);
    let allow_fallback = analyze
        .allow_fallback
        .or(defaults.allow_fallback)
        .unwrap_or(true);
    let auto_system_objects = analyze
        .auto_system_objects
        .or(defaults.auto_system_objects)
        .unwrap_or(true);
    let no_prefetch = analyze
        .no_prefetch
        .or(defaults.no_prefetch)
        .unwrap_or(false);
    let prefetch_depth = analyze
        .prefetch_depth
        .or(defaults.prefetch_depth)
        .unwrap_or(3);
    let prefetch_limit = analyze
        .prefetch_limit
        .or(defaults.prefetch_limit)
        .unwrap_or(200);
    let mut output = if source == WorkflowSource::Local {
        let cache_dir = default_local_cache_dir();
        let provider = FileStateProvider::new(&cache_dir).with_context(|| {
            format!(
                "failed to open workflow local replay cache {}",
                cache_dir.display()
            )
        })?;
        let replay_state = provider.get_state(digest)?;
        replay_loaded_state_inner(
            replay_state,
            source.as_cli_value(),
            "local_cache",
            None,
            allow_fallback,
            auto_system_objects,
            false,
            false,
            false,
            true,
            false,
            mm2_enabled,
            rpc_url,
            verbose,
        )?
    } else {
        replay_inner(
            digest,
            rpc_url,
            source.as_cli_value(),
            analyze.checkpoint,
            None,
            allow_fallback,
            prefetch_depth,
            prefetch_limit,
            auto_system_objects,
            no_prefetch,
            false,
            false,
            false,
            false,
            true,
            mm2_enabled,
            verbose,
        )?
    };
    let local_success = output
        .get("local_success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let exit_code = if local_success { 0 } else { 1 };
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "workflow_source".to_string(),
            serde_json::json!(source.as_cli_value()),
        );
        object.insert(
            "workflow_profile".to_string(),
            serde_json::json!(profile.as_cli_value()),
        );
    }
    Ok(WorkflowRunStepExecution { exit_code, output })
}

pub(crate) fn write_workflow_run_report(path: &Path, report: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create workflow report output directory {}",
                    parent.display()
                )
            })?;
        }
    }
    let payload = serde_json::to_string_pretty(report)?;
    std::fs::write(path, payload)
        .with_context(|| format!("Failed to write workflow report {}", path.display()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn workflow_run_spec_inner(
    spec: WorkflowSpec,
    spec_label: String,
    dry_run: bool,
    continue_on_error: bool,
    report_path: Option<String>,
    rpc_url: &str,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> Result<serde_json::Value> {
    let prepared_steps = spec
        .steps
        .iter()
        .enumerate()
        .map(|(idx, step)| WorkflowPreparedStep {
            index: idx + 1,
            id: step.id.clone(),
            name: step.name.clone(),
            kind: workflow_step_kind(&step.action).to_string(),
            continue_on_error: step.continue_on_error,
            command: workflow_build_step_command(&spec.defaults, step)
                .map_err(|err| err.to_string()),
        })
        .collect::<Vec<_>>();

    let report_struct = run_prepared_workflow_steps(
        spec_label,
        &spec,
        prepared_steps,
        dry_run,
        continue_on_error,
        |step, prepared| {
            if verbose {
                eprintln!(
                    "[workflow:{}] {}",
                    workflow_step_label(step, prepared.index),
                    prepared.command_display()
                );
            }
        },
        |step, _prepared| {
            let step_output = match &step.action {
                WorkflowStepAction::Replay(replay) => workflow_execute_replay_step(
                    &spec.defaults,
                    replay,
                    rpc_url,
                    walrus_network,
                    walrus_caching_url,
                    walrus_aggregator_url,
                    verbose,
                )?,
                WorkflowStepAction::AnalyzeReplay(analyze) => {
                    workflow_execute_analyze_replay_step(&spec.defaults, analyze, rpc_url, verbose)?
                }
                WorkflowStepAction::Command(command_step) => {
                    workflow_execute_command_step(command_step, rpc_url)?
                }
            };

            let error = if step_output.exit_code == 0 {
                None
            } else {
                step_output
                    .output
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        step_output
                            .output
                            .get("local_error")
                            .and_then(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                    })
            };

            Ok(WorkflowStepExecution {
                exit_code: step_output.exit_code,
                output: Some(step_output.output),
                error,
            })
        },
    );

    let mut report = serde_json::to_value(&report_struct)?;

    if let Some(path) = report_path.as_deref() {
        let report_path = PathBuf::from(path);
        write_workflow_run_report(&report_path, &report)?;
        if let Some(object) = report.as_object_mut() {
            object.insert(
                "report_file".to_string(),
                serde_json::json!(report_path.display().to_string()),
            );
        }
    }

    Ok(report)
}
