//! Public BCS codec helpers for replay/import workflows.
//!
//! This module centralizes transaction/package deserialization used by:
//! - extended `--state-json` ingestion
//! - file import/cache pipelines
//! - Python bindings

use anyhow::{Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use serde_json::Value;
use sui_sandbox_types::{
    FetchedTransaction, PtbArgument, PtbCommand, TransactionDigest, TransactionEffectsSummary,
    TransactionInput,
};
use sui_types::move_package::MovePackage;
use sui_types::object::{Data as SuiData, Object as SuiObject};
use sui_types::transaction::{
    Argument as SuiArgument, CallArg, Command as SuiCommand, ObjectArg, SharedObjectMutability,
    TransactionData, TransactionDataAPI, TransactionKind,
};

use crate::provider::package_data_from_move_package;
use crate::types::PackageData;

/// Decode base64 data with/without padding.
pub fn decode_base64_bytes(encoded: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
        .with_context(|| "Failed to decode base64 bytes")
}

/// Deserialize raw BCS bytes into `TransactionData`.
pub fn deserialize_transaction_data(raw_bcs: &[u8]) -> Result<TransactionData> {
    bcs::from_bytes(raw_bcs).context("Failed to parse transaction BCS")
}

/// Deserialize base64-encoded BCS into `TransactionData`.
pub fn deserialize_transaction_data_base64(encoded: &str) -> Result<TransactionData> {
    let raw = decode_base64_bytes(encoded).context("Failed to decode transaction BCS base64")?;
    deserialize_transaction_data(&raw)
}

/// Normalize Snowflake-style transaction JSON into a serde shape accepted by Sui `TransactionData`.
pub fn normalize_transaction_json(value: &Value) -> Value {
    let mut normalized = value.clone();
    normalize_transaction_json_value(&mut normalized);
    normalized
}

/// Deserialize transaction JSON (Snowflake or canonical serde shape) into `TransactionData`.
pub fn deserialize_transaction_data_json_value(value: &Value) -> Result<TransactionData> {
    let normalized = normalize_transaction_json(value);
    serde_json::from_value(normalized)
        .context("Failed to parse transaction JSON as Sui TransactionData")
}

/// Deserialize transaction JSON text (Snowflake or canonical serde shape) into `TransactionData`.
pub fn deserialize_transaction_data_json_str(json_str: &str) -> Result<TransactionData> {
    let value: Value =
        serde_json::from_str(json_str).context("Failed to parse transaction JSON text")?;
    deserialize_transaction_data_json_value(&value)
}

/// Convert Snowflake `TRANSACTION_JSON` (or canonical serde JSON) into raw transaction BCS bytes.
pub fn transaction_json_to_bcs(json_str: &str) -> Result<Vec<u8>> {
    let tx_data = deserialize_transaction_data_json_str(json_str)?;
    bcs::to_bytes(&tx_data).context("Failed to serialize transaction JSON to BCS")
}

/// Convert transaction JSON into base64-encoded BCS bytes.
pub fn transaction_json_to_bcs_base64(json_str: &str) -> Result<String> {
    let raw = transaction_json_to_bcs(json_str)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(raw))
}

/// Build a sandbox `FetchedTransaction` from Sui `TransactionData`.
pub fn transaction_data_to_fetched_transaction(
    tx_data: &TransactionData,
    digest: impl Into<String>,
    effects: Option<TransactionEffectsSummary>,
    timestamp_ms: Option<u64>,
    checkpoint: Option<u64>,
) -> FetchedTransaction {
    let (commands, inputs) = match tx_data.kind() {
        TransactionKind::ProgrammableTransaction(ptb) => (
            ptb.commands.iter().map(convert_sui_command).collect(),
            ptb.inputs.iter().map(convert_call_arg).collect(),
        ),
        _ => (Vec::new(), Vec::new()),
    };

    FetchedTransaction {
        digest: TransactionDigest::new(digest),
        sender: AccountAddress::from(tx_data.sender()),
        gas_budget: tx_data.gas_budget(),
        gas_price: tx_data.gas_price(),
        commands,
        inputs,
        effects,
        timestamp_ms,
        checkpoint,
    }
}

/// Deserialize a transaction from raw BCS bytes into sandbox format.
pub fn deserialize_transaction(
    raw_bcs: &[u8],
    digest: impl Into<String>,
    effects: Option<TransactionEffectsSummary>,
    timestamp_ms: Option<u64>,
    checkpoint: Option<u64>,
) -> Result<FetchedTransaction> {
    let tx_data = deserialize_transaction_data(raw_bcs)?;
    Ok(transaction_data_to_fetched_transaction(
        &tx_data,
        digest,
        effects,
        timestamp_ms,
        checkpoint,
    ))
}

