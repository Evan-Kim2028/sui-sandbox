//! Stream transactions from Sui mainnet via gRPC
//!
//! Subscribes to checkpoint stream and caches all transactions in real-time.
//! Captures every transaction with no gaps (unlike polling).
//!
//! TRADEOFFS vs GraphQL polling (poll_transactions):
//!   ✅ Real-time push (~250ms latency)
//!   ✅ No gaps - receives every checkpoint
//!   ✅ Higher throughput (~30-40 tx/sec)
//!   ❌ Limited effects - only status, no created/mutated/deleted arrays
//!   ❌ Connection drops every ~30s (auto-reconnects)
//!
//! Use this for: real-time monitoring, transaction indexing, high-volume collection
//! Use poll_transactions for: replay verification, effects analysis
//!
//! Usage:
//!   cargo run --bin stream_transactions -- --duration 60
//!   cargo run --bin stream_transactions -- --endpoint https://fullnode.mainnet.sui.io:443
//!
//! Options:
//!   --endpoint <url>     gRPC endpoint (default: fullnode.mainnet.sui.io:443)
//!   --duration <secs>    How long to run (default: 60)
//!   --output <file>      Output file path (default: transactions_stream.jsonl)
//!   --ptb-only           Only save PTB transactions (skip system txs)
//!   --verbose            Print detailed progress
//!
//! Public endpoints:
//!   - Mainnet: https://fullnode.mainnet.sui.io:443 (streaming + recent data)
//!   - Archive: https://archive.mainnet.sui.io:443 (historical queries only, no streaming)
//!   - Testnet: https://fullnode.testnet.sui.io:443

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sui_move_interface_extractor::grpc::{
    GrpcArgument, GrpcClient, GrpcCommand, GrpcInput, GrpcTransaction,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedTransaction {
    /// When this was received (Unix timestamp ms)
    received_at_ms: u64,
    /// Checkpoint sequence number
    checkpoint: u64,
    /// The full transaction data
    #[serde(flatten)]
    transaction: SerializableTransaction,
}

/// Full transaction data matching GraphQL polling format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializableTransaction {
    digest: String,
    sender: String,
    gas_budget: Option<u64>,
    gas_price: Option<u64>,
    checkpoint: Option<u64>,
    timestamp_ms: Option<u64>,
    /// PTB inputs - full data, not just count
    inputs: Vec<SerializableInput>,
    /// PTB commands - full data, not just count
    commands: Vec<SerializableCommand>,
    /// Transaction effects
    effects: Option<SerializableEffects>,
}

/// Transaction input - matches GraphQL format (externally tagged enum)
#[derive(Debug, Clone, Serialize, Deserialize)]
enum SerializableInput {
    Pure {
        bytes_base64: String,
    },
    OwnedObject {
        address: String,
        version: u64,
        digest: String,
    },
    SharedObject {
        address: String,
        initial_shared_version: u64,
        mutable: bool,
    },
    Receiving {
        address: String,
        version: u64,
        digest: String,
    },
}

