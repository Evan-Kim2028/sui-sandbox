//! Stream transactions from Sui mainnet via gRPC

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sui_sandbox::grpc::{GrpcArgument, GrpcClient, GrpcCommand, GrpcInput, GrpcTransaction};

#[derive(Debug, Parser)]
#[command(name = "stream-transactions", about = "Stream transactions via gRPC")]
pub struct StreamTransactionsCmd {
    /// gRPC endpoint URL (defaults to env var or public endpoint)
    #[arg(long, value_name = "URL")]
    endpoint: Option<String>,

    /// How long to run (seconds)
    #[arg(long, default_value_t = 60, value_name = "SECS")]
    duration: u64,

    /// Output file path (JSONL)
    #[arg(long, default_value = "transactions_stream.jsonl", value_name = "FILE")]
    output: PathBuf,

    /// Only save PTB transactions (skip system txs)
    #[arg(long, default_value_t = false)]
    ptb_only: bool,

    /// Print detailed progress
    #[arg(long, short, default_value_t = false)]
    verbose: bool,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializableTransaction {
    digest: String,
    sender: String,
    gas_budget: Option<u64>,
    gas_price: Option<u64>,
    checkpoint: Option<u64>,
    timestamp_ms: Option<u64>,
    inputs: Vec<SerializableInput>,
    commands: Vec<SerializableCommand>,
    effects: Option<SerializableEffects>,
}

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

#[derive(Debug, Clone)]
enum SerializableArgument {
    GasCoin,
    Input(u32),
    Result(u32),
    NestedResult(u32, u32),
}

impl Serialize for SerializableArgument {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            SerializableArgument::GasCoin => serializer.serialize_str("GasCoin"),
            SerializableArgument::Input(i) => {
                let mut map = serde_json::Map::new();
                map.insert("Input".to_string(), serde_json::Value::Number((*i).into()));
                map.serialize(serializer)
            }
            SerializableArgument::Result(i) => {
                let mut map = serde_json::Map::new();
                map.insert("Result".to_string(), serde_json::Value::Number((*i).into()));
                map.serialize(serializer)
            }
            SerializableArgument::NestedResult(i, j) => {
                let mut map = serde_json::Map::new();
                map.insert(
                    "NestedResult".to_string(),
                    serde_json::Value::Array(vec![
                        serde_json::Value::Number((*i).into()),
                        serde_json::Value::Number((*j).into()),
                    ]),
                );
                map.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for SerializableArgument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        if let serde_json::Value::String(s) = &v {
            if s == "GasCoin" {
                return Ok(SerializableArgument::GasCoin);
            }
        }
        if let serde_json::Value::Object(map) = &v {
            if let Some(val) = map.get("Input").and_then(|v| v.as_u64()) {
                return Ok(SerializableArgument::Input(val as u32));
            }
            if let Some(val) = map.get("Result").and_then(|v| v.as_u64()) {
                return Ok(SerializableArgument::Result(val as u32));
            }
            if let Some(arr) = map.get("NestedResult").and_then(|v| v.as_array()) {
                if arr.len() == 2 {
                    let a = arr[0].as_u64().unwrap_or(0) as u32;
                    let b = arr[1].as_u64().unwrap_or(0) as u32;
                    return Ok(SerializableArgument::NestedResult(a, b));
                }
            }
        }
        Err(serde::de::Error::custom("Invalid argument format"))
    }
}

impl From<&GrpcArgument> for SerializableArgument {
    fn from(arg: &GrpcArgument) -> Self {
        match arg {
            GrpcArgument::GasCoin => SerializableArgument::GasCoin,
            GrpcArgument::Input(i) => SerializableArgument::Input(*i),
            GrpcArgument::Result(i) => SerializableArgument::Result(*i),
            GrpcArgument::NestedResult(i, j) => SerializableArgument::NestedResult(*i, *j),
        }
    }
}

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

impl StreamTransactionsCmd {
    pub async fn execute(&self) -> Result<()> {
        let endpoint = self
            .endpoint
            .clone()
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
        println!("Duration: {}s", self.duration);
        println!("Output: {}", self.output.display());
        println!("PTB only: {}", self.ptb_only);
        println!();

        println!("Connecting to gRPC endpoint...");
        let client = GrpcClient::new(&endpoint).await?;

        let info = client.get_service_info().await?;
        println!("Connected to {} (chain: {})", info.chain_id, info.chain);
        println!("Current checkpoint: {}", info.checkpoint_height);
        println!("Current epoch: {}", info.epoch);
        println!();

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output)?;
        let mut writer = BufWriter::new(file);

