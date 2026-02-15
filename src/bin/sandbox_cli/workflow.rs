//! Structured workflow command.
//!
//! This command executes a typed workflow spec focused on replay/analyze sequences.
//! It complements `run-flow` (raw argv YAML steps) with a reusable schema that can
//! be shared by future protocol adapters.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::{Args, Parser, Subcommand, ValueEnum};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use sui_sandbox_core::checkpoint_discovery::{
    build_walrus_client as core_build_walrus_client,
    discover_checkpoint_targets as core_discover_checkpoint_targets,
    WalrusArchiveNetwork as CoreWalrusArchiveNetwork,
};
use sui_sandbox_core::utilities::unresolved_package_dependencies_for_modules;
use sui_sandbox_core::workflow::{WorkflowSpec, WorkflowStepAction};
use sui_sandbox_core::workflow_adapter::{
    build_builtin_workflow, BuiltinWorkflowInput, BuiltinWorkflowTemplate,
};
use sui_sandbox_core::workflow_planner::{
    infer_workflow_template_from_modules as core_infer_workflow_template_from_modules,
    short_package_id as core_short_package_id,
    summarize_failure_output as core_summarize_failure_output,
    workflow_build_step_command as core_workflow_build_step_command,
    workflow_step_kind as core_workflow_step_kind, workflow_step_label as core_workflow_step_label,
    WorkflowTemplateInference as CoreWorkflowTemplateInference,
};
use sui_sandbox_core::workflow_runner::{
    run_prepared_workflow_steps, WorkflowPreparedStep, WorkflowRunReport, WorkflowStepExecution,
};
use sui_transport::decode_graphql_modules;
use sui_transport::graphql::GraphQLClient;

use super::fetch::{fetch_package_with_bytecodes_into_state, PackageInfo};
use super::network::resolve_graphql_endpoint;
use super::SandboxState;

mod native_exec;
#[cfg(feature = "analysis")]
use native_exec::execute_workflow_analyze_step_native;
use native_exec::execute_workflow_replay_step_native;
#[cfg(test)]
use native_exec::parse_replay_cli_from_workflow_argv;

#[derive(Parser, Debug)]
#[command(about = "Validate or run typed pipeline specs (workflow alias)")]
pub struct WorkflowCmd {
    #[command(subcommand)]
    command: WorkflowSubcommand,
}

#[derive(Subcommand, Debug)]
enum WorkflowSubcommand {
    /// Generate a typed workflow spec from built-in protocol templates
    Init(WorkflowInitCmd),
    /// Auto-generate a draft adapter workflow from a package id
    Auto(WorkflowAutoCmd),
    /// Validate a workflow JSON/YAML spec file
    Validate(WorkflowValidateCmd),
    /// Run a workflow spec deterministically
    Run(WorkflowRunCmd),
}

#[derive(Args, Debug)]
pub struct WorkflowInitCmd {
    /// Load workflow-init options from a JSON/YAML config file
    #[arg(long = "from-config")]
    pub from_config: Option<PathBuf>,

    /// Built-in template family to generate
    #[arg(long, value_enum)]
    pub template: Option<WorkflowTemplateArg>,

    /// Output path for generated workflow spec
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Output workflow spec format
    #[arg(long, value_enum)]
    pub format: Option<WorkflowSpecFormat>,

    /// Transaction digest to seed template replay/analyze steps
    #[arg(long)]
    pub digest: Option<String>,

    /// Checkpoint number for replay/analyze steps
    #[arg(long)]
    pub checkpoint: Option<u64>,

    /// Skip the analyze_replay step in the generated workflow
    #[arg(long, default_value_t = false)]
    pub no_analyze: bool,

    /// Disable strict replay failure mode in generated workflow
    #[arg(long, default_value_t = false)]
    pub no_strict: bool,

    /// Override generated workflow name
    #[arg(long)]
    pub name: Option<String>,

    /// Package id to include as an `analyze package` step in the generated workflow
    #[arg(long)]
    pub package_id: Option<String>,

    /// Object id to include as a `view object` step (repeatable)
    #[arg(long = "view-object")]
    pub view_objects: Vec<String>,

