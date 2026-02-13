//! File-backed replay state provider and import pipeline.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sui_sandbox_types::{FetchedTransaction, TransactionDigest};

use crate::bcs_codec::{
    deserialize_package_base64, deserialize_transaction_base64,
    deserialize_transaction_data_json_str, deserialize_transaction_data_json_value,
    transaction_data_to_fetched_transaction,
};
use crate::replay_builder::ReplayStateConfig;
use crate::replay_provider::ReplayStateProvider;
use crate::state_json::parse_replay_states_file;
use crate::types::{PackageData, ReplayState, VersionedObject};

const STATES_DIR_NAME: &str = "states";
const INDEX_FILE_NAME: &str = "index.json";

/// Import request for file-based replay state ingestion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportSpec {
    /// Single state file (strict or extended replay schema).
    pub state: Option<PathBuf>,
    /// Transactions rows (JSON/JSONL/CSV).
    pub transactions: Option<PathBuf>,
    /// Objects rows (JSON/JSONL/CSV).
    pub objects: Option<PathBuf>,
    /// Packages rows (JSON/JSONL/CSV).
    pub packages: Option<PathBuf>,
}

/// Result summary for import operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSummary {
    pub cache_dir: PathBuf,
    pub states_imported: usize,
    pub objects_imported: usize,
    pub packages_imported: usize,
    pub digests: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StateIndex {
    /// digest -> relative path (within cache dir)
    states: HashMap<String, String>,
}

/// File-backed `ReplayStateProvider`.
///
/// Stores normalized replay state files under:
/// `<cache_dir>/states/<hex-encoded-digest>.json`
#[derive(Debug, Clone)]
pub struct FileStateProvider {
    cache_dir: PathBuf,
    states_dir: PathBuf,
    index_path: PathBuf,
}

impl FileStateProvider {
    /// Create or open a file-backed replay cache directory.
    pub fn new(cache_dir: impl AsRef<Path>) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        let states_dir = cache_dir.join(STATES_DIR_NAME);
        let index_path = cache_dir.join(INDEX_FILE_NAME);

        fs::create_dir_all(&states_dir).with_context(|| {
            format!(
                "Failed to create file replay cache directory: {}",
                states_dir.display()
            )
        })?;

        let this = Self {
            cache_dir,
            states_dir,
            index_path,
        };

        if !this.index_path.exists() {
            this.write_index(&StateIndex::default())?;
        }

        Ok(this)
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Import states into this cache.
    pub fn import(&self, spec: &ImportSpec) -> Result<ImportSummary> {
        validate_import_spec(spec)?;

        let mut states: HashMap<String, ReplayState> = HashMap::new();
        let mut objects_imported = 0usize;
        let mut packages_imported = 0usize;

        if let Some(state_file) = &spec.state {
            for state in parse_replay_states_file(state_file)? {
                let digest = canonical_digest(&state.transaction.digest.0)?;
                states.insert(digest, state);
            }
        } else {
            let tx_file = spec.transactions.as_ref().ok_or_else(|| {
                anyhow!("transactions file is required unless --state is provided")
            })?;
            for (idx, row) in load_rows(tx_file)?.iter().enumerate() {
                let state = parse_transaction_row(row)
                    .with_context(|| format!("Failed to parse transaction row {}", idx))?;
                let digest = canonical_digest(&state.transaction.digest.0)?;
                states.insert(digest, state);
            }

            if let Some(objects_file) = &spec.objects {
                for (idx, row) in load_rows(objects_file)?.iter().enumerate() {
                    let parsed = parse_object_row(row)
                        .with_context(|| format!("Failed to parse object row {}", idx))?;
                    attach_to_state(
                        &mut states,
                        parsed.tx_digest.as_deref(),
                        "object",
                        |state| {
                            state
                                .objects
                                .insert(parsed.object.id, parsed.object.clone());
                        },
                    )?;
                    objects_imported += 1;
                }
            }

            if let Some(packages_file) = &spec.packages {
                for (idx, row) in load_rows(packages_file)?.iter().enumerate() {
                    let parsed = parse_package_row(row)
                        .with_context(|| format!("Failed to parse package row {}", idx))?;
                    attach_to_state(
                        &mut states,
                        parsed.tx_digest.as_deref(),
                        "package",
                        |state| {
                            state
                                .packages
                                .insert(parsed.package.address, parsed.package.clone());
                        },
                    )?;
                    packages_imported += 1;
                }
            }
        }

        let mut digests: Vec<String> = states.keys().cloned().collect();
        digests.sort();

        for digest in &digests {
            if let Some(state) = states.get(digest) {
                self.put_state(state)?;
            }
        }

        Ok(ImportSummary {
            cache_dir: self.cache_dir.clone(),
            states_imported: digests.len(),
            objects_imported,
            packages_imported,
            digests,
        })
    }

