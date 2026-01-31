//! sui-sandbox: A developer CLI for local Move/Sui development
//!
//! This CLI provides a powerful local development environment for Sui Move,
//! replacing and extending the functionality of `move sandbox`.
//!
//! ## Features
//!
//! - **publish**: Compile and deploy Move packages locally
//! - **run**: Execute single Move function calls
//! - **ptb**: Execute full Programmable Transaction Blocks from JSON specs
//! - **fetch**: Import packages and objects from mainnet
//! - **replay**: Replay historical transactions locally
//! - **view**: Inspect modules, objects, and session state
//! - **bridge**: Generate sui client commands for deployment
//!
//! ## Example Usage
//!
//! ```bash
//! # Publish a local package
//! sui-sandbox publish ./my_package
//!
//! # Run a function
//! sui-sandbox run 0x123::counter::increment --args 42
//!
//! # Execute a PTB from JSON
//! sui-sandbox ptb --spec tx.json --sender 0xABC
//!
//! # Fetch a package from mainnet
//! sui-sandbox fetch package 0x1eabed72...
//!
//! # Replay a transaction
//! sui-sandbox replay 9V3xKM... --compare
//!
//! # Generate sui client command for deployment
//! sui-sandbox bridge publish ./my_package
//! ```

use anyhow::Result;
use clap::{Parser, Subcommand};

mod sandbox_cli;

use sandbox_cli::{
    bridge::BridgeCmd, fetch::FetchCmd, ptb::PtbCmd, publish::PublishCmd, replay::ReplayCmd,
    run::RunCmd, tool::ToolCmd, view::ViewCmd, SandboxState,
};

#[derive(Parser)]
#[command(
    name = "sui-sandbox",
    author,
    version,
    about = "Local Move/Sui development environment",
    long_about = "A powerful CLI for local Sui Move development, testing, and simulation.\n\n\
                  Provides publish, run, PTB execution, mainnet state fetching, and transaction replay."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// State file for session persistence (legacy CLI uses ~/.sui-sandbox/state.bin; MCP tool uses ~/.sui-sandbox/mcp-state.json)
    #[arg(long, global = true)]
    state_file: Option<std::path::PathBuf>,

    /// RPC URL for mainnet fetching (default: mainnet fullnode)
    #[arg(
        long,
        global = true,
        default_value = "https://fullnode.mainnet.sui.io:443"
    )]
    rpc_url: String,

    /// Output as JSON instead of human-readable format
    #[arg(long, global = true)]
    json: bool,

    /// Verbose output (show execution traces)
    #[arg(long, short, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Compile and publish a Move package locally
    Publish(PublishCmd),

    /// Execute a single Move function call
    Run(RunCmd),

    /// Execute a Programmable Transaction Block from JSON spec
    Ptb(PtbCmd),

    /// Fetch packages or objects from mainnet
    Fetch(FetchCmd),

    /// Replay a historical transaction locally
    Replay(ReplayCmd),

    /// View modules, objects, or session state
    View(ViewCmd),

    /// Generate sui client commands for deployment (transition helper)
    Bridge(BridgeCmd),

    /// Invoke MCP tools directly from the CLI (JSON in, JSON out)
    Tool(ToolCmd),

    /// Clean session state (remove persisted state file)
    Clean,

    /// Show session status and loaded packages
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Cli {
        command,
        state_file,
        rpc_url,
        json,
        verbose,
    } = Cli::parse();

    match command {
        Commands::Tool(cmd) => {
            let base = sandbox_cli::network::sandbox_home();
            let tool_state_file = state_file.unwrap_or_else(|| base.join("mcp-state.json"));
            cmd.execute(json, Some(tool_state_file.as_path()), &rpc_url)
                .await
        }
        command => {
            // Determine state file path for legacy CLI commands
            let base = sandbox_cli::network::sandbox_home();
            let state_file = state_file.unwrap_or_else(|| base.join("state.bin"));

            // Load or create session state
            let mut state = SandboxState::load_or_create(&state_file, &rpc_url)?;

            let result = match command {
                Commands::Publish(cmd) => cmd.execute(&mut state, json, verbose).await,
                Commands::Run(cmd) => cmd.execute(&mut state, json, verbose).await,
                Commands::Ptb(cmd) => cmd.execute(&mut state, json, verbose).await,
                Commands::Fetch(cmd) => cmd.execute(&mut state, json, verbose).await,
                Commands::Replay(cmd) => cmd.execute(&mut state, json, verbose).await,
                Commands::View(cmd) => cmd.execute(&state, json).await,
                Commands::Bridge(cmd) => cmd.execute(json),
                Commands::Tool(_) => Ok(()),
                Commands::Clean => {
                    if state_file.exists() {
                        std::fs::remove_file(&state_file)?;
                        println!("Removed state file: {}", state_file.display());
                    } else {
                        println!("No state file to remove");
                    }
                    Ok(())
                }
                Commands::Status => {
                    sandbox_cli::output::print_status(&state, json);
                    Ok(())
                }
            };

            // Save state on success
            if result.is_ok() {
                state.save(&state_file)?;
            }

            result
        }
    }
}
