//! Replay command - replay historical transactions locally

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::Serialize;

use super::output::format_error;
use super::SandboxState;

#[derive(Parser, Debug)]
pub struct ReplayCmd {
    /// Transaction digest
    pub digest: String,

    /// Compare local execution with on-chain effects
    #[arg(long)]
    pub compare: bool,

    /// Show detailed execution trace
    #[arg(long, short)]
    pub verbose: bool,
}

#[derive(Debug, Serialize)]
pub struct ReplayOutput {
    pub digest: String,
    pub local_success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ComparisonResult>,
    pub commands_executed: usize,
}

#[derive(Debug, Serialize)]
pub struct ComparisonResult {
    pub status_match: bool,
    pub created_match: bool,
    pub mutated_match: bool,
    pub deleted_match: bool,
    pub on_chain_status: String,
    pub local_status: String,
}

impl ReplayCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let result = self.execute_inner(state, verbose || self.verbose);

        match result {
            Ok(output) => {
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    print_replay_result(&output, self.compare);
                }

                if output.local_success {
                    Ok(())
                } else {
                    Err(anyhow!(output
                        .local_error
                        .unwrap_or_else(|| "Replay failed".to_string())))
                }
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                Err(e)
            }
        }
    }

    fn execute_inner(&self, state: &SandboxState, verbose: bool) -> Result<ReplayOutput> {
        if verbose {
            eprintln!("Fetching transaction {}...", self.digest);
        }

        // Fetch transaction using GraphQL (synchronous)
        let client = sui_data_fetcher::graphql::GraphQLClient::new(&state.rpc_url);

        let tx_data = client
            .fetch_transaction(&self.digest)
            .context("Failed to fetch transaction")?;

        if verbose {
            eprintln!("  Sender: {}", tx_data.sender);
            eprintln!("  Commands: {}", tx_data.commands.len());
            eprintln!("  Inputs: {}", tx_data.inputs.len());
        }

        // Convert to FetchedTransaction format
        let fetched_tx = sui_sandbox_core::tx_replay::graphql_to_fetched_transaction(&tx_data)?;

        if verbose {
            eprintln!("Fetching required packages...");
        }

        // Collect unique packages needed
        let packages_needed: Vec<String> =
            sui_sandbox_core::tx_replay::third_party_packages(&fetched_tx);

        // Create a resolver with packages
        let mut resolver = state.resolver.clone();

        for pkg_id in &packages_needed {
            if verbose {
                eprintln!("  Fetching package: {}", pkg_id);
            }

            match client.fetch_package(pkg_id) {
                Ok(pkg_data) => {
                    let modules: Vec<(String, Vec<u8>)> = pkg_data
                        .modules
                        .iter()
                        .filter_map(|m| {
                            m.bytecode_base64.as_ref().and_then(|b64| {
                                base64::Engine::decode(
                                    &base64::engine::general_purpose::STANDARD,
                                    b64,
                                )
                                .ok()
                                .map(|bytes| (m.name.clone(), bytes))
                            })
                        })
                        .collect();

                    let _ = resolver.add_package_modules(modules);
                }
                Err(e) => {
                    if verbose {
                        eprintln!("    Warning: Failed to fetch {}: {}", pkg_id, e);
                    }
                }
            }
        }

        if verbose {
            eprintln!("Executing locally...");
        }

        // Create harness and replay
        let harness_result = sui_sandbox_core::vm::VMHarness::new(&resolver, false);

        let mut harness = match harness_result {
            Ok(h) => h,
            Err(e) => {
                return Ok(ReplayOutput {
                    digest: self.digest.clone(),
                    local_success: false,
                    local_error: Some(format!("Failed to create VM harness: {}", e)),
                    comparison: None,
                    commands_executed: 0,
                });
            }
        };

        // Replay the transaction
        let replay_result = sui_sandbox_core::tx_replay::replay(&fetched_tx, &mut harness);

        match replay_result {
            Ok(result) => {
                let comparison = if self.compare {
                    result.comparison.map(|c| ComparisonResult {
                        status_match: c.status_match,
                        created_match: c.created_count_match,
                        mutated_match: c.mutated_count_match,
                        deleted_match: c.deleted_count_match,
                        on_chain_status: if c.status_match && result.local_success {
                            "success".to_string()
                        } else if c.status_match && !result.local_success {
                            "failed".to_string()
                        } else {
                            "unknown".to_string()
                        },
                        local_status: if result.local_success {
                            "success".to_string()
                        } else {
                            "failed".to_string()
                        },
                    })
                } else {
                    None
                };

                Ok(ReplayOutput {
                    digest: self.digest.clone(),
                    local_success: result.local_success,
                    local_error: result.local_error,
                    comparison,
                    commands_executed: result.commands_executed,
                })
            }
            Err(e) => Ok(ReplayOutput {
                digest: self.digest.clone(),
                local_success: false,
                local_error: Some(e.to_string()),
                comparison: None,
                commands_executed: 0,
            }),
        }
    }
}

fn print_replay_result(result: &ReplayOutput, show_comparison: bool) {
    println!("\x1b[1mTransaction Replay: {}\x1b[0m\n", result.digest);

    if result.local_success {
        println!("\x1b[32m✓ Local execution succeeded\x1b[0m");
    } else {
        println!("\x1b[31m✗ Local execution failed\x1b[0m");
        if let Some(err) = &result.local_error {
            println!("  Error: {}", err);
        }
    }

    println!("  Commands executed: {}", result.commands_executed);

    if show_comparison {
        if let Some(cmp) = &result.comparison {
            println!("\n\x1b[1mComparison with on-chain:\x1b[0m");
            println!(
                "  Status: {} (local: {}, on-chain: {})",
                if cmp.status_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[31m✗ mismatch\x1b[0m"
                },
                cmp.local_status,
                cmp.on_chain_status
            );
            println!(
                "  Created objects: {}",
                if cmp.created_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
            println!(
                "  Mutated objects: {}",
                if cmp.mutated_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
            println!(
                "  Deleted objects: {}",
                if cmp.deleted_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
        } else {
            println!("\n\x1b[33mNote: No on-chain effects available for comparison\x1b[0m");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_output_serialization() {
        let output = ReplayOutput {
            digest: "test123".to_string(),
            local_success: true,
            local_error: None,
            comparison: Some(ComparisonResult {
                status_match: true,
                created_match: true,
                mutated_match: true,
                deleted_match: true,
                on_chain_status: "success".to_string(),
                local_status: "success".to_string(),
            }),
            commands_executed: 3,
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"local_success\":true"));
        assert!(json.contains("\"status_match\":true"));
    }
}
