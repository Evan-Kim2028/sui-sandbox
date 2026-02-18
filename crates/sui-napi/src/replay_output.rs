use super::*;

pub(crate) fn load_replay_state_from_file(
    path: &Path,
    digest: Option<&str>,
) -> Result<ReplayState> {
    let states = parse_replay_states_file(path)?;
    if states.is_empty() {
        return Err(anyhow!(
            "Replay state file '{}' did not contain any states",
            path.display()
        ));
    }
    if states.len() == 1 {
        return Ok(states.into_iter().next().expect("single replay state"));
    }
    let digest = digest.ok_or_else(|| {
        anyhow!(
            "Replay state file '{}' contains multiple states; provide digest",
            path.display()
        )
    })?;
    states
        .into_iter()
        .find(|state| state.transaction.digest.0 == digest)
        .ok_or_else(|| {
            anyhow!(
                "Replay state file '{}' does not contain digest '{}'",
                path.display(),
                digest
            )
        })
}

pub(crate) fn import_state_inner(
    state: Option<&str>,
    transactions: Option<&str>,
    objects: Option<&str>,
    packages: Option<&str>,
    cache_dir: Option<&str>,
) -> Result<serde_json::Value> {
    let cache_dir = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_local_cache_dir);

    let spec = ImportSpec {
        state: state.map(PathBuf::from),
        transactions: transactions.map(PathBuf::from),
        objects: objects.map(PathBuf::from),
        packages: packages.map(PathBuf::from),
    };
    let summary = import_replay_states(&cache_dir, &spec)?;
    Ok(serde_json::json!({
        "cache_dir": summary.cache_dir,
        "states_imported": summary.states_imported,
        "objects_imported": summary.objects_imported,
        "packages_imported": summary.packages_imported,
        "digests": summary.digests,
    }))
}

pub(crate) fn deserialize_transaction_inner(raw_bcs: &[u8]) -> Result<serde_json::Value> {
    let decoded = bcs_codec::deserialize_transaction(raw_bcs, "decoded_tx", None, None, None)?;
    serde_json::to_value(decoded).context("Failed to serialize decoded transaction")
}

pub(crate) fn deserialize_package_inner(raw_bcs: &[u8]) -> Result<serde_json::Value> {
    let decoded = bcs_codec::deserialize_package(raw_bcs)?;
    serde_json::to_value(decoded).context("Failed to serialize decoded package")
}

/// Build JSON output for analyze-only mode (no VM execution).
pub(crate) fn build_analyze_output(
    replay_state: &sui_state_fetcher::ReplayState,
    source: &str,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    verbose: bool,
) -> Result<serde_json::Value> {
    let mut result = core_build_replay_analysis_summary(
        replay_state,
        source,
        allow_fallback,
        auto_system_objects,
        dynamic_field_prefetch,
        prefetch_depth,
        prefetch_limit,
        verbose,
    );
    if let Some(diag) = build_replay_diagnostics_inner(replay_state) {
        result["missing_inputs"] = diag
            .get("missing_input_objects")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        result["missing_packages"] = diag
            .get("missing_packages")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        result["suggestions"] = diag
            .get("suggestions")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
    } else {
        result["missing_inputs"] = serde_json::json!([]);
        result["missing_packages"] = serde_json::json!([]);
        result["suggestions"] = serde_json::json!([]);
    }
    Ok(result)
}

/// Build envelope JSON for analyze-only replay mode.
pub(crate) fn build_analyze_replay_output(
    replay_state: &ReplayState,
    requested_source: &str,
    effective_source: &str,
    vm_only: bool,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    verbose: bool,
) -> Result<serde_json::Value> {
    let analysis = build_analyze_output(
        replay_state,
        effective_source,
        allow_fallback,
        auto_system_objects,
        dynamic_field_prefetch,
        prefetch_depth,
        prefetch_limit,
        verbose,
    )?;

    let execution_path = serde_json::json!({
        "requested_source": requested_source,
        "effective_source": effective_source,
        "vm_only": vm_only,
        "allow_fallback": allow_fallback,
        "auto_system_objects": auto_system_objects,
        "fallback_used": false,
        "dynamic_field_prefetch": dynamic_field_prefetch,
        "prefetch_depth": prefetch_depth,
        "prefetch_limit": prefetch_limit,
        "dependency_fetch_mode": "hydration_only",
        "dependency_packages_fetched": 0,
        "synthetic_inputs": 0,
    });

    let mut output = serde_json::json!({
        "digest": replay_state.transaction.digest.0,
        "local_success": true,
        "execution_path": execution_path,
        "analysis": analysis.clone(),
        "commands_executed": 0,
    });

    if let Some(summary) = analysis.as_object() {
        for (key, value) in summary {
            if output.get(key).is_none() {
                output[key] = value.clone();
            }
        }
    }

    Ok(output)
}

