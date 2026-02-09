//! Walrus checkpoint to ReplayState conversion.
//!
//! Converts a typed `CheckpointData` (BCS-decoded from Walrus) directly into
//! a `ReplayState` suitable for local VM replay. This bypasses gRPC and GraphQL
//! entirely — no API keys or authentication required.
//!
//! # Usage
//!
//! ```ignore
//! use sui_transport::walrus::WalrusClient;
//! use sui_state_fetcher::walrus_replay::checkpoint_to_replay_state;
//!
//! let walrus = WalrusClient::mainnet();
//! let checkpoint_data = walrus.get_checkpoint(239_615_000)?;
//! let state = checkpoint_to_replay_state(&checkpoint_data, "D9sMA7x...")?;
//! // state is ready for VM execution
//! ```

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use sui_sandbox_types::{
    encoding::base64_encode, FetchedTransaction, GasSummary, PtbArgument, PtbCommand,
    TransactionDigest, TransactionEffectsSummary, TransactionInput, TransactionStatus,
};
use sui_types::effects::TransactionEffectsAPI;
use sui_types::full_checkpoint_content::CheckpointData;
use sui_types::object::{Data as SuiData, Owner};
use sui_types::transaction::{
    Argument as SuiArgument, CallArg, Command as SuiCommand, ObjectArg, SharedObjectMutability,
    TransactionDataAPI, TransactionKind,
};

use crate::provider::package_data_from_move_package;
use crate::types::{PackageData, ReplayState, VersionedObject};

/// Convert a Walrus `CheckpointData` + transaction digest into a `ReplayState`.
///
/// This is the main entry point. It:
/// 1. Finds the transaction by digest within the checkpoint
/// 2. Extracts the `FetchedTransaction` (commands, inputs, effects)
/// 3. Extracts input objects as `VersionedObject`s
/// 4. Extracts packages from input objects and sibling transactions
/// 5. Returns a complete `ReplayState` ready for VM execution
pub fn checkpoint_to_replay_state(
    checkpoint_data: &CheckpointData,
    digest: &str,
) -> Result<ReplayState> {
    let checkpoint_seq = checkpoint_data.checkpoint_summary.sequence_number;
    let timestamp_ms = checkpoint_data.checkpoint_summary.timestamp_ms;
    let epoch = checkpoint_data.checkpoint_summary.epoch;

    // Find the transaction
    let tx_index = find_tx_in_checkpoint(checkpoint_data, digest).ok_or_else(|| {
        anyhow!(
            "Transaction {} not found in checkpoint {}",
            digest,
            checkpoint_seq
        )
    })?;

    let checkpoint_tx = &checkpoint_data.transactions[tx_index];

    // Build FetchedTransaction
    let transaction =
        checkpoint_tx_to_fetched_transaction(checkpoint_tx, checkpoint_seq, timestamp_ms)
            .context("Failed to convert checkpoint transaction")?;

    // Extract objects from input_objects
    let mut objects: HashMap<AccountAddress, VersionedObject> = HashMap::new();
    let mut packages: HashMap<AccountAddress, PackageData> = HashMap::new();

    for obj in &checkpoint_tx.input_objects {
        match &obj.data {
            SuiData::Package(pkg) => {
                let pkg_data = package_data_from_move_package(pkg);
                packages.insert(pkg_data.address, pkg_data);
            }
            SuiData::Move(move_obj) => {
                let id = AccountAddress::from(obj.id());
                let version = obj.version().value();
                let bcs_bytes = move_obj.contents().to_vec();
                let type_tag = Some(move_obj.type_().to_string());
                let (is_shared, is_immutable) = owner_flags(&obj.owner);

                objects.insert(
                    id,
                    VersionedObject {
                        id,
                        version,
                        digest: None,
                        type_tag,
                        bcs_bytes,
                        is_shared,
                        is_immutable,
                    },
                );
            }
        }
    }

    // Scan sibling transactions for additional packages (dependencies)
    for (i, sibling_tx) in checkpoint_data.transactions.iter().enumerate() {
        if i == tx_index {
            continue; // Already processed
        }
        for obj in &sibling_tx.input_objects {
            if let SuiData::Package(pkg) = &obj.data {
                let pkg_data = package_data_from_move_package(pkg);
                packages.entry(pkg_data.address).or_insert(pkg_data);
            }
        }
        for obj in &sibling_tx.output_objects {
            if let SuiData::Package(pkg) = &obj.data {
                let pkg_data = package_data_from_move_package(pkg);
                packages.entry(pkg_data.address).or_insert(pkg_data);
            }
        }
    }

    Ok(ReplayState {
        transaction,
        objects,
        packages,
        protocol_version: 107, // Protocol version active at recent mainnet checkpoints
        epoch,
        reference_gas_price: None, // Not available from checkpoint summary
        checkpoint: Some(checkpoint_seq),
    })
}

