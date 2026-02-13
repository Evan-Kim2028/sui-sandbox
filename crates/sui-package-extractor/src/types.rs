use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct PackageInterfaceJson {
    pub schema_version: u64,
    pub package_id: String,
    pub module_names: Vec<String>,
    pub modules: Value,
}

#[derive(Debug, Serialize)]
pub struct BytecodePackageInterfaceJson {
    pub schema_version: u64,
    pub package_id: String,
    pub module_names: Vec<String>,
    pub modules: Value,
}

#[derive(Debug, Serialize)]
pub struct SanityCounts {
    pub modules: usize,
    pub structs: usize,
    pub functions: usize,
    pub key_structs: usize,
}

#[derive(Debug, Serialize)]
pub struct BytecodeStructTypeParamJson {
    pub constraints: Vec<String>,
    pub is_phantom: bool,
}

#[derive(Debug, Serialize)]
pub struct BytecodeFunctionTypeParamJson {
    pub constraints: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeFieldJson {
    pub name: String,
    pub r#type: Value,
}

#[derive(Debug, Serialize)]
pub struct BytecodeStructJson {
    pub abilities: Vec<String>,
    pub type_params: Vec<BytecodeStructTypeParamJson>,
    pub is_native: bool,
    pub fields: Vec<BytecodeFieldJson>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeStructRefJson {
    pub address: String,
    pub module: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct BytecodeFunctionJson {
    pub visibility: String,
    pub is_entry: bool,
    pub is_native: bool,
    pub type_params: Vec<BytecodeFunctionTypeParamJson>,
    pub params: Vec<Value>,
    pub returns: Vec<Value>,
    pub acquires: Vec<BytecodeStructRefJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<BytecodeFunctionBodyJson>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeConstantJson {
    pub r#type: Value,
    pub data_hex: String,
    pub data_len: usize,
}

#[derive(Debug, Serialize)]
pub struct BytecodeStructInstantiationJson {
    pub name: String,
    pub type_arguments: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeFunctionInstantiationJson {
    pub address: String,
    pub module: String,
    pub function: String,
    pub type_arguments: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeMetadataJson {
    pub key_hex: String,
    pub key_utf8: Option<String>,
    pub value_hex: String,
    pub value_len: usize,
}

#[derive(Debug, Serialize)]
pub struct BytecodeBoundsCheckJson {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeEnumVariantJson {
    pub tag: u16,
    pub name: String,
    pub fields: Vec<BytecodeFieldJson>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeEnumJson {
    pub abilities: Vec<String>,
    pub type_params: Vec<BytecodeStructTypeParamJson>,
    pub variants: Vec<BytecodeEnumVariantJson>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeInstructionJson {
    pub offset: u16,
    pub opcode: String,
    pub operands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeJumpTableJson {
    pub head_enum: String,
    pub offsets: Vec<u16>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeFunctionBodyJson {
    pub locals: Vec<Value>,
    pub instructions: Vec<BytecodeInstructionJson>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub jump_tables: Vec<BytecodeJumpTableJson>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeModuleJson {
    pub address: String,
    pub structs: BTreeMap<String, BytecodeStructJson>,
    pub enums: BTreeMap<String, BytecodeEnumJson>,
    pub functions: BTreeMap<String, BytecodeFunctionJson>,
    pub constants: Vec<BytecodeConstantJson>,
    pub struct_instantiations: Vec<BytecodeStructInstantiationJson>,
    pub function_instantiations: Vec<BytecodeFunctionInstantiationJson>,
    pub metadata: Vec<BytecodeMetadataJson>,
    pub bounds_check: BytecodeBoundsCheckJson,
    pub friends: Vec<String>,
}

#[derive(Debug, Serialize, Copy, Clone)]
pub struct InterfaceCompareSummary {
    pub modules_compared: usize,
    pub modules_missing_in_bytecode: usize,
    pub modules_extra_in_bytecode: usize,
    pub structs_compared: usize,
    pub struct_mismatches: usize,
    pub functions_compared: usize,
    pub function_mismatches: usize,
    pub mismatches_total: usize,
}

#[derive(Debug, Serialize)]
pub struct InterfaceCompareMismatch {
    pub path: String,
    pub reason: String,
    pub rpc: Option<Value>,
    pub bytecode: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct InterfaceCompareReport {
    pub package_id: String,
    pub summary: InterfaceCompareSummary,
    pub mismatches: Vec<InterfaceCompareMismatch>,
}

#[derive(Debug, Serialize)]
pub struct BatchSummaryRow {
    pub input_id: String,
    pub package_id: Option<String>,
    pub resolved_from_package_info: bool,
    pub ok: bool,
    pub skipped: bool,
    pub output_path: Option<String>,
    pub sanity: Option<SanityCounts>,
    pub bytecode: Option<BytecodeModuleCheck>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BytecodeModuleCheck {
    pub normalized_modules: usize,
    pub bcs_modules: usize,
    pub missing_in_bcs: Vec<String>,
    pub extra_in_bcs: Vec<String>,
}

/// Bytecode-level counts for a package's modules, structs, and functions.
#[derive(Debug, Serialize, Clone, Copy, Default)]
pub struct LocalBytecodeCounts {
    pub modules: usize,
    pub structs: usize,
    pub structs_has_copy: usize,
    pub structs_has_drop: usize,
    pub structs_has_store: usize,
    pub structs_has_key: usize,
    pub functions_total: usize,
    pub functions_public: usize,
    pub functions_friend: usize,
    pub functions_private: usize,
    pub functions_native: usize,
    pub entry_functions: usize,
    pub public_entry_functions: usize,
    pub friend_entry_functions: usize,
    pub private_entry_functions: usize,
    pub key_structs: usize,
}

#[derive(Debug, Serialize)]
pub struct CorpusIndexRow {
    pub package_id: String,
    pub package_dir: String,
}

/// Difference between two sets of modules (e.g., local vs BCS, RPC vs local).
#[derive(Debug, Serialize, Default)]
pub struct ModuleSetDiff {
    pub left_count: usize,
    pub right_count: usize,
    pub missing_in_right: Vec<String>,
    pub extra_in_right: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalBytesCheck {
    pub mv_modules: usize,
    pub bcs_modules: usize,
    pub exact_match_modules: usize,
    pub mismatches_total: usize,
    pub missing_in_bcs: Vec<String>,
    pub missing_in_mv: Vec<String>,
    pub mismatches_sample: Vec<ModuleBytesMismatch>,
}

#[derive(Debug, Serialize)]
pub struct ModuleBytesMismatch {
    pub module: String,
    pub reason: String,
    pub mv_len: Option<usize>,
    pub bcs_len: Option<usize>,
    pub mv_sha256: Option<String>,
    pub bcs_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CorpusRow {
    pub package_id: String,
    pub package_dir: String,
    pub local: LocalBytecodeCounts,
    pub local_vs_bcs: ModuleSetDiff,
    pub local_bytes_check: Option<LocalBytesCheck>,
    pub local_bytes_check_error: Option<String>,
    pub rpc: Option<SanityCounts>,
    pub rpc_vs_local: Option<ModuleSetDiff>,
    pub interface_compare: Option<InterfaceCompareSummary>,
    pub interface_compare_sample: Option<Vec<InterfaceCompareMismatch>>,
    pub error: Option<String>,
}

/// Core statistics from a corpus analysis run.
///
/// This struct contains the shared statistical fields used by both
/// `CorpusSummary` (full output with file paths) and `SubmissionSummary` (nested stats).
#[derive(Debug, Clone, Serialize)]
pub struct CorpusStats {
    pub total: usize,
    pub local_ok: usize,
    pub local_vs_bcs_module_match: usize,
    pub local_bytes_check_enabled: bool,
    pub local_bytes_ok: usize,
    pub local_bytes_mismatch_packages: usize,
    pub local_bytes_mismatches_total: usize,
    pub rpc_enabled: bool,
    pub rpc_ok: usize,
    pub rpc_module_match: usize,
    pub rpc_exposed_function_count_match: usize,
    pub interface_compare_enabled: bool,
    pub interface_ok: usize,
    pub interface_mismatch_packages: usize,
    pub interface_mismatches_total: usize,
    pub problems: usize,
}

/// Full corpus summary including statistics and output file paths.
#[derive(Debug, Serialize)]
pub struct CorpusSummary {
    /// Core statistics (flattened into the JSON output)
    #[serde(flatten)]
    pub stats: CorpusStats,
    /// Path to the detailed report JSONL file
    pub report_jsonl: String,
    /// Path to the index JSONL file
    pub index_jsonl: String,
    /// Path to the problems JSONL file
    pub problems_jsonl: String,
    /// Path to the sample IDs file (if sampling was used)
    pub sample_ids: Option<String>,
    /// Path to the run metadata JSON file
    pub run_metadata_json: String,
}

#[derive(Debug, Serialize)]
pub struct GitMetadata {
    pub git_root: String,
    pub head: String,
    pub head_commit_time: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GitHeadMetadata {
    pub head: String,
    pub head_commit_time: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunMetadata {
    pub started_at_unix_seconds: u64,
    pub finished_at_unix_seconds: u64,
    pub argv: Vec<String>,
    pub rpc_url: String,
    pub bytecode_corpus_root: Option<String>,
    pub sui_packages_git: Option<GitMetadata>,
}

#[derive(Debug, Serialize)]
pub struct SubmissionSummary {
    pub tool: String,
    pub tool_version: String,
    pub started_at_unix_seconds: u64,
    pub finished_at_unix_seconds: u64,
    pub rpc_url: String,
    pub corpus_name: Option<String>,
    pub sui_packages_git: Option<GitHeadMetadata>,
    pub stats: CorpusSummaryStats,
}

/// Alias for CorpusStats for backwards compatibility.
///
/// Used in `SubmissionSummary` to nest the statistics under a `stats` field.
pub type CorpusSummaryStats = CorpusStats;
