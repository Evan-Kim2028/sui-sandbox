//! Draft adapter workflow from package id (Rust example).
//!
//! This example runs:
//! 1. `workflow auto` (generate draft spec)
//! 2. `workflow validate`
//! 3. `workflow run --dry-run` (or `--run`)
//!
//! Run:
//!   cargo run --example workflow_auto_from_package
//!   cargo run --example workflow_auto_from_package -- --package-id 0x2 --digest <DIGEST> --checkpoint <CP>

use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

#[derive(Clone, Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Yaml,
}

impl OutputFormat {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
enum Template {
    Generic,
    Cetus,
    Suilend,
    Scallop,
}

impl Template {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Cetus => "cetus",
            Self::Suilend => "suilend",
            Self::Scallop => "scallop",
        }
    }
}

#[derive(Parser, Debug)]
#[command(about = "Generate + validate + dry-run an auto draft adapter workflow")]
struct Args {
    /// Package id used by `workflow auto`
    #[arg(long = "package-id", default_value = "0x2")]
    package_id: String,

    /// Optional template override
    #[arg(long, value_enum)]
    template: Option<Template>,

    /// Optional replay seed digest (must be paired with --checkpoint)
    #[arg(long)]
    digest: Option<String>,

    /// Optional replay seed checkpoint (must be paired with --digest)
    #[arg(long)]
    checkpoint: Option<u64>,

    /// Emit scaffold even when dependency closure validation fails
    #[arg(long = "best-effort")]
    best_effort: bool,

    /// Output format for generated workflow spec
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    format: OutputFormat,

    /// Optional output path. If omitted, uses a temp file.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Execute workflow for real (default is dry-run)
    #[arg(long)]
    run: bool,

    /// Keep generated temp files (ignored when --output is provided)
    #[arg(long = "keep-files")]
    keep_files: bool,

    /// Optional path to sui-sandbox CLI binary
    #[arg(long = "sui-sandbox-bin")]
    sui_sandbox_bin: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}

fn run(args: Args) -> Result<()> {
    match (&args.digest, args.checkpoint) {
        (Some(_), None) => {
            return Err(anyhow!(
                "--checkpoint is required when --digest is provided"
            ));
        }
        (None, Some(_)) => {
            return Err(anyhow!("--digest is required when --checkpoint is provided"));
        }
        _ => {}
    }

    let cli_bin = resolve_cli_binary(args.sui_sandbox_bin)?;
    let temp_dir = tempfile::tempdir().context("failed to create temp directory")?;

    let output_was_explicit = args.output.is_some();
    let spec_path = args.output.unwrap_or_else(|| {
        temp_dir
            .path()
            .join(format!("workflow.auto.{}", args.format.as_str()))
    });
    let report_path = spec_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("workflow.auto.report.json");

    if let Some(parent) = spec_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    println!("=== Workflow Auto (Rust) ===");
    println!("binary:     {}", cli_bin.display());
    println!("package_id: {}", args.package_id);
    println!(
        "template:   {}",
        args.template
            .as_ref()
            .map(Template::as_str)
            .unwrap_or("inferred")
    );
    println!("spec:       {}", spec_path.display());
    println!("report:     {}", report_path.display());
    println!("mode:       {}", if args.run { "execute" } else { "dry-run" });

    let mut auto_cmd = Command::new(&cli_bin);
    auto_cmd
        .arg("workflow")
        .arg("auto")
        .arg("--package-id")
        .arg(&args.package_id)
        .arg("--format")
        .arg(args.format.as_str())
        .arg("--output")
        .arg(&spec_path)
        .arg("--force");
    if let Some(template) = args.template {
        auto_cmd.arg("--template").arg(template.as_str());
    }
    if let (Some(digest), Some(checkpoint)) = (args.digest, args.checkpoint) {
        auto_cmd.arg("--digest").arg(digest);
        auto_cmd.arg("--checkpoint").arg(checkpoint.to_string());
    }
    if args.best_effort {
        auto_cmd.arg("--best-effort");
    }
    run_or_fail(auto_cmd, "workflow auto")?;

    let mut validate_cmd = Command::new(&cli_bin);
    validate_cmd
        .arg("workflow")
        .arg("validate")
        .arg("--spec")
        .arg(&spec_path);
    run_or_fail(validate_cmd, "workflow validate")?;

    let mut run_cmd = Command::new(&cli_bin);
    run_cmd
        .arg("workflow")
        .arg("run")
        .arg("--spec")
        .arg(&spec_path)
        .arg("--report")
        .arg(&report_path);
    if !args.run {
        run_cmd.arg("--dry-run");
    }
    run_or_fail(run_cmd, "workflow run")?;

    if args.keep_files || output_was_explicit {
        println!("kept files in: {}", report_path.parent().unwrap().display());
    }

    Ok(())
}

fn resolve_cli_binary(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    if let Ok(from_env) = std::env::var("SUI_SANDBOX_BIN") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(candidate) = exe.parent().and_then(|p| p.parent().map(|pp| pp.join("sui-sandbox")))
        {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    let local = PathBuf::from("target/debug/sui-sandbox");
    if local.exists() {
        return Ok(local);
    }

    Ok(PathBuf::from("sui-sandbox"))
}

fn run_or_fail(mut cmd: Command, step: &str) -> Result<()> {
    println!("-> {}", render_command(&cmd));
    let status = cmd.status().with_context(|| {
        format!(
            "{step} failed to start; set --sui-sandbox-bin or SUI_SANDBOX_BIN, or build `cargo build --bin sui-sandbox`"
        )
    })?;
    ensure_success(status, step)
}

fn ensure_success(status: ExitStatus, step: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{step} failed with exit status {}", status))
    }
}

fn render_command(cmd: &Command) -> String {
    let program = cmd.get_program().to_string_lossy();
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {args}")
    }
}
