//! Public BCS codec helpers for replay/import workflows.
//!
//! This module centralizes transaction/package deserialization used by:
//! - extended `--state-json` ingestion
//! - file import/cache pipelines
//! - Python bindings

use anyhow::{Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
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