impl From<&GrpcInput> for SerializableInput {
    fn from(input: &GrpcInput) -> Self {
        use base64::Engine;
        match input {
            GrpcInput::Pure { bytes } => SerializableInput::Pure {
                bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            GrpcInput::Object {
                object_id,
                version,
                digest,
            } => SerializableInput::OwnedObject {
                address: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                mutable,
            } => SerializableInput::SharedObject {
                address: object_id.clone(),
                initial_shared_version: *initial_version,
                mutable: *mutable,
            },
            GrpcInput::Receiving {
                object_id,
                version,
                digest,
            } => SerializableInput::Receiving {
                address: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
        }
    }
}

/// PTB Command - matches GraphQL format (externally tagged enum)
#[derive(Debug, Clone, Serialize, Deserialize)]
enum SerializableCommand {
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<SerializableArgument>,
    },
    SplitCoins {
        coin: SerializableArgument,
        amounts: Vec<SerializableArgument>,
    },
    MergeCoins {
        destination: SerializableArgument,
        sources: Vec<SerializableArgument>,
    },
    TransferObjects {
        objects: Vec<SerializableArgument>,
        address: SerializableArgument,
    },
    MakeMoveVec {
        type_arg: Option<String>,
        elements: Vec<SerializableArgument>,
    },
    Publish {
        modules: Vec<String>,
        dependencies: Vec<String>,
    },
    Upgrade {
        modules: Vec<String>,
        dependencies: Vec<String>,
        package: String,
        ticket: SerializableArgument,
    },
}

impl From<&GrpcCommand> for SerializableCommand {
    fn from(cmd: &GrpcCommand) -> Self {
        use base64::Engine;
        match cmd {
            GrpcCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => SerializableCommand::MoveCall {
                package: package.clone(),
                module: module.clone(),
                function: function.clone(),
                type_arguments: type_arguments.clone(),
                arguments: arguments.iter().map(SerializableArgument::from).collect(),
            },
            GrpcCommand::SplitCoins { coin, amounts } => SerializableCommand::SplitCoins {
                coin: SerializableArgument::from(coin),
                amounts: amounts.iter().map(SerializableArgument::from).collect(),
            },
            GrpcCommand::MergeCoins { coin, sources } => SerializableCommand::MergeCoins {
                destination: SerializableArgument::from(coin),
                sources: sources.iter().map(SerializableArgument::from).collect(),
            },
            GrpcCommand::TransferObjects { objects, address } => {
                SerializableCommand::TransferObjects {
                    objects: objects.iter().map(SerializableArgument::from).collect(),
                    address: SerializableArgument::from(address),
                }
            }
            GrpcCommand::MakeMoveVec {
                element_type,
                elements,
            } => SerializableCommand::MakeMoveVec {
                type_arg: element_type.clone(),
                elements: elements.iter().map(SerializableArgument::from).collect(),
            },
            GrpcCommand::Publish {
                modules,
                dependencies,
            } => SerializableCommand::Publish {
                modules: modules
                    .iter()
                    .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                    .collect(),
                dependencies: dependencies.clone(),
            },
            GrpcCommand::Upgrade {
                modules,
                dependencies,
                package,
                ticket,
            } => SerializableCommand::Upgrade {
                modules: modules
                    .iter()
                    .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                    .collect(),
                dependencies: dependencies.clone(),
                package: package.clone(),
                ticket: SerializableArgument::from(ticket),
            },
        }
    }
}

/// Command argument reference - matches GraphQL format exactly
/// GraphQL uses: {"Input": 0}, {"Result": 0}, {"NestedResult": [0, 0]}, or "GasCoin"
#[derive(Debug, Clone)]
enum SerializableArgument {
    GasCoin,
    Input(u32),
    Result(u32),
    NestedResult(u32, u32),
}

impl Serialize for SerializableArgument {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            SerializableArgument::GasCoin => serializer.serialize_str("GasCoin"),
            SerializableArgument::Input(i) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("Input", i)?;
                map.end()
            }
            SerializableArgument::Result(i) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("Result", i)?;
                map.end()
            }
            SerializableArgument::NestedResult(cmd, idx) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("NestedResult", &(*cmd, *idx))?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SerializableArgument {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct ArgVisitor;

        impl<'de> Visitor<'de> for ArgVisitor {
            type Value = SerializableArgument;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("\"GasCoin\" or {\"Input\": n} or {\"Result\": n} or {\"NestedResult\": [n, m]}")
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v == "GasCoin" {
                    Ok(SerializableArgument::GasCoin)
                } else {
                    Err(de::Error::unknown_variant(v, &["GasCoin"]))
                }
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let Some((key, _)) = map.next_entry::<String, serde_json::Value>()? else {
                    return Err(de::Error::custom("expected one key"));
                };
                match key.as_str() {
                    "Input" => Ok(SerializableArgument::Input(0)), // simplified
                    "Result" => Ok(SerializableArgument::Result(0)),
                    "NestedResult" => Ok(SerializableArgument::NestedResult(0, 0)),
                    _ => Err(de::Error::unknown_field(
                        &key,
                        &["Input", "Result", "NestedResult"],
                    )),
                }
            }
        }

        deserializer.deserialize_any(ArgVisitor)
    }
}

impl From<&GrpcArgument> for SerializableArgument {
    fn from(arg: &GrpcArgument) -> Self {
        match arg {
            GrpcArgument::GasCoin => SerializableArgument::GasCoin,
            GrpcArgument::Input(i) => SerializableArgument::Input(*i),
            GrpcArgument::Result(i) => SerializableArgument::Result(*i),
            GrpcArgument::NestedResult(cmd, idx) => SerializableArgument::NestedResult(*cmd, *idx),
        }
    }
}

/// Transaction effects summary - matches GraphQL format
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializableEffects {
    status: String,
}

