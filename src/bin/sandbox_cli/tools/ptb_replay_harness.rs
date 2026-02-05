//! Internal PTB replay harness for robustness testing.

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use sui_sandbox::ptb_classifier::{classify_ptb, PtbClassification};
use sui_transport::graphql::GraphQLClient;
use sui_transport::walrus::WalrusClient;
use sui_types::transaction::{TransactionDataAPI, TransactionKind};

#[derive(Parser, Debug)]
#[command(name = "ptb-replay-harness")]
pub struct PtbReplayHarnessCmd {
    /// Network (mainnet or testnet)
    #[arg(long, default_value = "mainnet")]
    network: String,

    /// Starting checkpoint (defaults to latest - count + 1)
    #[arg(long)]
    start_checkpoint: Option<u64>,

    /// Number of checkpoints to scan
    #[arg(long, default_value_t = 10)]
    count: u64,

    /// Max PTBs per checkpoint to consider
    #[arg(long, default_value_t = 50)]
    max_per_checkpoint: usize,

    /// Max total PTBs to replay
    #[arg(long, default_value_t = 200)]
    max_total: usize,

    /// Skip trivial framework-only PTBs
    #[arg(long, default_value_t = true)]
    skip_trivial: bool,

    /// Do not run replay; only classify + emit digests
    #[arg(long, default_value_t = false)]
    dry_run: bool,

    /// Compare local effects against on-chain
    #[arg(long, default_value_t = true)]
    compare: bool,

    /// Self-heal dynamic fields (synthesize values)
    #[arg(long, default_value_t = true)]
    self_heal_dynamic_fields: bool,

    /// Synthesize missing input objects on replay failure
    #[arg(long, default_value_t = false)]
    synthesize_missing: bool,

    /// RPC URL for gRPC replay
    #[arg(long, default_value = "https://fullnode.mainnet.sui.io:443")]
    rpc_url: String,

    /// Output JSONL path
    #[arg(long, default_value = "logs/ptb_replay_harness.jsonl")]
    output: PathBuf,

    /// Capture timing lines from stderr (prefixed with [timing])
    #[arg(long, default_value_t = true)]
    capture_timing: bool,

    /// Max timing lines to keep per replay
    #[arg(long, default_value_t = 200)]
    max_timing_lines: usize,

