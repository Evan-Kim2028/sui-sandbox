use anyhow::{anyhow, Result};

use crate::orchestrator::ReplayOrchestrator;
use crate::workflow::{
    normalize_command_args, WorkflowAnalyzeReplayStep, WorkflowDefaults, WorkflowFetchStrategy,
    WorkflowReplayProfile, WorkflowReplayStep, WorkflowStep, WorkflowStepAction,
};
use crate::workflow_adapter::BuiltinWorkflowTemplate;

#[derive(Debug, Clone)]
pub struct WorkflowTemplateInference {
    pub template: BuiltinWorkflowTemplate,
    pub confidence: &'static str,
    pub source: &'static str,
    pub reason: Option<String>,
}

pub fn parse_builtin_workflow_template(template: &str) -> Result<BuiltinWorkflowTemplate> {
    match template.trim().to_ascii_lowercase().as_str() {
        "generic" => Ok(BuiltinWorkflowTemplate::Generic),
        "cetus" => Ok(BuiltinWorkflowTemplate::Cetus),
        "suilend" => Ok(BuiltinWorkflowTemplate::Suilend),
        "scallop" => Ok(BuiltinWorkflowTemplate::Scallop),
        other => Err(anyhow!(
            "invalid template '{}': expected one of generic, cetus, suilend, scallop",
            other
        )),
    }
}

pub fn infer_workflow_template_from_modules(module_names: &[String]) -> WorkflowTemplateInference {
    if module_names.is_empty() {
        return WorkflowTemplateInference {
            template: BuiltinWorkflowTemplate::Generic,
            confidence: "low",
            source: "fallback",
            reason: Some("no module names available from package probe".to_string()),
        };
    }

    let cetus_keywords = ["cetus", "clmm", "dlmm", "pool_script", "position_manager"];
    let suilend_keywords = ["suilend", "lending", "reserve", "obligation", "liquidation"];
    let scallop_keywords = ["scallop", "scoin", "spool", "collateral", "market"];

    let mut cetus_score = 0usize;
    let mut suilend_score = 0usize;
    let mut scallop_score = 0usize;
    for name in module_names.iter().map(|value| value.to_ascii_lowercase()) {
        if cetus_keywords.iter().any(|kw| name.contains(kw)) {
            cetus_score += 1;
        }
        if suilend_keywords.iter().any(|kw| name.contains(kw)) {
            suilend_score += 1;
        }
        if scallop_keywords.iter().any(|kw| name.contains(kw)) {
            scallop_score += 1;
        }
    }

    let mut ranked = [
        (BuiltinWorkflowTemplate::Cetus, cetus_score),
        (BuiltinWorkflowTemplate::Suilend, suilend_score),
        (BuiltinWorkflowTemplate::Scallop, scallop_score),
        (BuiltinWorkflowTemplate::Generic, 0usize),
    ];
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let (top_template, top_score) = ranked[0];
    let second_score = ranked[1].1;

    if top_score == 0 {
        return WorkflowTemplateInference {
            template: BuiltinWorkflowTemplate::Generic,
            confidence: "low",
            source: "module_probe",
            reason: Some("no template keyword matches found in module names".to_string()),
        };
    }
    if top_score == second_score {
        return WorkflowTemplateInference {
            template: BuiltinWorkflowTemplate::Generic,
            confidence: "low",
            source: "module_probe",
            reason: Some(format!(
                "ambiguous module matches (cetus={}, suilend={}, scallop={})",
                cetus_score, suilend_score, scallop_score
            )),
        };
    }

    let confidence = if top_score >= 4 {
        "high"
    } else if top_score >= 2 {
        "medium"
    } else {
        "low"
    };
    WorkflowTemplateInference {
        template: top_template,
        confidence,
        source: "module_probe",
        reason: Some(format!(
            "module keyword matches: cetus={}, suilend={}, scallop={}",
            cetus_score, suilend_score, scallop_score
        )),
    }
}

pub fn short_package_id(package_id: &str) -> String {
    let trimmed = package_id.trim_start_matches("0x");
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.chars().take(12).collect()
    }
}

pub fn workflow_step_kind(action: &WorkflowStepAction) -> &'static str {
    match action {
        WorkflowStepAction::Replay(_) => "replay",
        WorkflowStepAction::AnalyzeReplay(_) => "analyze_replay",
        WorkflowStepAction::Command(_) => "command",
    }
}