    /// Persist a normalized replay state into cache.
    pub fn put_state(&self, state: &ReplayState) -> Result<PathBuf> {
        let digest = canonical_digest(&state.transaction.digest.0)?;
        let filename = digest_filename(&digest);
        let rel_path = Path::new(STATES_DIR_NAME).join(&filename);
        let abs_path = self.cache_dir.join(&rel_path);

        let json =
            serde_json::to_string_pretty(state).context("Failed to serialize replay state")?;
        fs::write(&abs_path, json).with_context(|| {
            format!("Failed to write replay state file: {}", abs_path.display())
        })?;

        let mut index = self.read_index()?;
        index
            .states
            .insert(digest.to_string(), rel_path.to_string_lossy().to_string());
        self.write_index(&index)?;

        Ok(abs_path)
    }

    /// Load a replay state by digest.
    pub fn get_state(&self, digest: &str) -> Result<ReplayState> {
        let digest = canonical_digest(digest)?;
        let index = self.read_index()?;

        let direct_candidate = self.states_dir.join(format!("{}.json", digest));
        let indexed_path = index
            .states
            .get(&digest)
            .map(|p| self.cache_dir.join(p))
            .or_else(|| {
                let by_hex = self.states_dir.join(digest_filename(&digest));
                by_hex.exists().then_some(by_hex)
            })
            .or_else(|| direct_candidate.exists().then_some(direct_candidate))
            .ok_or_else(|| {
                anyhow!(
                    "Replay state not found for digest '{}' in {}",
                    digest,
                    self.cache_dir.display()
                )
            })?;

        let states = parse_replay_states_file(&indexed_path)?;
        if states.len() == 1 {
            return Ok(states.into_iter().next().expect("single state"));
        }

        states
            .into_iter()
            .find(|s| s.transaction.digest.0 == digest)
            .ok_or_else(|| {
                anyhow!(
                    "State file '{}' contains multiple states but none for digest '{}'",
                    indexed_path.display(),
                    digest
                )
            })
    }

    pub fn list_digests(&self) -> Result<Vec<String>> {
        let mut digests: Vec<String> = self.read_index()?.states.keys().cloned().collect();
        digests.sort();
        Ok(digests)
    }

    fn read_index(&self) -> Result<StateIndex> {
        if !self.index_path.exists() {
            return Ok(StateIndex::default());
        }
        let raw = fs::read_to_string(&self.index_path).with_context(|| {
            format!(
                "Failed to read file replay index: {}",
                self.index_path.display()
            )
        })?;
        serde_json::from_str::<StateIndex>(&raw)
            .with_context(|| format!("Invalid file replay index: {}", self.index_path.display()))
    }

    fn write_index(&self, index: &StateIndex) -> Result<()> {
        let raw =
            serde_json::to_string_pretty(index).context("Failed to serialize replay index")?;
        fs::write(&self.index_path, raw).with_context(|| {
            format!(
                "Failed to write replay index: {}",
                self.index_path.display()
            )
        })
    }
}

#[async_trait::async_trait]
impl ReplayStateProvider for FileStateProvider {
    async fn fetch_replay_state(&self, digest: &str) -> Result<ReplayState> {
        self.get_state(digest)
    }

    async fn fetch_replay_state_with_config(
        &self,
        digest: &str,
        _config: &ReplayStateConfig,
    ) -> Result<ReplayState> {
        self.get_state(digest)
    }
}

/// Import replay states into a file-backed cache directory.
pub fn import_replay_states(
    cache_dir: impl AsRef<Path>,
    spec: &ImportSpec,
) -> Result<ImportSummary> {
    let provider = FileStateProvider::new(cache_dir)?;
    provider.import(spec)
}

#[derive(Debug, Clone)]
struct ObjectRow {
    tx_digest: Option<String>,
    object: VersionedObject,
}