/// Deserialize a transaction from base64 BCS into sandbox format.
pub fn deserialize_transaction_base64(
    encoded: &str,
    digest: impl Into<String>,
    effects: Option<TransactionEffectsSummary>,
    timestamp_ms: Option<u64>,
    checkpoint: Option<u64>,
) -> Result<FetchedTransaction> {
    let raw = decode_base64_bytes(encoded).context("Failed to decode transaction BCS base64")?;
    deserialize_transaction(&raw, digest, effects, timestamp_ms, checkpoint)
}

/// Deserialize package bytes from either:
/// - BCS-encoded `MovePackage`, or
/// - BCS-encoded package `Object` wrapper.
pub fn deserialize_package(raw_bcs: &[u8]) -> Result<PackageData> {
    if let Ok(obj) = bcs::from_bytes::<SuiObject>(raw_bcs) {
        if let SuiData::Package(pkg) = &obj.data {
            return Ok(package_data_from_move_package(pkg));
        }
    }

    let pkg: MovePackage = bcs::from_bytes(raw_bcs).context("Failed to parse package BCS")?;
    Ok(package_data_from_move_package(&pkg))
}

/// Deserialize package data from base64 BCS.
pub fn deserialize_package_base64(encoded: &str) -> Result<PackageData> {
    let raw = decode_base64_bytes(encoded).context("Failed to decode package BCS base64")?;
    deserialize_package(&raw)
}

fn convert_sui_command(cmd: &SuiCommand) -> PtbCommand {
    match cmd {
        SuiCommand::MoveCall(mc) => PtbCommand::MoveCall {
            package: mc.package.to_hex_literal(),
            module: mc.module.to_string(),
            function: mc.function.to_string(),
            type_arguments: mc.type_arguments.iter().map(|t| t.to_string()).collect(),
            arguments: mc.arguments.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::TransferObjects(objects, address) => PtbCommand::TransferObjects {
            objects: objects.iter().map(convert_sui_argument).collect(),
            address: convert_sui_argument(address),
        },
        SuiCommand::SplitCoins(coin, amounts) => PtbCommand::SplitCoins {
            coin: convert_sui_argument(coin),
            amounts: amounts.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::MergeCoins(dest, sources) => PtbCommand::MergeCoins {
            destination: convert_sui_argument(dest),
            sources: sources.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::MakeMoveVec(type_arg, elements) => PtbCommand::MakeMoveVec {
            type_arg: type_arg.as_ref().map(|t| t.to_string()),
            elements: elements.iter().map(convert_sui_argument).collect(),
        },
        SuiCommand::Publish(modules, dependencies) => PtbCommand::Publish {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            dependencies: dependencies.iter().map(|d| d.to_hex_literal()).collect(),
        },
        SuiCommand::Upgrade(modules, _dependencies, package, ticket) => PtbCommand::Upgrade {
            modules: modules
                .iter()
                .map(|m| base64::engine::general_purpose::STANDARD.encode(m))
                .collect(),
            package: package.to_hex_literal(),
            ticket: convert_sui_argument(ticket),
        },
    }
}

fn convert_sui_argument(arg: &SuiArgument) -> PtbArgument {
    match arg {
        SuiArgument::GasCoin => PtbArgument::GasCoin,
        SuiArgument::Input(i) => PtbArgument::Input { index: *i },
        SuiArgument::Result(i) => PtbArgument::Result { index: *i },
        SuiArgument::NestedResult(i, j) => PtbArgument::NestedResult {
            index: *i,
            result_index: *j,
        },
    }
}

fn convert_call_arg(arg: &CallArg) -> TransactionInput {
    match arg {
        CallArg::Pure(bytes) => TransactionInput::Pure {
            bytes: bytes.clone(),
        },
        CallArg::Object(obj_arg) => match obj_arg {
            ObjectArg::ImmOrOwnedObject(obj_ref) => TransactionInput::Object {
                object_id: obj_ref.0.to_hex_literal(),
                version: obj_ref.1.value(),
                digest: obj_ref.2.to_string(),
            },
            ObjectArg::SharedObject {
                id,
                initial_shared_version,
                mutability,
            } => TransactionInput::SharedObject {
                object_id: id.to_hex_literal(),
                initial_shared_version: initial_shared_version.value(),
                mutable: matches!(mutability, SharedObjectMutability::Mutable),
            },
            ObjectArg::Receiving(obj_ref) => TransactionInput::Receiving {
                object_id: obj_ref.0.to_hex_literal(),
                version: obj_ref.1.value(),
                digest: obj_ref.2.to_string(),
            },
        },
        CallArg::FundsWithdrawal(_) => {
            // Not used for replay execution; keep shape-compatible placeholder.
            TransactionInput::Pure { bytes: Vec::new() }
        }
    }
}

fn normalize_transaction_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // Snowflake may emit StructTag as { ..., "type_args": [...] }.
            if let Some(type_args) = map.remove("type_args") {
                map.entry("type_params".to_string()).or_insert(type_args);
            }

            for key in [
                "address",
                "sender",
                "owner",
                "package",
                "id",
                "object_id",
                "package_id",
                "original_id",
            ] {
                if let Some(Value::String(s)) = map.get_mut(key) {
                    normalize_hex_address_in_place(s);
                }
            }

            // Accept flexible casing/format for shared-object mutability.
            if let Some(Value::String(s)) = map.get_mut("mutability") {
                *s = normalize_mutability_string(s);
            }

            for child in map.values_mut() {
                normalize_transaction_json_value(child);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                normalize_transaction_json_value(item);
            }
        }
        _ => {}
    }
}

