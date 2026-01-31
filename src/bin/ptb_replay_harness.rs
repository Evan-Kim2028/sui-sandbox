//! Internal PTB replay harness for robustness testing.
//!
//! This is not user-facing. It samples PTBs from Walrus checkpoints, filters out
//! trivial framework-only transactions, and then replays the remaining digests
//! via the sui-sandbox CLI to validate parity.

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;

use sui_sandbox::ptb_classifier::{classify_ptb, PtbClassification};
use sui_transport::graphql::GraphQLClient;
use sui_transport::walrus::WalrusClient;
use sui_types::transaction::{TransactionDataAPI, TransactionKind};

#[derive(Parser, Debug)]
#[command(name = "ptb-replay-harness")]
struct Args {
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

    /// Max seconds to wait per replay before aborting
    #[arg(long, default_value_t = 90)]
    replay_timeout_secs: u64,

    /// Capture timing lines from stderr (prefixed with [timing])
    #[arg(long, default_value_t = true)]
    capture_timing: bool,

    /// Max timing lines to keep per replay
    #[arg(long, default_value_t = 200)]
    max_timing_lines: usize,

    /// Number of parallel replays to run
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: usize,

    /// Use MCP replay tool instead of CLI replay
    #[arg(long, default_value_t = false)]
    mcp: bool,
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

fn main() -> Result<()> {
    let args = Args::parse();
    let output_path = args.output.clone();
    let start_time = Instant::now();

    let walrus = match args.network.as_str() {
        "testnet" => WalrusClient::testnet(),
        _ => WalrusClient::mainnet(),
    };
    let graphql = match args.network.as_str() {
        "testnet" => GraphQLClient::testnet(),
        _ => GraphQLClient::mainnet(),
    };

    let latest = walrus
        .get_latest_checkpoint()
        .context("walrus latest checkpoint")?;
    let count = args.count.max(1);
    let start_checkpoint = args
        .start_checkpoint
        .unwrap_or_else(|| latest.saturating_sub(count - 1));

    let checkpoints: Vec<u64> = (start_checkpoint..start_checkpoint + count).collect();

    println!(
        "PTB replay harness: network={} checkpoints={}..={} ({} total)",
        args.network,
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
            if seen >= args.max_per_checkpoint {
                break;
            }
            let kind = tx.transaction.data().transaction_data().kind();
            if !matches!(kind, TransactionKind::ProgrammableTransaction(_)) {
                continue;
            }
            let digest = tx.transaction.digest().to_string();
            digests.push((digest, *checkpoint));
            seen += 1;
            if digests.len() >= args.max_total {
                break;
            }
        }
        if digests.len() >= args.max_total {
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
            .open(&args.output)
            .context("open output file")?,
    );

    let sandbox_bin = resolve_sandbox_binary().context("locate sui-sandbox binary")?;
    let mut seen_digests: BTreeSet<String> = BTreeSet::new();

    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    let mut processed = 0usize;
    let mut skipped_trivial = 0usize;
    let mut replay_success = 0usize;
    let mut replay_fail = 0usize;

    let mut work_items: Vec<WorkItem> = Vec::new();

    for (digest, checkpoint) in digests {
        if !seen_digests.insert(digest.clone()) {
            continue;
        }
        let tx = match graphql.fetch_transaction(&digest) {
            Ok(tx) => tx,
            Err(err) => {
                eprintln!("[harness] graphql fetch failed digest={}: {}", digest, err);
                continue;
            }
        };

        let classification = classify_ptb(&tx);
        if args.skip_trivial && classification.is_trivial_framework {
            skipped_trivial += 1;
            continue;
        }

        for tag in &classification.tags {
            *stats.entry(tag.clone()).or_insert(0) += 1;
        }

        work_items.push(WorkItem {
            digest: digest.clone(),
            checkpoint,
            classification,
        });
        processed += 1;
    }

    if args.dry_run {
        for item in work_items {
            let record = HarnessRecord {
                digest: item.digest,
                checkpoint: Some(item.checkpoint),
                classification: item.classification,
                replay: None,
            };
            let line = serde_json::to_string(&record)?;
            writeln!(out, "{}", line)?;
        }
        out.flush()?;
    } else {
        let total = work_items.len();
        let worker_count = args.concurrency.max(1).min(total.max(1));
        let (task_tx, task_rx) = std::sync::mpsc::channel::<WorkItem>();
        let (result_tx, result_rx) =
            std::sync::mpsc::channel::<(WorkItem, ReplaySummary)>();
        let task_rx = Arc::new(Mutex::new(task_rx));
        let args = Arc::new(args);
        let sandbox_bin = Arc::new(sandbox_bin);
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let task_rx = Arc::clone(&task_rx);
            let result_tx = result_tx.clone();
            let args = Arc::clone(&args);
            let sandbox_bin = Arc::clone(&sandbox_bin);
            handles.push(std::thread::spawn(move || loop {
                let item = {
                    let guard = task_rx.lock().ok();
                    match guard {
                        Some(rx) => rx.recv(),
                        None => return,
                    }
                };
                let item = match item {
                    Ok(item) => item,
                    Err(_) => break,
                };
                let replay = match run_replay(&sandbox_bin, &args, &item.digest, item.checkpoint) {
                    Ok(rep) => rep,
                    Err(err) => ReplaySummary {
                        local_success: false,
                        local_error: Some(err.to_string()),
                        comparison: None,
                        timing: Vec::new(),
                    },
                };
                if result_tx.send((item, replay)).is_err() {
                    break;
                }
            }));
        }

        for item in work_items {
            let _ = task_tx.send(item);
        }
        drop(task_tx);

        for _ in 0..total {
            if let Ok((item, replay)) = result_rx.recv() {
                if replay.local_success {
                    replay_success += 1;
                } else {
                    replay_fail += 1;
                }
                let record = HarnessRecord {
                    digest: item.digest,
                    checkpoint: Some(item.checkpoint),
                    classification: item.classification,
                    replay: Some(replay),
                };
                let line = serde_json::to_string(&record)?;
                writeln!(out, "{}", line)?;
            }
        }
        out.flush()?;

        for handle in handles {
            let _ = handle.join();
        }
    }

