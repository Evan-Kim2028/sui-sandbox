//! Warm up the local Walrus checkpoint store and object index.
//!
//! This is an internal utility to ingest checkpoint ranges into the
//! filesystem store so replay can hydrate objects without hitting gRPC.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use serde_json::Value;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

use sui_historical_cache::{
    DynamicFieldEntry, FsDynamicFieldCache, FsObjectIndex, FsObjectStore, FsPackageIndex,
    FsTxDigestIndex, ObjectMeta, ObjectVersionStore,
};
use sui_transport::walrus::WalrusClient;
use sui_types::move_package::MovePackage;

#[derive(Parser, Debug)]
#[command(name = "walrus-warmup")]
struct Args {
    /// Network (mainnet or testnet)
    #[arg(long, default_value = "mainnet")]
    network: String,

    /// Starting checkpoint (defaults to latest - count + 1)
    #[arg(long)]
    start_checkpoint: Option<u64>,

    /// Number of checkpoints to ingest
    #[arg(long, default_value_t = 50)]
    count: u64,

    /// Max bytes per blob fetch when batching
    #[arg(long, default_value_t = 8 * 1024 * 1024)]
    max_chunk_bytes: u64,

    /// Max checkpoints per batch
    #[arg(long, default_value_t = 50)]
    batch_size: usize,

    /// Override local store dir
    #[arg(long)]
    store_dir: Option<PathBuf>,

    /// Dump each fetched checkpoint JSON into this directory (one file per checkpoint)
    #[arg(long)]
    dump_dir: Option<PathBuf>,

    /// Skip ingesting objects into the local store (useful for inspection only)
    #[arg(long, default_value_t = false)]
    no_ingest: bool,

    /// Print a summary of each checkpoint's contents
    #[arg(long, default_value_t = false)]
    summary: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let walrus = match args.network.as_str() {
        "testnet" => WalrusClient::testnet(),
        _ => WalrusClient::mainnet(),
    };

    let latest = walrus
        .get_latest_checkpoint()
        .context("walrus latest checkpoint")?;
    let count = args.count.max(1);
    let start_checkpoint = args
        .start_checkpoint
        .unwrap_or_else(|| latest.saturating_sub(count - 1));
    let checkpoints: Vec<u64> = (start_checkpoint..start_checkpoint + count).collect();

    let store_dir = args
        .store_dir
        .or_else(|| {
            std::env::var("SUI_WALRUS_STORE_DIR")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| default_store_dir(&args.network));

    let store = if args.no_ingest {
        None
    } else {
        Some(
            FsObjectStore::new(&store_dir)
                .with_context(|| format!("init object store at {}", store_dir.display()))?,
        )
    };
    let index = if args.no_ingest {
        None
    } else {
        Some(
            FsObjectIndex::new(&store_dir)
                .with_context(|| format!("init object index at {}", store_dir.display()))?,
        )
    };
    let tx_index = if args.no_ingest {
        None
    } else {
        Some(
            FsTxDigestIndex::new(&store_dir)
                .with_context(|| format!("init tx index at {}", store_dir.display()))?,
        )
    };
    let dynamic_fields = if args.no_ingest {
        None
    } else {
        Some(
            FsDynamicFieldCache::new(&store_dir)
                .with_context(|| format!("init dynamic field cache at {}", store_dir.display()))?,
        )
    };
    let package_index = if args.no_ingest {
        None
    } else {
        Some(
            FsPackageIndex::new(&store_dir)
                .with_context(|| format!("init package index at {}", store_dir.display()))?,
        )
    };

    println!(
        "Walrus warmup: network={} checkpoints={}..={} ({} total) store={}",
        args.network,
        checkpoints.first().copied().unwrap_or(start_checkpoint),
        checkpoints.last().copied().unwrap_or(start_checkpoint),
        checkpoints.len(),
        store_dir.display()
    );

    let mut total_objects = 0usize;
    let start = Instant::now();

    for chunk in checkpoints.chunks(args.batch_size) {
        let batch_start = Instant::now();
        let mut decoded: Vec<(u64, Value)> = Vec::with_capacity(chunk.len());
        match walrus.get_checkpoints_batched(chunk, args.max_chunk_bytes) {
            Ok(batch) => {
                for (cp, data) in batch {
                    let value = serde_json::to_value(&data)
                        .map_err(|e| anyhow!("serialize checkpoint {}: {}", cp, e))?;
                    decoded.push((cp, value));
                }
            }
            Err(e) => {
                eprintln!(
                    "[warning] batched walrus fetch failed ({}); falling back to per-checkpoint",
                    e
                );
                for &cp in chunk {
                    match walrus.get_checkpoint_json(cp) {
                        Ok(value) => decoded.push((cp, value)),
                        Err(err) => eprintln!(
                            "[warning] walrus checkpoint {} failed in fallback: {}",
                            cp, err
                        ),
                    }
                }
            }
        }
        for (cp, value) in decoded {
            if let Some(dir) = &args.dump_dir {
                let path = dir.join(format!("checkpoint_{}.json", cp));
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create dump dir {}", dir.display()))?;
                std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap_or_default())
                    .with_context(|| format!("write checkpoint dump {}", path.display()))?;
            }
            if args.summary {
                let summary = summarize_checkpoint(&value);
                println!(
                    "  checkpoint={} txs={} inputs={} outputs={} packages={} move_objects={} dyn_fields={}",
                    cp,
                    summary.transactions,
                    summary.input_objects,
                    summary.output_objects,
                    summary.packages,
                    summary.move_objects,
                    summary.dynamic_fields,
                );
            }
            if let (
                Some(tx_index),
                Some(store),
                Some(index),
                Some(package_index),
                Some(dynamic_fields),
            ) = (
                tx_index.as_ref(),
                store.as_ref(),
                index.as_ref(),
                package_index.as_ref(),
                dynamic_fields.as_ref(),
            ) {
                ingest_checkpoint_tx_index(&value, tx_index, cp);
                let ingested = ingest_checkpoint_objects(
                    &value,
                    store,
                    index,
                    package_index,
                    dynamic_fields,
                    cp,
                );
                total_objects += ingested;
            }
        }
        println!(
            "  batch {}..{} ingested={} elapsed_ms={}",
            chunk.first().copied().unwrap_or(0),
            chunk.last().copied().unwrap_or(0),
            total_objects,
            batch_start.elapsed().as_millis()
        );
    }

