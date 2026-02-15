//! First-class protocol adapter commands.
//!
//! This surface keeps protocol-specific runtime inputs explicit while reusing
//! the generic flow runtime for package preparation and replay execution.

use anyhow::{anyhow, Result};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use sui_sandbox_core::adapter::{
    resolve_discovery_package_filter as core_resolve_discovery_package_filter,
    resolve_required_package_id as core_resolve_required_package_id,
    ProtocolAdapter as CoreProtocolAdapter,
};

use super::flow::{
    FlowDiscoverCmd, FlowPrepareCmd, FlowRunCmd, ReplayExecutionArgs, ReplayTargetArgs,
    WalrusEndpointArgs,
};
use super::SandboxState;

#[derive(Parser, Debug)]
#[command(about = "Protocol-first adapter entrypoint")]
pub struct ProtocolCli {
    #[command(subcommand)]
    command: ProtocolSubcommand,
}

#[derive(Subcommand, Debug)]
enum ProtocolSubcommand {
    /// Prepare package context for a protocol adapter
    Prepare(ProtocolPrepareCmd),
    /// Run protocol adapter replay flow (prepare + replay)
    Run(ProtocolRunCmd),
    /// Discover protocol-specific replay targets from checkpoints
    Discover(ProtocolDiscoverCmd),
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ProtocolName {
    Generic,
    Deepbook,
    Cetus,
    Suilend,
    Scallop,
}

impl ProtocolName {
    fn as_core(self) -> CoreProtocolAdapter {
        match self {
            Self::Generic => CoreProtocolAdapter::Generic,
            Self::Deepbook => CoreProtocolAdapter::Deepbook,
            Self::Cetus => CoreProtocolAdapter::Cetus,
            Self::Suilend => CoreProtocolAdapter::Suilend,
            Self::Scallop => CoreProtocolAdapter::Scallop,
        }
    }
}

#[derive(Args, Debug)]
pub struct ProtocolPrepareCmd {
    /// Protocol adapter family
    #[arg(long, value_enum, default_value = "generic")]
    pub protocol: ProtocolName,

    /// Root package id (required for all non-generic protocol adapters)
    #[arg(long = "package-id")]
    pub package_id: Option<String>,

    /// Also fetch transitive dependencies (default: true)
    #[arg(long = "with-deps", default_value_t = true, action = ArgAction::Set)]
    pub with_deps: bool,

    /// Output context JSON path
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Overwrite existing context file
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct ProtocolRunCmd {
    /// Protocol adapter family
    #[arg(long, value_enum, default_value = "generic")]
    pub protocol: ProtocolName,

    /// Root package id (required for all non-generic protocol adapters)
    #[arg(long = "package-id")]
    pub package_id: Option<String>,

    /// Transaction digest to replay (optional when --state-json has a single state)
    #[arg(long)]
    pub digest: Option<String>,

    /// Also fetch transitive dependencies (default: true)
    #[arg(long = "with-deps", default_value_t = true, action = ArgAction::Set)]
    pub with_deps: bool,

    /// Optional path to persist context JSON (portable for later replay)
    #[arg(long = "context-out")]
    pub context_out: Option<PathBuf>,

    /// Overwrite existing context file when --context-out is used
    #[arg(long, default_value_t = false)]
    pub force: bool,

    #[command(flatten)]
    pub target: ReplayTargetArgs,

    #[command(flatten)]
    pub walrus: WalrusEndpointArgs,

    #[command(flatten)]
    pub execution: ReplayExecutionArgs,
}

#[derive(Args, Debug)]
pub struct ProtocolDiscoverCmd {
    /// Protocol adapter family
    #[arg(long, value_enum, default_value = "generic")]
    pub protocol: ProtocolName,

    /// Package filter override (required for non-generic protocol adapters)
    #[arg(long = "package-id")]
    pub package_id: Option<String>,

    /// Checkpoint spec: single (239615926), range (239615900..239615926), or list (239615900,239615910)
    #[arg(long, conflicts_with = "latest")]
    pub checkpoint: Option<String>,

    /// Scan latest N checkpoints (auto-discovers tip)
    #[arg(long, conflicts_with = "checkpoint")]
    pub latest: Option<u64>,