fn normalize_hex_address_in_place(s: &mut String) {
    let trimmed = s.trim();
    if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
        if trimmed != s {
            *s = trimmed.to_string();
        }
        return;
    }
    if is_hex_address_like(trimmed) {
        *s = format!("0x{trimmed}");
    }
}

fn is_hex_address_like(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_mutability_string(s: &str) -> String {
    let collapsed: String = s
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '_' && *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    match collapsed.as_str() {
        "mutable" => "Mutable".to_string(),
        "immutable" => "Immutable".to_string(),
        "nonexclusivewrite" => "NonExclusiveWrite".to_string(),
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use move_core_types::account_address::AccountAddress;
    use sui_types::base_types::SuiAddress;
    use sui_types::transaction::{ProgrammableTransaction, TransactionData, TransactionKind};

    #[test]
    fn transaction_json_to_bcs_roundtrip_canonical() {
        let sender = SuiAddress::from(AccountAddress::from_hex_literal("0x1").expect("sender"));
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

        let canonical_json = serde_json::to_string(&tx_data).expect("serialize tx json");
        let expected_bcs = bcs::to_bytes(&tx_data).expect("tx bcs");
        let got_bcs = transaction_json_to_bcs(&canonical_json).expect("json->bcs");
        assert_eq!(got_bcs, expected_bcs);
    }

    #[test]
    fn transaction_json_to_bcs_accepts_unprefixed_hex_sender_owner() {
        let sender = SuiAddress::from(AccountAddress::from_hex_literal("0x1").expect("sender"));
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
        let mut tx_json = serde_json::to_value(&tx_data).expect("tx json");
        strip_hex_prefix_for_key_recursively(&mut tx_json, "sender");
        strip_hex_prefix_for_key_recursively(&mut tx_json, "owner");

        let expected_bcs = bcs::to_bytes(&tx_data).expect("tx bcs");
        let input = serde_json::to_string(&tx_json).expect("stringify");
        let got_bcs = transaction_json_to_bcs(&input).expect("json->bcs");
        assert_eq!(got_bcs, expected_bcs);
    }

    #[test]
    fn normalize_transaction_json_renames_type_args_and_prefixes_struct_address() {
        let input = serde_json::json!({
            "V1": {
                "kind": {
                    "ProgrammableTransaction": {
                        "inputs": [],
                        "commands": [{
                            "MoveCall": {
                                "package": "cafebabe",
                                "module": "pool",
                                "function": "f",
                                "type_arguments": [{
                                    "struct": {
                                        "address": "5145494a",
                                        "module": "ns",
                                        "name": "NS",
                                        "type_args": []
                                    }
                                }],
                                "arguments": []
                            }
                        }]
                    }
                },
                "sender": "9042937",
                "gas_data": {
                    "price": 1,
                    "owner": "9042937",
                    "payment": [],
                    "budget": 1000
                },
                "expiration": "None"
            }
        });

        let normalized = normalize_transaction_json(&input);
        let struct_tag = normalized["V1"]["kind"]["ProgrammableTransaction"]["commands"][0]
            ["MoveCall"]["type_arguments"][0]["struct"]
            .as_object()
            .expect("struct tag object");

        assert_eq!(
            struct_tag.get("address").and_then(Value::as_str),
            Some("0x5145494a")
        );
        assert!(struct_tag.get("type_args").is_none());
        assert!(struct_tag.get("type_params").is_some());
        assert_eq!(
            normalized["V1"]["kind"]["ProgrammableTransaction"]["commands"][0]["MoveCall"]
                ["package"]
                .as_str(),
            Some("0xcafebabe")
        );
        assert_eq!(normalized["V1"]["sender"].as_str(), Some("0x9042937"));
        assert_eq!(
            normalized["V1"]["gas_data"]["owner"].as_str(),
            Some("0x9042937")
        );
    }

    fn strip_hex_prefix_for_key_recursively(value: &mut Value, key: &str) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(s)) = map.get_mut(key) {
                    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                        *s = rest.to_string();
                    }
                }
                for child in map.values_mut() {
                    strip_hex_prefix_for_key_recursively(child, key);
                }
            }
            Value::Array(arr) => {
                for child in arr {
                    strip_hex_prefix_for_key_recursively(child, key);
                }
            }
            _ => {}
        }
    }
}
