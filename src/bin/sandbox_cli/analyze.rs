//! Analyze command - package and replay introspection

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::replay::ReplayHydrationArgs;
use super::SandboxState;

mod mm2_common;
mod objects_classifier;
mod objects_cmd;
mod objects_profile;
mod package_cmd;
mod replay_cmd;

#[derive(Parser, Debug)]
#[command(
    after_help = "Examples:\n  sui-sandbox analyze package --package-id 0x2 --list-modules --mm2\n  sui-sandbox analyze package --bytecode-dir ./path/to/pkg --mm2\n  sui-sandbox analyze replay <DIGEST> --source hybrid --allow-fallback true\n  sui-sandbox analyze objects --corpus-dir ./sui-packages/packages/mainnet_most_used --profile hybrid"
)]
pub struct AnalyzeCmd {
    #[command(subcommand)]
    command: AnalyzeCommand,
}

#[derive(Subcommand, Debug)]
enum AnalyzeCommand {
    /// Analyze a package by id or local bytecode directory
    #[command(alias = "pkg")]
    Package(AnalyzePackageCmd),
    /// Analyze replay state hydration for a transaction digest
    #[command(alias = "tx")]
    Replay(AnalyzeReplayCmd),
    /// Analyze object type usage across a local package corpus
    #[command(alias = "corpus", alias = "objs")]
    Objects(AnalyzeObjectsCmd),
}

#[derive(Parser, Debug)]
#[command(group(
    clap::ArgGroup::new("source")
        .required(true)
        .args(["package_id", "bytecode_dir"])
))]
pub struct AnalyzePackageCmd {
    /// Package id (0x...)
    #[arg(
        long,
        value_name = "ID",
        conflicts_with = "bytecode_dir",
        help_heading = "Source"
    )]
    pub package_id: Option<String>,

    /// Local package directory containing bytecode_modules/*.mv
    #[arg(
        long,
        value_name = "DIR",
        conflicts_with = "package_id",
        help_heading = "Source"
    )]
    pub bytecode_dir: Option<PathBuf>,

    /// Include module names in output
    #[arg(long, default_value_t = false, help_heading = "Analysis")]
    pub list_modules: bool,

    /// Include full package interface in JSON output
    #[arg(long, default_value_t = false, help_heading = "Analysis")]
    pub include_interface: bool,

    /// Attempt MM2 model build for the package
    #[arg(long, default_value_t = false, help_heading = "Analysis")]
    pub mm2: bool,
}

#[derive(Parser, Debug)]
pub struct AnalyzeReplayCmd {
    /// Transaction digest
    pub digest: String,

    #[command(flatten)]
    pub hydration: ReplayHydrationArgs,

    /// Attempt MM2 model build across replay packages
    #[arg(long, default_value_t = false, help_heading = "Analysis")]
    pub mm2: bool,

    /// Checkpoint number for Walrus-first analysis (no gRPC/API key needed)
    #[arg(long, help_heading = "Hydration")]
    pub checkpoint: Option<u64>,
}

#[derive(Parser, Debug)]
pub struct AnalyzeObjectsCmd {
    /// Root corpus directory (e.g. .../sui-packages/packages/mainnet_most_used)
    #[arg(
        long,
        visible_alias = "corpus",
        value_name = "DIR",
        help_heading = "Source"
    )]
    pub corpus_dir: PathBuf,

    /// Analysis profile name: built-in (broad|strict|hybrid) or custom from profile dirs
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with = "profile_file",
        help_heading = "Profile"
    )]
    pub profile: Option<String>,

    /// Explicit analysis profile YAML file
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with = "profile",
        help_heading = "Profile"
    )]
    pub profile_file: Option<PathBuf>,

    /// Override semantic mode regardless of selected profile
    #[arg(long, value_enum, help_heading = "Profile")]
    pub semantic_mode: Option<AnalyzeSemanticMode>,

    /// Override dynamic-field UID lookback window regardless of selected profile
    #[arg(long, value_name = "N", help_heading = "Profile")]
    pub dynamic_lookback: Option<usize>,

    /// Include per-type records in output
    #[arg(long, default_value_t = false, help_heading = "Output")]
    pub list_types: bool,

    /// Max records per category/example list
    #[arg(long, default_value_t = 20, help_heading = "Output")]
    pub top: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzeSemanticMode {
    Broad,
    Strict,
    Hybrid,
}

impl AnalyzeSemanticMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Broad => "broad",
            Self::Strict => "strict",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AnalyzeDynamicMode {
    Broad,
    Strict,
    Hybrid,
}