#[derive(Debug, Clone)]
struct PackageRow {
    tx_digest: Option<String>,
    package: PackageData,
}

fn validate_import_spec(spec: &ImportSpec) -> Result<()> {
    if spec.state.is_some()
        && (spec.transactions.is_some() || spec.objects.is_some() || spec.packages.is_some())
    {
        return Err(anyhow!(
            "--state cannot be combined with --transactions/--objects/--packages"
        ));
    }

    if spec.state.is_none() && spec.transactions.is_none() {
        return Err(anyhow!(
            "Provide either --state or --transactions (with optional --objects/--packages)"
        ));
    }

    Ok(())
}

fn canonical_digest(digest: &str) -> Result<String> {
    let digest = digest.trim();
    if digest.is_empty() {
        return Err(anyhow!("transaction digest is empty"));
    }
    Ok(digest.to_string())
}

fn digest_filename(digest: &str) -> String {
    format!("{}.json", hex::encode(digest.as_bytes()))
}

fn parse_transaction_row(row: &Value) -> Result<ReplayState> {
    let obj = row
        .as_object()
        .ok_or_else(|| anyhow!("transaction row must be an object"))?;

    let digest = string_value(obj, &["digest", "transaction_digest", "tx_digest"], true)?
        .expect("required digest");
    let checkpoint = u64_value(obj, &["checkpoint"])?;
    let timestamp_ms = u64_value(obj, &["timestamp_ms"])?;

    let transaction = if let Some(raw_bcs) = string_value(
        obj,
        &[
            "raw_bcs",
            "raw_bcs_base64",
            "transaction_bcs",
            "transaction_bcs_base64",
            "bcs",
            "bcs_base64",
        ],
        false,
    )? {
        deserialize_transaction_base64(&raw_bcs, digest.clone(), None, timestamp_ms, checkpoint)
            .context("Failed to deserialize transaction bcs")?
    } else if let Some(tx_json_value) = first_value(
        obj,
        &["transaction_json", "transaction_data_json", "tx_json"],
    ) {
        let tx_data = match tx_json_value {
            Value::String(s) => deserialize_transaction_data_json_str(s),
            other => deserialize_transaction_data_json_value(other),
        }
        .context("Failed to parse transaction_json")?;
        transaction_data_to_fetched_transaction(
            &tx_data,
            digest.clone(),
            None,
            timestamp_ms,
            checkpoint,
        )
    } else {
        let sender = string_value(obj, &["sender"], false)?.unwrap_or_else(|| "0x0".to_string());
        let sender = AccountAddress::from_hex_literal(&sender)
            .with_context(|| format!("Invalid sender address '{}'", sender))?;

        let commands = parse_vec_from_row::<sui_sandbox_types::PtbCommand>(
            obj,
            &["commands", "commands_json"],
        )?
        .unwrap_or_default();
        let inputs = parse_vec_from_row::<sui_sandbox_types::TransactionInput>(
            obj,
            &["inputs", "inputs_json"],
        )?
        .unwrap_or_default();

        FetchedTransaction {
            digest: TransactionDigest(digest.clone()),
            sender,
            gas_budget: u64_value(obj, &["gas_budget"])?.unwrap_or_default(),
            gas_price: u64_value(obj, &["gas_price"])?.unwrap_or_default(),
            commands,
            inputs,
            effects: None,
            timestamp_ms,
            checkpoint,
        }
    };

    Ok(ReplayState {
        transaction,
        objects: HashMap::new(),
        packages: HashMap::new(),
        protocol_version: u64_value(obj, &["protocol_version"])?.unwrap_or(0),
        epoch: u64_value(obj, &["epoch"])?.unwrap_or(0),
        reference_gas_price: u64_value(obj, &["reference_gas_price"])?,
        checkpoint,
    })
}

