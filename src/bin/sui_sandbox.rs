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
//! - **analyze**: Package and replay-state introspection
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

#[cfg(feature = "analysis")]
use sandbox_cli::analyze::AnalyzeCmd;
use sandbox_cli::{
    bridge::BridgeCmd,
    fetch::FetchCmd,
    flow::{InitCmd, RunFlowCmd},
    ptb::PtbCmd,
    publish::PublishCmd,
    replay::ReplayCli,
    run::RunCmd,
    snapshot::SnapshotCmd,
    test::TestCli,
    tools::ToolsCmd,
    view::ViewCmd,
    SandboxState,
};

#[derive(Parser)]
#[command(
    name = "sui-sandbox",
    author,
    version,
    about = "Local Move/Sui development environment",
    long_about = "A powerful CLI for local Sui Move development, testing, and simulation.\n\n\
                  Provides replay, publish, run, PTB execution, analysis, and state fetching."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// State file for session persistence
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

    /// Emit structured debug diagnostics JSON for failures
    #[arg(long, global = true)]
    debug_json: bool,

    /// Verbose output (show execution traces)
    #[arg(long, short, global = true)]
    verbose: bool,
}

#[allow(clippy::large_enum_variant)]
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
    Replay(ReplayCli),

    /// Analyze packages or replay state
    #[cfg(feature = "analysis")]
    Analyze(AnalyzeCmd),

    /// View modules, objects, or session state
    View(ViewCmd),

    /// Generate sui client commands for deployment (transition helper)
    Bridge(BridgeCmd),

    /// Test Move functions (fuzz, property-based, coverage)
    Test(TestCli),

    /// Extra utilities (polling, streaming, tx simulation)
    Tools(ToolsCmd),

    /// Scaffold a task-oriented project/workflow template
    Init(InitCmd),

    /// Execute a YAML flow file with deterministic step sequencing
    RunFlow(RunFlowCmd),

    /// Save/list/load/delete named local sandbox snapshots
    Snapshot(SnapshotCmd),

    /// Reset in-memory session state while keeping configuration
    Reset,

    /// Clean session state (remove persisted state file)
    Clean,

    /// Show session status and loaded packages
    Status,
}

impl Commands {
    fn name(&self) -> &'static str {
        match self {
            Commands::Publish(_) => "publish",
            Commands::Run(_) => "run",
            Commands::Ptb(_) => "ptb",
            Commands::Fetch(_) => "fetch",
            Commands::Replay(_) => "replay",
            #[cfg(feature = "analysis")]
            Commands::Analyze(_) => "analyze",
            Commands::View(_) => "view",
            Commands::Bridge(_) => "bridge",
            Commands::Test(_) => "test",
            Commands::Tools(_) => "tools",
            Commands::Init(_) => "init",
            Commands::RunFlow(_) => "run-flow",
            Commands::Snapshot(_) => "snapshot",
            Commands::Reset => "reset",
            Commands::Clean => "clean",
            Commands::Status => "status",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Cli {
        command,
        state_file,
        rpc_url,
        json,
        debug_json,
        verbose,
    } = Cli::parse();
    let base = sandbox_cli::network::sandbox_home();
    let state_file = state_file.unwrap_or_else(|| base.join("state.json"));
    let command_name = command.name().to_string();

    if debug_json {
        std::env::set_var("SUI_SANDBOX_DEBUG_JSON", "1");
    }

    // Load or create session state
    let mut state = SandboxState::load_or_create(&state_file, &rpc_url)?;

    let result = match command {
        Commands::Publish(cmd) => cmd.execute(&mut state, json, verbose).await,
        Commands::Run(cmd) => cmd.execute(&mut state, json, verbose).await,
        Commands::Ptb(cmd) => cmd.execute(&mut state, json, verbose).await,
        Commands::Fetch(cmd) => cmd.execute(&mut state, json, verbose).await,
        Commands::Replay(cmd) => cmd.execute(&mut state, json, verbose).await,
        #[cfg(feature = "analysis")]
        Commands::Analyze(cmd) => cmd.execute(&mut state, json, verbose).await,
        Commands::View(cmd) => cmd.execute(&state, json).await,
        Commands::Bridge(cmd) => cmd.execute(json),
        Commands::Test(cmd) => cmd.execute(&mut state, json, verbose).await,
        Commands::Tools(cmd) => cmd.execute(json).await,
        Commands::Init(cmd) => cmd.execute().await,
        Commands::RunFlow(cmd) => cmd.execute(&state_file, &rpc_url, json, verbose).await,
        Commands::Snapshot(cmd) => cmd.execute(&mut state, &state_file, json).await,
        Commands::Reset => {
            state.reset_session()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "success": true,
                        "message": "Session reset",
                    }))?
                );
            } else {
                println!("Session reset");
            }
            Ok(())
        }
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
            sandbox_cli::output::print_status(&state, json, Some(&state_file));
            Ok(())
        }
    };

    if debug_json {
        if let Err(err) = &result {
            eprintln!(
                "{}",
                sandbox_cli::output::format_debug_diagnostic_json(
                    &command_name,
                    err,
                    None,
                    sandbox_cli::output::default_diagnostic_hints(&command_name, err),
                )
            );
        }
    }

    // Save state on success
    if result.is_ok() {
        state.save(&state_file)?;
    }

    result
}
