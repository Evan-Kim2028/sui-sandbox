//! Task-oriented workflow commands.
//!
//! - `init`: scaffold reproducible flow templates
//! - `run-flow`: execute YAML-defined command steps deterministically

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(about = "Scaffold a task-oriented workflow template")]
pub struct InitCmd {
    /// Template name (currently: quickstart)
    #[arg(long, default_value = "quickstart")]
    pub example: String,

    /// Output directory for generated files
    #[arg(long, default_value = ".")]
    pub output_dir: PathBuf,

    /// Overwrite existing files
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Parser, Debug)]
#[command(about = "Execute a YAML flow file")]
pub struct RunFlowCmd {
    /// Path to flow YAML file
    pub flow_file: PathBuf,

    /// Print commands without executing
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Continue executing later steps even when one fails
    #[arg(long, default_value_t = false)]
    pub continue_on_error: bool,
}

#[derive(Debug, Deserialize)]
struct FlowFile {
    version: u32,
    name: Option<String>,
    description: Option<String>,
    steps: Vec<FlowStep>,
}

#[derive(Debug, Deserialize)]
struct FlowStep {
    name: Option<String>,
    command: Vec<String>,
    #[serde(default)]
    continue_on_error: bool,
}

#[derive(Debug, Serialize)]
struct FlowRunReport {
    flow_file: String,
    name: Option<String>,
    description: Option<String>,
    dry_run: bool,
    total_steps: usize,
    succeeded_steps: usize,
    failed_steps: usize,
    elapsed_ms: u128,
    steps: Vec<FlowStepReport>,
}

#[derive(Debug, Serialize, Clone)]
struct FlowStepReport {
    index: usize,
    name: Option<String>,
    command: Vec<String>,
    success: bool,
    exit_code: i32,
    elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl InitCmd {
    pub async fn execute(&self) -> Result<()> {
        if self.example != "quickstart" {
            return Err(anyhow!(
                "Unknown template '{}'. Supported templates: quickstart",
                self.example
            ));
        }

        fs::create_dir_all(&self.output_dir).with_context(|| {
            format!(
                "Failed to create output directory {}",
                self.output_dir.display()
            )
        })?;

        let flow_path = self.output_dir.join("flow.quickstart.yaml");
        let readme_path = self.output_dir.join("FLOW_README.md");

        write_template(
            &flow_path,
            QUICKSTART_FLOW_TEMPLATE,
            self.force,
            "flow template",
        )?;
        write_template(
            &readme_path,
            QUICKSTART_FLOW_README,
            self.force,
            "flow guide",
        )?;

        println!(
            "Initialized workflow template at {}",
            self.output_dir.display()
        );
        println!("  - {}", flow_path.display());
        println!("  - {}", readme_path.display());
        println!(
            "\nRun with:\n  sui-sandbox run-flow {}",
            flow_path.display()
        );

        Ok(())
    }
}

impl RunFlowCmd {
    pub async fn execute(
        &self,
        state_file: &Path,
        rpc_url: &str,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let raw = fs::read_to_string(&self.flow_file)
            .with_context(|| format!("Failed to read flow file {}", self.flow_file.display()))?;
        let flow: FlowFile = serde_yaml::from_str(&raw)
            .with_context(|| format!("Invalid flow YAML in {}", self.flow_file.display()))?;

        if flow.version != 1 {
            return Err(anyhow!(
                "Unsupported flow version {} (expected 1)",
                flow.version
            ));
        }
        if flow.steps.is_empty() {
            return Err(anyhow!("Flow has no steps"));
        }

        if let Some(name) = flow.name.as_deref() {
            println!("Flow: {name}");
        }
        if let Some(description) = flow.description.as_deref() {
            println!("Description: {description}");
        }

        let start = Instant::now();
        let mut reports = Vec::with_capacity(flow.steps.len());

        for (idx, step) in flow.steps.iter().enumerate() {
            let step_name = step.name.clone();
            if step.command.is_empty() {
                return Err(anyhow!("Step {} has empty command", idx + 1));
            }
            if step.command.first().is_some_and(|c| c == "run-flow") {
                return Err(anyhow!(
                    "Step {} uses run-flow recursively; this is not allowed",
                    idx + 1
                ));
            }

            let step_start = Instant::now();
            let display_cmd = step.command.join(" ");
            if !json_output {
                match step_name.as_deref() {
                    Some(name) => println!("[flow:{}:{}] {}", idx + 1, name, display_cmd),
                    None => println!("[flow:{}] {}", idx + 1, display_cmd),
                }
            }

            if self.dry_run {
                reports.push(FlowStepReport {
                    index: idx + 1,
                    name: step_name.clone(),
                    command: step.command.clone(),
                    success: true,
                    exit_code: 0,
                    elapsed_ms: step_start.elapsed().as_millis(),
                    error: None,
                });
                continue;
            }

            let exe = std::env::current_exe().context("Failed to resolve current executable")?;
            let mut cmd = Command::new(exe);
            cmd.arg("--state-file")
                .arg(state_file)
                .arg("--rpc-url")
                .arg(rpc_url);
            if verbose {
                cmd.arg("--verbose");
            }
            cmd.args(&step.command);

            let output = cmd
                .output()
                .with_context(|| format!("Failed to execute step {}", idx + 1))?;

            let ok = output.status.success();
            let code = output.status.code().unwrap_or(-1);
            if !json_output {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stdout.trim().is_empty() {
                    print!("{}", stdout);
                }
                if !stderr.trim().is_empty() {
                    eprint!("{}", stderr);
                }
            }

            let err = if ok {
                None
            } else {
                Some(format!("step {} failed with exit code {}", idx + 1, code))
            };
            reports.push(FlowStepReport {
                index: idx + 1,
                name: step_name.clone(),
                command: step.command.clone(),
                success: ok,
                exit_code: code,
                elapsed_ms: step_start.elapsed().as_millis(),
                error: err.clone(),
            });

            if !(ok || self.continue_on_error || step.continue_on_error) {
                let report = build_report(
                    &self.flow_file,
                    &flow,
                    self.dry_run,
                    &reports,
                    start.elapsed(),
                );
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "Flow stopped at step {} (use --continue-on-error to continue)",
                        idx + 1
                    );
                }
                return Err(anyhow!(
                    "flow execution failed at step {} ({})",
                    idx + 1,
                    display_cmd
                ));
            }
        }