fn parse_object_row(row: &Value) -> Result<ObjectRow> {
    let obj = row
        .as_object()
        .ok_or_else(|| anyhow!("object row must be an object"))?;

    let object_id = string_value(obj, &["object_id", "id"], true)?.expect("required object id");
    let id = AccountAddress::from_hex_literal(&object_id)
        .with_context(|| format!("Invalid object_id '{}'", object_id))?;
    let version = u64_value(obj, &["version"])?.unwrap_or(1);
    let type_tag = string_value(obj, &["type_tag", "type"], false)?;
    let digest = string_value(obj, &["digest", "object_digest"], false)?;

    let bytes = row_bytes(obj, &["bcs", "bcs_base64", "bcs_bytes", "bytes", "raw_bcs"])?
        .ok_or_else(|| anyhow!("object row missing bcs bytes (bcs/bcs_base64/bcs_bytes)"))?;

    let owner_type = string_value(obj, &["owner_type"], false)?.map(|s| s.to_ascii_lowercase());
    let (is_shared, is_immutable) = match owner_type.as_deref() {
        Some("shared") => (true, false),
        Some("immutable") => (false, true),
        Some("addressowner") | Some("objectowner") | Some("owned") => (false, false),
        Some(other) => {
            return Err(anyhow!(
                "Unsupported owner_type '{}'. Expected Shared/Immutable/AddressOwner",
                other
            ))
        }
        None => (
            bool_value(obj, &["is_shared"])?.unwrap_or(false),
            bool_value(obj, &["is_immutable"])?.unwrap_or(false),
        ),
    };

    Ok(ObjectRow {
        tx_digest: string_value(obj, &["tx_digest", "transaction_digest"], false)?,
        object: VersionedObject {
            id,
            version,
            digest,
            type_tag,
            bcs_bytes: bytes,
            is_shared,
            is_immutable,
        },
    })
}

fn parse_package_row(row: &Value) -> Result<PackageRow> {
    let obj = row
        .as_object()
        .ok_or_else(|| anyhow!("package row must be an object"))?;

    let package_id =
        string_value(obj, &["package_id", "address"], true)?.expect("required package id");
    let address = AccountAddress::from_hex_literal(&package_id)
        .with_context(|| format!("Invalid package_id '{}'", package_id))?;

    let encoded = string_value(obj, &["bcs", "bcs_base64"], true)?.expect("required package bcs");
    let mut package = deserialize_package_base64(&encoded)
        .with_context(|| format!("Failed to deserialize package bcs for '{}'", package_id))?;
    package.address = address;

    if let Some(version) = u64_value(obj, &["version"])? {
        package.version = version;
    }

    Ok(PackageRow {
        tx_digest: string_value(obj, &["tx_digest", "transaction_digest"], false)?,
        package,
    })
}

fn attach_to_state<F>(
    states: &mut HashMap<String, ReplayState>,
    tx_digest: Option<&str>,
    item_name: &str,
    attach: F,
) -> Result<()>
where
    F: Fn(&mut ReplayState),
{
    if let Some(digest) = tx_digest {
        let digest = canonical_digest(digest)?;
        let state = states.get_mut(&digest).ok_or_else(|| {
            anyhow!(
                "{} row references unknown tx_digest '{}'",
                item_name,
                digest
            )
        })?;
        attach(state);
        return Ok(());
    }

    if states.len() == 1 {
        if let Some((_digest, state)) = states.iter_mut().next() {
            attach(state);
            return Ok(());
        }
    }

    Err(anyhow!(
        "{} row missing tx_digest while multiple transactions are present",
        item_name
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataFileFormat {
    Json,
    Jsonl,
    Csv,
}

fn detect_file_format(path: &Path) -> DataFileFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("jsonl") => DataFileFormat::Jsonl,
        Some("csv") => DataFileFormat::Csv,
        _ => DataFileFormat::Json,
    }
}

fn load_rows(path: &Path) -> Result<Vec<Value>> {
    let format = detect_file_format(path);
    match format {
        DataFileFormat::Json => {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path.display()))?;
            let value: Value = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse JSON: {}", path.display()))?;
            match value {
                Value::Array(items) => Ok(items),
                other => Ok(vec![other]),
            }
        }
        DataFileFormat::Jsonl => {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path.display()))?;
            raw.lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .map(|(i, line)| {
                    serde_json::from_str::<Value>(line).with_context(|| {
                        format!("Invalid JSONL at {} line {}", path.display(), i + 1)
                    })
                })
                .collect()
        }
        DataFileFormat::Csv => {
            let mut reader = csv::Reader::from_path(path)
                .with_context(|| format!("Failed to read CSV: {}", path.display()))?;
            let headers = reader
                .headers()
                .context("Failed to read CSV headers")?
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();

            let mut rows = Vec::new();
            for (i, rec) in reader.records().enumerate() {
                let record = rec.with_context(|| {
                    format!("Failed to read CSV record {} in {}", i + 1, path.display())
                })?;
                let mut obj = Map::new();
                for (idx, field) in record.iter().enumerate() {
                    if let Some(header) = headers.get(idx) {
                        obj.insert(header.clone(), Value::String(field.to_string()));
                    }
                }
                rows.push(Value::Object(obj));
            }
            Ok(rows)
        }
    }
}