/// Find a transaction by digest within a checkpoint.
///
/// Returns the index into `checkpoint_data.transactions`.
pub fn find_tx_in_checkpoint(checkpoint_data: &CheckpointData, digest: &str) -> Option<usize> {
    checkpoint_data
        .transactions
        .iter()
        .position(|tx| tx.transaction.digest().to_string() == digest)
}

/// Convert a `CheckpointTransaction` to a `FetchedTransaction`.
fn checkpoint_tx_to_fetched_transaction(
    checkpoint_tx: &sui_types::full_checkpoint_content::CheckpointTransaction,
    checkpoint_seq: u64,
    timestamp_ms: u64,
) -> Result<FetchedTransaction> {
    let tx_data = checkpoint_tx.transaction.data().transaction_data();
    let digest_str = checkpoint_tx.transaction.digest().to_string();
    let sender = AccountAddress::from(tx_data.sender());

    // Extract PTB commands and inputs
    let (commands, inputs) = match tx_data.kind() {
        TransactionKind::ProgrammableTransaction(ptb) => {
            let cmds: Vec<PtbCommand> = ptb
                .commands
                .iter()
                .filter_map(convert_sui_command)
                .collect();
            let inps: Vec<TransactionInput> = ptb.inputs.iter().map(convert_call_arg).collect();
            (cmds, inps)
        }
        _ => (Vec::new(), Vec::new()),
    };

    // Extract effects
    let effects = build_effects_summary(&checkpoint_tx.effects);

    Ok(FetchedTransaction {
        digest: TransactionDigest::new(digest_str),
        sender,
        gas_budget: tx_data.gas_budget(),
        gas_price: tx_data.gas_price(),
        commands,
        inputs,
        effects: Some(effects),
        timestamp_ms: Some(timestamp_ms),
        checkpoint: Some(checkpoint_seq),
    })
}

/// Convert a `sui_types::transaction::Command` to a `PtbCommand`.
fn convert_sui_command(cmd: &SuiCommand) -> Option<PtbCommand> {
    match cmd {
        SuiCommand::MoveCall(mc) => Some(PtbCommand::MoveCall {
            package: mc.package.to_hex_literal(),
            module: mc.module.to_string(),
            function: mc.function.to_string(),
            type_arguments: mc.type_arguments.iter().map(|t| t.to_string()).collect(),
            arguments: mc.arguments.iter().map(convert_sui_argument).collect(),
        }),
        SuiCommand::TransferObjects(objects, address) => Some(PtbCommand::TransferObjects {
            objects: objects.iter().map(convert_sui_argument).collect(),
            address: convert_sui_argument(address),
        }),
        SuiCommand::SplitCoins(coin, amounts) => Some(PtbCommand::SplitCoins {
            coin: convert_sui_argument(coin),
            amounts: amounts.iter().map(convert_sui_argument).collect(),
        }),
        SuiCommand::MergeCoins(dest, sources) => Some(PtbCommand::MergeCoins {
            destination: convert_sui_argument(dest),
            sources: sources.iter().map(convert_sui_argument).collect(),
        }),
        SuiCommand::MakeMoveVec(type_arg, elements) => Some(PtbCommand::MakeMoveVec {
            type_arg: type_arg.as_ref().map(|t| t.to_string()),
            elements: elements.iter().map(convert_sui_argument).collect(),
        }),
        SuiCommand::Publish(modules, dependencies) => Some(PtbCommand::Publish {
            modules: modules.iter().map(|m| base64_encode(m)).collect(),
            dependencies: dependencies.iter().map(|d| d.to_hex_literal()).collect(),
        }),
        SuiCommand::Upgrade(modules, _dependencies, package, ticket) => Some(PtbCommand::Upgrade {
            modules: modules.iter().map(|m| base64_encode(m)).collect(),
            package: package.to_hex_literal(),
            ticket: convert_sui_argument(ticket),
        }),
    }
}