    /// Number of parallel replays to run
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReplaySummary {
    local_success: bool,
    local_error: Option<String>,
    comparison: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    timing: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HarnessRecord {
    digest: String,
    checkpoint: Option<u64>,
    classification: PtbClassification,
    replay: Option<ReplaySummary>,
}

#[derive(Debug, Clone)]
struct WorkItem {
    digest: String,
    checkpoint: u64,
    classification: PtbClassification,
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(4)
}

impl PtbReplayHarnessCmd {
    pub fn execute(&self) -> Result<()> {
        let output_path = self.output.clone();
        let start_time = Instant::now();

        let walrus = match self.network.as_str() {
            "testnet" => WalrusClient::testnet(),
            _ => WalrusClient::mainnet(),
        };
        let graphql = match self.network.as_str() {
            "testnet" => GraphQLClient::testnet(),
            _ => GraphQLClient::mainnet(),
        };

        let latest = walrus
            .get_latest_checkpoint()
            .context("walrus latest checkpoint")?;
        let count = self.count.max(1);
        let start_checkpoint = self
            .start_checkpoint
            .unwrap_or_else(|| latest.saturating_sub(count - 1));

        let checkpoints: Vec<u64> = (start_checkpoint..start_checkpoint + count).collect();

        println!(
            "PTB replay harness: network={} checkpoints={}..={} ({} total)",
            self.network,
            checkpoints.first().copied().unwrap_or(start_checkpoint),
            checkpoints.last().copied().unwrap_or(start_checkpoint),
            checkpoints.len()
        );

        let batched = walrus
            .get_checkpoints_batched(&checkpoints, 8 * 1024 * 1024)
            .context("walrus checkpoint batch fetch")?;

        let mut digests: Vec<(String, u64)> = Vec::new();
        for (checkpoint, data) in &batched {
            let mut seen = 0usize;
            for tx in &data.transactions {
                if seen >= self.max_per_checkpoint {
                    break;
                }
                let kind = tx.transaction.data().transaction_data().kind();
                if !matches!(kind, TransactionKind::ProgrammableTransaction(_)) {
                    continue;
                }
                let digest = tx.transaction.digest().to_string();
                digests.push((digest, *checkpoint));
                seen += 1;
                if digests.len() >= self.max_total {
                    break;
                }
            }
            if digests.len() >= self.max_total {
                break;
            }
        }

        if digests.is_empty() {
            println!("No PTB digests found in checkpoint range.");
            return Ok(());
        }

        let mut out = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.output)
                .context("open output file")?,
        );

        let sandbox_bin = resolve_sandbox_binary().context("locate sui-sandbox binary")?;
        let mut seen_digests: BTreeSet<String> = BTreeSet::new();

        let mut stats: BTreeMap<String, usize> = BTreeMap::new();
        let mut processed = 0usize;

        let mut work: Vec<WorkItem> = Vec::new();

        for (digest, checkpoint) in digests {
            if seen_digests.contains(&digest) {
                continue;
            }
            let tx = graphql
                .fetch_transaction(&digest)
                .with_context(|| format!("fetch transaction {digest}"))?;
            let classification = classify_ptb(&tx);
            if self.skip_trivial && classification.is_framework_only {
                *stats.entry("trivial_skipped".to_string()).or_insert(0) += 1;
                continue;
            }
            work.push(WorkItem {
                digest: digest.clone(),
                checkpoint,
                classification,
            });
            seen_digests.insert(digest);
        }

        if self.dry_run {
            for item in work {
                let record = HarnessRecord {
                    digest: item.digest,
                    checkpoint: Some(item.checkpoint),
                    classification: item.classification,
                    replay: None,
                };
                writeln!(out, "{}", serde_json::to_string(&record)?)?;
            }
            out.flush()?;
            return Ok(());
        }

        let work = Arc::new(Mutex::new(work));
        let results: Arc<Mutex<Vec<HarnessRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..self.concurrency {
            let work = Arc::clone(&work);
            let results = Arc::clone(&results);
            let sandbox_bin = sandbox_bin.clone();
            let rpc_url = self.rpc_url.clone();
            let capture_timing = self.capture_timing;
            let max_timing_lines = self.max_timing_lines;
            let compare = self.compare;
            let self_heal_dynamic_fields = self.self_heal_dynamic_fields;
            let synthesize_missing = self.synthesize_missing;

            let handle = std::thread::spawn(move || loop {
                let item = {
                    let mut guard = work.lock().unwrap();
                    guard.pop()
                };

                let Some(item) = item else { break };

                let replay = run_replay(
                    &sandbox_bin,
                    &rpc_url,
                    &item.digest,
                    compare,
                    self_heal_dynamic_fields,
                    synthesize_missing,
                    capture_timing,
                    max_timing_lines,
                );

                let record = HarnessRecord {
                    digest: item.digest,
                    checkpoint: Some(item.checkpoint),
                    classification: item.classification,
                    replay: Some(replay),
                };

                results.lock().unwrap().push(record);
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.join();
        }

        let mut results = results.lock().unwrap();
        results.sort_by(|a, b| a.digest.cmp(&b.digest));

        for record in results.iter() {
            writeln!(out, "{}", serde_json::to_string(record)?)?;
            processed += 1;
        }

        out.flush()?;

        let elapsed = start_time.elapsed().as_secs();
        println!(
            "PTB replay harness complete: processed={} elapsed={}s output={}",
            processed,
            elapsed,
            output_path.display()
        );

        Ok(())
    }
}

fn resolve_sandbox_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolve current exe")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve current exe dir"))?;
    let mut candidate = dir.join("sui-sandbox");
    if candidate.exists() {
        return Ok(candidate);
    }
    candidate = dir.join("sui_sandbox");
    if candidate.exists() {
        return Ok(candidate);
    }
    Ok(PathBuf::from("sui-sandbox"))
}

#[allow(clippy::too_many_arguments)]
fn run_replay(
    sandbox_bin: &PathBuf,
    rpc_url: &str,
    digest: &str,
    compare: bool,
    self_heal_dynamic_fields: bool,
    synthesize_missing: bool,
    capture_timing: bool,
    max_timing_lines: usize,
) -> ReplaySummary {
    let mut cmd = Command::new(sandbox_bin);
    cmd.arg("replay");
    cmd.arg(digest);
    cmd.arg("--rpc-url");
    cmd.arg(rpc_url);
    cmd.arg("--verbose");

    if compare {
        cmd.arg("--compare");
    }
    if self_heal_dynamic_fields {
        cmd.arg("--self-heal");
    }
    if synthesize_missing {
        cmd.arg("--synthesize");
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            return ReplaySummary {
                local_success: false,
                local_error: Some(format!("spawn replay: {}", e)),
                comparison: None,
                timing: Vec::new(),
            }
        }
    };

    let mut timing: Vec<String> = Vec::new();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let mut comparison: Option<Value> = None;
    let local_success = output.status.success();
    let mut local_error = None;

    if capture_timing {
        for line in stderr.lines() {
            if line.contains("[timing]") {
                timing.push(line.to_string());
                if timing.len() >= max_timing_lines {
                    break;
                }
            }
        }
    }

    if compare {
        if let Some(json_line) = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
        {
            if let Ok(val) = serde_json::from_str::<Value>(json_line) {
                comparison = val.get("comparison").cloned();
            }
        }
    }

    if !output.status.success() {
        local_error = Some(stderr.lines().last().unwrap_or("replay failed").to_string());
    }

    ReplaySummary {
        local_success,
        local_error,
        comparison,
        timing,
    }
}
