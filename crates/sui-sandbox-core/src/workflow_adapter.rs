use anyhow::{anyhow, Result};

use crate::workflow::{
    WorkflowAnalyzeReplayStep, WorkflowCommandStep, WorkflowDefaults, WorkflowReplayStep,
    WorkflowSource, WorkflowSpec, WorkflowStep, WorkflowStepAction,
};

const DEFAULT_DEMO_DIGEST: &str = "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2";
const DEFAULT_DEMO_CHECKPOINT: u64 = 239615926;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinWorkflowTemplate {
    Generic,
    Cetus,
    Suilend,
    Scallop,
}

impl BuiltinWorkflowTemplate {
    pub fn key(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Cetus => "cetus",
            Self::Suilend => "suilend",
            Self::Scallop => "scallop",
        }
    }

    pub fn protocol_name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Cetus => "cetus",
            Self::Suilend => "suilend",
            Self::Scallop => "scallop",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Generic => {
                "Protocol-agnostic replay/analyze scaffold. Replace digest/checkpoint as needed."
            }
            Self::Cetus => "Cetus-oriented replay/analyze scaffold.",
            Self::Suilend => "Suilend-oriented replay/analyze scaffold.",
            Self::Scallop => "Scallop-oriented replay/analyze scaffold.",
        }
    }

    pub fn default_digest(self) -> &'static str {
        DEFAULT_DEMO_DIGEST
    }

    pub fn default_checkpoint(self) -> u64 {
        DEFAULT_DEMO_CHECKPOINT
    }
}

#[derive(Debug, Clone)]
pub struct BuiltinWorkflowInput {
    pub digest: Option<String>,
    pub checkpoint: Option<u64>,
    pub include_analyze_step: bool,
    pub include_replay_step: bool,
    pub strict_replay: bool,
    pub package_id: Option<String>,
    pub view_objects: Vec<String>,
}

impl BuiltinWorkflowInput {
    pub fn validate(&self) -> Result<()> {
        if self.include_analyze_step || self.include_replay_step {
            let digest = self.digest.as_deref().ok_or_else(|| {
                anyhow!("digest is required when replay/analyze steps are enabled")
            })?;
            if digest.trim().is_empty() {
                return Err(anyhow!("digest cannot be empty"));
            }
            let checkpoint = self.checkpoint.ok_or_else(|| {
                anyhow!("checkpoint is required when replay/analyze steps are enabled")
            })?;
            if checkpoint == 0 {
                return Err(anyhow!("checkpoint must be >= 1"));
            }
        } else if matches!(self.digest.as_deref(), Some(digest) if digest.trim().is_empty()) {
            return Err(anyhow!("digest cannot be empty"));
        } else if self.checkpoint == Some(0) {
            return Err(anyhow!("checkpoint must be >= 1"));
        }
        if let Some(package_id) = self.package_id.as_deref() {
            if package_id.trim().is_empty() {
                return Err(anyhow!("package_id cannot be empty"));
            }
        }
        for object_id in &self.view_objects {
            if object_id.trim().is_empty() {
                return Err(anyhow!("view_objects contains an empty object id"));
            }
        }
        Ok(())
    }
}