pub(crate) fn build_replay_diagnostics_inner(
    replay_state: &ReplayState,
) -> Option<serde_json::Value> {
    core_build_replay_diagnostics(
        replay_state,
        core_missing_input_objects_from_state(replay_state),
        |address| replay_state.packages.contains_key(address),
        CoreReplayDiagnosticsOptions {
            allow_fallback: true,
            missing_input_message:
                "Missing input objects detected; provide full object state via state_file or better hydration source.",
            missing_package_message:
                "Missing package bytecode detected; prepare a package context and replay with context_path.",
            fallback_message: "",
        },
    )
    .and_then(|diagnostics| serde_json::to_value(diagnostics).ok())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_replay_output(
    replay_state: &sui_state_fetcher::ReplayState,
    replay_result: Result<sui_sandbox_core::tx_replay::ReplayExecution>,
    requested_source: &str,
    effective_source: &str,
    vm_only: bool,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    dependency_fetch_mode: &str,
    dependency_packages_fetched: usize,
    synthetic_inputs: usize,
    compare: bool,
) -> Result<serde_json::Value> {
    let execution_path = serde_json::json!({
        "requested_source": requested_source,
        "effective_source": effective_source,
        "vm_only": vm_only,
        "allow_fallback": allow_fallback,
        "auto_system_objects": auto_system_objects,
        "fallback_used": false,
        "dynamic_field_prefetch": dynamic_field_prefetch,
        "prefetch_depth": prefetch_depth,
        "prefetch_limit": prefetch_limit,
        "dependency_fetch_mode": dependency_fetch_mode,
        "dependency_packages_fetched": dependency_packages_fetched,
        "synthetic_inputs": synthetic_inputs,
    });

    match replay_result {
        Ok(execution) => {
            let result = execution.result;
            let effects = &execution.effects;
            let diagnostics = if result.local_success {
                None
            } else {
                build_replay_diagnostics_inner(replay_state)
            };

            let effects_summary = serde_json::json!({
                "success": effects.success,
                "error": effects.error,
                "gas_used": effects.gas_used,
                "created": effects.created.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "mutated": effects.mutated.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "deleted": effects.deleted.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "wrapped": effects.wrapped.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "unwrapped": effects.unwrapped.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "transferred": effects.transferred.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "received": effects.received.iter().map(|id| id.to_hex_literal()).collect::<Vec<_>>(),
                "events_count": effects.events.len(),
                "failed_command_index": effects.failed_command_index,
                "failed_command_description": effects.failed_command_description,
                "commands_succeeded": effects.commands_succeeded,
                "return_values": effects.return_values.iter().map(|v| v.len()).collect::<Vec<_>>(),
            });

            let comparison = if compare {
                result.comparison.map(|c| {
                    serde_json::json!({
                        "status_match": c.status_match,
                        "created_match": c.created_count_match,
                        "mutated_match": c.mutated_count_match,
                        "deleted_match": c.deleted_count_match,
                        "on_chain_status": if c.status_match && result.local_success {
                            "success"
                        } else if c.status_match && !result.local_success {
                            "failed"
                        } else {
                            "unknown"
                        },
                        "local_status": if result.local_success { "success" } else { "failed" },
                        "notes": c.notes,
                    })
                })
            } else {
                None
            };

            let mut output = serde_json::json!({
                "digest": replay_state.transaction.digest.0,
                "local_success": result.local_success,
                "execution_path": execution_path,
                "effects": effects_summary,
                "commands_executed": result.commands_executed,
            });

            if let Some(err) = &result.local_error {
                output["local_error"] = serde_json::json!(err);
            }
            if let Some(diagnostics) = diagnostics {
                output["diagnostics"] = diagnostics;
            }
            if let Some(cmp) = comparison {
                output["comparison"] = cmp;
            }

            Ok(output)
        }
        Err(e) => {
            let mut output = serde_json::json!({
                "digest": replay_state.transaction.digest.0,
                "local_success": false,
                "local_error": e.to_string(),
                "execution_path": execution_path,
                "commands_executed": 0,
            });
            if let Some(diagnostics) = build_replay_diagnostics_inner(replay_state) {
                output["diagnostics"] = diagnostics;
            }
            Ok(output)
        }
    }
}

pub(crate) fn classify_replay_output(result: &serde_json::Value) -> serde_json::Value {
    serde_json::to_value(core_classify_replay_output(result)).unwrap_or_else(|_| {
        serde_json::json!({
            "failed": true,
            "category": "classification_error",
            "retryable": false,
            "local_error": "failed to serialize classification",
        })
    })
}
