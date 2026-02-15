use anyhow::{anyhow, Context, Result};
use std::path::Path;

use super::super::SandboxState;
use super::analysis::{build_analyze_replay_output, build_replay_diagnostics};
use super::effects::build_effects_summary;
use super::support::{
    build_replay_object_maps, build_simulation_config, emit_linkage_debug_info,
    hydrate_resolver_from_replay_state, maybe_patch_replay_objects,
};
use super::{ComparisonResult, ReplayCmd, ReplayExecutionPath, ReplayOutput};
use sui_sandbox_core::tx_replay::EffectsReconcilePolicy;
use sui_state_fetcher::{
    build_aliases as build_aliases_shared, parse_replay_states_file, ReplayState,
};

pub(super) async fn execute_from_json(
    cmd: &ReplayCmd,
    state: &SandboxState,
    verbose: bool,
    json_path: &Path,
    _replay_progress: bool,
) -> Result<ReplayOutput> {
    let allow_fallback = cmd.hydration.allow_fallback && !cmd.vm_only;
    let states = parse_replay_states_file(json_path)
        .with_context(|| format!("Failed to parse state JSON from {}", json_path.display()))?;
    let replay_state = if states.len() == 1 {
        states.into_iter().next().expect("single replay state")
    } else if let Some(digest) = cmd.digest.as_deref() {
        states
            .into_iter()
            .find(|s| s.transaction.digest.0 == digest)
            .ok_or_else(|| {
                anyhow!(
                    "State file {} contains multiple states but none for digest {}",
                    json_path.display(),
                    digest
                )
            })?
    } else {
        return Err(anyhow!(
            "State file {} contains multiple states; provide digest explicitly",
            json_path.display()
        ));
    };

    if verbose {
        eprintln!(
            "[json] loaded state from {} ({} objects, {} packages)",
            json_path.display(),
            replay_state.objects.len(),
            replay_state.packages.len()
        );
    }

    execute_replay_state(
        cmd,
        state,
        &replay_state,
        "json",
        "state_json",
        allow_fallback,
        verbose,
    )
}

pub(super) fn execute_replay_state(
    cmd: &ReplayCmd,
    state: &SandboxState,
    replay_state: &ReplayState,
    requested_source: &str,
    effective_source: &str,
    allow_fallback: bool,
    _verbose: bool,
) -> Result<ReplayOutput> {
    if cmd.analyze_only {
        return Ok(build_analyze_replay_output(
            cmd,
            replay_state,
            requested_source,
            effective_source,
            allow_fallback,
            false,
            0,
            0,
        ));
    }

    let pkg_aliases = build_aliases_shared(&replay_state.packages, None, replay_state.checkpoint);
    let resolver = hydrate_resolver_from_replay_state(
        state,
        replay_state,
        &pkg_aliases.linkage_upgrades,
        &pkg_aliases.aliases,
    );
    emit_linkage_debug_info(&resolver, &pkg_aliases.aliases);
    let mut maps = build_replay_object_maps(replay_state, &pkg_aliases.versions);
    maybe_patch_replay_objects(
        &resolver,
        replay_state,
        &pkg_aliases.versions,
        &pkg_aliases.aliases,
        &mut maps,
        false,
        false,
    );
    let versions_str = maps.versions_str.clone();
    let cached_objects = maps.cached_objects;

    let reconcile_policy = if cmd.reconcile_dynamic_fields {
        EffectsReconcilePolicy::DynamicFields
    } else {
        EffectsReconcilePolicy::Strict
    };

    let config = build_simulation_config(replay_state);
    let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
    harness.set_address_aliases_with_versions(pkg_aliases.aliases.clone(), versions_str.clone());

    let replay_result =
        sui_sandbox_core::tx_replay::replay_with_version_tracking_with_policy_with_effects(
            &replay_state.transaction,
            &mut harness,
            &cached_objects,
            &pkg_aliases.aliases,
            Some(&versions_str),
            reconcile_policy,
        );

    match replay_result {
        Ok(execution) => {
            let result = execution.result;
            let effects_summary = build_effects_summary(&execution.effects);
            let comparison = if cmd.compare {
                result.comparison.map(|c| ComparisonResult {
                    status_match: c.status_match,
                    created_match: c.created_count_match,
                    mutated_match: c.mutated_count_match,
                    deleted_match: c.deleted_count_match,
                    on_chain_status: if c.status_match && result.local_success {
                        "success".to_string()
                    } else if c.status_match && !result.local_success {
                        "failed".to_string()
                    } else {
                        "unknown".to_string()
                    },
                    local_status: if result.local_success {
                        "success".to_string()
                    } else {
                        "failed".to_string()
                    },
                    notes: c.notes.clone(),
                })
            } else {
                None
            };
            let diagnostics = if result.local_success {
                None
            } else {
                build_replay_diagnostics(replay_state, &cached_objects, &resolver, allow_fallback)
            };

            Ok(ReplayOutput {
                digest: replay_state.transaction.digest.0.clone(),
                local_success: result.local_success,
                local_error: result.local_error,
                diagnostics,
                execution_path: ReplayExecutionPath {
                    requested_source: requested_source.to_string(),
                    effective_source: effective_source.to_string(),
                    vm_only: cmd.vm_only,
                    allow_fallback,
                    auto_system_objects: cmd.hydration.auto_system_objects,
                    fallback_used: false,
                    fallback_reasons: Vec::new(),
                    dynamic_field_prefetch: false,
                    prefetch_depth: 0,
                    prefetch_limit: 0,
                    dependency_fetch_mode: effective_source.to_string(),
                    dependency_packages_fetched: 0,
                    synthetic_inputs: 0,
                },
                comparison,
                analysis: None,
                effects: Some(effects_summary),
                effects_full: Some(execution.effects),
                commands_executed: result.commands_executed,
                batch_summary_printed: false,
            })
        }
        Err(e) => {
            let diagnostics =
                build_replay_diagnostics(replay_state, &cached_objects, &resolver, allow_fallback);
            Ok(ReplayOutput {
                digest: replay_state.transaction.digest.0.clone(),
                local_success: false,
                local_error: Some(e.to_string()),
                diagnostics,
                execution_path: ReplayExecutionPath {
                    requested_source: requested_source.to_string(),
                    effective_source: effective_source.to_string(),
                    vm_only: cmd.vm_only,
                    allow_fallback,
                    auto_system_objects: cmd.hydration.auto_system_objects,
                    fallback_used: false,
                    fallback_reasons: Vec::new(),
                    dynamic_field_prefetch: false,
                    prefetch_depth: 0,
                    prefetch_limit: 0,
                    dependency_fetch_mode: effective_source.to_string(),
                    dependency_packages_fetched: 0,
                    synthetic_inputs: 0,
                },
                comparison: None,
                analysis: None,
                effects: None,
                effects_full: None,
                commands_executed: 0,
                batch_summary_printed: false,
            })
        }
    }
}
