//! Replay-state JSON parsing.
//!
//! Supports both:
//! - strict `ReplayState` JSON schema (legacy)
//! - extended external schema with base64 BCS blobs

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use serde_json::{Map, Value};
use sui_sandbox_types::{FetchedTransaction, PtbCommand, TransactionInput};

use crate::bcs_codec::{deserialize_package_base64, deserialize_transaction_base64};
use crate::types::{PackageData, ReplayState, VersionedObject};

/// Parse one or many replay states from a JSON string.
///
/// Accepts either:
/// - a single JSON object
/// - a JSON array of state objects
pub fn parse_replay_states_json(contents: &str) -> Result<Vec<ReplayState>> {
    let value: Value =
        serde_json::from_str(contents).context("Failed to parse replay state JSON")?;
    parse_replay_states_value(&value)
}

/// Parse one or many replay states from a JSON file.
pub fn parse_replay_states_file(path: &Path) -> Result<Vec<ReplayState>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read replay state file: {}", path.display()))?;
    parse_replay_states_json(&contents)
}

/// Parse one or many replay states from a JSON value.
pub fn parse_replay_states_value(value: &Value) -> Result<Vec<ReplayState>> {
    match value {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                parse_replay_state_value(item)
                    .with_context(|| format!("Failed to parse replay state at index {i}"))
            })
            .collect(),
        _ => Ok(vec![parse_replay_state_value(value)?]),
    }
}

/// Parse a single replay state from strict or extended schema.
pub fn parse_replay_state_value(value: &Value) -> Result<ReplayState> {
    // Fast path: strict schema
    if let Ok(state) = serde_json::from_value::<ReplayState>(value.clone()) {
        return Ok(state);
    }

    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("Replay state must be a JSON object"))?;

    let checkpoint = optional_u64(obj, "checkpoint");
    let transaction = parse_transaction(obj.get("transaction"), checkpoint)?;
    let objects = parse_objects(obj.get("objects"))?;
    let packages = parse_packages(obj.get("packages"))?;

    Ok(ReplayState {
        transaction,
        objects,
        packages,
        protocol_version: optional_u64(obj, "protocol_version").unwrap_or(0),
        epoch: optional_u64(obj, "epoch").unwrap_or(0),
        reference_gas_price: optional_u64(obj, "reference_gas_price"),
        checkpoint,
    })
}

fn parse_transaction(
    value: Option<&Value>,
    fallback_checkpoint: Option<u64>,
) -> Result<FetchedTransaction> {
    let value = value.ok_or_else(|| anyhow!("Missing 'transaction' field in replay state"))?;

    if let Ok(mut tx) = serde_json::from_value::<FetchedTransaction>(value.clone()) {
        if tx.checkpoint.is_none() {
            tx.checkpoint = fallback_checkpoint;
        }
        return Ok(tx);
    }

    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("'transaction' must be an object"))?;

    let digest = optional_string(obj, &["digest"]).unwrap_or_else(|| "unknown".to_string());
    let tx_checkpoint = optional_u64(obj, "checkpoint").or(fallback_checkpoint);
    let timestamp_ms = optional_u64(obj, "timestamp_ms");
    let effects = parse_optional_effects(obj.get("effects"))?;

    if let Some(raw_bcs) = optional_string(
        obj,
        &[
            "raw_bcs",
            "raw_bcs_base64",
            "transaction_bcs",
            "bcs",
            "bcs_base64",
        ],
    ) {
        return deserialize_transaction_base64(
            &raw_bcs,
            digest,
            effects,
            timestamp_ms,
            tx_checkpoint,
        )
        .context("Failed to parse transaction.raw_bcs");
    }

    let sender_str = optional_string(obj, &["sender"]).unwrap_or_else(|| "0x0".to_string());
    let sender = AccountAddress::from_hex_literal(&sender_str)
        .with_context(|| format!("Invalid transaction sender: {}", sender_str))?;
    let commands = parse_optional_vec::<PtbCommand>(obj.get("commands"), "transaction.commands")?;
    let inputs = parse_optional_vec::<TransactionInput>(obj.get("inputs"), "transaction.inputs")?;

    Ok(FetchedTransaction {
        digest: sui_sandbox_types::TransactionDigest(digest),
        sender,
        gas_budget: optional_u64(obj, "gas_budget").unwrap_or_default(),
        gas_price: optional_u64(obj, "gas_price").unwrap_or_default(),
        commands,
        inputs,
        effects,
        timestamp_ms,
        checkpoint: tx_checkpoint,
    })
}