    /// Overwrite output file if it exists
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct WorkflowAutoCmd {
    /// Package id to build a draft adapter scaffold for
    #[arg(long)]
    pub package_id: String,

    /// Optional template override (otherwise inferred from package modules)
    #[arg(long, value_enum)]
    pub template: Option<WorkflowTemplateArg>,

    /// Output path for generated workflow spec
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Output workflow spec format
    #[arg(long, value_enum)]
    pub format: Option<WorkflowSpecFormat>,

    /// Seed digest to include replay/analyze replay steps
    #[arg(long)]
    pub digest: Option<String>,

    /// Auto-discover replay digest/checkpoint from latest N checkpoints for --package-id
    #[arg(long, conflicts_with_all = ["digest", "checkpoint"])]
    pub discover_latest: Option<u64>,

    /// Checkpoint for replay/analyze replay steps
    #[arg(long)]
    pub checkpoint: Option<u64>,

    /// Walrus archive network used for --discover-latest
    #[arg(long, value_enum, default_value = "mainnet")]
    pub walrus_network: WorkflowWalrusNetwork,

    /// Override Walrus caching endpoint (requires --walrus-aggregator-url)
    #[arg(long)]
    pub walrus_caching_url: Option<String>,

    /// Override Walrus aggregator endpoint (requires --walrus-caching-url)
    #[arg(long)]
    pub walrus_aggregator_url: Option<String>,

    /// Override generated workflow name
    #[arg(long)]
    pub name: Option<String>,

    /// Overwrite output file if it exists
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Emit workflow scaffold even when dependency-closure validation fails
    #[arg(long, default_value_t = false)]
    pub best_effort: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowTemplateArg {
    Generic,
    Cetus,
    Suilend,
    Scallop,
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSpecFormat {
    Json,
    Yaml,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WorkflowWalrusNetwork {
    Mainnet,
    Testnet,
}

impl WorkflowWalrusNetwork {
    fn as_cli_value(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
        }
    }
}

impl WorkflowSpecFormat {
    fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

impl WorkflowTemplateArg {
    fn as_builtin(self) -> BuiltinWorkflowTemplate {
        match self {
            Self::Generic => BuiltinWorkflowTemplate::Generic,
            Self::Cetus => BuiltinWorkflowTemplate::Cetus,
            Self::Suilend => BuiltinWorkflowTemplate::Suilend,
            Self::Scallop => BuiltinWorkflowTemplate::Scallop,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct WorkflowInitConfigFile {
    #[serde(default)]
    template: Option<WorkflowTemplateArg>,
    #[serde(default)]
    output: Option<PathBuf>,
    #[serde(default)]
    format: Option<WorkflowSpecFormat>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    checkpoint: Option<u64>,
    #[serde(default)]
    include_analyze_step: Option<bool>,
    #[serde(default)]
    strict_replay: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    package_id: Option<String>,
    #[serde(default)]
    view_objects: Vec<String>,
}

#[derive(Args, Debug)]
pub struct WorkflowValidateCmd {
    /// Path to workflow spec (JSON or YAML)
    #[arg(long)]
    pub spec: PathBuf,
}

#[derive(Args, Debug)]
pub struct WorkflowRunCmd {
    /// Path to workflow spec (JSON or YAML)
    #[arg(long)]
    pub spec: PathBuf,

    /// Print resolved commands without executing
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Continue executing later steps when one fails
    #[arg(long, default_value_t = false)]
    pub continue_on_error: bool,

    /// Write final workflow run report JSON to this path
    #[arg(long)]
    pub report: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct WorkflowValidateOutput {
    spec_file: String,
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    steps: usize,
    replay_steps: usize,
    analyze_replay_steps: usize,
    command_steps: usize,
}

#[derive(Debug, Serialize)]
struct WorkflowInitOutput {
    template: String,
    output_file: String,
    format: WorkflowSpecFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_file: Option<String>,
    digest: String,
    checkpoint: u64,
    include_analyze_step: bool,
    strict_replay: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_id: Option<String>,
    view_objects: usize,
    workflow_name: Option<String>,
    steps: usize,
}

#[derive(Debug, Serialize)]
struct WorkflowAutoOutput {
    package_id: String,
    template: String,
    inference_source: String,
    inference_confidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_reason: Option<String>,
    output_file: String,
    format: WorkflowSpecFormat,
    replay_steps_included: bool,
    replay_seed_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    discover_latest: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discovered_checkpoint: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discovery_probe_error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    missing_inputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_module_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_module_probe_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency_packages_fetched: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unresolved_dependencies: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency_probe_error: Option<String>,
    steps: usize,
}

type TemplateInference = CoreWorkflowTemplateInference;

#[derive(Debug, Deserialize)]
struct AnalyzePackageProbeOutput {
    modules: usize,
    #[serde(default)]
    module_names: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct DependencyClosureProbe {
    fetched_packages: usize,
    unresolved_dependencies: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct FlowDiscoverProbeTarget {
    checkpoint: u64,
    digest: String,
}

impl WorkflowCmd {
    pub async fn execute(
        &self,
        state_file: &Path,
        rpc_url: &str,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        match &self.command {
            WorkflowSubcommand::Init(cmd) => cmd.execute(json_output),
            WorkflowSubcommand::Auto(cmd) => cmd.execute(state_file, rpc_url, json_output, verbose),
            WorkflowSubcommand::Validate(cmd) => cmd.execute(json_output),
            WorkflowSubcommand::Run(cmd) => {
                cmd.execute(state_file, rpc_url, json_output, verbose).await
            }
        }
    }
}

impl WorkflowValidateCmd {
    fn execute(&self, json_output: bool) -> Result<()> {
        let spec = WorkflowSpec::load_from_path(&self.spec)?;
        let mut replay_steps = 0usize;
        let mut analyze_replay_steps = 0usize;
        let mut command_steps = 0usize;
        for step in &spec.steps {
            match step.action {
                WorkflowStepAction::Replay(_) => replay_steps += 1,
                WorkflowStepAction::AnalyzeReplay(_) => analyze_replay_steps += 1,
                WorkflowStepAction::Command(_) => command_steps += 1,
            }
        }

        let output = WorkflowValidateOutput {
            spec_file: self.spec.display().to_string(),
            version: spec.version,
            name: spec.name.clone(),
            steps: spec.steps.len(),
            replay_steps,
            analyze_replay_steps,
            command_steps,
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Workflow spec valid: {}", output.spec_file);
            println!("  version: {}", output.version);
            if let Some(name) = output.name.as_deref() {
                println!("  name: {name}");
            }
            println!("  steps: {}", output.steps);
            println!("  replay steps: {}", output.replay_steps);
            println!("  analyze_replay steps: {}", output.analyze_replay_steps);
            println!("  command steps: {}", output.command_steps);
        }

        Ok(())
    }
}

impl WorkflowInitCmd {
    fn execute(&self, json_output: bool) -> Result<()> {
        let config = match self.from_config.as_ref() {
            Some(path) => Some(load_init_config(path)?),
            None => None,
        };
        let cfg = config.as_ref();

        let template = self
            .template
            .or_else(|| cfg.and_then(|entry| entry.template))
            .unwrap_or(WorkflowTemplateArg::Generic)
            .as_builtin();
        let digest = self
            .digest
            .clone()
            .or_else(|| cfg.and_then(|entry| entry.digest.clone()))
            .unwrap_or_else(|| template.default_digest().to_string());
        let checkpoint = self
            .checkpoint
            .or_else(|| cfg.and_then(|entry| entry.checkpoint))
            .unwrap_or(template.default_checkpoint());
        let strict_replay = if self.no_strict {
            false
        } else {
            cfg.and_then(|entry| entry.strict_replay).unwrap_or(true)
        };
        let include_analyze_step = if self.no_analyze {
            false
        } else {
            cfg.and_then(|entry| entry.include_analyze_step)
                .unwrap_or(true)
        };
        let package_id = self
            .package_id
            .clone()
            .or_else(|| cfg.and_then(|entry| entry.package_id.clone()));
        let view_objects = if self.view_objects.is_empty() {
            cfg.map_or_else(Vec::new, |entry| entry.view_objects.clone())
        } else {
            self.view_objects.clone()
        };

        let mut spec = build_builtin_workflow(
            template,
            &BuiltinWorkflowInput {
                digest: Some(digest.clone()),
                checkpoint: Some(checkpoint),
                include_analyze_step,
                include_replay_step: true,
                strict_replay,
                package_id: package_id.clone(),
                view_objects: view_objects.clone(),
            },
        )?;
        if let Some(name) = self
            .name
            .clone()
            .or_else(|| cfg.and_then(|entry| entry.name.clone()))
        {
            spec.name = Some(name);
        }
        spec.validate()?;

        let format_hint = self
            .format
            .or_else(|| cfg.and_then(|entry| entry.format))
            .unwrap_or(WorkflowSpecFormat::Json);
        let output_path = self
            .output
            .clone()
            .or_else(|| cfg.and_then(|entry| entry.output.clone()))
            .unwrap_or_else(|| {
                PathBuf::from(format!(
                    "workflow.{}.{}",
                    template.key(),
                    format_hint.extension()
                ))
            });
        let output_format = self
            .format
            .or_else(|| WorkflowSpecFormat::from_path(&output_path))
            .or_else(|| cfg.and_then(|entry| entry.format))
            .unwrap_or(WorkflowSpecFormat::Json);
        if output_path.exists() && !self.force {
            return Err(anyhow!(
                "Refusing to overwrite existing workflow spec at {} (pass --force)",
                output_path.display()
            ));
        }
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create workflow spec output directory {}",
                        parent.display()
                    )
                })?;
            }
        }
        let serialized = match output_format {
            WorkflowSpecFormat::Json => serde_json::to_string_pretty(&spec)?,
            WorkflowSpecFormat::Yaml => serde_yaml::to_string(&spec)?,
        };
        fs::write(&output_path, serialized)
            .with_context(|| format!("Failed to write workflow spec {}", output_path.display()))?;

        let output = WorkflowInitOutput {
            template: template.key().to_string(),
            output_file: output_path.display().to_string(),
            format: output_format,
            config_file: self
                .from_config
                .as_ref()
                .map(|path| path.display().to_string()),
            digest,
            checkpoint,
            include_analyze_step,
            strict_replay,
            package_id,
            view_objects: view_objects.len(),
            workflow_name: spec.name.clone(),
            steps: spec.steps.len(),
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!(
                "Generated workflow template `{}` at {}",
                output.template, output.output_file
            );
            println!("  format: {}", output.format.as_str());
            if let Some(config_file) = output.config_file.as_deref() {
                println!("  config_file: {config_file}");
            }
            println!("  digest: {}", output.digest);
            println!("  checkpoint: {}", output.checkpoint);
            if let Some(package_id) = output.package_id.as_deref() {
                println!("  package_id: {package_id}");
            }
            if output.view_objects > 0 {
                println!("  view_objects: {}", output.view_objects);
            }
            println!("  steps: {}", output.steps);
            println!(
                "\nNext:\n  sui-sandbox pipeline validate --spec {}\n  sui-sandbox pipeline run --spec {} --dry-run",
                output.output_file, output.output_file
            );
        }

        Ok(())
    }
}

fn load_init_config(path: &Path) -> Result<WorkflowInitConfigFile> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read workflow init config {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if ext == "yaml" || ext == "yml" {
        serde_yaml::from_str::<WorkflowInitConfigFile>(&raw)
            .with_context(|| format!("Invalid YAML workflow init config in {}", path.display()))
    } else {
        serde_json::from_str::<WorkflowInitConfigFile>(&raw)
            .with_context(|| format!("Invalid JSON workflow init config in {}", path.display()))
    }
}

impl WorkflowAutoCmd {
    fn execute(
        &self,
        state_file: &Path,
        rpc_url: &str,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let package_id = self.package_id.trim();
        if package_id.is_empty() {
            return Err(anyhow!("package_id cannot be empty"));
        }

        let mut dependency_packages_fetched = None;
        let mut unresolved_dependencies = Vec::new();
        let mut dependency_probe_error = None;
        match probe_dependency_closure(state_file, rpc_url, package_id, verbose) {
            Ok(probe) => {
                dependency_packages_fetched = Some(probe.fetched_packages);
                unresolved_dependencies = probe.unresolved_dependencies;
            }
            Err(err) => {
                if self.best_effort {
                    dependency_probe_error = Some(err.to_string());
                } else {
                    return Err(anyhow!(
                        "AUTO_CLOSURE_INCOMPLETE: dependency closure probe failed for package {}: {}\nHint: resolve package fetch issues, or rerun with --best-effort to emit scaffold output.",
                        package_id,
                        err
                    ));
                }
            }
        }
        if !unresolved_dependencies.is_empty() && !self.best_effort {
            return Err(anyhow!(
                "AUTO_CLOSURE_INCOMPLETE: unresolved package dependencies after closure fetch for package {}: {}\nHint: ensure transitive package bytecode is available, or rerun with --best-effort to emit scaffold output.",
                package_id,
                unresolved_dependencies.join(", ")
            ));
        }

        let mut module_count = None;
        let mut module_names = Vec::new();
        let mut probe_error = None;
        if cfg!(feature = "analysis") {
            match probe_package_modules(state_file, rpc_url, package_id, verbose) {
                Ok(probe) => {
                    module_count = Some(probe.modules);
                    module_names = probe.module_names.unwrap_or_default();
                }
                Err(err) => {
                    probe_error = Some(err.to_string());
                }
            }
        } else {
            probe_error = Some(
                "analysis feature is disabled; template inference fell back to generic".to_string(),
            );
        }

        let inference = if let Some(template) = self.template {
            TemplateInference {
                template: template.as_builtin(),
                confidence: "manual",
                source: "user",
                reason: None,
            }
        } else {
            core_infer_workflow_template_from_modules(&module_names)
        };
        let template = inference.template;

        let explicit_digest = self
            .digest
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let mut discovery_probe_error = None;
        let discovered_target = if let Some(latest) = self.discover_latest {
            match probe_flow_discover_latest_target(
                state_file,
                rpc_url,
                package_id,
                latest,
                self.walrus_network,
                self.walrus_caching_url.as_deref(),
                self.walrus_aggregator_url.as_deref(),
                verbose,
            ) {
                Ok(target) => Some(target),
                Err(err) => {
                    if self.best_effort {
                        discovery_probe_error = Some(err.to_string());
                        None
                    } else {
                        return Err(anyhow!(
                            "AUTO_DISCOVERY_EMPTY: failed to auto-discover replay target for package {}: {}\nHint: rerun with a larger --discover-latest window, provide --digest explicitly, or use --best-effort for scaffold-only output.",
                            package_id,
                            err
                        ));
                    }
                }
            }
        } else {
            None
        };
        let digest = explicit_digest.clone().or_else(|| {
            discovered_target
                .as_ref()
                .map(|target| target.digest.clone())
        });
        let include_replay = digest.is_some();
        let checkpoint = if include_replay {
            if let Some(target) = discovered_target.as_ref() {
                Some(target.checkpoint)
            } else {
                Some(self.checkpoint.unwrap_or(template.default_checkpoint()))
            }
        } else {
            None
        };
        let replay_seed_source = if explicit_digest.is_some() {
            "digest"
        } else if discovered_target.is_some() {
            "discover_latest"
        } else {
            "none"
        };

        let mut missing_inputs = Vec::new();
        if !include_replay {
            if self.discover_latest.is_some() {
                missing_inputs.push(
                    "auto-discovery target (rerun with larger --discover-latest window)"
                        .to_string(),
                );
            } else {
                missing_inputs.push("digest".to_string());
                missing_inputs
                    .push("checkpoint (optional; default inferred per template)".to_string());
            }
        }

        let mut spec = build_builtin_workflow(
            template,
            &BuiltinWorkflowInput {
                digest,
                checkpoint,
                include_analyze_step: include_replay,
                include_replay_step: include_replay,
                strict_replay: true,
                package_id: Some(package_id.to_string()),
                view_objects: Vec::new(),
            },
        )?;

        let pkg_suffix = core_short_package_id(package_id);
        spec.name = Some(
            self.name
                .clone()
                .unwrap_or_else(|| format!("auto_{}_{}", template.key(), pkg_suffix)),
        );
        spec.description = Some(format!(
            "Auto draft adapter generated from package {} (template: {}).",
            package_id,
            template.key()
        ));
        spec.validate()?;

        let format_hint = self.format.unwrap_or(WorkflowSpecFormat::Json);
        let output_path = self.output.clone().unwrap_or_else(|| {
            PathBuf::from(format!(
                "workflow.auto.{}.{}.{}",
                template.key(),
                pkg_suffix,
                format_hint.extension()
            ))
        });
        let output_format = self
            .format
            .or_else(|| WorkflowSpecFormat::from_path(&output_path))
            .unwrap_or(format_hint);

        if output_path.exists() && !self.force {
            return Err(anyhow!(
                "Refusing to overwrite existing workflow spec at {} (pass --force)",
                output_path.display()
            ));
        }
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create workflow spec output directory {}",
                        parent.display()
                    )
                })?;
            }
        }

        let serialized = match output_format {
            WorkflowSpecFormat::Json => serde_json::to_string_pretty(&spec)?,
            WorkflowSpecFormat::Yaml => serde_yaml::to_string(&spec)?,
        };
        fs::write(&output_path, serialized)
            .with_context(|| format!("Failed to write workflow spec {}", output_path.display()))?;

        let output = WorkflowAutoOutput {
            package_id: package_id.to_string(),
            template: template.key().to_string(),
            inference_source: inference.source.to_string(),
            inference_confidence: inference.confidence.to_string(),
            inference_reason: inference.reason,
            output_file: output_path.display().to_string(),
            format: output_format,
            replay_steps_included: include_replay,
            replay_seed_source: replay_seed_source.to_string(),
            discover_latest: self.discover_latest,
            discovered_checkpoint: discovered_target.as_ref().map(|target| target.checkpoint),
            discovery_probe_error,
            missing_inputs,
            package_module_count: module_count,
            package_module_probe_error: probe_error,
            dependency_packages_fetched,
            unresolved_dependencies,
            dependency_probe_error,
            steps: spec.steps.len(),
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!(
                "Generated auto draft adapter at {} (template: {})",
                output.output_file, output.template
            );
            println!("  format: {}", output.format.as_str());
            println!(
                "  inference: {} (confidence: {})",
                output.inference_source, output.inference_confidence
            );
            if let Some(reason) = output.inference_reason.as_deref() {
                println!("  inference_reason: {reason}");
            }
            if let Some(count) = output.package_module_count {
                println!("  package_modules: {count}");
            }
            if let Some(err) = output.package_module_probe_error.as_deref() {
                println!("  module_probe_warning: {err}");
            }
            if let Some(count) = output.dependency_packages_fetched {
                println!("  dependency_packages_fetched: {count}");
            }
            if !output.unresolved_dependencies.is_empty() {
                println!(
                    "  unresolved_dependencies: {}",
                    output.unresolved_dependencies.join(", ")
                );
            }
            if let Some(err) = output.dependency_probe_error.as_deref() {
                println!("  dependency_probe_warning: {err}");
            }
            println!("  replay_steps_included: {}", output.replay_steps_included);
            println!("  replay_seed_source: {}", output.replay_seed_source);
            if let Some(window) = output.discover_latest {
                println!("  discover_latest: {}", window);
            }
            if let Some(checkpoint) = output.discovered_checkpoint {
                println!("  discovered_checkpoint: {}", checkpoint);
            }
            if let Some(err) = output.discovery_probe_error.as_deref() {
                println!("  discovery_probe_warning: {err}");
            }
            if !output.missing_inputs.is_empty() {
                println!("  missing_inputs: {}", output.missing_inputs.join(", "));
            }
            println!(
                "\nNext:\n  sui-sandbox pipeline validate --spec {}\n  sui-sandbox pipeline run --spec {} --dry-run",
                output.output_file, output.output_file
            );
            if !output.replay_steps_included {
                println!(
                    "\nTo include replay steps:\n  sui-sandbox pipeline auto --package-id {} --discover-latest 25 --output {} --force\n  # or explicit input path:\n  sui-sandbox pipeline auto --package-id {} --digest <DIGEST> --checkpoint <CHECKPOINT> --output {} --force",
                    output.package_id, output.output_file, output.package_id, output.output_file
                );
            }
        }

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn probe_flow_discover_latest_target(
    _state_file: &Path,
    _rpc_url: &str,
    package_id: &str,
    latest: u64,
    walrus_network: WorkflowWalrusNetwork,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
    verbose: bool,
) -> Result<FlowDiscoverProbeTarget> {
    if latest == 0 {
        return Err(anyhow!("discover_latest must be greater than zero"));
    }
    match (
        walrus_caching_url
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        walrus_aggregator_url
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(anyhow!(
                "provide both --walrus-caching-url and --walrus-aggregator-url for custom endpoints"
            ));
        }
        _ => {}
    }

    let network = CoreWalrusArchiveNetwork::parse(walrus_network.as_cli_value())
        .with_context(|| format!("invalid walrus network {}", walrus_network.as_cli_value()))?;
    let walrus = core_build_walrus_client(network, walrus_caching_url, walrus_aggregator_url)?;
    let discovered =
        core_discover_checkpoint_targets(&walrus, None, Some(latest), Some(package_id), false, 1)?;
    if verbose {
        eprintln!(
            "[workflow-auto] discovery probe checkpoints={} txs={} candidates={} for package {}",
            discovered.checkpoints_scanned,
            discovered.transactions_scanned,
            discovered.targets.len(),
            package_id
        );
    }
    discovered
        .targets
        .first()
        .map(|target| FlowDiscoverProbeTarget {
            checkpoint: target.checkpoint,
            digest: target.digest.clone(),
        })
        .ok_or_else(|| {
            anyhow!(
                "no candidate transactions discovered for package {} in latest {} checkpoint(s)",
                package_id,
                latest
            )
        })
}

fn probe_package_modules(
    _state_file: &Path,
    rpc_url: &str,
    package_id: &str,
    _verbose: bool,
) -> Result<AnalyzePackageProbeOutput> {
    let graphql_endpoint = resolve_graphql_endpoint(rpc_url);
    let graphql = GraphQLClient::new(&graphql_endpoint);
    let pkg = graphql
        .fetch_package(package_id)
        .with_context(|| format!("fetch package {}", package_id))?;
    let modules = decode_graphql_modules(package_id, &pkg.modules)?;
    let mut module_names = modules
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    module_names.sort();
    Ok(AnalyzePackageProbeOutput {
        modules: module_names.len(),
        module_names: Some(module_names),
    })
}

fn probe_dependency_closure(
    state_file: &Path,
    rpc_url: &str,
    package_id: &str,
    verbose: bool,
) -> Result<DependencyClosureProbe> {
    let mut state = SandboxState::load_or_create(state_file, rpc_url)?;
    let initial = fetch_package_with_bytecodes_into_state(&mut state, package_id, true, verbose)?;

    let mut decoded_packages: std::collections::BTreeMap<AccountAddress, Vec<(String, Vec<u8>)>> =
        std::collections::BTreeMap::new();
    for pkg in &initial.packages_fetched {
        let (id, modules) = decode_fetched_package_modules(pkg)?;
        decoded_packages.insert(id, modules);
    }

    const MAX_REPAIR_ROUNDS: usize = 4;
    let mut unresolved = Vec::<AccountAddress>::new();
    for _round in 0..MAX_REPAIR_ROUNDS {
        unresolved = unresolved_package_dependencies_for_modules(
            decoded_packages
                .iter()
                .map(|(id, modules)| (*id, modules.clone()))
                .collect(),
        )?
        .into_iter()
        .collect();
        if unresolved.is_empty() {
            break;
        }

        let mut fetched_any = false;
        for dep in &unresolved {
            if decoded_packages.contains_key(dep) {
                continue;
            }
            let dep_hex = dep.to_hex_literal();
            let dep_result =
                match fetch_package_with_bytecodes_into_state(&mut state, &dep_hex, false, verbose)
                {
                    Ok(value) => value,
                    Err(_) => continue,
                };
            for pkg in &dep_result.packages_fetched {
                let (id, modules) = decode_fetched_package_modules(pkg)?;
                if decoded_packages.insert(id, modules).is_none() {
                    fetched_any = true;
                }
            }
        }
        if !fetched_any {
            break;
        }
    }

    let probe = DependencyClosureProbe {
        fetched_packages: decoded_packages.len(),
        unresolved_dependencies: unresolved
            .into_iter()
            .map(|address| address.to_hex_literal())
            .collect(),
    };
    state.save(state_file).with_context(|| {
        format!(
            "failed to persist workflow probe state {}",
            state_file.display()
        )
    })?;
    Ok(probe)
}

#[allow(clippy::type_complexity)]
fn decode_fetched_package_modules(
    package: &PackageInfo,
) -> Result<(AccountAddress, Vec<(String, Vec<u8>)>)> {
    let package_id = AccountAddress::from_hex_literal(&package.address).with_context(|| {
        format!(
            "invalid package id in fetch probe output: {}",
            package.address
        )
    })?;
    let bytecodes = package.bytecodes.as_ref().ok_or_else(|| {
        anyhow!(
            "fetch probe package {} did not include bytecodes (expected --bytecodes output)",
            package.address
        )
    })?;

    let mut modules = Vec::with_capacity(bytecodes.len());
    for (idx, encoded) in bytecodes.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .with_context(|| {
                format!(
                    "failed to decode base64 bytecode for package {} module {}",
                    package.address, idx
                )
            })?;
        let module_name = CompiledModule::deserialize_with_defaults(&bytes)
            .ok()
            .map(|module| module.self_id().name().to_string())
            .or_else(|| package.modules.get(idx).cloned())
            .unwrap_or_else(|| format!("module_{idx}"));
        modules.push((module_name, bytes));
    }
    Ok((package_id, modules))
}

