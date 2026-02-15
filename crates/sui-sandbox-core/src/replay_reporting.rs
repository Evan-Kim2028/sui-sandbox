use std::collections::BTreeSet;

use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use sui_sandbox_types::{PtbCommand, TransactionInput};
use sui_state_fetcher::ReplayState;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReplayDiagnostics {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_input_objects: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_packages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<String>,
}

impl ReplayDiagnostics {
    pub fn is_empty(&self) -> bool {
        self.missing_input_objects.is_empty()
            && self.missing_packages.is_empty()
            && self.suggestions.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ReplayDiagnosticsOptions<'a> {
    pub allow_fallback: bool,
    pub missing_input_message: &'a str,
    pub missing_package_message: &'a str,
    pub fallback_message: &'a str,
}

impl<'a> Default for ReplayDiagnosticsOptions<'a> {
    fn default() -> Self {
        Self {
            allow_fallback: true,
            missing_input_message:
                "Missing input objects detected; provide full object state or a stronger historical source.",
            missing_package_message:
                "Missing package bytecode detected; prepare/fetch package context with deps.",
            fallback_message:
                "Fallback is disabled; enable fallback to allow secondary data sources.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayClassification {
    pub failed: bool,
    pub category: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_input_objects: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_packages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command_description: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_replay_analysis_summary(
    replay_state: &ReplayState,
    source: &str,
    allow_fallback: bool,
    auto_system_objects: bool,
    dynamic_field_prefetch: bool,
    prefetch_depth: usize,
    prefetch_limit: usize,
    verbose: bool,
) -> serde_json::Value {
    let modules_total = replay_state
        .packages
        .values()
        .map(|pkg| pkg.modules.len())
        .sum::<usize>();
    let package_ids = replay_state
        .packages
        .keys()
        .map(|id| id.to_hex_literal())
        .collect::<Vec<_>>();
    let object_ids = replay_state
        .objects
        .keys()
        .map(|id| id.to_hex_literal())
        .collect::<Vec<_>>();

    let command_summaries = replay_state
        .transaction
        .commands
        .iter()
        .map(|cmd| match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => serde_json::json!({
                "kind": "MoveCall",
                "target": format!("{}::{}::{}", package, module, function),
                "type_args": type_arguments.len(),
                "args": arguments.len(),
            }),
            PtbCommand::SplitCoins { amounts, .. } => serde_json::json!({
                "kind": "SplitCoins",
                "args": 1 + amounts.len(),
            }),
            PtbCommand::MergeCoins { sources, .. } => serde_json::json!({
                "kind": "MergeCoins",
                "args": 1 + sources.len(),
            }),
            PtbCommand::TransferObjects { objects, .. } => serde_json::json!({
                "kind": "TransferObjects",
                "args": 1 + objects.len(),
            }),
            PtbCommand::MakeMoveVec { elements, .. } => serde_json::json!({
                "kind": "MakeMoveVec",
                "args": elements.len(),
            }),
            PtbCommand::Publish { dependencies, .. } => serde_json::json!({
                "kind": "Publish",
                "args": dependencies.len(),
            }),
            PtbCommand::Upgrade { package, .. } => serde_json::json!({
                "kind": "Upgrade",
                "target": package,
            }),
        })
        .collect::<Vec<_>>();

    let mut pure = 0usize;
    let mut owned = 0usize;
    let mut shared_mutable = 0usize;
    let mut shared_immutable = 0usize;
    let mut immutable = 0usize;
    let mut receiving = 0usize;
    for input in &replay_state.transaction.inputs {
        match input {
            TransactionInput::Pure { .. } => pure += 1,
            TransactionInput::Object { .. } => owned += 1,
            TransactionInput::SharedObject { mutable, .. } => {
                if *mutable {
                    shared_mutable += 1;
                } else {
                    shared_immutable += 1;
                }
            }
            TransactionInput::ImmutableObject { .. } => immutable += 1,
            TransactionInput::Receiving { .. } => receiving += 1,
        }
    }

    let mut result = serde_json::json!({
        "digest": replay_state.transaction.digest.0,
        "sender": replay_state.transaction.sender.to_hex_literal(),
        "commands": replay_state.transaction.commands.len(),
        "inputs": replay_state.transaction.inputs.len(),
        "objects": replay_state.objects.len(),
        "packages": replay_state.packages.len(),
        "modules": modules_total,
        "input_summary": {
            "total": replay_state.transaction.inputs.len(),
            "pure": pure,
            "owned": owned,
            "shared_mutable": shared_mutable,
            "shared_immutable": shared_immutable,
            "immutable": immutable,
            "receiving": receiving,
        },
        "command_summaries": command_summaries,
        "hydration": {
            "source": source,
            "allow_fallback": allow_fallback,
            "auto_system_objects": auto_system_objects,
            "dynamic_field_prefetch": dynamic_field_prefetch,
            "prefetch_depth": prefetch_depth,
            "prefetch_limit": prefetch_limit,
        },
        "epoch": replay_state.epoch,
        "protocol_version": replay_state.protocol_version,
    });
    if let Some(cp) = replay_state.checkpoint {
        result["checkpoint"] = serde_json::json!(cp);
    }
    if let Some(rgp) = replay_state.reference_gas_price {
        result["reference_gas_price"] = serde_json::json!(rgp);
    }
    if verbose {
        result["package_ids"] = serde_json::json!(package_ids);
        result["object_ids"] = serde_json::json!(object_ids);
    }
    result
}

pub fn missing_input_objects_from_state(replay_state: &ReplayState) -> Vec<String> {
    let mut missing_inputs = Vec::new();
    for input in &replay_state.transaction.inputs {
        let object_id = match input {
            TransactionInput::Object { object_id, .. } => Some(object_id),
            TransactionInput::SharedObject { object_id, .. } => Some(object_id),
            TransactionInput::ImmutableObject { object_id, .. } => Some(object_id),
            TransactionInput::Receiving { object_id, .. } => Some(object_id),
            TransactionInput::Pure { .. } => None,
        };
        if let Some(object_id) = object_id {
            if let Ok(address) = AccountAddress::from_hex_literal(object_id) {
                if !replay_state.objects.contains_key(&address) {
                    missing_inputs.push(address.to_hex_literal());
                }
            } else {
                missing_inputs.push(object_id.clone());
            }
        }
    }
    missing_inputs
}

pub fn collect_required_packages(replay_state: &ReplayState) -> BTreeSet<AccountAddress> {
    let mut required_packages: BTreeSet<AccountAddress> = BTreeSet::new();
    for cmd in &replay_state.transaction.commands {
        match cmd {
            PtbCommand::MoveCall {
                package,
                type_arguments,
                ..
            } => {
                if let Ok(address) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(address);
                }
                for ty in type_arguments {
                    for pkg in crate::utilities::extract_package_ids_from_type(ty) {
                        if let Ok(address) = AccountAddress::from_hex_literal(&pkg) {
                            required_packages.insert(address);
                        }
                    }
                }
            }
            PtbCommand::Upgrade { package, .. } => {
                if let Ok(address) = AccountAddress::from_hex_literal(package) {
                    required_packages.insert(address);
                }
            }
            PtbCommand::Publish { dependencies, .. } => {
                for dep in dependencies {
                    if let Ok(address) = AccountAddress::from_hex_literal(dep) {
                        required_packages.insert(address);
                    }
                }
            }
            _ => {}
        }
    }
    required_packages
}

pub fn collect_missing_packages<F>(replay_state: &ReplayState, mut has_package: F) -> Vec<String>
where
    F: FnMut(&AccountAddress) -> bool,
{
    collect_required_packages(replay_state)
        .into_iter()
        .filter(|address| !has_package(address))
        .map(|address| address.to_hex_literal())
        .collect::<Vec<_>>()
}

pub fn build_replay_diagnostics<F>(
    replay_state: &ReplayState,
    missing_input_objects: Vec<String>,
    has_package: F,
    options: ReplayDiagnosticsOptions<'_>,
) -> Option<ReplayDiagnostics>
where
    F: FnMut(&AccountAddress) -> bool,
{
    let missing_packages = collect_missing_packages(replay_state, has_package);
    let mut suggestions = Vec::new();
    if !missing_input_objects.is_empty() {
        suggestions.push(options.missing_input_message.to_string());
    }
    if !missing_packages.is_empty() {
        suggestions.push(options.missing_package_message.to_string());
    }
    if !options.allow_fallback {
        suggestions.push(options.fallback_message.to_string());
    }
    let diagnostics = ReplayDiagnostics {
        missing_input_objects,
        missing_packages,
        suggestions,
    };
    if diagnostics.is_empty() {
        None
    } else {
        Some(diagnostics)
    }
}

fn parse_json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub fn replay_has_comparison_mismatch(result: &serde_json::Value) -> bool {
    let Some(comparison) = result.get("comparison") else {
        return false;
    };
    let read = |key: &str| comparison.get(key).and_then(serde_json::Value::as_bool);
    matches!(read("status_match"), Some(false))
        || matches!(read("created_match"), Some(false))
        || matches!(read("mutated_match"), Some(false))
        || matches!(read("deleted_match"), Some(false))
}

pub fn classify_replay_output(result: &serde_json::Value) -> ReplayClassification {
    let local_success = result
        .get("local_success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let diagnostics = result.get("diagnostics");
    let missing_input_objects =
        parse_json_string_list(diagnostics.and_then(|d| d.get("missing_input_objects")));
    let missing_packages =
        parse_json_string_list(diagnostics.and_then(|d| d.get("missing_packages")));
    let mut suggestions = parse_json_string_list(diagnostics.and_then(|d| d.get("suggestions")));

    let local_error = result
        .get("local_error")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            result
                .get("effects")
                .and_then(|v| v.get("error"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        });

    let mut failed_command_index = result
        .get("effects")
        .and_then(|v| v.get("failed_command_index"))
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as usize);
    let mut failed_command_description = result
        .get("effects")
        .and_then(|v| v.get("failed_command_description"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);

    if local_success {
        if replay_has_comparison_mismatch(result) {
            if suggestions.is_empty() {
                suggestions.push("Local replay succeeded but on-chain comparison mismatched; inspect `comparison.notes` and hydrate exact historical versions.".to_string());
            }
            return ReplayClassification {
                failed: false,
                category: "comparison_mismatch".to_string(),
                retryable: false,
                local_error,
                missing_input_objects,
                missing_packages,
                suggestions,
                failed_command_index,
                failed_command_description,
            };
        }
        return ReplayClassification {
            failed: false,
            category: "success".to_string(),
            retryable: false,
            local_error,
            missing_input_objects,
            missing_packages,
            suggestions,
            failed_command_index,
            failed_command_description,
        };
    }

    let local_error_lower = local_error
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let category = if !missing_input_objects.is_empty() {
        "missing_input_objects"
    } else if !missing_packages.is_empty() {
        "missing_packages"
    } else if local_error_lower.contains("archive")
        || local_error_lower.contains("historical")
        || local_error_lower.contains("not found")
    {
        "archive_data_gap"
    } else if local_error_lower.contains("api key")
        || local_error_lower.contains("unauthorized")
        || local_error_lower.contains("forbidden")
    {
        "auth_or_endpoint"
    } else if local_error_lower.contains("outofgas")
        || (local_error_lower.contains("gas") && local_error_lower.contains("budget"))
    {
        "gas_failure"
    } else if local_error_lower.contains("abort") {
        "move_abort"
    } else if local_error_lower.contains("type")
        || local_error_lower.contains("argument")
        || local_error_lower.contains("deserialize")
    {
        "input_shape_error"
    } else {
        "execution_error"
    };

    if suggestions.is_empty() {
        let hint = match category {
            "missing_input_objects" => {
                "Replay is missing input objects; retry with stronger hydration, `state_file`, or `synthesize_missing=True`."
            }
            "missing_packages" => {
                "Replay is missing package bytecode; prepare context with deps and retry via `context_path`."
            }
            "archive_data_gap" => {
                "Archive endpoint likely missed historical state; switch endpoint or checkpoint source and retry."
            }
            "auth_or_endpoint" => {
                "Endpoint/auth issue detected; verify endpoint and API key environment variables."
            }
            "gas_failure" => "Execution appears gas-related; verify transaction inputs and gas assumptions.",
            "move_abort" => "Move abort detected; inspect abort code and function-level preconditions.",
            "input_shape_error" => {
                "Input/type mismatch detected; verify object versions, type args, and command argument shapes."
            }
            _ => "Replay failed; inspect `local_error`, `effects`, and `diagnostics` for details.",
        };
        suggestions.push(hint.to_string());
    }

    let retryable = matches!(
        category,
        "missing_input_objects" | "missing_packages" | "archive_data_gap" | "auth_or_endpoint"
    );

    if failed_command_index.is_none() {
        failed_command_index = None;
    }
    if failed_command_description.is_none() {
        failed_command_description = None;
    }

    ReplayClassification {
        failed: true,
        category: category.to_string(),
        retryable,
        local_error,
        missing_input_objects,
        missing_packages,
        suggestions,
        failed_command_index,
        failed_command_description,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_success_output() {
        let result = serde_json::json!({
            "local_success": true
        });
        let classified = classify_replay_output(&result);
        assert!(!classified.failed);
        assert_eq!(classified.category, "success");
    }

    #[test]
    fn classify_missing_inputs_retryable() {
        let result = serde_json::json!({
            "local_success": false,
            "local_error": "missing input object",
            "diagnostics": {
                "missing_input_objects": ["0x1"]
            }
        });
        let classified = classify_replay_output(&result);
        assert!(classified.failed);
        assert_eq!(classified.category, "missing_input_objects");
        assert!(classified.retryable);
    }

    #[test]
    fn classify_comparison_mismatch_without_failure() {
        let result = serde_json::json!({
            "local_success": true,
            "comparison": {
                "status_match": false,
                "created_match": true,
                "mutated_match": true,
                "deleted_match": true
            }
        });
        let classified = classify_replay_output(&result);
        assert!(!classified.failed);
        assert_eq!(classified.category, "comparison_mismatch");
        assert!(!classified.retryable);
    }

    #[test]
    fn classify_archive_data_gap_retryable() {
        let result = serde_json::json!({
            "local_success": false,
            "local_error": "historical object not found in archive"
        });
        let classified = classify_replay_output(&result);
        assert!(classified.failed);
        assert_eq!(classified.category, "archive_data_gap");
        assert!(classified.retryable);
    }

    #[test]
    fn classify_auth_or_endpoint_retryable() {
        let result = serde_json::json!({
            "local_success": false,
            "local_error": "unauthorized: api key missing"
        });
        let classified = classify_replay_output(&result);
        assert!(classified.failed);
        assert_eq!(classified.category, "auth_or_endpoint");
        assert!(classified.retryable);
    }

    #[test]
    fn classify_extracts_failed_command_metadata() {
        let result = serde_json::json!({
            "local_success": false,
            "local_error": "MoveAbort",
            "effects": {
                "failed_command_index": 3,
                "failed_command_description": "MoveCall 3"
            }
        });
        let classified = classify_replay_output(&result);
        assert_eq!(classified.failed_command_index, Some(3));
        assert_eq!(
            classified.failed_command_description.as_deref(),
            Some("MoveCall 3")
        );
    }
}
