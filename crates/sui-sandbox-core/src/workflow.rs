use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const SUPPORTED_WORKFLOW_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSpec {
    #[serde(default = "default_workflow_version")]
    pub version: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub defaults: WorkflowDefaults,
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowDefaults {
    #[serde(default)]
    pub source: Option<WorkflowSource>,
    #[serde(default)]
    pub profile: Option<WorkflowReplayProfile>,
    #[serde(default)]
    pub fetch_strategy: Option<WorkflowFetchStrategy>,
    #[serde(default)]
    pub allow_fallback: Option<bool>,
    #[serde(default)]
    pub auto_system_objects: Option<bool>,
    #[serde(default)]
    pub no_prefetch: Option<bool>,
    #[serde(default)]
    pub prefetch_depth: Option<usize>,
    #[serde(default)]
    pub prefetch_limit: Option<usize>,
    #[serde(default)]
    pub compare: Option<bool>,
    #[serde(default)]
    pub strict: Option<bool>,
    #[serde(default)]
    pub vm_only: Option<bool>,
    #[serde(default)]
    pub synthesize_missing: Option<bool>,
    #[serde(default)]
    pub self_heal_dynamic_fields: Option<bool>,
    #[serde(default)]
    pub mm2: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub continue_on_error: bool,
    #[serde(flatten)]
    pub action: WorkflowStepAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowStepAction {
    Replay(WorkflowReplayStep),
    AnalyzeReplay(WorkflowAnalyzeReplayStep),
    Command(WorkflowCommandStep),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowReplayStep {
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub latest: Option<u64>,
    #[serde(default)]
    pub state_json: Option<PathBuf>,
    #[serde(default)]
    pub source: Option<WorkflowSource>,
    #[serde(default)]
    pub profile: Option<WorkflowReplayProfile>,
    #[serde(default)]
    pub fetch_strategy: Option<WorkflowFetchStrategy>,
    #[serde(default)]
    pub allow_fallback: Option<bool>,
    #[serde(default)]
    pub auto_system_objects: Option<bool>,
    #[serde(default)]
    pub no_prefetch: Option<bool>,
    #[serde(default)]
    pub prefetch_depth: Option<usize>,
    #[serde(default)]
    pub prefetch_limit: Option<usize>,
    #[serde(default)]
    pub compare: Option<bool>,
    #[serde(default)]
    pub strict: Option<bool>,
    #[serde(default)]
    pub vm_only: Option<bool>,
    #[serde(default)]
    pub synthesize_missing: Option<bool>,
    #[serde(default)]
    pub self_heal_dynamic_fields: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowAnalyzeReplayStep {
    pub digest: String,
    #[serde(default)]
    pub checkpoint: Option<u64>,
    #[serde(default)]
    pub source: Option<WorkflowSource>,
    #[serde(default)]
    pub allow_fallback: Option<bool>,
    #[serde(default)]
    pub auto_system_objects: Option<bool>,
    #[serde(default)]
    pub no_prefetch: Option<bool>,
    #[serde(default)]
    pub prefetch_depth: Option<usize>,
    #[serde(default)]
    pub prefetch_limit: Option<usize>,
    #[serde(default)]
    pub mm2: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowCommandStep {
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSource {
    Grpc,
    Walrus,
    Hybrid,
    Local,
}

impl WorkflowSource {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Grpc => "grpc",
            Self::Walrus => "walrus",
            Self::Hybrid => "hybrid",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReplayProfile {
    Safe,
    Balanced,
    Fast,
}

impl WorkflowReplayProfile {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Balanced => "balanced",
            Self::Fast => "fast",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowFetchStrategy {
    Eager,
    Full,
}

impl WorkflowFetchStrategy {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Eager => "eager",
            Self::Full => "full",
        }
    }
}

impl WorkflowSpec {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("Failed to read workflow spec {}", path.display()))?;
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let spec = if ext == "yaml" || ext == "yml" {
            serde_yaml::from_str::<Self>(&raw)
                .with_context(|| format!("Invalid YAML workflow spec in {}", path.display()))?
        } else {
            serde_json::from_str::<Self>(&raw)
                .with_context(|| format!("Invalid JSON workflow spec in {}", path.display()))?
        };

        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<()> {
        let mut issues = Vec::new();
        self.collect_validation_issues(&mut issues);
        if issues.is_empty() {
            return Ok(());
        }

        let formatted = issues
            .into_iter()
            .enumerate()
            .map(|(idx, issue)| format!("  {}. {}", idx + 1, issue))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("Workflow spec validation failed:\n{formatted}");
    }

    fn collect_validation_issues(&self, issues: &mut Vec<String>) {
        if self.version != SUPPORTED_WORKFLOW_VERSION {
            issues.push(format!(
                "version {} is not supported (expected {})",
                self.version, SUPPORTED_WORKFLOW_VERSION
            ));
        }
        if matches!(self.name.as_deref(), Some(name) if name.trim().is_empty()) {
            issues.push("name cannot be empty".to_string());
        }
        if matches!(self.description.as_deref(), Some(description) if description.trim().is_empty())
        {
            issues.push("description cannot be empty".to_string());
        }
        if self.steps.is_empty() {
            issues.push("steps must contain at least one entry".to_string());
        }

        validate_true_only("defaults.compare", self.defaults.compare, issues);
        validate_true_only("defaults.strict", self.defaults.strict, issues);
        validate_true_only("defaults.vm_only", self.defaults.vm_only, issues);
        validate_true_only("defaults.no_prefetch", self.defaults.no_prefetch, issues);
        validate_true_only(
            "defaults.synthesize_missing",
            self.defaults.synthesize_missing,
            issues,
        );
        validate_true_only(
            "defaults.self_heal_dynamic_fields",
            self.defaults.self_heal_dynamic_fields,
            issues,
        );
        validate_true_only("defaults.mm2", self.defaults.mm2, issues);

        let mut seen_step_ids = HashSet::new();
        for (idx, step) in self.steps.iter().enumerate() {
            let step_number = idx + 1;
            let step_label = format_step_label(step, step_number);

            if matches!(step.id.as_deref(), Some(id) if id.trim().is_empty()) {
                issues.push(format!("step {step_number} has an empty `id`"));
            }
            if matches!(step.name.as_deref(), Some(name) if name.trim().is_empty()) {
                issues.push(format!("step {step_number} has an empty `name`"));
            }
            if let Some(step_id) = step.id.as_deref() {
                if !step_id.trim().is_empty() && !seen_step_ids.insert(step_id.to_string()) {
                    issues.push(format!("duplicate step id `{step_id}`"));
                }
            }

            match &step.action {
                WorkflowStepAction::Replay(replay) => {
                    let has_digest = replay
                        .digest
                        .as_deref()
                        .is_some_and(|digest| !digest.trim().is_empty());
                    if matches!(replay.digest.as_deref(), Some(digest) if digest.trim().is_empty())
                    {
                        issues.push(format!("{step_label}: replay `digest` cannot be empty"));
                    }
                    if !has_digest
                        && replay.checkpoint.is_none()
                        && replay.latest.is_none()
                        && replay.state_json.is_none()
                    {
                        issues.push(format!(
                            "{step_label}: replay step must set at least one of `digest`, `checkpoint`, `latest`, or `state_json`"
                        ));
                    }
                    if replay.latest == Some(0) {
                        issues.push(format!("{step_label}: replay `latest` must be >= 1"));
                    }
                    if replay.latest.is_some() && replay.checkpoint.is_some() {
                        issues.push(format!(
                            "{step_label}: replay cannot set both `latest` and `checkpoint`"
                        ));
                    }
                    if replay.state_json.is_some() && replay.latest.is_some() {
                        issues.push(format!(
                            "{step_label}: replay cannot set both `state_json` and `latest`"
                        ));
                    }
                    if replay.state_json.is_some() && replay.checkpoint.is_some() {
                        issues.push(format!(
                            "{step_label}: replay cannot set both `state_json` and `checkpoint`"
                        ));
                    }
                    validate_true_only(&format!("{step_label}.compare"), replay.compare, issues);
                    validate_true_only(&format!("{step_label}.strict"), replay.strict, issues);
                    validate_true_only(&format!("{step_label}.vm_only"), replay.vm_only, issues);
                    validate_true_only(
                        &format!("{step_label}.no_prefetch"),
                        replay.no_prefetch,
                        issues,
                    );
                    validate_true_only(
                        &format!("{step_label}.synthesize_missing"),
                        replay.synthesize_missing,
                        issues,
                    );
                    validate_true_only(
                        &format!("{step_label}.self_heal_dynamic_fields"),
                        replay.self_heal_dynamic_fields,
                        issues,
                    );
                }
                WorkflowStepAction::AnalyzeReplay(analyze) => {
                    if analyze.digest.trim().is_empty() {
                        issues.push(format!(
                            "{step_label}: analyze_replay `digest` cannot be empty"
                        ));
                    }
                    if analyze.checkpoint == Some(0) {
                        issues.push(format!(
                            "{step_label}: analyze_replay `checkpoint` must be >= 1"
                        ));
                    }
                    validate_true_only(&format!("{step_label}.mm2"), analyze.mm2, issues);
                    validate_true_only(
                        &format!("{step_label}.no_prefetch"),
                        analyze.no_prefetch,
                        issues,
                    );
                }
                WorkflowStepAction::Command(command) => {
                    if command.args.is_empty() {
                        issues.push(format!(
                            "{step_label}: command step requires non-empty `args`"
                        ));
                    } else if let Err(err) = normalize_command_args(&command.args) {
                        issues.push(format!("{step_label}: {err}"));
                    }
                }
            }
        }
    }
}

fn format_step_label(step: &WorkflowStep, index: usize) -> String {
    if let Some(id) = step.id.as_deref() {
        if !id.trim().is_empty() {
            return format!("step {index} (`{id}`)");
        }
    }
    if let Some(name) = step.name.as_deref() {
        if !name.trim().is_empty() {
            return format!("step {index} (`{name}`)");
        }
    }
    format!("step {index}")
}

fn validate_true_only(field: &str, value: Option<bool>, issues: &mut Vec<String>) {
    if value == Some(false) {
        issues.push(format!(
            "{field} only supports `true` (omit the field for default false)"
        ));
    }
}

fn default_workflow_version() -> u32 {
    SUPPORTED_WORKFLOW_VERSION
}

pub fn normalize_command_args(args: &[String]) -> Result<Vec<String>> {
    if args.is_empty() {
        return Err(anyhow!("command step args cannot be empty"));
    }
    let mut normalized = args.to_vec();
    if normalized.first().is_some_and(|arg| arg == "sui-sandbox") {
        normalized.remove(0);
    }
    if normalized.is_empty() {
        return Err(anyhow!(
            "command step args became empty after removing leading `sui-sandbox`"
        ));
    }
    if normalized.first().is_some_and(|arg| arg == "workflow") {
        return Err(anyhow!(
            "workflow command recursion is not allowed in command steps"
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimal_replay_step() {
        let spec = WorkflowSpec {
            version: SUPPORTED_WORKFLOW_VERSION,
            name: Some("smoke".to_string()),
            description: None,
            defaults: WorkflowDefaults::default(),
            steps: vec![WorkflowStep {
                id: Some("replay-1".to_string()),
                name: Some("Replay tx".to_string()),
                continue_on_error: false,
                action: WorkflowStepAction::Replay(WorkflowReplayStep {
                    digest: Some("9V3xKMn".to_string()),
                    checkpoint: None,
                    latest: None,
                    state_json: None,
                    source: None,
                    profile: None,
                    fetch_strategy: None,
                    allow_fallback: None,
                    auto_system_objects: None,
                    no_prefetch: None,
                    prefetch_depth: None,
                    prefetch_limit: None,
                    compare: Some(true),
                    strict: None,
                    vm_only: None,
                    synthesize_missing: None,
                    self_heal_dynamic_fields: None,
                }),
            }],
        };

        assert!(spec.validate().is_ok());
    }

    #[test]
    fn rejects_duplicate_step_ids() {
        let spec = WorkflowSpec {
            version: SUPPORTED_WORKFLOW_VERSION,
            name: None,
            description: None,
            defaults: WorkflowDefaults::default(),
            steps: vec![
                WorkflowStep {
                    id: Some("dup".to_string()),
                    name: None,
                    continue_on_error: false,
                    action: WorkflowStepAction::Command(WorkflowCommandStep {
                        args: vec!["status".to_string()],
                    }),
                },
                WorkflowStep {
                    id: Some("dup".to_string()),
                    name: None,
                    continue_on_error: false,
                    action: WorkflowStepAction::Command(WorkflowCommandStep {
                        args: vec!["status".to_string()],
                    }),
                },
            ],
        };

        let err = spec.validate().expect_err("expected duplicate id error");
        assert!(err.to_string().contains("duplicate step id"));
    }

    #[test]
    fn rejects_false_for_positive_only_flags() {
        let spec = WorkflowSpec {
            version: SUPPORTED_WORKFLOW_VERSION,
            name: None,
            description: None,
            defaults: WorkflowDefaults {
                compare: Some(false),
                ..WorkflowDefaults::default()
            },
            steps: vec![WorkflowStep {
                id: None,
                name: None,
                continue_on_error: false,
                action: WorkflowStepAction::Replay(WorkflowReplayStep {
                    digest: Some("tx".to_string()),
                    checkpoint: None,
                    latest: None,
                    state_json: None,
                    source: None,
                    profile: None,
                    fetch_strategy: None,
                    allow_fallback: None,
                    auto_system_objects: None,
                    no_prefetch: None,
                    prefetch_depth: None,
                    prefetch_limit: None,
                    compare: None,
                    strict: None,
                    vm_only: None,
                    synthesize_missing: None,
                    self_heal_dynamic_fields: None,
                }),
            }],
        };

        let err = spec
            .validate()
            .expect_err("expected false positive flag validation failure");
        assert!(err.to_string().contains("defaults.compare"));
    }
}
