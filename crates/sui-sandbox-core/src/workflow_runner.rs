//! Shared workflow run-loop and report generation.
//!
//! CLI and Python bindings can prepare step commands differently, but both can
//! use this runner to keep stop/continue semantics and report shape aligned.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::workflow::{WorkflowSpec, WorkflowStep};

/// Prepared workflow step metadata plus command build result.
#[derive(Debug, Clone)]
pub struct WorkflowPreparedStep {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub kind: String,
    pub continue_on_error: bool,
    pub command: Result<Vec<String>, String>,
}

impl WorkflowPreparedStep {
    pub fn command_display(&self) -> String {
        match &self.command {
            Ok(argv) if !argv.is_empty() => argv.join(" "),
            Ok(_) => "<empty>".to_string(),
            Err(_) => "<build-error>".to_string(),
        }
    }
}

/// Result returned by a concrete workflow step executor.
#[derive(Debug, Clone, Default)]
pub struct WorkflowStepExecution {
    pub exit_code: i32,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Canonical per-step report entry.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowStepReport {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: String,
    pub command: Vec<String>,
    pub success: bool,
    pub exit_code: i32,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
}

/// Canonical workflow report.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkflowRunReport {
    pub spec_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub dry_run: bool,
    pub total_steps: usize,
    pub succeeded_steps: usize,
    pub failed_steps: usize,
    pub stopped_early: bool,
    pub elapsed_ms: u128,
    pub steps: Vec<WorkflowStepReport>,
}

