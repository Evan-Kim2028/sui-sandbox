use std::collections::HashMap;

use sui_sandbox_core::replay_reporting::{
    build_replay_analysis_summary as core_build_replay_analysis_summary,
    build_replay_diagnostics as core_build_replay_diagnostics, ReplayDiagnosticsOptions,
};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay;
use sui_state_fetcher::ReplayState;

use super::{ReplayCmd, ReplayDiagnostics, ReplayExecutionPath, ReplayOutput};

#[allow(clippy::too_many_arguments)]
pub(super) fn build_analyze_replay_output(
    cmd: &ReplayCmd,
    replay_state: &ReplayState,
    requested_source: &str,
    effective_source: &str,
    allow_fallback: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
) -> ReplayOutput {
    let analysis = core_build_replay_analysis_summary(
        replay_state,
        effective_source,
        allow_fallback,
        cmd.hydration.auto_system_objects,
        dynamic_field_prefetch,
        prefetch_depth,
        prefetch_limit,
        cmd.verbose,
    );
    ReplayOutput {
        digest: replay_state.transaction.digest.0.clone(),
        local_success: true,
        local_error: None,
        diagnostics: None,
        execution_path: ReplayExecutionPath {
            requested_source: requested_source.to_string(),
            effective_source: effective_source.to_string(),
            vm_only: cmd.vm_only,
            allow_fallback,
            auto_system_objects: cmd.hydration.auto_system_objects,
            fallback_used: false,
            fallback_reasons: Vec::new(),
            dynamic_field_prefetch,
            prefetch_depth,
            prefetch_limit,
            dependency_fetch_mode: "hydration_only".to_string(),
            dependency_packages_fetched: 0,
            synthetic_inputs: 0,
        },
        comparison: None,
        analysis: Some(analysis),
        effects: None,
        effects_full: None,
        commands_executed: 0,
        batch_summary_printed: false,
    }
}

pub(super) fn build_replay_diagnostics(
    replay_state: &ReplayState,
    cached_objects: &HashMap<String, String>,
    resolver: &LocalModuleResolver,
    allow_fallback: bool,
) -> Option<ReplayDiagnostics> {
    let missing_input_objects =
        tx_replay::find_missing_input_objects(&replay_state.transaction, cached_objects)
            .into_iter()
            .map(|entry| entry.object_id)
            .collect::<Vec<_>>();
    core_build_replay_diagnostics(
        replay_state,
        missing_input_objects,
        |address| replay_state.packages.contains_key(address) || resolver.has_package(address),
        ReplayDiagnosticsOptions {
            allow_fallback,
            missing_input_message:
                "Missing input objects detected. Provide --state-json with full objects or replay from a better historical source.",
            missing_package_message:
                "Missing package bytecode detected. Run `sui-sandbox context prepare --package-id <ID>` (or `flow prepare`) and replay with --context.",
            fallback_message:
                "Fallback is disabled; rerun with --allow-fallback true to permit secondary hydration paths.",
        },
    )
}