fn parse_objects(value: Option<&Value>) -> Result<HashMap<AccountAddress, VersionedObject>> {
    let Some(value) = value else {
        return Ok(HashMap::new());
    };

    let mut out = HashMap::new();
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let obj = parse_object_entry(Some(key), val)
                    .with_context(|| format!("Failed to parse object '{}'", key))?;
                out.insert(obj.id, obj);
            }
        }
        Value::Array(arr) => {
            for (idx, val) in arr.iter().enumerate() {
                let obj = parse_object_entry(None, val)
                    .with_context(|| format!("Failed to parse object at index {}", idx))?;
                out.insert(obj.id, obj);
            }
        }
        _ => return Err(anyhow!("'objects' must be a map or array")),
    }

    Ok(out)
}

fn parse_object_entry(key_hint: Option<&str>, value: &Value) -> Result<VersionedObject> {
    if let Ok(obj) = serde_json::from_value::<VersionedObject>(value.clone()) {
        return Ok(obj);
    }

    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("Object entry must be an object"))?;

    let id_str = optional_string(obj, &["object_id", "id"])
        .or_else(|| key_hint.map(ToString::to_string))
        .ok_or_else(|| anyhow!("Missing object id (object_id/id or map key)"))?;
    let id = AccountAddress::from_hex_literal(&id_str)
        .with_context(|| format!("Invalid object id: {}", id_str))?;

    let bytes = parse_object_bytes(obj)?;
    let owner_type = optional_string(obj, &["owner_type"]).map(|s| s.to_ascii_lowercase());
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
            optional_bool(obj, "is_shared").unwrap_or(false),
            optional_bool(obj, "is_immutable").unwrap_or(false),
        ),
    };

    Ok(VersionedObject {
        id,
        version: optional_u64(obj, "version").unwrap_or(1),
        digest: optional_string(obj, &["digest"]),
        type_tag: optional_string(obj, &["type_tag", "type"]),
        bcs_bytes: bytes,
        is_shared,
        is_immutable,
    })
}

fn parse_object_bytes(obj: &Map<String, Value>) -> Result<Vec<u8>> {
    if let Some(v) = first_value(obj, &["bcs_bytes", "bytes"]) {
        return parse_bytes_value(v).context("Invalid object bcs_bytes");
    }
    if let Some(v) = first_value(obj, &["bcs", "bcs_base64"]) {
        return parse_bytes_value(v).context("Invalid object bcs/bcs_base64");
    }
    Err(anyhow!(
        "Missing object bytes: expected one of bcs_bytes, bytes, bcs, bcs_base64"
    ))
}

fn parse_packages(value: Option<&Value>) -> Result<HashMap<AccountAddress, PackageData>> {
    let Some(value) = value else {
        return Ok(HashMap::new());
    };

    let mut out = HashMap::new();
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let pkg = parse_package_entry(Some(key), val)
                    .with_context(|| format!("Failed to parse package '{}'", key))?;
                out.insert(pkg.address, pkg);
            }
        }
        Value::Array(arr) => {
            for (idx, val) in arr.iter().enumerate() {
                let pkg = parse_package_entry(None, val)
                    .with_context(|| format!("Failed to parse package at index {}", idx))?;
                out.insert(pkg.address, pkg);
            }
        }
        _ => return Err(anyhow!("'packages' must be a map or array")),
    }

    Ok(out)
}

