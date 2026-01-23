//! gRPC to FetchedTransaction Conversion
//!
//! Converts gRPC transaction data to the `FetchedTransaction` format used
//! by the replay infrastructure.
//!
//! This enables using transactions fetched directly via gRPC (e.g., from Surflux)
//! with the CachedTransaction and replay infrastructure.
//!
//! gRPC transactions provide additional historical data that GraphQL doesn't:
//! - `unchanged_loaded_runtime_objects`: Objects read but not modified (with exact versions)
//! - `unchanged_consensus_objects`: Actual consensus versions for shared objects
//! - `changed_objects`: Objects modified with their INPUT versions

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use sui_sandbox_types::{
    FetchedTransaction, GasSummary, PtbArgument, PtbCommand, TransactionDigest,
    TransactionEffectsSummary, TransactionInput, TransactionStatus,
};

use crate::grpc::{GrpcArgument, GrpcCommand, GrpcInput, GrpcTransaction};

/// Convert a gRPC transaction to the internal FetchedTransaction format.
///
/// This enables using transactions fetched directly via gRPC (e.g., from Surflux)
/// with the CachedTransaction and replay infrastructure.
///
/// gRPC transactions provide additional historical data that GraphQL doesn't:
/// - `unchanged_loaded_runtime_objects`: Objects read but not modified (with exact versions)
/// - `unchanged_consensus_objects`: Actual consensus versions for shared objects
/// - `changed_objects`: Objects modified with their INPUT versions (before tx)
pub fn grpc_to_fetched_transaction(tx: &GrpcTransaction) -> Result<FetchedTransaction> {
    // Parse sender address
    let sender_hex = tx.sender.strip_prefix("0x").unwrap_or(&tx.sender);
    let sender = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))
        .map_err(|e| anyhow::anyhow!("Invalid sender address: {}", e))?;

    // Convert inputs
    let inputs: Vec<TransactionInput> = tx
        .inputs
        .iter()
        .map(|input| match input {
            GrpcInput::Pure { bytes } => TransactionInput::Pure {
                bytes: bytes.clone(),
            },
            GrpcInput::Object {
                object_id,
                version,
                digest,
            } => TransactionInput::Object {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                mutable,
            } => TransactionInput::SharedObject {
                object_id: object_id.clone(),
                initial_shared_version: *initial_version,
                mutable: *mutable,
            },
            GrpcInput::Receiving {
                object_id,
                version,
                digest,
            } => TransactionInput::Receiving {
                object_id: object_id.clone(),
                version: *version,
                digest: digest.clone(),
            },
        })
        .collect();

    // Convert commands
    let commands: Vec<PtbCommand> = tx
        .commands
        .iter()
        .filter_map(convert_grpc_command)
        .collect();

    // Convert effects status
    let effects = tx.status.as_ref().map(|status| {
        let tx_status = if status == "success" {
            TransactionStatus::Success
        } else {
            TransactionStatus::Failure {
                error: tx
                    .execution_error
                    .as_ref()
                    .and_then(|e| e.description.clone())
                    .unwrap_or_else(|| status.clone()),
            }
        };

        TransactionEffectsSummary {
            status: tx_status,
            created: tx
                .created_objects
                .iter()
                .map(|(id, _)| id.clone())
                .collect(),
            mutated: tx
                .changed_objects
                .iter()
                .map(|(id, _)| id.clone())
                .collect(),
            deleted: Vec::new(),
            wrapped: Vec::new(),
            unwrapped: Vec::new(),
            gas_used: GasSummary::default(),
            events_count: 0,
            shared_object_versions: std::collections::HashMap::new(),
        }
    });

    Ok(FetchedTransaction {
        digest: TransactionDigest(tx.digest.clone()),
        sender,
        gas_budget: tx.gas_budget.unwrap_or(0),
        gas_price: tx.gas_price.unwrap_or(0),
        commands,
        inputs,
        effects,
        timestamp_ms: tx.timestamp_ms,
        checkpoint: tx.checkpoint,
    })
}

