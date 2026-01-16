//! Command-line interface for sandbox execution.
//!
//! Provides single-shot and interactive modes for running sandbox commands.

use anyhow::{anyhow, Result};
use std::io::Read;

use super::{execute_request, SandboxRequest, SandboxResponse};
use crate::args::SandboxExecArgs;
use crate::benchmark::simulation::SimulationEnvironment;

/// Run the sandbox execution command.
pub fn run_sandbox_exec(args: &SandboxExecArgs) -> Result<()> {
    // Interactive mode - read JSON lines from stdin, write responses to stdout
    if args.interactive {
        return run_interactive_sandbox(args);
    }

    // Single-shot mode
    // Read input
    let input_json: String = if args.input.as_os_str() == "-" {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        std::fs::read_to_string(&args.input)?
    };

    // Parse request
    let request: SandboxRequest = serde_json::from_str(&input_json)
        .map_err(|e| anyhow!("Failed to parse request JSON: {}", e))?;

    if args.verbose {
        eprintln!("Received request: {:?}", request);
    }

    // Create environment - load from state file if provided
    let mut env = if let Some(ref state_file) = args.state_file {
        if state_file.exists() {
            if args.verbose {
                eprintln!("Loading state from {}", state_file.display());
            }
            SimulationEnvironment::from_state_file(state_file)?
        } else {
            if args.verbose {
                eprintln!(
                    "State file {} does not exist, creating new environment",
                    state_file.display()
                );
            }
            SimulationEnvironment::new()?
        }
    } else {
        SimulationEnvironment::new()?
    };

    if args.enable_fetching {
        env = env.with_mainnet_fetching();
    }

    // Load bytecode from persistent directory if specified
    if let Some(ref bytecode_dir) = args.bytecode_dir {
        if bytecode_dir.exists() {
            if args.verbose {
                eprintln!("Loading bytecode from {}", bytecode_dir.display());
            }
            let load_req = SandboxRequest::LoadModule {
                bytecode_path: bytecode_dir.to_string_lossy().to_string(),
                module_name: None,
            };
            let _ = execute_request(&mut env, &load_req, args.verbose);
        }
    }

    // Execute request
    let response = execute_request(&mut env, &request, args.verbose);

    // Save state after execution if state file is specified and saving is enabled
    if let Some(ref state_file) = args.state_file {
        if !args.no_save_state {
            if args.verbose {
                eprintln!("Saving state to {}", state_file.display());
            }
            if let Err(e) = env.save_state(state_file) {
                eprintln!("Warning: Failed to save state: {}", e);
            }
        }
    }

    // Write output
    let output_json = serde_json::to_string_pretty(&response)?;

    if args.output.as_os_str() == "-" {
        println!("{}", output_json);
    } else {
        std::fs::write(&args.output, output_json)?;
    }

    Ok(())
}

/// Run sandbox in interactive mode - JSON line protocol.
fn run_interactive_sandbox(args: &SandboxExecArgs) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};

    // Create environment
    let mut env = if let Some(ref state_file) = args.state_file {
        if state_file.exists() {
            if args.verbose {
                eprintln!("Loading state from {}", state_file.display());
            }
            SimulationEnvironment::from_state_file(state_file)?
        } else {
            SimulationEnvironment::new()?
        }
    } else {
        SimulationEnvironment::new()?
    };

    if args.enable_fetching {
        env = env.with_mainnet_fetching();
    }

    // Load bytecode from persistent directory if specified
    if let Some(ref bytecode_dir) = args.bytecode_dir {
        if bytecode_dir.exists() {
            if args.verbose {
                eprintln!("Loading bytecode from {}", bytecode_dir.display());
            }
            let load_req = SandboxRequest::LoadModule {
                bytecode_path: bytecode_dir.to_string_lossy().to_string(),
                module_name: None,
            };
            let _ = execute_request(&mut env, &load_req, args.verbose);
        }
    }

    if args.verbose {
        eprintln!("Interactive sandbox ready. Reading JSON lines from stdin...");
    }

    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                if args.verbose {
                    eprintln!("Error reading line: {}", e);
                }
                break;
            }
        };

        // Skip empty lines
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse request
        let request: SandboxRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let error_response = SandboxResponse::error_with_category(
                    format!("JSON parse error: {}", e),
                    "ParseError",
                );
                let json = serde_json::to_string(&error_response).unwrap_or_default();
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
                continue;
            }
        };

        if args.verbose {
            eprintln!("Request: {:?}", request);
        }

        // Execute request
        let response = execute_request(&mut env, &request, args.verbose);

        // Write response as single JSON line
        let json = serde_json::to_string(&response)?;
        writeln!(stdout, "{}", json)?;
        stdout.flush()?;

        // Optionally save state after each request
        if let Some(ref state_file) = args.state_file {
            if !args.no_save_state {
                if let Err(e) = env.save_state(state_file) {
                    if args.verbose {
                        eprintln!("Warning: Failed to save state: {}", e);
                    }
                }
            }
        }
    }

    if args.verbose {
        eprintln!("Interactive sandbox exiting.");
    }

    Ok(())
}
