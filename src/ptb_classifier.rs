use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use sui_transport::graphql::{GraphQLCommand, GraphQLTransaction, GraphQLTransactionInput};

/// Internal PTB classification for replay robustness testing.
///
/// This is not user-facing; it exists to filter out trivial framework-only
/// transactions so parity stats aren't inflated by simple cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtbClassification {
    pub digest: String,
    pub checkpoint: Option<u64>,
    pub tags: Vec<String>,
    pub is_framework_only: bool,
    pub is_trivial_framework: bool,
    pub non_system_packages: Vec<String>,
    pub system_packages: Vec<String>,
    pub has_publish: bool,
    pub has_upgrade: bool,
    pub has_shared_inputs: bool,
    pub has_receiving_inputs: bool,
    pub command_kinds: Vec<String>,
}

pub fn classify_ptb(tx: &GraphQLTransaction) -> PtbClassification {
    let mut system_packages: BTreeSet<String> = BTreeSet::new();
    let mut non_system_packages: BTreeSet<String> = BTreeSet::new();
    let mut command_kinds: BTreeSet<String> = BTreeSet::new();
    let mut has_publish = false;
    let mut has_upgrade = false;

    for cmd in &tx.commands {
        match cmd {
            GraphQLCommand::MoveCall { package, .. } => {
                command_kinds.insert("MoveCall".to_string());
                let norm = normalize_package(package);
                if is_system_package(&norm) {
                    system_packages.insert(norm);
                } else {
                    non_system_packages.insert(norm);
                }
            }
            GraphQLCommand::SplitCoins { .. } => {
                command_kinds.insert("SplitCoins".to_string());
            }
            GraphQLCommand::MergeCoins { .. } => {
                command_kinds.insert("MergeCoins".to_string());
            }
            GraphQLCommand::TransferObjects { .. } => {
                command_kinds.insert("TransferObjects".to_string());
            }
            GraphQLCommand::MakeMoveVec { .. } => {
                command_kinds.insert("MakeMoveVec".to_string());
            }
            GraphQLCommand::Publish { .. } => {
                command_kinds.insert("Publish".to_string());
                has_publish = true;
            }
            GraphQLCommand::Upgrade { .. } => {
                command_kinds.insert("Upgrade".to_string());
                has_upgrade = true;
            }
            GraphQLCommand::Other { .. } => {
                command_kinds.insert("Other".to_string());
            }
        }
    }

    let mut has_shared_inputs = false;
    let mut has_receiving_inputs = false;
    for input in &tx.inputs {
        match input {
            GraphQLTransactionInput::SharedObject { .. } => has_shared_inputs = true,
            GraphQLTransactionInput::Receiving { .. } => has_receiving_inputs = true,
            _ => {}
        }
    }

    let is_framework_only = non_system_packages.is_empty();

    let simple_cmds_only = command_kinds.iter().all(|k| {
        matches!(
            k.as_str(),
            "MoveCall" | "SplitCoins" | "MergeCoins" | "TransferObjects" | "MakeMoveVec"
        )
    });

    let is_trivial_framework =
        is_framework_only && simple_cmds_only && !has_publish && !has_upgrade && !has_shared_inputs;

    let mut tags = Vec::new();
    if is_framework_only {
        tags.push("framework_only".to_string());
    } else {
        tags.push("app_call".to_string());
    }
    if has_publish {
        tags.push("publish".to_string());
    }
    if has_upgrade {
        tags.push("upgrade".to_string());
    }
    if has_shared_inputs {
        tags.push("shared".to_string());
    }
    if has_receiving_inputs {
        tags.push("receiving".to_string());
    }
    if non_system_packages.len() > 1 {
        tags.push("cross_package".to_string());
    }
    if simple_cmds_only {
        tags.push("simple_cmds_only".to_string());
    }
    if is_trivial_framework {
        tags.push("trivial_framework".to_string());
    }

    PtbClassification {
        digest: tx.digest.clone(),
        checkpoint: tx.checkpoint,
        tags,
        is_framework_only,
        is_trivial_framework,
        non_system_packages: non_system_packages.into_iter().collect(),
        system_packages: system_packages.into_iter().collect(),
        has_publish,
        has_upgrade,
        has_shared_inputs,
        has_receiving_inputs,
        command_kinds: command_kinds.into_iter().collect(),
    }
}

fn normalize_package(pkg: &str) -> String {
    let trimmed = pkg.trim();
    if trimmed.starts_with("0x") {
        trimmed.to_lowercase()
    } else {
        format!("0x{}", trimmed.to_lowercase())
    }
}

fn is_system_package(pkg: &str) -> bool {
    matches!(pkg, "0x1" | "0x2" | "0x3")
}