    println!(
        "Warmup complete: checkpoints={} objects_ingested={} elapsed_s={}",
        checkpoints.len(),
        total_objects,
        start.elapsed().as_secs()
    );

    Ok(())
}

#[derive(Default)]
struct CheckpointSummary {
    transactions: usize,
    input_objects: usize,
    output_objects: usize,
    packages: usize,
    move_objects: usize,
    dynamic_fields: usize,
}

fn summarize_checkpoint(checkpoint_json: &Value) -> CheckpointSummary {
    let mut summary = CheckpointSummary::default();
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return summary;
    };
    summary.transactions = transactions.len();
    for tx_json in transactions {
        for key in ["input_objects", "output_objects"] {
            let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
                continue;
            };
            if key == "input_objects" {
                summary.input_objects += arr.len();
            } else {
                summary.output_objects += arr.len();
            }
            for obj_json in arr {
                if obj_json
                    .get("data")
                    .and_then(|d| d.get("Package"))
                    .is_some()
                {
                    summary.packages += 1;
                    continue;
                }
                if let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) {
                    summary.move_objects += 1;
                    if move_obj
                        .get("type_")
                        .and_then(|t| t.as_str())
                        .map(|t| t.contains("::dynamic_field::Field"))
                        .unwrap_or(false)
                    {
                        summary.dynamic_fields += 1;
                    }
                }
            }
        }
    }
    summary
}

fn default_store_dir(network: &str) -> PathBuf {
    let base = std::env::var("SUI_SANDBOX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sui-sandbox")
        });
    base.join("walrus-store").join(network)
}

fn ingest_checkpoint_objects(
    checkpoint_json: &Value,
    store: &FsObjectStore,
    index: &FsObjectIndex,
    package_index: &FsPackageIndex,
    dynamic_fields: &FsDynamicFieldCache,
    checkpoint: u64,
) -> usize {
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return 0;
    };
    let mut total = 0usize;
    for tx_json in transactions {
        total += ingest_objects_for_tx(
            tx_json,
            store,
            index,
            package_index,
            dynamic_fields,
            checkpoint,
        );
    }
    total
}

