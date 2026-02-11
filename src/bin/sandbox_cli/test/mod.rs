//! Test subcommands for Move function testing.

pub mod fuzz;

use anyhow::Result;
use clap::{Parser, Subcommand};

use super::SandboxState;

#[derive(Parser, Debug)]
#[command(about = "Test Move functions (fuzz, property-based, coverage)")]
pub struct TestCli {
    #[command(subcommand)]
    pub command: TestSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum TestSubcommand {
    /// Fuzz a Move function with random inputs
    Fuzz(fuzz::FuzzCmd),
}

impl TestCli {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        match &self.command {
            TestSubcommand::Fuzz(cmd) => cmd.execute(state, json_output, verbose).await,
        }
    }
}
