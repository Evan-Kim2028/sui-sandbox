use anyhow::Result;
use clap::{Parser, Subcommand};

mod poll_transactions;
#[cfg(feature = "walrus")]
mod ptb_replay_harness;
mod stream_transactions;
mod tx_sim;
#[cfg(feature = "walrus")]
mod walrus_warmup;

pub use poll_transactions::PollTransactionsCmd;
#[cfg(feature = "walrus")]
pub use ptb_replay_harness::PtbReplayHarnessCmd;
pub use stream_transactions::StreamTransactionsCmd;
pub use tx_sim::TxSimCmd;
#[cfg(feature = "walrus")]
pub use walrus_warmup::WalrusWarmupCmd;

#[derive(Parser, Debug)]
pub struct ToolsCmd {
    #[command(subcommand)]
    command: ToolsSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ToolsSubcommand {
    /// Poll recent transactions via GraphQL and write JSONL output
    PollTransactions(PollTransactionsCmd),
    /// Stream transactions via gRPC and write JSONL output
    StreamTransactions(StreamTransactionsCmd),
    /// Simulate a PTB via gRPC (dev-inspect or dry-run)
    TxSim(TxSimCmd),
    /// Internal PTB replay harness (Walrus)
    #[cfg(feature = "walrus")]
    PtbReplayHarness(PtbReplayHarnessCmd),
    /// Warm the local Walrus checkpoint store
    #[cfg(feature = "walrus")]
    WalrusWarmup(WalrusWarmupCmd),
}

impl ToolsCmd {
    pub async fn execute(&self) -> Result<()> {
        match &self.command {
            ToolsSubcommand::PollTransactions(cmd) => cmd.execute(),
            ToolsSubcommand::StreamTransactions(cmd) => cmd.execute().await,
            ToolsSubcommand::TxSim(cmd) => cmd.execute().await,
            #[cfg(feature = "walrus")]
            ToolsSubcommand::PtbReplayHarness(cmd) => cmd.execute(),
            #[cfg(feature = "walrus")]
            ToolsSubcommand::WalrusWarmup(cmd) => cmd.execute(),
        }
    }
}