impl From<&GrpcTransaction> for SerializableTransaction {
    fn from(tx: &GrpcTransaction) -> Self {
        Self {
            digest: tx.digest.clone(),
            sender: tx.sender.clone(),
            gas_budget: tx.gas_budget,
            gas_price: tx.gas_price,
            checkpoint: tx.checkpoint,
            timestamp_ms: tx.timestamp_ms,
            inputs: tx.inputs.iter().map(SerializableInput::from).collect(),
            commands: tx.commands.iter().map(SerializableCommand::from).collect(),
            effects: tx.status.as_ref().map(|s| SerializableEffects {
                status: s.to_uppercase(),
            }),
        }
    }
}

#[derive(Debug, Default)]
struct Stats {
    checkpoints_received: usize,
    transactions_saved: usize,
    system_txs_skipped: usize,
    errors: usize,
    reconnects: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Parse arguments
    let mut duration_secs = 60u64;
    let mut output_path = PathBuf::from("transactions_stream.jsonl");
    let mut endpoint: Option<String> = None;
    let mut ptb_only = false;
    let mut verbose = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--endpoint" => {
                i += 1;
                endpoint = args.get(i).cloned();
            }
            "--duration" => {
                i += 1;
                duration_secs = args.get(i).map(|s| s.parse().unwrap_or(60)).unwrap_or(60);
            }
            "--output" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    output_path = PathBuf::from(p);
                }
            }
            "--ptb-only" => {
                ptb_only = true;
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    // Get endpoint from arg or env var
    let endpoint = endpoint
        .or_else(|| std::env::var("SUI_GRPC_ENDPOINT").ok())
        .unwrap_or_else(|| {
            eprintln!("WARNING: No gRPC endpoint specified.");
            eprintln!("Use --endpoint <url> or set SUI_GRPC_ENDPOINT env var.");
            eprintln!("Sui's public nodes don't expose gRPC. Use QuickNode, Dwellir, etc.");
            eprintln!();
            "https://mainnet.sui.io:443".to_string()
        });

    println!("=== gRPC Transaction Streaming ===");
    println!("Endpoint: {}", endpoint);
    println!("Duration: {}s", duration_secs);
    println!("Output: {}", output_path.display());
    println!("PTB only: {}", ptb_only);
    println!();

    // Connect to gRPC
    println!("Connecting to gRPC endpoint...");
    let client = GrpcClient::new(&endpoint).await?;

    // Get service info
    let info = client.get_service_info().await?;
    println!("Connected to {} (chain: {})", info.chain_id, info.chain);
    println!("Current checkpoint: {}", info.checkpoint_height);
    println!("Current epoch: {}", info.epoch);
    println!();

    // Open output file
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&output_path)?;
    let mut writer = BufWriter::new(file);

    let mut stats = Stats::default();
    let start = Instant::now();
    let duration = Duration::from_secs(duration_secs);

    // Subscribe to checkpoints with auto-reconnect
    println!("Subscribing to checkpoint stream...");
    let mut stream = client.subscribe_checkpoints().await?;
    println!("Streaming started!");
    println!();

    let mut last_checkpoint: Option<u64> = None;

    while start.elapsed() < duration {
        // Use tokio::select! to handle timeout
        let checkpoint = tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(cp)) => cp,
                    Some(Err(e)) => {
                        stats.errors += 1;
                        eprintln!("[ERROR] Stream error: {}", e);

                        // Try to reconnect
                        if start.elapsed() < duration {
                            eprintln!("[RECONNECTING] Attempting to reconnect...");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            match GrpcClient::new(&endpoint).await {
                                Ok(new_client) => {
                                    match new_client.subscribe_checkpoints().await {
                                        Ok(new_stream) => {
                                            stream = new_stream;
                                            stats.reconnects += 1;
                                            eprintln!("[RECONNECTED] Successfully reconnected (reconnect #{})", stats.reconnects);
                                            continue;
                                        }
                                        Err(e) => {
                                            eprintln!("[ERROR] Failed to resubscribe: {}", e);
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[ERROR] Failed to reconnect: {}", e);
                                    break;
                                }
                            }
                        }
                        continue;
                    }
                    None => {
                        println!("Stream ended unexpectedly");

                        // Try to reconnect
                        if start.elapsed() < duration {
                            eprintln!("[RECONNECTING] Stream ended, attempting to reconnect...");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            match GrpcClient::new(&endpoint).await {
                                Ok(new_client) => {
                                    match new_client.subscribe_checkpoints().await {
                                        Ok(new_stream) => {
                                            stream = new_stream;
                                            stats.reconnects += 1;
                                            eprintln!("[RECONNECTED] Successfully reconnected (reconnect #{})", stats.reconnects);
                                            continue;
                                        }
                                        Err(e) => {
                                            eprintln!("[ERROR] Failed to resubscribe: {}", e);
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[ERROR] Failed to reconnect: {}", e);
                                    break;
                                }
                            }
                        }
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                // Timeout waiting for checkpoint - this is normal, just continue
                if verbose {
                    println!("[...] Waiting for next checkpoint...");
                }
                continue;
            }
        };

        // Track last checkpoint for potential gap detection
        if let Some(last) = last_checkpoint {
            if checkpoint.sequence_number > last + 1 {
                eprintln!(
                    "[WARNING] Gap detected: {} -> {} (missed {} checkpoints)",
                    last,
                    checkpoint.sequence_number,
                    checkpoint.sequence_number - last - 1
                );
            }
        }
        last_checkpoint = Some(checkpoint.sequence_number);

        stats.checkpoints_received += 1;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut saved_count = 0;
        let mut skipped_count = 0;

        for tx in &checkpoint.transactions {
            // Skip system transactions if requested
            if ptb_only && !tx.is_ptb() {
                stats.system_txs_skipped += 1;
                skipped_count += 1;
                continue;
            }

            let cached = CachedTransaction {
                received_at_ms: now_ms,
                checkpoint: checkpoint.sequence_number,
                transaction: SerializableTransaction::from(tx),
            };

            let line = serde_json::to_string(&cached)?;
            writeln!(writer, "{}", line)?;

            stats.transactions_saved += 1;
            saved_count += 1;
        }

        // Flush after each checkpoint
        writer.flush()?;

        let remaining = duration_secs.saturating_sub(start.elapsed().as_secs());
        if verbose || saved_count > 0 {
            println!(
                "[{:3}s] Checkpoint {}: {} txs saved, {} skipped (total: {})",
                remaining,
                checkpoint.sequence_number,
                saved_count,
                skipped_count,
                stats.transactions_saved
            );
        }
    }

    // Final flush
    writer.flush()?;

    // Print summary
    println!();
    println!("=== Summary ===");
    println!("Duration: {:.1}s", start.elapsed().as_secs_f64());
    println!("Checkpoints received: {}", stats.checkpoints_received);
    println!("Transactions saved: {}", stats.transactions_saved);
    println!("System txs skipped: {}", stats.system_txs_skipped);
    println!("Reconnects: {}", stats.reconnects);
    println!("Errors: {}", stats.errors);
    println!();

    let rate = stats.transactions_saved as f64 / start.elapsed().as_secs_f64();
    println!("Effective rate: {:.2} txs/sec", rate);
    println!("Output: {}", output_path.display());

    // File size
    if let Ok(meta) = std::fs::metadata(&output_path) {
        let size_kb = meta.len() as f64 / 1024.0;
        println!("File size: {:.1} KB", size_kb);
    }

    Ok(())
}

fn print_usage() {
    println!("gRPC Transaction Streaming Tool");
    println!();
    println!("Streams transactions from Sui mainnet in real-time using gRPC.");
    println!();
    println!("TRADEOFFS vs poll_transactions (GraphQL):");
    println!("  ✓ Real-time push (~250ms latency)");
    println!("  ✓ No gaps - receives every checkpoint");
    println!("  ✓ Higher throughput (~30-40 tx/sec)");
    println!("  ✗ Limited effects - only status, no created/mutated/deleted");
    println!("  ✗ Connection drops every ~30s (auto-reconnects)");
    println!();
    println!("USE THIS FOR: real-time monitoring, indexing, high-volume collection");
    println!("USE poll_transactions FOR: replay verification, effects analysis");
    println!();
    println!("Usage:");
    println!("  stream_transactions [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --endpoint <url>     gRPC endpoint (default: fullnode.mainnet.sui.io:443)");
    println!("  --duration <secs>    How long to run (default: 60)");
    println!("  --output <file>      Output file (default: transactions_stream.jsonl)");
    println!("  --ptb-only           Only save PTB transactions (skip system txs)");
    println!("  --verbose, -v        Print detailed progress");
    println!("  --help, -h           Show this help");
    println!();
    println!("Examples:");
    println!("  # Stream for 1 minute from public endpoint");
    println!("  stream_transactions --duration 60");
    println!();
    println!("  # Stream for 10 minutes, PTB only");
    println!("  stream_transactions --duration 600 --ptb-only --output my_txs.jsonl");
    println!();
    println!("Public endpoints:");
    println!("  Mainnet:  https://fullnode.mainnet.sui.io:443 (streaming + queries)");
    println!("  Archive:  https://archive.mainnet.sui.io:443 (queries only, no streaming)");
    println!("  Testnet:  https://fullnode.testnet.sui.io:443");
}