    /// Include framework packages (0x1/0x2/0x3) in results
    #[arg(long, default_value_t = false)]
    pub include_framework: bool,

    /// Max matching transactions to return
    #[arg(long, default_value_t = 200)]
    pub limit: usize,

    #[command(flatten)]
    pub walrus: WalrusEndpointArgs,
}

impl ProtocolCli {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        match &self.command {
            ProtocolSubcommand::Prepare(cmd) => cmd.execute(state, json_output, verbose).await,
            ProtocolSubcommand::Run(cmd) => cmd.execute(state, json_output, verbose).await,
            ProtocolSubcommand::Discover(cmd) => cmd.execute(json_output).await,
        }
    }
}

impl ProtocolPrepareCmd {
    async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let package_id = resolve_required_package_id(self.protocol, self.package_id.as_deref())?;
        FlowPrepareCmd {
            package_id,
            with_deps: self.with_deps,
            output: self.output.clone(),
            force: self.force,
        }
        .execute(state, json_output, verbose)
        .await
    }
}

impl ProtocolRunCmd {
    async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let package_id = resolve_required_package_id(self.protocol, self.package_id.as_deref())?;
        FlowRunCmd {
            package_id,
            digest: self.digest.clone(),
            with_deps: self.with_deps,
            context_out: self.context_out.clone(),
            force: self.force,
            target: self.target.clone(),
            walrus: self.walrus.clone(),
            execution: self.execution.clone(),
        }
        .execute(state, json_output, verbose)
        .await
    }
}

impl ProtocolDiscoverCmd {
    async fn execute(&self, json_output: bool) -> Result<()> {
        let package_id =
            resolve_discovery_package_filter(self.protocol, self.package_id.as_deref())?;
        FlowDiscoverCmd {
            checkpoint: self.checkpoint.clone(),
            latest: self.latest,
            package_id,
            include_framework: self.include_framework,
            limit: self.limit,
            walrus: self.walrus.clone(),
        }
        .execute(json_output)
        .await
    }
}

fn resolve_required_package_id(protocol: ProtocolName, package_id: Option<&str>) -> Result<String> {
    core_resolve_required_package_id(protocol.as_core(), package_id).map_err(|err| {
        let message = err.to_string();
        if message.contains("requires package_id") {
            anyhow!(
                "protocol `{}` requires --package-id (no built-in protocol package defaults)",
                protocol.as_core().as_str()
            )
        } else {
            err
        }
    })
}

fn resolve_discovery_package_filter(
    protocol: ProtocolName,
    package_id: Option<&str>,
) -> Result<Option<String>> {
    core_resolve_discovery_package_filter(protocol.as_core(), package_id).map_err(|err| {
        let message = err.to_string();
        if message.contains("requires package_id") {
            anyhow!(
                "protocol `{}` requires --package-id (no built-in protocol package defaults)",
                protocol.as_core().as_str()
            )
        } else {
            err
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{resolve_discovery_package_filter, resolve_required_package_id, ProtocolCli};
    use clap::Parser;

    #[test]
    fn parses_protocol_run() {
        let parsed = ProtocolCli::try_parse_from([
            "protocol",
            "run",
            "--protocol",
            "deepbook",
            "--package-id",
            "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
            "--digest",
            "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
            "--checkpoint",
            "239615926",
            "--source",
            "walrus",
        ]);
        assert!(parsed.is_ok());
    }

    #[test]
    fn parses_protocol_run_with_discover_latest() {
        let parsed = ProtocolCli::try_parse_from([
            "protocol",
            "run",
            "--protocol",
            "deepbook",
            "--package-id",
            "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
            "--discover-latest",
            "5",
            "--source",
            "walrus",
        ]);
        assert!(parsed.is_ok());
    }

    #[test]
    fn generic_discover_allows_no_package_filter() {
        let filter = resolve_discovery_package_filter(super::ProtocolName::Generic, None)
            .expect("generic discovery should allow broad scan");
        assert!(filter.is_none());
    }

    #[test]
    fn non_generic_requires_package_override() {
        let err = resolve_required_package_id(super::ProtocolName::Deepbook, None)
            .expect_err("non-generic adapters should require package id");
        assert!(err.to_string().contains("requires --package-id"));
    }
}