fn parse_package_entry(key_hint: Option<&str>, value: &Value) -> Result<PackageData> {
    if let Ok(pkg) = serde_json::from_value::<PackageData>(value.clone()) {
        return Ok(pkg);
    }

    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("Package entry must be an object"))?;

    let key_addr = key_hint
        .map(AccountAddress::from_hex_literal)
        .transpose()
        .ok()
        .flatten();

    // Preferred external format: full package bcs blob
    if let Some(encoded) = optional_string(obj, &["bcs", "bcs_base64"]) {
        let mut pkg = deserialize_package_base64(&encoded)
            .context("Failed to deserialize package bcs/bcs_base64")?;
        if let Some(addr) = key_addr {
            pkg.address = addr;
        }
        if let Some(version) = optional_u64(obj, "version") {
            pkg.version = version;
        }
        if let Some(original_id) = optional_string(obj, &["original_id"]) {
            pkg.original_id = Some(
                AccountAddress::from_hex_literal(&original_id)
                    .with_context(|| format!("Invalid package original_id: {}", original_id))?,
            );
        }
        if let Some(linkage_value) = obj.get("linkage") {
            pkg.linkage = parse_linkage(linkage_value)?;
        }
        return Ok(pkg);
    }

    // Fallback: explicit module blobs
    let modules = match obj.get("modules") {
        Some(v) => parse_modules(v)?,
        None => {
            return Err(anyhow!(
                "Missing package payload: expected bcs/bcs_base64 or modules"
            ))
        }
    };

    let address = if let Some(s) = optional_string(obj, &["package_id", "address"]) {
        AccountAddress::from_hex_literal(&s)
            .with_context(|| format!("Invalid package id: {}", s))?
    } else if let Some(addr) = key_addr {
        addr
    } else {
        return Err(anyhow!(
            "Missing package address: provide package_id/address or map key"
        ));
    };

    Ok(PackageData {
        address,
        version: optional_u64(obj, "version").unwrap_or(1),
        modules,
        linkage: obj
            .get("linkage")
            .map(parse_linkage)
            .transpose()?
            .unwrap_or_default(),
        original_id: optional_string(obj, &["original_id"])
            .map(|s| {
                AccountAddress::from_hex_literal(&s)
                    .with_context(|| format!("Invalid package original_id: {}", s))
            })
            .transpose()?,
    })
}

fn parse_modules(value: &Value) -> Result<Vec<(String, Vec<u8>)>> {
    if let Ok(modules) = serde_json::from_value::<Vec<(String, Vec<u8>)>>(value.clone()) {
        return Ok(modules);
    }

    match value {
        Value::Object(map) => map
            .iter()
            .map(|(name, bytes)| {
                parse_bytes_value(bytes)
                    .with_context(|| format!("Invalid module bytes for '{}'", name))
                    .map(|b| (name.clone(), b))
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let obj = item
                    .as_object()
                    .ok_or_else(|| anyhow!("modules[{}] must be an object", i))?;
                let name = optional_string(obj, &["name", "module"])
                    .ok_or_else(|| anyhow!("modules[{}] missing module name (name/module)", i))?;
                let bytes_value = first_value(obj, &["bytes", "bcs", "bcs_base64", "bytecode"])
                    .ok_or_else(|| anyhow!("modules[{}] missing module bytes", i))?;
                let bytes = parse_bytes_value(bytes_value)
                    .with_context(|| format!("Invalid module bytes at modules[{}]", i))?;
                Ok((name, bytes))
            })
            .collect(),
        _ => Err(anyhow!("'modules' must be map/array")),
    }
}

fn parse_linkage(value: &Value) -> Result<HashMap<AccountAddress, AccountAddress>> {
    let map = value
        .as_object()
        .ok_or_else(|| anyhow!("Package linkage must be an object"))?;
    map.iter()
        .map(|(k, v)| {
            let orig = AccountAddress::from_hex_literal(k)
                .with_context(|| format!("Invalid linkage key address: {}", k))?;
            let upgraded_str = v
                .as_str()
                .ok_or_else(|| anyhow!("Invalid linkage value for {}: expected string", k))?;
            let upgraded = AccountAddress::from_hex_literal(upgraded_str)
                .with_context(|| format!("Invalid linkage value address: {}", upgraded_str))?;
            Ok((orig, upgraded))
        })
        .collect()
}