/// Convert a `sui_types::transaction::Argument` to a `PtbArgument`.
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

/// Convert a `CallArg` to a `TransactionInput`.
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
        CallArg::FundsWithdrawal(_) => TransactionInput::Pure { bytes: Vec::new() },
    }
}

/// Build a `TransactionEffectsSummary` from `TransactionEffects`.
fn build_effects_summary(
    effects: &sui_types::effects::TransactionEffects,
) -> TransactionEffectsSummary {
    let status = if effects.status().is_ok() {
        TransactionStatus::Success
    } else {
        TransactionStatus::Failure {
            error: format!("{:?}", effects.status()),
        }
    };

    let created: Vec<String> = effects
        .created()
        .iter()
        .map(|(obj_ref, _)| obj_ref.0.to_hex_literal())
        .collect();

    let mutated: Vec<String> = effects
        .mutated()
        .iter()
        .map(|(obj_ref, _)| obj_ref.0.to_hex_literal())
        .collect();

    let deleted: Vec<String> = effects
        .deleted()
        .iter()
        .map(|obj_ref| obj_ref.0.to_hex_literal())
        .collect();

    let wrapped: Vec<String> = effects
        .wrapped()
        .iter()
        .map(|obj_ref| obj_ref.0.to_hex_literal())
        .collect();

    let unwrapped: Vec<String> = effects
        .unwrapped()
        .iter()
        .map(|(obj_ref, _)| obj_ref.0.to_hex_literal())
        .collect();

    let gas_summary = effects.gas_cost_summary();
    let gas_used = GasSummary {
        computation_cost: gas_summary.computation_cost,
        storage_cost: gas_summary.storage_cost,
        storage_rebate: gas_summary.storage_rebate,
        non_refundable_storage_fee: gas_summary.non_refundable_storage_fee,
    };

    TransactionEffectsSummary {
        status,
        created,
        mutated,
        deleted,
        wrapped,
        unwrapped,
        gas_used,
        events_count: 0,
        shared_object_versions: HashMap::new(),
    }
}

/// Extract shared/immutable flags from an object owner.
fn owner_flags(owner: &Owner) -> (bool, bool) {
    match owner {
        Owner::Shared { .. } => (true, false),
        Owner::Immutable => (false, true),
        _ => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_sui_argument() {
        assert!(matches!(
            convert_sui_argument(&SuiArgument::GasCoin),
            PtbArgument::GasCoin
        ));

        match convert_sui_argument(&SuiArgument::Input(5)) {
            PtbArgument::Input { index } => assert_eq!(index, 5),
            _ => panic!("Expected Input"),
        }

        match convert_sui_argument(&SuiArgument::Result(3)) {
            PtbArgument::Result { index } => assert_eq!(index, 3),
            _ => panic!("Expected Result"),
        }

        match convert_sui_argument(&SuiArgument::NestedResult(2, 1)) {
            PtbArgument::NestedResult {
                index,
                result_index,
            } => {
                assert_eq!(index, 2);
                assert_eq!(result_index, 1);
            }
            _ => panic!("Expected NestedResult"),
        }
    }
}
