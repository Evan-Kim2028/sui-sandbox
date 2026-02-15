use clap::ValueEnum;

use super::{ReplayCmd, ReplayEffectsSummary, ReplayExecutionPath};

#[allow(clippy::too_many_arguments)]
pub(super) fn build_execution_path(
    cmd: &ReplayCmd,
    allow_fallback: bool,
    enable_dynamic_fields: bool,
    dependency_fetch_mode: String,
    fetched_deps: usize,
    fallback_used: bool,
    fallback_reasons: Vec<String>,
    synthetic_inputs: usize,
) -> ReplayExecutionPath {
    ReplayExecutionPath {
        requested_source: cmd
            .hydration
            .source
            .to_possible_value()
            .map_or_else(|| "hybrid".to_string(), |v| v.get_name().to_string()),
        effective_source: cmd
            .hydration
            .source
            .to_possible_value()
            .map_or_else(|| "unknown".to_string(), |v| v.get_name().to_string()),
        vm_only: cmd.vm_only,
        allow_fallback,
        auto_system_objects: cmd.hydration.auto_system_objects,
        fallback_used,
        fallback_reasons,
        dynamic_field_prefetch: enable_dynamic_fields,
        prefetch_depth: cmd.hydration.prefetch_depth,
        prefetch_limit: cmd.hydration.prefetch_limit,
        dependency_fetch_mode,
        dependency_packages_fetched: fetched_deps,
        synthetic_inputs,
    }
}

pub(super) fn build_effects_summary(
    effects: &sui_sandbox_core::ptb::TransactionEffects,
) -> ReplayEffectsSummary {
    ReplayEffectsSummary {
        success: effects.success,
        error: effects.error.clone(),
        gas_used: effects.gas_used,
        created: effects
            .created
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        mutated: effects
            .mutated
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        deleted: effects
            .deleted
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        wrapped: effects
            .wrapped
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        unwrapped: effects
            .unwrapped
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        transferred: effects
            .transferred
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        received: effects
            .received
            .iter()
            .map(|id| id.to_hex_literal())
            .collect(),
        events_count: effects.events.len(),
        failed_command_index: effects.failed_command_index,
        failed_command_description: effects.failed_command_description.clone(),
        commands_succeeded: effects.commands_succeeded,
        return_values: effects
            .return_values
            .iter()
            .map(|vals| vals.len())
            .collect(),
    }
}