impl WorkflowRunCmd {
    async fn execute(
        &self,
        state_file: &Path,
        rpc_url: &str,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let spec = WorkflowSpec::load_from_path(&self.spec)?;
        if let Some(name) = spec.name.as_deref() {
            if !json_output {
                println!("Workflow: {name}");
            }
        }
        if let Some(description) = spec.description.as_deref() {
            if !json_output {
                println!("Description: {description}");
            }
        }

        let prepared_steps = spec
            .steps
            .iter()
            .enumerate()
            .map(|(idx, step)| WorkflowPreparedStep {
                index: idx + 1,
                id: step.id.clone(),
                name: step.name.clone(),
                kind: core_workflow_step_kind(&step.action).to_string(),
                continue_on_error: step.continue_on_error,
                command: core_workflow_build_step_command(&spec.defaults, step)
                    .map_err(|err| err.to_string()),
            })
            .collect::<Vec<_>>();
        let mut executable: Option<PathBuf> = None;

        let report = run_prepared_workflow_steps(
            self.spec.display().to_string(),
            &spec,
            prepared_steps,
            self.dry_run,
            self.continue_on_error,
            |step, prepared| {
                if !json_output {
                    let label = core_workflow_step_label(step, prepared.index);
                    println!("[workflow:{label}] {}", prepared.command_display());
                }
            },
            |step, prepared| {
                let argv = prepared.command.clone().map_err(anyhow::Error::msg)?;
                let display_cmd = argv.join(" ");
                if !json_output {
                    match &step.action {
                        WorkflowStepAction::Replay(_) => {
                            return execute_workflow_replay_step_native(
                                &argv,
                                state_file,
                                rpc_url,
                                false,
                                verbose,
                                prepared.index,
                            );
                        }
                        WorkflowStepAction::AnalyzeReplay(_) => {
                            #[cfg(feature = "analysis")]
                            {
                                return execute_workflow_analyze_step_native(
                                    &argv,
                                    state_file,
                                    rpc_url,
                                    false,
                                    verbose,
                                    prepared.index,
                                );
                            }
                            #[cfg(not(feature = "analysis"))]
                            {
                                return Ok(WorkflowStepExecution {
                                    exit_code: 1,
                                    output: None,
                                    error: Some("workflow analyze_replay step requires the `analysis` feature".to_string()),
                                });
                            }
                        }
                        WorkflowStepAction::Command(_) => {}
                    }
                }

                let executable = if let Some(path) = executable.as_ref() {
                    path.clone()
                } else {
                    let path =
                        std::env::current_exe().context("Failed to resolve current executable")?;
                    executable = Some(path.clone());
                    path
                };

                let mut cmd = Command::new(&executable);
                cmd.arg("--state-file")
                    .arg(state_file)
                    .arg("--rpc-url")
                    .arg(rpc_url);
                if verbose {
                    cmd.arg("--verbose");
                }
                cmd.args(&argv);

                let output = cmd.output().with_context(|| {
                    format!(
                        "Failed to execute workflow step {} ({})",
                        prepared.index, display_cmd
                    )
                })?;

                let ok = output.status.success();
                let exit_code = output.status.code().unwrap_or(-1);
                let failure_summary = if ok {
                    None
                } else {
                    core_summarize_failure_output(&output.stdout, &output.stderr)
                };

                if !json_output {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stdout.trim().is_empty() {
                        print!("{stdout}");
                    }
                    if !stderr.trim().is_empty() {
                        eprint!("{stderr}");
                    }
                }

                let error = if ok {
                    None
                } else {
                    Some(match failure_summary.as_deref() {
                        Some(summary) => format!(
                            "step {} failed with exit code {}: {}",
                            prepared.index, exit_code, summary
                        ),
                        None => format!(
                            "step {} failed with exit code {}",
                            prepared.index, exit_code
                        ),
                    })
                };
                Ok(WorkflowStepExecution {
                    exit_code,
                    output: None,
                    error,
                })
            },
        );

        maybe_write_report(self.report.as_ref(), &report, json_output)?;
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            if report.stopped_early {
                if let Some(last) = report.steps.last() {
                    println!(
                        "Workflow stopped at step {} (use --continue-on-error to continue)",
                        last.index
                    );
                }
            }
            println!(
                "Workflow complete: {}/{} succeeded ({} failed)",
                report.succeeded_steps, report.total_steps, report.failed_steps
            );
        }

