//! Input structs for MCP tool handlers.
//!
//! This module contains all the deserializable input types that tool handlers accept.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Options for cache behavior during fetching.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    Default,
    Bypass,
}

impl CachePolicy {
    pub fn is_bypass(self) -> bool {
        matches!(self, CachePolicy::Bypass)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CachePolicy::Default => "default",
            CachePolicy::Bypass => "bypass",
        }
    }
}

/// Strategy for fetching transaction inputs during replay.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FetchStrategy {
    Eager,
    #[default]
    Full,
}

/// Options for PTB execution.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct PtbOptions {
    #[serde(default)]
    pub resolution_mode: Option<String>,
    #[serde(default)]
    pub enable_on_demand_fetch: Option<bool>,
    #[serde(default)]
    pub fetch_missing_packages: Option<bool>,
    #[serde(default)]
    pub fetch_missing_objects: Option<bool>,
    #[serde(default)]
    pub gas_budget: Option<u64>,
    #[serde(default)]
    pub gas_price: Option<u64>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub cache_policy: Option<CachePolicy>,
}

/// Options for transaction replay.
#[derive(Debug, Deserialize, Default)]
pub struct ReplayOptions {
    #[serde(default)]
    pub compare_effects: Option<bool>,
    #[serde(default)]
    pub fetch_strategy: Option<FetchStrategy>,
    #[serde(default)]
    pub auto_system_objects: Option<bool>,
    #[serde(default)]
    pub sync_env: Option<bool>,
    #[serde(default)]
    pub reconcile_dynamic_fields: Option<bool>,
    #[serde(default)]
    pub prefetch_depth: Option<usize>,
    #[serde(default)]
    pub prefetch_limit: Option<usize>,
    #[serde(default)]
    pub synthesize_missing: Option<bool>,
    #[serde(default)]
    pub self_heal_dynamic_fields: Option<bool>,
}

// Tool-specific input types

#[derive(Debug, Deserialize)]
pub struct CallFunctionInput {
    pub package: String,
    pub module: String,
    pub function: String,
    #[serde(default)]
    pub type_args: Vec<String>,
    #[serde(default)]
    pub args: Vec<Value>,
    #[serde(default)]
    pub options: Option<PtbOptions>,
}

#[derive(Debug, Deserialize)]
pub struct ExecutePtbInput {
    #[serde(default)]
    pub inputs: Vec<Value>,
    #[serde(default)]
    pub commands: Vec<Value>,
    #[serde(default)]
    pub options: Option<PtbOptions>,
}

#[derive(Debug, Deserialize)]
pub struct ReplayInput {
    pub digest: String,
    #[serde(default)]
    pub options: Option<ReplayOptions>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectInput {
    pub name: String,
    #[serde(default)]
    pub initial_module: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub persist: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ReadFileInput {
    pub project_id: String,
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct EditFileInput {
    pub project_id: String,
    pub file: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub edits: Option<Vec<FileEdit>>,
}

#[derive(Debug, Deserialize)]
pub struct FileEdit {
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Deserialize)]
pub struct ProjectIdInput {
    pub project_id: String,
}

#[derive(Debug, Deserialize)]
pub struct TestProjectInput {
    pub project_id: String,
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListProjectsInput {
    #[serde(default)]
    pub include_paths: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ListPackagesInput {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SetActivePackageInput {
    pub project_id: String,
    pub package_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpgradeProjectInput {
    pub project_id: String,
    #[serde(default)]
    pub upgrade_cap: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReadObjectInput {
    pub object_id: String,
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub fetch: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAssetInput {
    #[serde(rename = "type")]
    pub asset_type: String,
    #[serde(default)]
    pub amount: Option<u64>,
    #[serde(default)]
    pub type_tag: Option<String>,
    #[serde(default)]
    pub fields: Option<HashMap<String, Value>>,
    #[serde(default)]
    pub bcs_bytes_b64: Option<String>,
    #[serde(default)]
    pub object_id: Option<String>,
    #[serde(default)]
    pub shared: Option<bool>,
    #[serde(default)]
    pub immutable: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LoadFromMainnetInput {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub cache_policy: Option<CachePolicy>,
}

#[derive(Debug, Deserialize)]
pub struct LoadPackageBytesInput {
    pub package_id: String,
    pub modules: Vec<ModuleBytesInput>,
    #[serde(default)]
    pub version: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ModuleBytesInput {
    pub name: String,
    pub bytes_b64: String,
}

#[derive(Debug, Deserialize)]
pub struct GetInterfaceInput {
    pub package: String,
    #[serde(default)]
    pub module: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchInput {
    pub pattern: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub entry_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct GetStateInput {
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigureInput {
    pub action: String,
    #[serde(default)]
    pub params: Option<Value>,
}