fn parse_vec_from_row<T>(obj: &Map<String, Value>, keys: &[&str]) -> Result<Option<Vec<T>>>
where
    T: serde::de::DeserializeOwned,
{
    let Some(value) = first_value(obj, keys) else {
        return Ok(None);
    };

    match value {
        Value::Null => Ok(None),
        Value::Array(_) | Value::Object(_) => serde_json::from_value(value.clone())
            .map(Some)
            .with_context(|| format!("Failed to parse JSON array/object for field '{}'", keys[0])),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            serde_json::from_str::<Vec<T>>(trimmed)
                .map(Some)
                .with_context(|| format!("Failed to parse JSON string for field '{}'", keys[0]))
        }
        _ => Err(anyhow!("Unsupported JSON type for field '{}'", keys[0])),
    }
}

fn row_bytes(obj: &Map<String, Value>, keys: &[&str]) -> Result<Option<Vec<u8>>> {
    let Some(value) = first_value(obj, keys) else {
        return Ok(None);
    };
    bytes_from_value(value).map(Some)
}

fn bytes_from_value(value: &Value) -> Result<Vec<u8>> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(Vec::new());
            }
            if trimmed.starts_with('[') {
                let vec: Vec<u8> = serde_json::from_str(trimmed)
                    .context("Failed to parse byte array JSON string")?;
                Ok(vec)
            } else {
                crate::bcs_codec::decode_base64_bytes(trimmed)
                    .context("Failed to decode base64 bytes")
            }
        }
        Value::Array(arr) => arr
            .iter()
            .map(|v| {
                let n = v
                    .as_u64()
                    .ok_or_else(|| anyhow!("byte array values must be integers"))?;
                u8::try_from(n).map_err(|_| anyhow!("byte value out of range: {}", n))
            })
            .collect(),
        _ => Err(anyhow!("expected base64 string or byte array")),
    }
}

fn string_value(obj: &Map<String, Value>, keys: &[&str], required: bool) -> Result<Option<String>> {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            let out = match value {
                Value::Null => None,
                Value::String(s) => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => {
                    return Err(anyhow!(
                        "Field '{}' must be string/number (got {})",
                        key,
                        value
                    ))
                }
            };
            if let Some(ref s) = out {
                if s.is_empty() {
                    continue;
                }
            }
            return Ok(out);
        }
    }

    if required {
        Err(anyhow!("Missing required field: {}", keys.join(" or ")))
    } else {
        Ok(None)
    }
}

fn u64_value(obj: &Map<String, Value>, keys: &[&str]) -> Result<Option<u64>> {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            let out = match value {
                Value::Null => None,
                Value::Number(n) => n.as_u64(),
                Value::String(s) => {
                    let t = s.trim();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t.parse::<u64>().with_context(|| {
                            format!("Field '{}' expected u64 string, got '{}'", key, t)
                        })?)
                    }
                }
                _ => {
                    return Err(anyhow!(
                        "Field '{}' must be a u64/string (got {})",
                        key,
                        value
                    ))
                }
            };
            return Ok(out);
        }
    }
    Ok(None)
}

fn bool_value(obj: &Map<String, Value>, keys: &[&str]) -> Result<Option<bool>> {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            let out = match value {
                Value::Null => None,
                Value::Bool(b) => Some(*b),
                Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" | "on" => Some(true),
                    "0" | "false" | "no" | "off" => Some(false),
                    other => {
                        return Err(anyhow!(
                            "Field '{}' expected bool string, got '{}'",
                            key,
                            other
                        ))
                    }
                },
                _ => {
                    return Err(anyhow!(
                        "Field '{}' must be bool/string (got {})",
                        key,
                        value
                    ))
                }
            };
            return Ok(out);
        }
    }
    Ok(None)
}