        if report.failed_steps > 0 {
            if let Some(last_error) = report.steps.last().and_then(|entry| entry.error.clone()) {
                return Err(anyhow!("workflow execution failed: {}", last_error));
            }
            Err(anyhow!(
                "workflow completed with {} failed step(s)",
                report.failed_steps
            ))
        } else {
            Ok(())
        }
    }
}

fn maybe_write_report(
    report_path: Option<&PathBuf>,
    report: &WorkflowRunReport,
    json_output: bool,
) -> Result<()> {
    let Some(path) = report_path else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create workflow report directory {}",
                    parent.display()
                )
            })?;
        }
    }

    let payload = serde_json::to_string_pretty(report)?;
    fs::write(path, payload)
        .with_context(|| format!("Failed to write workflow report {}", path.display()))?;
    if !json_output {
        println!("Workflow report written: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_cetus_from_module_keywords() {
        let modules = vec![
            "pool_script".to_string(),
            "clmm_math".to_string(),
            "position_manager".to_string(),
        ];
        let inferred = core_infer_workflow_template_from_modules(&modules);
        assert!(matches!(inferred.template, BuiltinWorkflowTemplate::Cetus));
    }

    #[test]
    fn falls_back_to_generic_when_ambiguous() {
        let modules = vec![
            "reserve".to_string(),
            "collateral".to_string(),
            "suilend_router".to_string(),
            "scallop_router".to_string(),
        ];
        let inferred = core_infer_workflow_template_from_modules(&modules);
        assert!(matches!(
            inferred.template,
            BuiltinWorkflowTemplate::Generic
        ));
    }

    #[test]
    fn package_suffix_is_trimmed_and_shortened() {
        assert_eq!(core_short_package_id("0x1234567890abcdef"), "1234567890ab");
        assert_eq!(core_short_package_id("0x"), "unknown");
    }

    #[test]
    fn decodes_probe_package_modules_with_fallback_names() {
        let pkg = PackageInfo {
            address: "0x1234".to_string(),
            modules: vec!["fallback_mod".to_string()],
            bytecodes: Some(vec![
                base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3])
            ]),
        };
        let (_, modules) = decode_fetched_package_modules(&pkg).expect("decode probe package");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].0, "fallback_mod");
        assert_eq!(modules[0].1, vec![1u8, 2, 3]);
    }

    #[test]
    fn summarizes_failure_output_prefers_stderr() {
        let summary = core_summarize_failure_output(
            b"stdout line",
            b"\n  meaningful stderr line  \nmore details\n",
        )
        .expect("summary");
        assert_eq!(summary, "meaningful stderr line");
    }

    #[test]
    fn summarizes_failure_output_falls_back_to_stdout() {
        let summary = core_summarize_failure_output(b"\nstdout failure\n", b"\n")
            .expect("summary from stdout");
        assert_eq!(summary, "stdout failure");
    }

    #[test]
    fn parses_replay_cli_from_workflow_argv() {
        let argv = vec![
            "replay".to_string(),
            "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2".to_string(),
            "--source".to_string(),
            "walrus".to_string(),
            "--checkpoint".to_string(),
            "239615926".to_string(),
        ];
        let parsed = parse_replay_cli_from_workflow_argv(&argv).expect("parse replay argv");
        assert_eq!(
            parsed.replay.digest.as_deref(),
            Some("At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2")
        );
    }
}
