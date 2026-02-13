//! Import command - ingest replay data files into a local cache.

use anyhow::{anyhow, Result};
use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

use super::network::sandbox_home;
use super::output::format_error;
use super::SandboxState;
use sui_state_fetcher::{import_replay_states, ImportSpec};

#[derive(Parser, Debug)]
pub struct ImportCmd {
    /// Single replay state JSON file (strict or extended schema)
    #[arg(long)]
    pub state: Option<PathBuf>,

    /// Transactions input file (JSON/JSONL/CSV)
    #[arg(long)]
    pub transactions: Option<PathBuf>,

    /// Objects input file (JSON/JSONL/CSV)
    #[arg(long)]
    pub objects: Option<PathBuf>,

    /// Packages input file (JSON/JSONL/CSV)
    #[arg(long)]
    pub packages: Option<PathBuf>,

    /// Output cache directory for imported replay states
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct ImportOutput {
    cache_dir: String,
    states_imported: usize,
    objects_imported: usize,
    packages_imported: usize,
    digests: Vec<String>,
}

impl ImportCmd {
    pub async fn execute(
        &self,
        _state: &mut SandboxState,
        json_output: bool,
        _verbose: bool,
    ) -> Result<()> {
        if self.state.is_none() && self.transactions.is_none() {
            let err = anyhow!("Provide --state or --transactions");
            eprintln!("{}", format_error(&err, json_output));
            return Err(err);
        }

        let output_dir = self
            .output
            .clone()
            .unwrap_or_else(|| sandbox_home().join("cache").join("local"));
        let spec = ImportSpec {
            state: self.state.clone(),
            transactions: self.transactions.clone(),
            objects: self.objects.clone(),
            packages: self.packages.clone(),
        };

        let result = import_replay_states(&output_dir, &spec);
        match result {
            Ok(summary) => {
                let out = ImportOutput {
                    cache_dir: summary.cache_dir.display().to_string(),
                    states_imported: summary.states_imported,
                    objects_imported: summary.objects_imported,
                    packages_imported: summary.packages_imported,
                    digests: summary.digests,
                };

                if json_output {
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("Imported {} replay state(s)", out.states_imported);
                    println!("Cache dir: {}", out.cache_dir);
                    if out.objects_imported > 0 || out.packages_imported > 0 {
                        println!(
                            "Attached rows: objects={} packages={}",
                            out.objects_imported, out.packages_imported
                        );
                    }
                    if !out.digests.is_empty() {
                        println!("Digests:");
                        for digest in out.digests {
                            println!("  {}", digest);
                        }
                    }
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                Err(e)
            }
        }
    }
}