fn parse_optional_effects(
    value: Option<&Value>,
) -> Result<Option<sui_sandbox_types::TransactionEffectsSummary>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .context("Invalid transaction effects")
            .map(Some),
    }
}

fn parse_optional_vec<T>(value: Option<&Value>, label: &str) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    match value {
        None => Ok(Vec::new()),
        Some(v) => serde_json::from_value(v.clone()).with_context(|| format!("Invalid {}", label)),
    }
}

fn parse_bytes_value(value: &Value) -> Result<Vec<u8>> {
    match value {
        Value::String(s) => crate::bcs_codec::decode_base64_bytes(s),
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

fn optional_string(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    first_value(obj, keys)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn optional_u64(obj: &Map<String, Value>, key: &str) -> Option<u64> {
    match obj.get(key) {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn optional_bool(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    match obj.get(key) {
        Some(Value::Bool(v)) => Some(*v),
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn first_value<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| obj.get(*k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use move_core_types::account_address::AccountAddress;
    use sui_types::base_types::SuiAddress;
    use sui_types::transaction::{ProgrammableTransaction, TransactionData, TransactionKind};

    #[test]
    fn parses_strict_schema() {
        let strict = serde_json::json!({
            "transaction": {
                "digest": "abc",
                "sender": "0x1",
                "gas_budget": 10,
                "gas_price": 1,
                "commands": [],
                "inputs": [],
                "effects": null,
                "timestamp_ms": null,
                "checkpoint": null
            },
            "objects": {},
            "packages": {},
            "protocol_version": 107,
            "epoch": 1,
            "reference_gas_price": null,
            "checkpoint": 10
        });

        let parsed = parse_replay_state_value(&strict).expect("parse strict");
        assert_eq!(parsed.transaction.digest.0, "abc");
        assert_eq!(parsed.epoch, 1);
        assert_eq!(parsed.checkpoint, Some(10));
    }

    #[test]
    fn parses_extended_base64_objects_and_raw_tx() {
        let sender = SuiAddress::from(AccountAddress::from_hex_literal("0x1").unwrap());
        let tx_data = TransactionData::new_with_gas_coins(
            TransactionKind::ProgrammableTransaction(ProgrammableTransaction {
                inputs: vec![],
                commands: vec![],
            }),
            sender,
            vec![],
            123,
            9,
        );
        let tx_bcs = bcs::to_bytes(&tx_data).expect("tx bcs");
        let tx_b64 = base64::engine::general_purpose::STANDARD.encode(&tx_bcs);

        let extended = serde_json::json!({
            "transaction": {
                "digest": "digest-1",
                "raw_bcs": tx_b64,
                "checkpoint": 22
            },
            "objects": {
                "0x6": {
                    "type_tag": "0x2::clock::Clock",
                    "version": 7,
                    "owner_type": "Shared",
                    "bcs": base64::engine::general_purpose::STANDARD.encode([1u8,2,3])
                }
            },
            "packages": {},
            "epoch": 12,
            "protocol_version": 107,
            "checkpoint": 22
        });

        let parsed = parse_replay_state_value(&extended).expect("parse extended");
        assert_eq!(parsed.transaction.digest.0, "digest-1");
        assert_eq!(parsed.transaction.gas_budget, 123);
        assert_eq!(parsed.transaction.gas_price, 9);
        assert_eq!(parsed.objects.len(), 1);
        let obj = parsed
            .objects
            .get(&AccountAddress::from_hex_literal("0x6").unwrap())
            .expect("clock object");
        assert!(obj.is_shared);
        assert_eq!(obj.bcs_bytes, vec![1, 2, 3]);
    }
}