impl AnalyzeDynamicMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Broad => "broad",
            Self::Strict => "strict",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct AnalyzeObjectsProfileInfo {
    pub name: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub semantic_mode: AnalyzeSemanticMode,
    pub dynamic: AnalyzeObjectsDynamicSettings,
}

#[derive(Debug, Clone, Serialize)]
struct AnalyzeObjectsDynamicSettings {
    pub mode: AnalyzeDynamicMode,
    pub lookback: usize,
    pub include_wrapper_apis: bool,
    pub field_container_heuristic: bool,
    pub use_uid_owner_flow: bool,
    pub use_ref_param_owner_fallback: bool,
}

#[derive(Debug, Deserialize, Default)]
struct AnalyzeObjectsProfileFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    extends: Option<String>,
    #[serde(default)]
    semantic_mode: Option<AnalyzeSemanticMode>,
    #[serde(default)]
    dynamic: AnalyzeObjectsDynamicSettingsOverride,
}

#[derive(Debug, Deserialize, Default)]
struct AnalyzeObjectsDynamicSettingsOverride {
    #[serde(default)]
    mode: Option<AnalyzeDynamicMode>,
    #[serde(default)]
    lookback: Option<usize>,
    #[serde(default)]
    include_wrapper_apis: Option<bool>,
    #[serde(default)]
    field_container_heuristic: Option<bool>,
    #[serde(default)]
    use_uid_owner_flow: Option<bool>,
    #[serde(default)]
    use_ref_param_owner_fallback: Option<bool>,
}

