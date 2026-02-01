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
    /// Human-readable description for transaction history
    #[serde(default)]
    pub description: Option<String>,
    /// Tags to add to the transaction in history
    #[serde(default)]
    pub tags: Option<Vec<String>>,
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

#[derive(Debug, Deserialize)]
pub struct WalrusFetchInput {
    /// Network (mainnet or testnet)
    #[serde(default)]
    pub network: Option<String>,
    /// Explicit checkpoint list to fetch
    #[serde(default)]
    pub checkpoints: Option<Vec<u64>>,
    /// Starting checkpoint (defaults to latest - count + 1)
    #[serde(default)]
    pub start_checkpoint: Option<u64>,
    /// Number of checkpoints to fetch (default 1)
    #[serde(default)]
    pub count: Option<u64>,
    /// Max bytes per aggregated blob fetch
    #[serde(default)]
    pub max_chunk_bytes: Option<u64>,
    /// Max checkpoints per batch
    #[serde(default)]
    pub batch_size: Option<usize>,
    /// Dump checkpoint JSON files into this directory
    #[serde(default)]
    pub dump_dir: Option<String>,
    /// Include per-checkpoint summary in response
    #[serde(default)]
    pub summary: Option<bool>,
}

// ============================================================================
// World Management Inputs
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct WorldCreateInput {
    /// World name (lowercase, underscores allowed)
    pub name: String,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
    /// Network target (local, mainnet, testnet)
    #[serde(default)]
    pub network: Option<String>,
    /// Default sender address
    #[serde(default)]
    pub default_sender: Option<String>,
    /// Template to use: "blank", "token", "nft", "defi"
    #[serde(default)]
    pub template: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorldOpenInput {
    /// World name or ID (partial ID match supported)
    pub name_or_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorldListInput {
    /// Include full details (not just summary)
    #[serde(default)]
    pub include_details: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorldStatusInput {
    /// Include git status
    #[serde(default)]
    pub include_git: Option<bool>,
    /// Include state summary
    #[serde(default)]
    pub include_state: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WorldCloseInput {
    /// Save state before closing (default: true)
    #[serde(default)]
    pub save: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WorldDeleteInput {
    /// World name or ID
    pub name_or_id: String,
    /// Force delete even if active
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WorldSnapshotInput {
    /// Snapshot name
    pub name: String,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorldRestoreInput {
    /// Snapshot name to restore
    pub snapshot: String,
}

#[derive(Debug, Deserialize)]
pub struct WorldBuildInput {
    /// Auto-commit on success (overrides world config)
    #[serde(default)]
    pub auto_commit: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WorldDeployInput {
    /// Optional notes for the deployment
    #[serde(default)]
    pub notes: Option<String>,
    /// Auto-snapshot (overrides world config)
    #[serde(default)]
    pub auto_snapshot: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WorldCommitInput {
    /// Commit message (auto-generated if omitted)
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorldLogInput {
    /// Number of commits to show (default: 10)
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorldTemplatesInput {}

#[derive(Debug, Deserialize)]
pub struct WorldExportInput {
    /// World name or ID to export
    pub name_or_id: Option<String>,
    /// Export format: "zip" or "tar"
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorldReadFileInput {
    /// File path relative to the world root
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct WorldWriteFileInput {
    /// File path relative to the world root
    pub file: String,
    /// Full content to write (overwrites existing file)
    #[serde(default)]
    pub content: Option<String>,
    /// Find/replace edits to apply (ignored if content is provided)
    #[serde(default)]
    pub edits: Option<Vec<FileEdit>>,
    /// Create parent directories if missing (default: true)
    #[serde(default)]
    pub create_parents: Option<bool>,
}

// =============================================================================
// CLI Parity Inputs
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct NamedAddressInput {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Deserialize)]
pub struct PublishInput {
    /// Path to Move package directory (with Move.toml)
    pub path: String,
    /// Named address assignments (e.g., my_pkg=0x0)
    #[serde(default)]
    pub addresses: Vec<NamedAddressInput>,
    /// Skip compilation, use existing bytecode_modules/ directory
    #[serde(default)]
    pub bytecode_only: Option<bool>,
    /// Don't persist to session state
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Assign package to this address (default: from bytecode)
    #[serde(default)]
    pub assign_address: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunInput {
    /// Target function: "0xPKG::module::function" or "module::function"
    pub target: String,
    /// Arguments (auto-parsed: 42, true, 0xABC, "string", b"bytes")
    #[serde(default)]
    pub args: Vec<String>,
    /// Type arguments (e.g., "0x2::sui::SUI")
    #[serde(default)]
    pub type_args: Vec<String>,
    /// Sender address (default: 0x0)
    #[serde(default)]
    pub sender: Option<String>,
    /// Gas budget (0 or missing = default)
    #[serde(default)]
    pub gas_budget: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PtbCliInput {
    /// Input values
    #[serde(default)]
    pub inputs: Vec<PtbCliInputSpec>,
    /// Commands to execute
    pub calls: Vec<PtbCliCallSpec>,
    /// Sender address (default: 0x0)
    #[serde(default)]
    pub sender: Option<String>,
    /// Gas budget (default: 10_000_000)
    #[serde(default)]
    pub gas_budget: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PtbCliInputSpec {
    Pure(PtbCliPureInput),
    Object(PtbCliObjectInputSpec),
}

#[derive(Debug, Deserialize)]
pub struct PtbCliPureInput {
    #[serde(flatten)]
    pub value: PtbCliPureValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtbCliPureValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    Bool(bool),
    Address(String),
    #[serde(rename = "vector_u8_utf8")]
    VectorU8Utf8(String),
    #[serde(rename = "vector_u8_hex")]
    VectorU8Hex(String),
    #[serde(rename = "vector_address")]
    VectorAddress(Vec<String>),
    #[serde(rename = "vector_u64")]
    VectorU64(Vec<u64>),
}

#[derive(Debug, Deserialize)]
pub struct PtbCliObjectInputSpec {
    #[serde(rename = "imm_or_owned_object")]
    pub imm_or_owned: Option<String>,
    #[serde(rename = "shared_object")]
    pub shared: Option<PtbCliSharedObjectSpec>,
}

#[derive(Debug, Deserialize)]
pub struct PtbCliSharedObjectSpec {
    pub id: String,
    pub mutable: bool,
}

#[derive(Debug, Deserialize)]
pub struct PtbCliCallSpec {
    /// Target: "0xADDR::module::function"
    pub target: String,
    /// Type arguments
    #[serde(default)]
    pub type_args: Vec<String>,
    /// Arguments (references to inputs or results)
    #[serde(default)]
    pub args: Vec<PtbCliArgSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PtbCliArgSpec {
    Inline(PtbCliInlineArg),
    Reference(PtbCliArgReference),
}

#[derive(Debug, Deserialize)]
pub struct PtbCliInlineArg {
    #[serde(flatten)]
    pub value: PtbCliPureValue,
}

#[derive(Debug, Deserialize)]
pub struct PtbCliArgReference {
    /// Input index
    pub input: Option<u16>,
    /// Result index from previous command
    pub result: Option<u16>,
    /// Nested result [cmd_index, result_index]
    pub nested_result: Option<[u16; 2]>,
    /// Gas coin reference
    pub gas_coin: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ViewInput {
    /// One of: module, object, packages, modules
    pub kind: String,
    /// Module path: "0xPKG::module" or "module"
    #[serde(default)]
    pub module: Option<String>,
    /// Object ID
    #[serde(default)]
    pub object_id: Option<String>,
    /// Package address
    #[serde(default)]
    pub package: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BridgeInput {
    /// One of: publish, call, ptb, info
    pub kind: String,
    /// Path to Move package or spec (publish/ptb)
    #[serde(default)]
    pub path: Option<String>,
    /// Function target (call)
    #[serde(default)]
    pub target: Option<String>,
    /// Arguments (call)
    #[serde(default)]
    pub args: Vec<String>,
    /// Type arguments (call)
    #[serde(default)]
    pub type_args: Vec<String>,
    /// Gas budget in MIST
    #[serde(default)]
    pub gas_budget: Option<u64>,
    /// Skip install instructions
    #[serde(default)]
    pub quiet: Option<bool>,
    /// Verbose output (info)
    #[serde(default)]
    pub verbose: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct StatusInput {
    /// Include packages list
    #[serde(default)]
    pub include_packages: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct CleanInput {}