/// Run prepared workflow steps with shared stop/continue semantics.
pub fn run_prepared_workflow_steps<StartFn, ExecFn>(
    spec_label: String,
    spec: &WorkflowSpec,
    prepared_steps: Vec<WorkflowPreparedStep>,
    dry_run: bool,
    continue_on_error: bool,
    mut on_step_start: StartFn,
    mut execute_step: ExecFn,
) -> WorkflowRunReport
where
    StartFn: FnMut(&WorkflowStep, &WorkflowPreparedStep),
    ExecFn: FnMut(&WorkflowStep, &WorkflowPreparedStep) -> Result<WorkflowStepExecution>,
{
    let started = Instant::now();
    let mut reports = Vec::with_capacity(prepared_steps.len());
    let mut stopped_early = false;

    for prepared in prepared_steps {
        let step_idx = prepared.index.saturating_sub(1);
        let Some(step) = spec.steps.get(step_idx) else {
            reports.push(WorkflowStepReport {
                index: prepared.index,
                id: prepared.id,
                name: prepared.name,
                kind: prepared.kind,
                command: Vec::new(),
                success: false,
                exit_code: -1,
                elapsed_ms: 0,
                error: Some(format!("invalid prepared step index {}", prepared.index)),
                output: None,
            });
            stopped_early = true;
            break;
        };
        let step_started = Instant::now();
        on_step_start(step, &prepared);
        let should_continue = continue_on_error || prepared.continue_on_error;

        let command = match &prepared.command {
            Ok(command) => command.clone(),
            Err(err) => {
                reports.push(WorkflowStepReport {
                    index: prepared.index,
                    id: prepared.id.clone(),
                    name: prepared.name.clone(),
                    kind: prepared.kind.clone(),
                    command: Vec::new(),
                    success: false,
                    exit_code: -1,
                    elapsed_ms: step_started.elapsed().as_millis(),
                    error: Some(format!("failed to build step command: {}", err)),
                    output: None,
                });
                if !should_continue {
                    stopped_early = true;
                    break;
                }
                continue;
            }
        };

        if dry_run {
            reports.push(WorkflowStepReport {
                index: prepared.index,
                id: prepared.id.clone(),
                name: prepared.name.clone(),
                kind: prepared.kind.clone(),
                command,
                success: true,
                exit_code: 0,
                elapsed_ms: step_started.elapsed().as_millis(),
                error: None,
                output: None,
            });
            continue;
        }

        match execute_step(step, &prepared) {
            Ok(executed) => {
                let success = executed.exit_code == 0;
                let error = if success {
                    None
                } else {
                    executed.error.or_else(|| {
                        Some(format!(
                            "step {} failed with exit code {}",
                            prepared.index, executed.exit_code
                        ))
                    })
                };

                reports.push(WorkflowStepReport {
                    index: prepared.index,
                    id: prepared.id.clone(),
                    name: prepared.name.clone(),
                    kind: prepared.kind.clone(),
                    command,
                    success,
                    exit_code: executed.exit_code,
                    elapsed_ms: step_started.elapsed().as_millis(),
                    error,
                    output: executed.output,
                });

                if !(success || should_continue) {
                    stopped_early = true;
                    break;
                }
            }
            Err(err) => {
                reports.push(WorkflowStepReport {
                    index: prepared.index,
                    id: prepared.id.clone(),
                    name: prepared.name.clone(),
                    kind: prepared.kind.clone(),
                    command,
                    success: false,
                    exit_code: -1,
                    elapsed_ms: step_started.elapsed().as_millis(),
                    error: Some(err.to_string()),
                    output: None,
                });
                if !should_continue {
                    stopped_early = true;
                    break;
                }
            }
        }
    }

    let succeeded_steps = reports.iter().filter(|entry| entry.success).count();
    let failed_steps = reports.len().saturating_sub(succeeded_steps);
    WorkflowRunReport {
        spec_file: spec_label,
        name: spec.name.clone(),
        description: spec.description.clone(),
        dry_run,
        total_steps: reports.len(),
        succeeded_steps,
        failed_steps,
        stopped_early,
        elapsed_ms: started.elapsed().as_millis(),
        steps: reports,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{
        WorkflowCommandStep, WorkflowDefaults, WorkflowSpec, WorkflowStep, WorkflowStepAction,
    };

    fn test_spec() -> WorkflowSpec {
        WorkflowSpec {
            version: 1,
            name: Some("runner_test".to_string()),
            description: Some("runner test spec".to_string()),
            defaults: WorkflowDefaults::default(),
            steps: vec![
                WorkflowStep {
                    id: Some("s1".to_string()),
                    name: Some("step1".to_string()),
                    continue_on_error: false,
                    action: WorkflowStepAction::Command(WorkflowCommandStep {
                        args: vec!["status".to_string()],
                    }),
                },
                WorkflowStep {
                    id: Some("s2".to_string()),
                    name: Some("step2".to_string()),
                    continue_on_error: false,
                    action: WorkflowStepAction::Command(WorkflowCommandStep {
                        args: vec!["status".to_string()],
                    }),
                },
            ],
        }
    }

    #[test]
    fn dry_run_skips_executor_calls() {
        let spec = test_spec();
        let prepared = vec![
            WorkflowPreparedStep {
                index: 1,
                id: Some("s1".to_string()),
                name: Some("step1".to_string()),
                kind: "command".to_string(),
                continue_on_error: false,
                command: Ok(vec!["status".to_string()]),
            },
            WorkflowPreparedStep {
                index: 2,
                id: Some("s2".to_string()),
                name: Some("step2".to_string()),
                kind: "command".to_string(),
                continue_on_error: false,
                command: Ok(vec!["status".to_string()]),
            },
        ];

        let mut execute_calls = 0usize;
        let report = run_prepared_workflow_steps(
            "<inline>".to_string(),
            &spec,
            prepared,
            true,
            false,
            |_step, _prepared| {},
            |_step, _prepared| {
                execute_calls += 1;
                Ok(WorkflowStepExecution {
                    exit_code: 0,
                    output: None,
                    error: None,
                })
            },
        );

        assert_eq!(execute_calls, 0);
        assert_eq!(report.total_steps, 2);
        assert_eq!(report.succeeded_steps, 2);
        assert_eq!(report.failed_steps, 0);
        assert!(!report.stopped_early);
    }

    #[test]
    fn stops_early_on_failure_without_continue() {
        let spec = test_spec();
        let prepared = vec![
            WorkflowPreparedStep {
                index: 1,
                id: Some("s1".to_string()),
                name: Some("step1".to_string()),
                kind: "command".to_string(),
                continue_on_error: false,
                command: Ok(vec!["status".to_string()]),
            },
            WorkflowPreparedStep {
                index: 2,
                id: Some("s2".to_string()),
                name: Some("step2".to_string()),
                kind: "command".to_string(),
                continue_on_error: false,
                command: Ok(vec!["status".to_string()]),
            },
        ];

        let mut execute_calls = 0usize;
        let report = run_prepared_workflow_steps(
            "<inline>".to_string(),
            &spec,
            prepared,
            false,
            false,
            |_step, _prepared| {},
            |_step, _prepared| {
                execute_calls += 1;
                if execute_calls == 1 {
                    Ok(WorkflowStepExecution {
                        exit_code: 1,
                        output: None,
                        error: Some("boom".to_string()),
                    })
                } else {
                    Ok(WorkflowStepExecution {
                        exit_code: 0,
                        output: None,
                        error: None,
                    })
                }
            },
        );

        assert_eq!(execute_calls, 1);
        assert_eq!(report.total_steps, 1);
        assert_eq!(report.succeeded_steps, 0);
        assert_eq!(report.failed_steps, 1);
        assert!(report.stopped_early);
        assert_eq!(report.steps[0].error.as_deref(), Some("boom"));
    }
}