        let report = build_report(
            &self.flow_file,
            &flow,
            self.dry_run,
            &reports,
            start.elapsed(),
        );
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "Flow complete: {}/{} succeeded ({} failed)",
                report.succeeded_steps, report.total_steps, report.failed_steps
            );
        }

        if report.failed_steps > 0 {
            Err(anyhow!(
                "flow completed with {} failed step(s)",
                report.failed_steps
            ))
        } else {
            Ok(())
        }
    }
}

fn build_report(
    flow_file: &Path,
    flow: &FlowFile,
    dry_run: bool,
    reports: &[FlowStepReport],
    elapsed: std::time::Duration,
) -> FlowRunReport {
    let succeeded = reports.iter().filter(|r| r.success).count();
    let failed = reports.len().saturating_sub(succeeded);
    FlowRunReport {
        flow_file: flow_file.display().to_string(),
        name: flow.name.clone(),
        description: flow.description.clone(),
        dry_run,
        total_steps: reports.len(),
        succeeded_steps: succeeded,
        failed_steps: failed,
        elapsed_ms: elapsed.as_millis(),
        steps: reports.to_vec(),
    }
}

fn write_template(path: &Path, contents: &str, force: bool, label: &str) -> Result<()> {
    if path.exists() && !force {
        return Err(anyhow!(
            "Refusing to overwrite existing {} at {} (pass --force)",
            label,
            path.display()
        ));
    }
    fs::write(path, contents).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

const QUICKSTART_FLOW_TEMPLATE: &str = r#"version: 1
name: quickstart
steps:
  - name: show-status
    command: ["status"]
  - name: publish-fixture
    command:
      [
        "publish",
        "tests/fixture/build/fixture",
        "--bytecode-only",
        "--address",
        "fixture=0x100",
      ]
  - name: list-packages
    command: ["view", "packages"]
"#;

const QUICKSTART_FLOW_README: &str = r#"# Flow Quickstart

This directory was scaffolded by `sui-sandbox init --example quickstart`.

## Run

```bash
sui-sandbox run-flow flow.quickstart.yaml
```

## Notes

- Flow version is pinned to `version: 1`.
- Each step executes exactly one `sui-sandbox` command.
- `command` is a YAML string array (argv style), not shell text.
"#;