    println!(
        "Processed={} skipped_trivial={} success={} fail={} elapsed={}s output={}",
        processed,
        skipped_trivial,
        replay_success,
        replay_fail,
        start_time.elapsed().as_secs(),
        output_path.display()
    );

    if !stats.is_empty() {
        println!("Classification tags:");
        for (tag, count) in stats {
            println!("  {:<20} {}", tag, count);
        }
    }

    Ok(())
}

fn resolve_sandbox_binary() -> Result<PathBuf> {
    let mut exe = std::env::current_exe().context("current_exe")?;
    exe.set_file_name(if cfg!(windows) {
        "sui-sandbox.exe"
    } else {
        "sui-sandbox"
    });
    Ok(exe)
}

fn run_replay(
    sandbox_bin: &PathBuf,
    args: &Args,
    digest: &str,
    _checkpoint: u64,
) -> Result<ReplaySummary> {
    let mut cmd = Command::new(sandbox_bin);
    if args.mcp {
        let mut input = serde_json::json!({ "digest": digest });
        let mut options = serde_json::Map::new();
        if args.compare {
            options.insert("compare_effects".to_string(), serde_json::json!(true));
        }
        if args.self_heal_dynamic_fields {
            options.insert(
                "self_heal_dynamic_fields".to_string(),
                serde_json::json!(true),
            );
        }
        if args.synthesize_missing {
            options.insert("synthesize_missing".to_string(), serde_json::json!(true));
        }
        if !options.is_empty() {
            input["options"] = serde_json::Value::Object(options);
        }
        cmd.arg("--json")
            .arg("--rpc-url")
            .arg(&args.rpc_url)
            .arg("tool")
            .arg("replay_transaction")
            .arg("--input")
            .arg(input.to_string());
    } else {
        cmd.arg("--json")
            .arg("--rpc-url")
            .arg(&args.rpc_url)
            .arg("replay")
            .arg(digest);
        if args.compare {
            cmd.arg("--compare");
        }
        if args.self_heal_dynamic_fields {
            cmd.arg("--self-heal-dynamic-fields");
        }
        if args.synthesize_missing {
            cmd.arg("--synthesize-missing");
        }
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("spawn sui-sandbox replay")?;
    let mut stdout = child.stdout.take().context("stdout pipe")?;
    let mut stderr = child.stderr.take().context("stderr pipe")?;

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let timeout = Duration::from_secs(args.replay_timeout_secs);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("check replay status")? {
            break Some(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    if status.is_none() {
        let _ = stdout_handle.join();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();
        let timing = if args.capture_timing {
            extract_timing_lines(&stderr_bytes, args.max_timing_lines)
        } else {
            Vec::new()
        };
        return Ok(ReplaySummary {
            local_success: false,
            local_error: Some("timeout".to_string()),
            comparison: None,
            timing,
        });
    }

    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();
    let stdout_str = String::from_utf8_lossy(&stdout_bytes);
    let timing = if args.capture_timing {
        extract_timing_lines(&stderr_bytes, args.max_timing_lines)
    } else {
        Vec::new()
    };

    let parsed: Value = serde_json::from_str(&stdout_str).unwrap_or_else(|_| {
        let stderr_str = String::from_utf8_lossy(&stderr_bytes);
        serde_json::json!({
            "local_success": false,
            "local_error": format!("invalid json output: {} / stderr: {}", stdout_str, stderr_str),
        })
    });

    if args.mcp {
        let success = parsed
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !success {
            return Ok(ReplaySummary {
                local_success: false,
                local_error: parsed
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                comparison: None,
                timing,
            });
        }
        let result = parsed.get("result").cloned().unwrap_or(Value::Null);
        return Ok(ReplaySummary {
            local_success: result
                .get("local_success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            local_error: result
                .get("local_error")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            comparison: result.get("comparison").cloned(),
            timing,
        });
    }

    Ok(ReplaySummary {
        local_success: parsed
            .get("local_success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        local_error: parsed
            .get("local_error")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        comparison: parsed.get("comparison").cloned(),
        timing,
    })
}

fn extract_timing_lines(stderr_bytes: &[u8], max_lines: usize) -> Vec<String> {
    let stderr_str = String::from_utf8_lossy(stderr_bytes);
    let mut out = Vec::new();
    for line in stderr_str.lines() {
        if line.starts_with("[timing]") {
            out.push(line.to_string());
            if out.len() >= max_lines {
                break;
            }
        }
    }
    out
}
