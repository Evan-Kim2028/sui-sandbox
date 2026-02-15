//! Shared argv builders for typed workflow replay/analyze steps.
//!
//! Keeping this outside `ReplayOrchestrator` keeps workflow planning concerns
//! distinct from execution/decode orchestration helpers.

use crate::workflow::{WorkflowAnalyzeReplayStep, WorkflowDefaults, WorkflowReplayStep};

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
    if let Some(auto_system_objects) = replay.auto_system_objects.or(defaults.auto_system_objects) {
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
    if let Some(auto_system_objects) = analyze.auto_system_objects.or(defaults.auto_system_objects)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{WorkflowFetchStrategy, WorkflowReplayProfile, WorkflowSource};
    use serde_json::json;

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

        let args = build_replay_command(&defaults, &replay);
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
        let args_default = build_analyze_replay_command(&defaults, &analyze_default);
        assert!(has_flag(&args_default, "--mm2"));

        let analyze_override: WorkflowAnalyzeReplayStep = serde_json::from_value(json!({
            "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "mm2": false
        }))
        .expect("valid analyze step override");
        let args_override = build_analyze_replay_command(&defaults, &analyze_override);
        assert!(!has_flag(&args_override, "--mm2"));
    }
}