fn first_value<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| obj.get(*k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sui_types::base_types::SuiAddress;
    use sui_types::transaction::{ProgrammableTransaction, TransactionData, TransactionKind};
    use tempfile::TempDir;

    fn sample_state(digest: &str) -> ReplayState {
        let tx = FetchedTransaction {
            digest: TransactionDigest(digest.to_string()),
            sender: AccountAddress::from_hex_literal("0x1").unwrap(),
            gas_budget: 1,
            gas_price: 1,
            commands: vec![],
            inputs: vec![],
            effects: None,
            timestamp_ms: None,
            checkpoint: Some(7),
        };

        ReplayState {
            transaction: tx,
            objects: HashMap::new(),
            packages: HashMap::new(),
            protocol_version: 107,
            epoch: 1,
            reference_gas_price: None,
            checkpoint: Some(7),
        }
    }

    #[test]
    fn put_and_get_round_trip() {
        let tmp = TempDir::new().unwrap();
        let provider = FileStateProvider::new(tmp.path()).unwrap();
        let state = sample_state("digest-1");
        provider.put_state(&state).unwrap();

        let loaded = provider.get_state("digest-1").unwrap();
        assert_eq!(loaded.transaction.digest.0, "digest-1");
        assert_eq!(loaded.protocol_version, 107);
    }

    #[test]
    fn import_state_file_round_trip() {
        let tmp = TempDir::new().unwrap();
        let provider = FileStateProvider::new(tmp.path()).unwrap();
        let input = tmp.path().join("state.json");

        let json = serde_json::to_string_pretty(&sample_state("digest-2")).unwrap();
        fs::write(&input, json).unwrap();

        let summary = provider
            .import(&ImportSpec {
                state: Some(input),
                transactions: None,
                objects: None,
                packages: None,
            })
            .unwrap();

        assert_eq!(summary.states_imported, 1);
        assert_eq!(summary.digests, vec!["digest-2".to_string()]);
    }

    #[test]
    fn import_csv_rows() {
        let tmp = TempDir::new().unwrap();
        let provider = FileStateProvider::new(tmp.path()).unwrap();

        let tx_file = tmp.path().join("transactions.csv");
        fs::write(
            &tx_file,
            "digest,sender,gas_budget,gas_price,checkpoint,epoch,protocol_version\nabc,0x1,10,1,9,2,107\n",
        )
        .unwrap();

        let obj_file = tmp.path().join("objects.csv");
        fs::write(
            &obj_file,
            "tx_digest,object_id,version,type_tag,bcs_base64,owner_type\nabc,0x6,1,0x2::clock::Clock,AQID,Shared\n",
        )
        .unwrap();

        let summary = provider
            .import(&ImportSpec {
                state: None,
                transactions: Some(tx_file),
                objects: Some(obj_file),
                packages: None,
            })
            .unwrap();

        assert_eq!(summary.states_imported, 1);
        let loaded = provider.get_state("abc").unwrap();
        assert_eq!(loaded.objects.len(), 1);
    }

    #[test]
    fn import_transactions_json_rows_with_transaction_json_payload() {
        let tmp = TempDir::new().unwrap();
        let provider = FileStateProvider::new(tmp.path()).unwrap();

        let sender = SuiAddress::from(AccountAddress::from_hex_literal("0x1").unwrap());
        let tx_data = TransactionData::new_with_gas_coins(
            TransactionKind::ProgrammableTransaction(ProgrammableTransaction {
                inputs: vec![],
                commands: vec![],
            }),
            sender,
            vec![],
            555,
            12,
        );
        let tx_json = serde_json::to_string(&tx_data).unwrap();

        let tx_file = tmp.path().join("transactions.json");
        fs::write(
            &tx_file,
            serde_json::to_string_pretty(&serde_json::json!([{
                "digest": "abc-json",
                "checkpoint": 42,
                "transaction_json": tx_json
            }]))
            .unwrap(),
        )
        .unwrap();

        let summary = provider
            .import(&ImportSpec {
                state: None,
                transactions: Some(tx_file),
                objects: None,
                packages: None,
            })
            .unwrap();

        assert_eq!(summary.states_imported, 1);
        let loaded = provider.get_state("abc-json").unwrap();
        assert_eq!(loaded.transaction.gas_budget, 555);
        assert_eq!(loaded.transaction.gas_price, 12);
        assert_eq!(loaded.transaction.checkpoint, Some(42));
    }
}