pub fn workflow_step_label(step: &WorkflowStep, index: usize) -> String {
    if let Some(id) = step.id.as_deref() {
        if !id.trim().is_empty() {
            return format!("{index}:{id}");
        }
    }
    if let Some(name) = step.name.as_deref() {
        if !name.trim().is_empty() {
            return format!("{index}:{name}");
        }
    }
    index.to_string()
}

pub fn workflow_first_nonempty_output_line(bytes: &[u8]) -> Option<String> {
    const MAX_LEN: usize = 240;
    let text = String::from_utf8_lossy(bytes);
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)?;

    if line.chars().count() > MAX_LEN {
        let truncated: String = line.chars().take(MAX_LEN).collect();
        return Some(format!("{truncated}..."));
    }
    Some(line)
}

pub fn summarize_failure_output(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    workflow_first_nonempty_output_line(stderr)
        .or_else(|| workflow_first_nonempty_output_line(stdout))
}

pub fn workflow_build_replay_command(
    defaults: &WorkflowDefaults,
    replay: &WorkflowReplayStep,
) -> Vec<String> {
    ReplayOrchestrator::build_replay_command(defaults, replay)
}

pub fn workflow_build_analyze_replay_command(
    defaults: &WorkflowDefaults,
    analyze: &WorkflowAnalyzeReplayStep,
) -> Vec<String> {
    ReplayOrchestrator::build_analyze_replay_command(defaults, analyze)
}

pub fn workflow_build_step_command(
    defaults: &WorkflowDefaults,
    step: &WorkflowStep,
) -> Result<Vec<String>> {
    match &step.action {
        WorkflowStepAction::Replay(replay) => Ok(workflow_build_replay_command(defaults, replay)),
        WorkflowStepAction::AnalyzeReplay(analyze) => {
            Ok(workflow_build_analyze_replay_command(defaults, analyze))
        }
        WorkflowStepAction::Command(command) => normalize_command_args(&command.args),
    }
}

pub fn parse_workflow_profile(value: Option<&str>) -> Result<WorkflowReplayProfile> {
    let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(WorkflowReplayProfile::Balanced);
    };
    match raw.to_ascii_lowercase().as_str() {
        "safe" => Ok(WorkflowReplayProfile::Safe),
        "balanced" => Ok(WorkflowReplayProfile::Balanced),
        "fast" => Ok(WorkflowReplayProfile::Fast),
        other => Err(anyhow!(
            "invalid profile `{}` (expected one of: safe, balanced, fast)",
            other
        )),
    }
}

pub fn parse_workflow_fetch_strategy(value: Option<&str>) -> Result<WorkflowFetchStrategy> {
    let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(WorkflowFetchStrategy::Full);
    };
    match raw.to_ascii_lowercase().as_str() {
        "eager" => Ok(WorkflowFetchStrategy::Eager),
        "full" => Ok(WorkflowFetchStrategy::Full),
        other => Err(anyhow!(
            "invalid fetch_strategy `{}` (expected one of: eager, full)",
            other
        )),
    }
}

pub fn profile_env_defaults(
    profile: WorkflowReplayProfile,
) -> &'static [(&'static str, &'static str)] {
    match profile {
        WorkflowReplayProfile::Safe => &[
            ("SUI_CHECKPOINT_LOOKUP_GRAPHQL", "1"),
            ("SUI_PACKAGE_LOOKUP_GRAPHQL", "1"),
            ("SUI_OBJECT_FETCH_CONCURRENCY", "8"),
            ("SUI_PACKAGE_FETCH_CONCURRENCY", "4"),
            ("SUI_PACKAGE_FETCH_PARALLEL", "1"),
        ],
        WorkflowReplayProfile::Balanced => &[],
        WorkflowReplayProfile::Fast => &[
            ("SUI_CHECKPOINT_LOOKUP_GRAPHQL", "0"),
            ("SUI_PACKAGE_LOOKUP_GRAPHQL", "0"),
            ("SUI_OBJECT_FETCH_CONCURRENCY", "32"),
            ("SUI_PACKAGE_FETCH_CONCURRENCY", "16"),
            ("SUI_PACKAGE_FETCH_PARALLEL", "1"),
        ],
    }
}

pub struct WorkflowEnvGuard {
    previous: Vec<(String, Option<String>)>,
}

impl Drop for WorkflowEnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

pub fn apply_workflow_profile_env(profile: WorkflowReplayProfile) -> WorkflowEnvGuard {
    let mut previous = Vec::new();
    for (key, value) in profile_env_defaults(profile) {
        if std::env::var(key).is_err() {
            previous.push(((*key).to_string(), None));
            std::env::set_var(key, value);
        }
    }
    WorkflowEnvGuard { previous }
}