pub fn build_builtin_workflow(
    template: BuiltinWorkflowTemplate,
    input: &BuiltinWorkflowInput,
) -> Result<WorkflowSpec> {
    input.validate()?;
    let digest = input
        .digest
        .as_deref()
        .map(str::trim)
        .map(ToOwned::to_owned);
    let checkpoint = input.checkpoint;
    let protocol = template.protocol_name();

    let mut steps = Vec::new();
    if let Some(package_id) = input.package_id.as_deref() {
        let package_id = package_id.trim().to_string();
        steps.push(WorkflowStep {
            id: Some(format!("{protocol}_package")),
            name: Some(format!("{protocol} package interface summary")),
            continue_on_error: false,
            action: WorkflowStepAction::Command(WorkflowCommandStep {
                args: vec![
                    "analyze".to_string(),
                    "package".to_string(),
                    "--package-id".to_string(),
                    package_id,
                    "--list-modules".to_string(),
                ],
            }),
        });
    }

    for (idx, object_id) in input.view_objects.iter().enumerate() {
        steps.push(WorkflowStep {
            id: Some(format!("{protocol}_view_object_{}", idx + 1)),
            name: Some(format!("{protocol} inspect object {}", idx + 1)),
            continue_on_error: true,
            action: WorkflowStepAction::Command(WorkflowCommandStep {
                args: vec![
                    "view".to_string(),
                    "object".to_string(),
                    object_id.trim().to_string(),
                ],
            }),
        });
    }

    if input.include_analyze_step {
        let digest = digest
            .clone()
            .ok_or_else(|| anyhow!("missing digest for analyze step generation"))?;
        let checkpoint =
            checkpoint.ok_or_else(|| anyhow!("missing checkpoint for analyze step generation"))?;
        steps.push(WorkflowStep {
            id: Some(format!("{protocol}_analyze")),
            name: Some(format!("{protocol} analyze replay hydration")),
            continue_on_error: false,
            action: WorkflowStepAction::AnalyzeReplay(WorkflowAnalyzeReplayStep {
                digest,
                checkpoint: Some(checkpoint),
                source: Some(WorkflowSource::Walrus),
                allow_fallback: Some(true),
                auto_system_objects: Some(true),
                no_prefetch: None,
                prefetch_depth: Some(3),
                prefetch_limit: Some(200),
                mm2: None,
            }),
        });
    }

    if input.include_replay_step {
        let digest = digest
            .clone()
            .ok_or_else(|| anyhow!("missing digest for replay step generation"))?;
        let checkpoint =
            checkpoint.ok_or_else(|| anyhow!("missing checkpoint for replay step generation"))?;
        steps.push(WorkflowStep {
            id: Some(format!("{protocol}_replay")),
            name: Some(format!("{protocol} replay execution")),
            continue_on_error: false,
            action: WorkflowStepAction::Replay(WorkflowReplayStep {
                digest: Some(digest),
                checkpoint: Some(checkpoint.to_string()),
                latest: None,
                state_json: None,
                source: Some(WorkflowSource::Walrus),
                profile: None,
                fetch_strategy: None,
                allow_fallback: Some(true),
                auto_system_objects: Some(true),
                no_prefetch: None,
                prefetch_depth: Some(3),
                prefetch_limit: Some(200),
                compare: Some(true),
                strict: Some(input.strict_replay),
                vm_only: None,
                synthesize_missing: None,
                self_heal_dynamic_fields: None,
            }),
        });
    }

    steps.push(WorkflowStep {
        id: Some(format!("{protocol}_status")),
        name: Some("session status".to_string()),
        continue_on_error: false,
        action: WorkflowStepAction::Command(WorkflowCommandStep {
            args: vec!["status".to_string()],
        }),
    });

    Ok(WorkflowSpec {
        version: 1,
        name: Some(format!("{protocol}_replay_analyze")),
        description: Some(format!(
            "{} Generated by workflow template planner.",
            template.description()
        )),
        defaults: WorkflowDefaults {
            source: Some(WorkflowSource::Walrus),
            profile: None,
            fetch_strategy: None,
            allow_fallback: Some(true),
            auto_system_objects: Some(true),
            no_prefetch: None,
            prefetch_depth: Some(3),
            prefetch_limit: Some(200),
            compare: Some(true),
            strict: None,
            vm_only: None,
            synthesize_missing: None,
            self_heal_dynamic_fields: None,
            mm2: None,
        },
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_validates_cetus_template() {
        let spec = build_builtin_workflow(
            BuiltinWorkflowTemplate::Cetus,
            &BuiltinWorkflowInput {
                digest: Some(BuiltinWorkflowTemplate::Cetus.default_digest().to_string()),
                checkpoint: Some(BuiltinWorkflowTemplate::Cetus.default_checkpoint()),
                include_analyze_step: true,
                include_replay_step: true,
                strict_replay: true,
                package_id: None,
                view_objects: Vec::new(),
            },
        )
        .expect("build template");
        spec.validate().expect("valid spec");
        assert_eq!(spec.steps.len(), 3);
    }

    #[test]
    fn rejects_empty_digest() {
        let err = build_builtin_workflow(
            BuiltinWorkflowTemplate::Generic,
            &BuiltinWorkflowInput {
                digest: Some("   ".to_string()),
                checkpoint: Some(1),
                include_analyze_step: true,
                include_replay_step: true,
                strict_replay: true,
                package_id: None,
                view_objects: Vec::new(),
            },
        )
        .expect_err("expected validation error");
        assert!(err.to_string().contains("digest cannot be empty"));
    }

    #[test]
    fn adds_protocol_context_steps_when_inputs_provided() {
        let spec = build_builtin_workflow(
            BuiltinWorkflowTemplate::Suilend,
            &BuiltinWorkflowInput {
                digest: Some(
                    BuiltinWorkflowTemplate::Suilend
                        .default_digest()
                        .to_string(),
                ),
                checkpoint: Some(BuiltinWorkflowTemplate::Suilend.default_checkpoint()),
                include_analyze_step: true,
                include_replay_step: true,
                strict_replay: true,
                package_id: Some("0x2".to_string()),
                view_objects: vec!["0x6".to_string(), "0x8".to_string()],
            },
        )
        .expect("build template with context");
        spec.validate().expect("valid contextual spec");
        assert_eq!(spec.steps.len(), 6);
        let first = &spec.steps[0];
        match &first.action {
            WorkflowStepAction::Command(cmd) => {
                assert_eq!(cmd.args[0], "analyze");
                assert_eq!(cmd.args[1], "package");
            }
            _ => panic!("expected command step"),
        }
    }

    #[test]
    fn supports_scaffold_only_generation_without_digest() {
        let spec = build_builtin_workflow(
            BuiltinWorkflowTemplate::Generic,
            &BuiltinWorkflowInput {
                digest: None,
                checkpoint: None,
                include_analyze_step: false,
                include_replay_step: false,
                strict_replay: true,
                package_id: Some("0x2".to_string()),
                view_objects: vec!["0x6".to_string()],
            },
        )
        .expect("build scaffold-only template");
        spec.validate().expect("valid scaffold-only spec");
        assert_eq!(spec.steps.len(), 3);
    }
}
