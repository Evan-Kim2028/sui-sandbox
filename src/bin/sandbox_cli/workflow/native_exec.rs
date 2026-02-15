use anyhow::{anyhow, Result};
use clap::FromArgMatches;
use std::path::Path;
use sui_sandbox_core::workflow_runner::WorkflowStepExecution;

use super::super::replay::ReplayCli;
use super::super::SandboxState;

#[cfg(feature = "analysis")]
use super::super::analyze::AnalyzeCmd;
#[cfg(feature = "analysis")]
use clap::Parser;

pub(super) fn parse_replay_cli_from_workflow_argv(argv: &[String]) -> Result<ReplayCli> {
    let cmd = <ReplayCli as clap::Args>::augment_args(clap::Command::new("replay"));
    let matches = cmd.try_get_matches_from(argv).map_err(|err| {
        anyhow!(
            "invalid workflow replay step arguments: {}",
            err.render().to_string().trim()
        )
    })?;
    ReplayCli::from_arg_matches(&matches)
        .map_err(|err| anyhow!("failed to parse workflow replay step arguments: {}", err))
}

pub(super) fn execute_workflow_replay_step_native(
    argv: &[String],
    state_file: &Path,
    rpc_url: &str,
    json_output: bool,
    verbose: bool,
    step_index: usize,
) -> Result<WorkflowStepExecution> {
    let replay_cli = parse_replay_cli_from_workflow_argv(argv)?;
    let mut state = SandboxState::load_or_create(state_file, rpc_url)?;

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async { replay_cli.execute(&mut state, json_output, verbose).await })
    });

    match result {
        Ok(()) => {
            state.save(state_file)?;
            Ok(WorkflowStepExecution {
                exit_code: 0,
                output: None,
                error: None,
            })
        }
        Err(err) => Ok(WorkflowStepExecution {
            exit_code: 1,
            output: None,
            error: Some(format!("step {} failed: {}", step_index, err)),
        }),
    }
}

#[cfg(feature = "analysis")]
pub(super) fn execute_workflow_analyze_step_native(
    argv: &[String],
    state_file: &Path,
    rpc_url: &str,
    json_output: bool,
    verbose: bool,
    step_index: usize,
) -> Result<WorkflowStepExecution> {
    let analyze_cmd = AnalyzeCmd::try_parse_from(argv).map_err(|err| {
        anyhow!(
            "invalid workflow analyze_replay step arguments: {}",
            err.render().to_string().trim()
        )
    })?;
    let mut state = SandboxState::load_or_create(state_file, rpc_url)?;

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async { analyze_cmd.execute(&mut state, json_output, verbose).await })
    });

    match result {
        Ok(()) => {
            state.save(state_file)?;
            Ok(WorkflowStepExecution {
                exit_code: 0,
                output: None,
                error: None,
            })
        }
        Err(err) => Ok(WorkflowStepExecution {
            exit_code: 1,
            output: None,
            error: Some(format!("step {} failed: {}", step_index, err)),
        }),
    }
}