        let mut stats = Stats::default();
        let start = Instant::now();
        let duration = Duration::from_secs(self.duration);

        println!("Subscribing to checkpoint stream...");
        let mut stream = client.subscribe_checkpoints().await?;
        println!("Streaming started!\n");

        let mut last_checkpoint: Option<u64> = None;

        while start.elapsed() < duration {
            let checkpoint = tokio::select! {
                result = stream.next() => {
                    match result {
                        Some(Ok(cp)) => cp,
                        Some(Err(e)) => {
                            stats.errors += 1;
                            eprintln!("Stream error: {}", e);
                            // Try reconnect
                            stats.reconnects += 1;
                            eprintln!("Reconnecting (attempt #{})...", stats.reconnects);
                            match GrpcClient::new(&endpoint).await {
                                Ok(client) => {
                                    match client.subscribe_checkpoints().await {
                                        Ok(new_stream) => {
                                            stream = new_stream;
                                            continue;
                                        }
                                        Err(err) => {
                                            eprintln!("Reconnect failed: {}", err);
                                        }
                                    }
                                }
                                Err(err) => {
                                    eprintln!("Reconnect failed: {}", err);
                                }
                            }
                            continue;
                        }
                        None => {
                            stats.errors += 1;
                            eprintln!("Stream ended unexpectedly");
                            stats.reconnects += 1;
                            match GrpcClient::new(&endpoint).await {
                                Ok(client) => {
                                    match client.subscribe_checkpoints().await {
                                        Ok(new_stream) => {
                                            stream = new_stream;
                                            continue;
                                        }
                                        Err(err) => {
                                            eprintln!("Reconnect failed: {}", err);
                                        }
                                    }
                                }
                                Err(err) => {
                                    eprintln!("Reconnect failed: {}", err);
                                }
                            }
                            continue;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    continue;
                }
            };

            stats.checkpoints_received += 1;

            if let Some(last) = last_checkpoint {
                if checkpoint.sequence_number > last + 1 {
                    eprintln!(
                        "[WARNING] Checkpoint gap: {} -> {}",
                        last, checkpoint.sequence_number
                    );
                }
            }
            last_checkpoint = Some(checkpoint.sequence_number);

            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            let mut new_count = 0;
            for tx in &checkpoint.transactions {
                if self.ptb_only && !tx.is_ptb() {
                    stats.system_txs_skipped += 1;
                    continue;
                }

                let cached = CachedTransaction {
                    received_at_ms: now_ms,
                    checkpoint: checkpoint.sequence_number,
                    transaction: SerializableTransaction::from(tx),
                };

                let line = serde_json::to_string(&cached)?;
                writeln!(writer, "{}", line)?;
                new_count += 1;
                stats.transactions_saved += 1;
            }

            writer.flush()?;

            if self.verbose || new_count > 0 {
                println!(
                    "[checkpoint {}] +{} txs (total: {})",
                    checkpoint.sequence_number, new_count, stats.transactions_saved
                );
            }
        }

        writer.flush()?;

        println!("\n=== Summary ===");
        println!("Checkpoints received: {}", stats.checkpoints_received);
        println!("Transactions saved: {}", stats.transactions_saved);
        println!("System txs skipped: {}", stats.system_txs_skipped);
        println!("Errors: {}", stats.errors);
        println!("Reconnects: {}", stats.reconnects);

        Ok(())
    }
}
