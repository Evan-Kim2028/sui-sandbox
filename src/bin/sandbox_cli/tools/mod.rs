use anyhow::Result;
use clap::{Parser, Subcommand};

mod poll_transactions;
mod stream_transactions;
mod tx_sim;

pub use poll_transactions::PollTransactionsCmd;
pub use stream_transactions::StreamTransactionsCmd;
pub use tx_sim::TxSimCmd;

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
}

impl ToolsCmd {
    pub async fn execute(&self) -> Result<()> {
        match &self.command {
            ToolsSubcommand::PollTransactions(cmd) => cmd.execute(),
            ToolsSubcommand::StreamTransactions(cmd) => cmd.execute().await,
            ToolsSubcommand::TxSim(cmd) => cmd.execute().await,
        }
    }
}