/// Convert a gRPC command to PtbCommand
fn convert_grpc_command(cmd: &GrpcCommand) -> Option<PtbCommand> {
    match cmd {
        GrpcCommand::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => Some(PtbCommand::MoveCall {
            package: package.clone(),
            module: module.clone(),
            function: function.clone(),
            type_arguments: type_arguments.clone(),
            arguments: arguments.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::TransferObjects { objects, address } => Some(PtbCommand::TransferObjects {
            objects: objects.iter().map(convert_grpc_argument).collect(),
            address: convert_grpc_argument(address),
        }),
        GrpcCommand::SplitCoins { coin, amounts } => Some(PtbCommand::SplitCoins {
            coin: convert_grpc_argument(coin),
            amounts: amounts.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::MergeCoins { coin, sources } => Some(PtbCommand::MergeCoins {
            destination: convert_grpc_argument(coin),
            sources: sources.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::MakeMoveVec {
            element_type,
            elements,
        } => Some(PtbCommand::MakeMoveVec {
            type_arg: element_type.clone(),
            elements: elements.iter().map(convert_grpc_argument).collect(),
        }),
        GrpcCommand::Publish {
            modules,
            dependencies,
        } => Some(PtbCommand::Publish {
            modules: modules
                .iter()
                .map(|m| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, m))
                .collect(),
            dependencies: dependencies.clone(),
        }),
        GrpcCommand::Upgrade {
            modules,
            package,
            ticket,
            ..
        } => Some(PtbCommand::Upgrade {
            modules: modules
                .iter()
                .map(|m| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, m))
                .collect(),
            package: package.clone(),
            ticket: convert_grpc_argument(ticket),
        }),
    }
}

/// Convert a gRPC argument to PtbArgument
fn convert_grpc_argument(arg: &GrpcArgument) -> PtbArgument {
    match arg {
        GrpcArgument::GasCoin => PtbArgument::GasCoin,
        GrpcArgument::Input(index) => PtbArgument::Input {
            index: *index as u16,
        },
        GrpcArgument::Result(index) => PtbArgument::Result {
            index: *index as u16,
        },
        GrpcArgument::NestedResult(index, result_idx) => PtbArgument::NestedResult {
            index: *index as u16,
            result_index: *result_idx as u16,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_grpc_argument_gas_coin() {
        let arg = convert_grpc_argument(&GrpcArgument::GasCoin);
        assert!(matches!(arg, PtbArgument::GasCoin));
    }

    #[test]
    fn test_convert_grpc_argument_input() {
        let arg = convert_grpc_argument(&GrpcArgument::Input(5));
        match arg {
            PtbArgument::Input { index } => assert_eq!(index, 5),
            _ => panic!("Expected Input argument"),
        }
    }

    #[test]
    fn test_convert_grpc_argument_result() {
        let arg = convert_grpc_argument(&GrpcArgument::Result(3));
        match arg {
            PtbArgument::Result { index } => assert_eq!(index, 3),
            _ => panic!("Expected Result argument"),
        }
    }

    #[test]
    fn test_convert_grpc_argument_nested_result() {
        let arg = convert_grpc_argument(&GrpcArgument::NestedResult(2, 1));
        match arg {
            PtbArgument::NestedResult {
                index,
                result_index,
            } => {
                assert_eq!(index, 2);
                assert_eq!(result_index, 1);
            }
            _ => panic!("Expected NestedResult argument"),
        }
    }

    #[test]
    fn test_convert_grpc_command_move_call() {
        let cmd = GrpcCommand::MoveCall {
            package: "0x2".to_string(),
            module: "coin".to_string(),
            function: "value".to_string(),
            type_arguments: vec!["0x2::sui::SUI".to_string()],
            arguments: vec![GrpcArgument::Input(0)],
        };
        let converted = convert_grpc_command(&cmd).expect("Should convert");
        match converted {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => {
                assert_eq!(package, "0x2");
                assert_eq!(module, "coin");
                assert_eq!(function, "value");
                assert_eq!(type_arguments, vec!["0x2::sui::SUI"]);
                assert_eq!(arguments.len(), 1);
            }
            _ => panic!("Expected MoveCall command"),
        }
    }

    #[test]
    fn test_convert_grpc_command_split_coins() {
        let cmd = GrpcCommand::SplitCoins {
            coin: GrpcArgument::GasCoin,
            amounts: vec![GrpcArgument::Input(0), GrpcArgument::Input(1)],
        };
        let converted = convert_grpc_command(&cmd).expect("Should convert");
        match converted {
            PtbCommand::SplitCoins { coin, amounts } => {
                assert!(matches!(coin, PtbArgument::GasCoin));
                assert_eq!(amounts.len(), 2);
            }
            _ => panic!("Expected SplitCoins command"),
        }
    }

    #[test]
    fn test_convert_grpc_command_transfer_objects() {
        let cmd = GrpcCommand::TransferObjects {
            objects: vec![GrpcArgument::Result(0)],
            address: GrpcArgument::Input(1),
        };
        let converted = convert_grpc_command(&cmd).expect("Should convert");
        match converted {
            PtbCommand::TransferObjects { objects, address } => {
                assert_eq!(objects.len(), 1);
                assert!(matches!(address, PtbArgument::Input { index: 1 }));
            }
            _ => panic!("Expected TransferObjects command"),
        }
    }

    #[test]
    fn test_convert_grpc_command_merge_coins() {
        let cmd = GrpcCommand::MergeCoins {
            coin: GrpcArgument::Input(0),
            sources: vec![GrpcArgument::Input(1), GrpcArgument::Input(2)],
        };
        let converted = convert_grpc_command(&cmd).expect("Should convert");
        match converted {
            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                assert!(matches!(destination, PtbArgument::Input { index: 0 }));
                assert_eq!(sources.len(), 2);
            }
            _ => panic!("Expected MergeCoins command"),
        }
    }

    #[test]
    fn test_convert_grpc_command_make_move_vec() {
        let cmd = GrpcCommand::MakeMoveVec {
            element_type: Some("u64".to_string()),
            elements: vec![GrpcArgument::Input(0)],
        };
        let converted = convert_grpc_command(&cmd).expect("Should convert");
        match converted {
            PtbCommand::MakeMoveVec { type_arg, elements } => {
                assert_eq!(type_arg, Some("u64".to_string()));
                assert_eq!(elements.len(), 1);
            }
            _ => panic!("Expected MakeMoveVec command"),
        }
    }

    #[test]
    fn test_grpc_to_fetched_transaction_basic() {
        let grpc_tx = GrpcTransaction {
            digest: "test_digest".to_string(),
            sender: "0x1".to_string(),
            gas_budget: Some(1000),
            gas_price: Some(1),
            checkpoint: Some(100),
            timestamp_ms: Some(1234567890),
            inputs: vec![GrpcInput::Pure {
                bytes: vec![1, 2, 3],
            }],
            commands: vec![GrpcCommand::MoveCall {
                package: "0x2".to_string(),
                module: "coin".to_string(),
                function: "value".to_string(),
                type_arguments: vec![],
                arguments: vec![GrpcArgument::Input(0)],
            }],
            status: Some("success".to_string()),
            execution_error: None,
            unchanged_loaded_runtime_objects: vec![],
            changed_objects: vec![],
            created_objects: vec![],
            unchanged_consensus_objects: vec![],
        };

        let fetched = grpc_to_fetched_transaction(&grpc_tx).expect("Should convert");
        assert_eq!(fetched.digest.0, "test_digest");
        assert_eq!(fetched.gas_budget, 1000);
        assert_eq!(fetched.gas_price, 1);
        assert_eq!(fetched.checkpoint, Some(100));
        assert_eq!(fetched.timestamp_ms, Some(1234567890));
        assert_eq!(fetched.inputs.len(), 1);
        assert_eq!(fetched.commands.len(), 1);
        assert!(fetched.effects.is_some());
    }

    #[test]
    fn test_grpc_to_fetched_transaction_with_failure() {
        let grpc_tx = GrpcTransaction {
            digest: "failed_tx".to_string(),
            sender: "0x1".to_string(),
            gas_budget: Some(1000),
            gas_price: Some(1),
            checkpoint: None,
            timestamp_ms: None,
            inputs: vec![],
            commands: vec![],
            status: Some("failure".to_string()),
            execution_error: Some(crate::grpc::GrpcExecutionError {
                description: Some("Out of gas".to_string()),
                command: Some(0),
                kind: Some("INSUFFICIENT_GAS".to_string()),
                move_abort: None,
            }),
            unchanged_loaded_runtime_objects: vec![],
            changed_objects: vec![],
            created_objects: vec![],
            unchanged_consensus_objects: vec![],
        };

        let fetched = grpc_to_fetched_transaction(&grpc_tx).expect("Should convert");
        let effects = fetched.effects.expect("Should have effects");
        match effects.status {
            TransactionStatus::Failure { error } => {
                assert_eq!(error, "Out of gas");
            }
            _ => panic!("Expected failure status"),
        }
    }
}