fn ingest_objects_for_tx(
    tx_json: &Value,
    store: &FsObjectStore,
    index: &FsObjectIndex,
    package_index: &FsPackageIndex,
    dynamic_fields: &FsDynamicFieldCache,
    checkpoint: u64,
) -> usize {
    let mut ingested = 0usize;
    for key in ["input_objects", "output_objects"] {
        let Some(arr) = tx_json.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for obj_json in arr {
            let prev_tx = obj_json
                .get("previous_transaction")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(pkg_json) = obj_json.get("data").and_then(|d| d.get("Package")) {
                if let Ok(pkg) = serde_json::from_value::<MovePackage>(pkg_json.clone()) {
                    let pkg_id: AccountAddress = pkg.id().into();
                    let version: u64 = pkg.version().into();
                    let _ = package_index.put(pkg_id, version, checkpoint, prev_tx.clone());
                    ingested += 1;
                }
                continue;
            }
            let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
                continue;
            };
            let contents = decode_walrus_contents(move_obj.get("contents"));
            let Some(bcs_bytes) = contents else {
                continue;
            };
            if bcs_bytes.len() < 32 {
                continue;
            }
            let id = AccountAddress::new({
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&bcs_bytes[0..32]);
                bytes
            });
            let version = match move_obj.get("version").and_then(|v| v.as_u64()) {
                Some(v) => v,
                None => continue,
            };
            let parsed_tag = move_obj
                .get("type_")
                .and_then(|t| parse_type_tag_json(t).ok());
            let type_tag = parsed_tag.as_ref().map(|t| t.to_string());
            let is_dynamic_field = parsed_tag
                .as_ref()
                .map(is_dynamic_field_type_tag)
                .unwrap_or(false);
            let owner_json = obj_json.get("owner").unwrap_or(&Value::Null);
            let owner_kind = owner_kind_string(owner_json);
            let parent_owner = parse_owner_parent(owner_json);

            if let Some(type_tag) = type_tag.clone() {
                let meta = ObjectMeta {
                    type_tag,
                    owner_kind,
                    source_checkpoint: Some(checkpoint),
                };
                let _ = store.put(id, version, &bcs_bytes, &meta);
                let _ = index.put(id, version, checkpoint, prev_tx.clone());
                if let Some(parent) = parent_owner {
                    if is_dynamic_field {
                        let entry = DynamicFieldEntry {
                            checkpoint,
                            parent_id: parent.to_hex_literal(),
                            child_id: id.to_hex_literal(),
                            version,
                            type_tag: Some(meta.type_tag.clone()),
                            prev_tx: prev_tx.clone(),
                        };
                        let _ = dynamic_fields.put_entry(entry);
                    }
                }
                ingested += 1;
            }
        }
    }
    ingested
}

fn ingest_checkpoint_tx_index(
    checkpoint_json: &Value,
    tx_index: &FsTxDigestIndex,
    checkpoint: u64,
) {
    let Some(transactions) = checkpoint_json
        .get("transactions")
        .and_then(|v| v.as_array())
    else {
        return;
    };
    for tx_json in transactions {
        if let Some(digest) = extract_walrus_tx_digest(tx_json) {
            let _ = tx_index.put(&digest, checkpoint);
        }
    }
}

fn decode_walrus_contents(value: Option<&Value>) -> Option<Vec<u8>> {
    let value = value?;
    if let Some(s) = value.as_str() {
        return base64::engine::general_purpose::STANDARD.decode(s).ok();
    }
    if let Some(arr) = value.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for x in arr {
            let n = x.as_u64()?;
            if n > 255 {
                return None;
            }
            out.push(n as u8);
        }
        return Some(out);
    }
    None
}

fn parse_type_tag_json(type_json: &Value) -> Result<TypeTag> {
    let struct_json = if let Some(other) = type_json.get("Other") {
        other
    } else if let Some(s) = type_json.get("struct") {
        s
    } else if type_json.get("address").is_some() {
        type_json
    } else {
        return Err(anyhow!("unsupported type tag JSON: {}", type_json));
    };

    let address = struct_json
        .get("address")
        .and_then(|a| a.as_str())
        .ok_or_else(|| anyhow!("Missing address in type"))?;
    let module = struct_json
        .get("module")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow!("Missing module in type"))?;
    let name = struct_json
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow!("Missing name in type"))?;
    let type_args = struct_json
        .get("type_args")
        .and_then(|t| t.as_array())
        .unwrap_or(&vec![])
        .iter()
        .map(parse_type_tag_json)
        .collect::<Result<Vec<_>>>()?;

    let address = if address.starts_with("0x") {
        address.to_string()
    } else {
        format!("0x{address}")
    };
    let mut s = format!("{address}::{module}::{name}");
    if !type_args.is_empty() {
        let inner = type_args
            .iter()
            .map(|t| format!("{t}"))
            .collect::<Vec<_>>()
            .join(", ");
        s.push('<');
        s.push_str(&inner);
        s.push('>');
    }
    TypeTag::from_str(&s).map_err(|e| anyhow!("parse TypeTag from {s:?}: {e}"))
}

fn owner_kind_string(owner_json: &Value) -> Option<String> {
    if owner_json.get("Immutable").is_some() {
        return Some("immutable".to_string());
    }
    if owner_json.get("Shared").is_some() {
        return Some("shared".to_string());
    }
    if owner_json.get("AddressOwner").is_some() || owner_json.get("ObjectOwner").is_some() {
        return Some("address".to_string());
    }
    None
}

fn is_dynamic_field_type_tag(tag: &TypeTag) -> bool {
    match tag {
        TypeTag::Struct(s) => {
            let module = s.module.as_str();
            let name = s.name.as_str();
            s.address == AccountAddress::TWO
                && name == "Field"
                && (module == "dynamic_field" || module == "dynamic_object_field")
        }
        _ => false,
    }
}

fn extract_walrus_tx_digest(tx_json: &Value) -> Option<String> {
    tx_json
        .pointer("/effects/V2/transaction_digest")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn parse_owner_parent(owner_json: &Value) -> Option<AccountAddress> {
    if let Some(parent) = owner_json.get("ObjectOwner").and_then(|v| v.as_str()) {
        return AccountAddress::from_hex_literal(parent).ok();
    }
    None
}