#[derive(Debug, Serialize)]
struct AnalyzePackageOutput {
    pub source: String,
    pub package_id: String,
    pub modules: usize,
    pub structs: usize,
    pub functions: usize,
    pub key_structs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_model_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AnalyzeObjectsOutput {
    pub corpus_dir: String,
    pub profile: AnalyzeObjectsProfileInfo,
    pub packages_scanned: usize,
    pub packages_failed: usize,
    pub modules_scanned: usize,
    pub object_types_discovered: usize,
    pub object_types_unique: usize,
    pub ownership: ObjectOwnershipCounts,
    pub ownership_unique: ObjectOwnershipCounts,
    pub party_transfer_eligible: ObjectCountSummary,
    pub party_transfer_observed_in_bytecode: ObjectCountSummary,
    pub singleton_types: usize,
    pub singleton_occurrences: usize,
    pub dynamic_field_types: usize,
    pub dynamic_field_occurrences: usize,
    pub unclassified_types: usize,
    pub multi_mode_types: usize,
    pub confidence: AnalyzeObjectsConfidence,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub immutable_examples: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub party_transfer_eligible_examples: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub party_transfer_eligible_not_observed_examples: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub party_examples: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub receive_examples: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<ObjectTypeRow>>,
}

#[derive(Debug, Serialize, Default)]
struct ObjectOwnershipCounts {
    pub owned: usize,
    pub shared: usize,
    pub immutable: usize,
    pub party: usize,
    pub receive: usize,
}

#[derive(Debug, Serialize, Default)]
struct ObjectCountSummary {
    pub types: usize,
    pub occurrences: usize,
}

#[derive(Debug, Serialize)]
struct AnalyzeObjectsConfidence {
    pub ownership: String,
    pub party_metrics: String,
    pub singleton: String,
    pub dynamic_fields: String,
}

#[derive(Debug, Serialize)]
struct ObjectTypeRow {
    pub type_tag: String,
    pub party_transfer_eligible: bool,
    pub owned: bool,
    pub shared: bool,
    pub immutable: bool,
    pub party: bool,
    pub receive: bool,
    pub singleton: bool,
    pub dynamic_fields: bool,
}

#[derive(Debug, Default, Clone)]
struct ObjectTypeStats {
    has_store: bool,
    owned: bool,
    shared: bool,
    immutable: bool,
    party: bool,
    receive: bool,
    dynamic_fields: bool,
    packed_in_init: bool,
    packed_outside_init: bool,
    pack_count: usize,
    key_struct: bool,
    occurrences: usize,
}

#[derive(Debug, Serialize)]
struct AnalyzeReplayOutput {
    pub digest: String,
    pub sender: String,
    pub commands: usize,
    pub inputs: usize,
    pub objects: usize,
    pub packages: usize,
    pub modules: usize,
    pub input_summary: ReplayInputSummary,
    pub command_summaries: Vec<ReplayCommandSummary>,
    pub hydration: AnalyzeReplayHydrationSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_objects: Option<Vec<ReplayInputObject>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_types: Option<Vec<ReplayObjectType>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_inputs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub missing_packages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<String>,
    pub epoch: u64,
    pub protocol_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_gas_price: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_model_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mm2_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnalyzeReplayHydrationSummary {
    pub source: String,
    pub allow_fallback: bool,
    pub auto_system_objects: bool,
    pub dynamic_field_prefetch: bool,
    pub prefetch_depth: usize,
    pub prefetch_limit: usize,
}

#[derive(Debug, Serialize, Default)]
struct ReplayInputSummary {
    pub total: usize,
    pub pure: usize,
    pub owned: usize,
    pub shared_mutable: usize,
    pub shared_immutable: usize,
    pub immutable: usize,
    pub receiving: usize,
}

#[derive(Debug, Serialize)]
struct ReplayCommandSummary {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub type_args: usize,
    pub args: usize,
}

#[derive(Debug, Serialize)]
struct ReplayInputObject {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutable: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ReplayObjectType {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_tag: Option<String>,
    pub version: u64,
    pub shared: bool,
    pub immutable: bool,
}

impl AnalyzeCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        match &self.command {
            AnalyzeCommand::Package(cmd) => {
                let output = cmd.execute(state, verbose).await?;
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    package_cmd::print_package_output(&output);
                }
                Ok(())
            }
            AnalyzeCommand::Replay(cmd) => {
                let output = cmd.execute(state, verbose).await?;
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    replay_cmd::print_replay_output(&output);
                }
                Ok(())
            }
            AnalyzeCommand::Objects(cmd) => {
                let output = cmd.execute(state, verbose).await?;
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    objects_cmd::print_objects_output(&output);
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_normalize_package_id() {
        let normalized = mm2_common::normalize_package_id("0x2").unwrap();
        assert_eq!(
            normalized,
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        let normalized_upper = mm2_common::normalize_package_id("0X2").unwrap();
        assert_eq!(
            normalized_upper,
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert!(mm2_common::normalize_package_id("xyz").is_none());
    }

    #[test]
    fn test_mode_from_transfer_function() {
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x2", "transfer", "public_transfer"),
            Some("owned")
        );
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x2", "transfer", "share_object"),
            Some("shared")
        );
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x2", "transfer", "freeze_object"),
            Some("immutable")
        );
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x2", "transfer", "party_transfer"),
            Some("party")
        );
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x2", "transfer", "receive"),
            Some("receive")
        );
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x2", "coin", "transfer"),
            None
        );
        assert_eq!(
            objects_classifier::mode_from_transfer_function("0x3", "transfer", "party_transfer"),
            None
        );
    }

    #[test]
    fn test_is_dynamic_field_api_function() {
        assert!(objects_classifier::is_dynamic_field_api_function(
            "0x2",
            "dynamic_field",
            "add",
            false
        ));
        assert!(objects_classifier::is_dynamic_field_api_function(
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            "dynamic_object_field",
            "borrow",
            false
        ));
        assert!(!objects_classifier::is_dynamic_field_api_function(
            "0x2", "table", "add", false
        ));
        assert!(objects_classifier::is_dynamic_field_api_function(
            "0x2", "table", "add", true
        ));
        assert!(!objects_classifier::is_dynamic_field_api_function(
            "0x2",
            "object_bag",
            "borrow",
            false
        ));
        assert!(objects_classifier::is_dynamic_field_api_function(
            "0x2",
            "object_bag",
            "borrow",
            true
        ));
        assert!(!objects_classifier::is_dynamic_field_api_function(
            "0x3",
            "dynamic_field",
            "add",
            true
        ));
        assert!(!objects_classifier::is_dynamic_field_api_function(
            "0x2", "coin", "mint", true
        ));
    }

    #[test]
    fn test_resolve_objects_profile_builtin() {
        let cmd = AnalyzeObjectsCmd {
            corpus_dir: PathBuf::from("/tmp/corpus"),
            profile: Some("strict".to_string()),
            profile_file: None,
            semantic_mode: None,
            dynamic_lookback: None,
            list_types: false,
            top: 20,
        };
        let profile = objects_profile::resolve_objects_profile(&cmd).unwrap();
        assert_eq!(profile.name, "strict");
        assert_eq!(profile.source, "builtin");
        assert_eq!(profile.semantic_mode, AnalyzeSemanticMode::Strict);
        assert_eq!(profile.dynamic.mode, AnalyzeDynamicMode::Strict);
        assert_eq!(profile.dynamic.lookback, 12);
    }

    #[test]
    fn test_resolve_objects_profile_file_extends_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let profile_path = dir.path().join("team.yaml");
        std::fs::write(
            &profile_path,
            r#"
name: team
extends: strict
dynamic:
  lookback: 33
  include_wrapper_apis: true
"#,
        )
        .unwrap();

        let cmd = AnalyzeObjectsCmd {
            corpus_dir: PathBuf::from("/tmp/corpus"),
            profile: None,
            profile_file: Some(profile_path.clone()),
            semantic_mode: None,
            dynamic_lookback: None,
            list_types: false,
            top: 20,
        };
        let profile = objects_profile::resolve_objects_profile(&cmd).unwrap();
        assert_eq!(profile.name, "team");
        assert_eq!(profile.source, "file");
        assert_eq!(profile.path, Some(profile_path.display().to_string()));
        assert_eq!(profile.semantic_mode, AnalyzeSemanticMode::Strict);
        assert_eq!(profile.dynamic.mode, AnalyzeDynamicMode::Strict);
        assert_eq!(profile.dynamic.lookback, 33);
        assert!(profile.dynamic.include_wrapper_apis);
    }

    #[test]
    fn test_analyze_package_cmd_requires_source() {
        let err = AnalyzePackageCmd::try_parse_from(["analyze-package"]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--package-id") || msg.contains("--bytecode-dir"),
            "unexpected clap error: {msg}"
        );
    }

    #[test]
    fn test_analyze_replay_cmd_bool_defaults_and_overrides() {
        let defaults = AnalyzeReplayCmd::try_parse_from(["analyze-replay", "dummy-digest"])
            .expect("parse replay defaults");
        assert!(defaults.hydration.allow_fallback);
        assert!(defaults.hydration.auto_system_objects);

        let disabled = AnalyzeReplayCmd::try_parse_from([
            "analyze-replay",
            "dummy-digest",
            "--allow-fallback",
            "false",
            "--auto-system-objects",
            "false",
        ])
        .expect("parse replay overrides");
        assert!(!disabled.hydration.allow_fallback);
        assert!(!disabled.hydration.auto_system_objects);

        let enabled = AnalyzeReplayCmd::try_parse_from([
            "analyze-replay",
            "dummy-digest",
            "--allow-fallback",
            "true",
            "--auto-system-objects",
            "true",
        ])
        .expect("parse explicit replay enables");
        assert!(enabled.hydration.allow_fallback);
        assert!(enabled.hydration.auto_system_objects);
    }

    #[test]
    fn test_analyze_replay_output_serialization_hydration_contract() {
        let output = AnalyzeReplayOutput {
            digest: "dummy".to_string(),
            sender: "0x1".to_string(),
            commands: 0,
            inputs: 0,
            objects: 0,
            packages: 0,
            modules: 0,
            input_summary: ReplayInputSummary::default(),
            command_summaries: Vec::new(),
            hydration: AnalyzeReplayHydrationSummary {
                source: "hybrid".to_string(),
                allow_fallback: true,
                auto_system_objects: false,
                dynamic_field_prefetch: true,
                prefetch_depth: 3,
                prefetch_limit: 200,
            },
            input_objects: None,
            object_types: None,
            missing_inputs: Vec::new(),
            missing_packages: Vec::new(),
            suggestions: Vec::new(),
            epoch: 0,
            protocol_version: 64,
            checkpoint: None,
            reference_gas_price: None,
            package_ids: None,
            object_ids: None,
            mm2_model_ok: None,
            mm2_error: None,
        };

        let value = serde_json::to_value(&output).expect("serialize replay output");
        let hydration = value
            .get("hydration")
            .and_then(serde_json::Value::as_object)
            .expect("hydration object");

        assert!(hydration
            .get("source")
            .and_then(serde_json::Value::as_str)
            .is_some());
        assert!(hydration
            .get("allow_fallback")
            .and_then(serde_json::Value::as_bool)
            .is_some());
        assert!(hydration
            .get("auto_system_objects")
            .and_then(serde_json::Value::as_bool)
            .is_some());
        assert!(hydration
            .get("dynamic_field_prefetch")
            .and_then(serde_json::Value::as_bool)
            .is_some());
        assert!(hydration
            .get("prefetch_depth")
            .and_then(serde_json::Value::as_u64)
            .is_some());
        assert!(hydration
            .get("prefetch_limit")
            .and_then(serde_json::Value::as_u64)
            .is_some());
    }

    #[test]
    fn test_parse_bcs_linkage_upgraded_ids() {
        let dir = tempfile::tempdir().unwrap();
        let bcs_path = dir.path().join("bcs.json");
        std::fs::write(
            &bcs_path,
            r#"{
                "linkageTable": {
                    "0x2": {"upgraded_id": "0x2", "upgraded_version": 1},
                    "0xabc": {"upgraded_id": "0x0000000000000000000000000000000000000000000000000000000000000abc", "upgraded_version": 7}
                }
            }"#,
        )
        .unwrap();

        let ids = mm2_common::parse_bcs_linkage_upgraded_ids(dir.path()).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            ids[0],
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert_eq!(
            ids[1],
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
    }
}
