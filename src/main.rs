//! Bytecode-first Sui Move research CLI.
//!
//! **Primary goal**: derive deterministic, diff-friendly artifacts from on-chain bytecode (or a local
//! `.mv` corpus) and validate them against RPC normalized interfaces.
//!
//! **Key modes**
//! - Single package (RPC normalized): `--package-id ... [--emit-json ...]`
//! - Single package (bytecode-derived): `--package-id ... --emit-bytecode-json ...`
//! - Rigorous compare (RPC vs bytecode-derived): `--package-id ... --compare-bytecode-rpc`
//! - Corpus scan: `--bytecode-corpus-root ... --out-dir ... [--corpus-rpc-compare] [--corpus-interface-compare]`
//!
//! **Guardrails**
//! - Corpus mode rejects single-package-only flags.
//! - Interface compare is only meaningful when RPC compare is enabled.
//! - Corpus runs write `run_metadata.json` so results are attributable to a dataset snapshot.
use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use sha2::Digest;
use sui_sdk::types::base_types::ObjectID;
use sui_sdk::SuiClientBuilder;

use move_binary_format::file_format::Ability;
use move_binary_format::file_format::AbilitySet;
use move_binary_format::file_format::CompiledModule;
use move_binary_format::file_format::SignatureToken;
use move_binary_format::file_format::StructFieldInformation;
use move_binary_format::file_format::Visibility;

#[derive(Debug, Copy, Clone, ValueEnum)]
enum MvrNetwork {
    Mainnet,
    Testnet,
}

#[derive(Debug, Copy, Clone, ValueEnum)]
enum InputKind {
    /// Treat inputs as package object IDs.
    Package,
    /// Treat inputs as `::package_info::PackageInfo` object IDs (resolve `package_address`).
    PackageInfo,
    /// Try as package ID first; on failure, try resolving as PackageInfo.
    Auto,
}

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    /// On-chain id (0x...). Can be provided multiple times.
    #[arg(long, value_name = "ID")]
    package_id: Vec<String>,

    /// Read additional ids from a file (1 id per line; '#' comments allowed).
    #[arg(long, value_name = "PATH")]
    package_ids_file: Option<PathBuf>,

    /// Read ids from an MVR catalog.json (uses *package_info_id fields).
    #[arg(long, value_name = "PATH")]
    mvr_catalog: Option<PathBuf>,

    /// Which MVR catalog id field to use.
    #[arg(long, value_enum, default_value_t = MvrNetwork::Mainnet)]
    mvr_network: MvrNetwork,

    /// How to interpret input ids.
    #[arg(long, value_enum, default_value_t = InputKind::Auto)]
    input_kind: InputKind,

    /// Root of a local bytecode corpus (expects `0x??/<id>/bytecode_modules/*.mv` style dirs).
    ///
    /// Example: `<sui-packages-checkout>/packages/mainnet_most_used`
    #[arg(long, value_name = "DIR")]
    bytecode_corpus_root: Option<PathBuf>,

    /// In corpus mode, also fetch RPC-normalized modules and compare counts/module names.
    #[arg(long, default_value_t = false)]
    corpus_rpc_compare: bool,

    /// In corpus mode, do a rigorous field-by-field compare between RPC normalized interface and
    /// bytecode-derived interface (types, fields, params/returns, etc).
    /// Requires `--corpus-rpc-compare`.
    #[arg(long, default_value_t = false)]
    corpus_interface_compare: bool,

    /// In corpus mode with `--corpus-interface-compare`, capture up to N mismatch samples per package
    /// to make failures actionable. `0` keeps only summary counts.
    #[arg(long, default_value_t = 10)]
    corpus_interface_compare_max_mismatches: usize,

    /// In corpus mode with `--corpus-interface-compare`, include RPC/bytecode values in mismatch samples.
    /// This can make outputs much larger; off by default.
    #[arg(long, default_value_t = false)]
    corpus_interface_compare_include_values: bool,

    /// In corpus mode, skip deserializing `.mv` bytecode and only derive module names from filenames.
    /// This is faster for very large corpora but disables struct/function/key counts.
    /// Not compatible with `--corpus-rpc-compare` or `--corpus-interface-compare`.
    #[arg(long, default_value_t = false)]
    corpus_module_names_only: bool,

    /// In corpus mode, write an index JSONL (defaults to <out-dir>/index.jsonl).
    #[arg(long, value_name = "PATH")]
    corpus_index_jsonl: Option<PathBuf>,

    /// In corpus mode, restrict analysis to package ids from a file (1 id per line; '#' comments allowed).
    /// This is useful for re-running a deterministic sample or focusing only on problematic ids.
    #[arg(long, value_name = "PATH")]
    corpus_ids_file: Option<PathBuf>,

    /// In corpus mode, select a deterministic sample of N packages.
    #[arg(long, value_name = "N")]
    corpus_sample: Option<usize>,

    /// Seed used for deterministic corpus sampling.
    #[arg(long, default_value_t = 0)]
    corpus_seed: u64,

    /// In corpus mode with --corpus-sample, write sampled package ids (defaults to <out-dir>/sample_ids.txt).
    #[arg(long, value_name = "PATH")]
    corpus_sample_ids_out: Option<PathBuf>,

    /// In corpus mode, verify local `.mv` bytes exactly match `bcs.json` moduleMap bytes.
    ///
    /// This is a strong local integrity check that does not require RPC.
    #[arg(long, default_value_t = false)]
    corpus_local_bytes_check: bool,

    /// In corpus mode with `--corpus-local-bytes-check`, capture up to N mismatch samples per package.
    #[arg(long, default_value_t = 10)]
    corpus_local_bytes_check_max_mismatches: usize,

    /// In corpus mode, write a sanitized “submission summary” JSON (no local filesystem paths).
    /// This is intended to be safe to share publicly (e.g., in a paper or benchmark repo).
    #[arg(long, value_name = "PATH")]
    emit_submission_summary: Option<PathBuf>,

    /// RPC URL (default: mainnet fullnode)
    #[arg(long, default_value = "https://fullnode.mainnet.sui.io:443")]
    rpc_url: String,

    /// Write canonical interface JSON to a file path (use '-' for stdout). Only valid for single-package mode.
    #[arg(long, value_name = "PATH")]
    emit_json: Option<PathBuf>,

    /// Write bytecode-derived canonical interface JSON to a file path (use '-' for stdout).
    /// Valid for single-package mode (`--package-id`) or local dir mode (`--bytecode-package-dir`).
    #[arg(long, value_name = "PATH")]
    emit_bytecode_json: Option<PathBuf>,

    /// Compare RPC normalized interface vs bytecode-derived interface for the package id.
    /// Use `--emit-compare-report` to write mismatch details.
    #[arg(long, default_value_t = false)]
    compare_bytecode_rpc: bool,

    /// Write a comparison report (JSON) showing mismatch counts and a sample of mismatches.
    /// Only valid for single-package mode with `--compare-bytecode-rpc`.
    #[arg(long, value_name = "PATH")]
    emit_compare_report: Option<PathBuf>,

    /// Maximum mismatches to include in `--emit-compare-report` (counts still include all mismatches).
    #[arg(long, default_value_t = 200)]
    compare_max_mismatches: usize,

    /// Local bytecode artifact directory (expects `bytecode_modules/*.mv`, optional `metadata.json`).
    /// Enables bytecode-first single-package mode without RPC.
    #[arg(long, value_name = "DIR")]
    bytecode_package_dir: Option<PathBuf>,

    /// Output directory for batch/corpus mode.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Write a batch summary as JSONL (defaults to <out-dir>/summary.jsonl).
    #[arg(long, value_name = "PATH")]
    summary_jsonl: Option<PathBuf>,

    /// Limit the number of packages processed (after dedup/sort).
    #[arg(long, value_name = "N")]
    max_packages: Option<usize>,

    /// Max concurrent RPC fetches in batch/corpus mode.
    #[arg(long, default_value_t = 2)]
    concurrency: usize,

    /// Number of retries for retryable RPC failures (e.g., 429 rate limiting).
    #[arg(long, default_value_t = 8)]
    retries: usize,

    /// Initial retry backoff in milliseconds.
    #[arg(long, default_value_t = 250)]
    retry_initial_ms: u64,

    /// Maximum retry backoff in milliseconds.
    #[arg(long, default_value_t = 5000)]
    retry_max_ms: u64,

    /// Skip packages whose output JSON already exists in --out-dir.
    #[arg(long, default_value_t = false)]
    skip_existing: bool,

    /// Print basic counts (modules/structs/functions/key_structs)
    #[arg(long, default_value_t = false)]
    sanity: bool,

    /// Print module names (single-package mode)
    #[arg(long, default_value_t = false)]
    list_modules: bool,

    /// Verify canonical JSON is stable under serialize/parse/serialize
    #[arg(long, default_value_t = false)]
    check_stability: bool,

    /// Compare normalized module list to on-chain MovePackage `bcs.moduleMap` keys.
    #[arg(long, default_value_t = false)]
    bytecode_check: bool,
}

#[derive(Debug, Serialize)]
struct PackageInterfaceJson {
    schema_version: u64,
    package_id: String,
    module_names: Vec<String>,
    modules: Value,
}

#[derive(Debug, Serialize)]
struct BytecodePackageInterfaceJson {
    schema_version: u64,
    package_id: String,
    module_names: Vec<String>,
    modules: Value,
}

#[derive(Debug, Serialize)]
struct SanityCounts {
    modules: usize,
    structs: usize,
    functions: usize,
    key_structs: usize,
}

#[derive(Debug, Serialize)]
struct BytecodeStructTypeParamJson {
    constraints: Vec<String>,
    is_phantom: bool,
}

#[derive(Debug, Serialize)]
struct BytecodeFunctionTypeParamJson {
    constraints: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BytecodeFieldJson {
    name: String,
    r#type: Value,
}

#[derive(Debug, Serialize)]
struct BytecodeStructJson {
    abilities: Vec<String>,
    type_params: Vec<BytecodeStructTypeParamJson>,
    is_native: bool,
    fields: Vec<BytecodeFieldJson>,
}

#[derive(Debug, Serialize)]
struct BytecodeStructRefJson {
    address: String,
    module: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct BytecodeFunctionJson {
    visibility: String,
    is_entry: bool,
    is_native: bool,
    type_params: Vec<BytecodeFunctionTypeParamJson>,
    params: Vec<Value>,
    returns: Vec<Value>,
    acquires: Vec<BytecodeStructRefJson>,
}

#[derive(Debug, Serialize)]
struct BytecodeModuleJson {
    address: String,
    structs: BTreeMap<String, BytecodeStructJson>,
    functions: BTreeMap<String, BytecodeFunctionJson>,
}

#[derive(Debug, Serialize, Copy, Clone)]
struct InterfaceCompareSummary {
    modules_compared: usize,
    modules_missing_in_bytecode: usize,
    modules_extra_in_bytecode: usize,
    structs_compared: usize,
    struct_mismatches: usize,
    functions_compared: usize, // RPC exposed functions compared
    function_mismatches: usize,
    mismatches_total: usize,
}

#[derive(Debug, Serialize)]
struct InterfaceCompareMismatch {
    path: String,
    reason: String,
    rpc: Option<Value>,
    bytecode: Option<Value>,
}

#[derive(Debug, Serialize)]
struct InterfaceCompareReport {
    package_id: String,
    summary: InterfaceCompareSummary,
    mismatches: Vec<InterfaceCompareMismatch>,
}

#[derive(Debug, Serialize)]
struct BatchSummaryRow {
    input_id: String,
    package_id: Option<String>,
    resolved_from_package_info: bool,
    ok: bool,
    skipped: bool,
    output_path: Option<String>,
    sanity: Option<SanityCounts>,
    bytecode: Option<BytecodeModuleCheck>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BytecodeModuleCheck {
    normalized_modules: usize,
    bcs_modules: usize,
    missing_in_bcs: Vec<String>,
    extra_in_bcs: Vec<String>,
}

#[derive(Debug, Serialize, Clone, Copy)]
struct LocalBytecodeCounts {
    modules: usize,
    structs: usize,
    functions_total: usize,
    functions_public: usize,
    functions_friend: usize,
    functions_private: usize,
    functions_native: usize,
    entry_functions: usize,
    public_entry_functions: usize,
    friend_entry_functions: usize,
    private_entry_functions: usize,
    key_structs: usize,
}

#[derive(Debug, Serialize)]
struct CorpusIndexRow {
    package_id: String,
    package_dir: String,
}

#[derive(Debug, Serialize)]
struct ModuleSetDiff {
    left_count: usize,
    right_count: usize,
    missing_in_right: Vec<String>,
    extra_in_right: Vec<String>,
}

#[derive(Debug, Serialize)]
struct LocalBytesCheck {
    mv_modules: usize,
    bcs_modules: usize,
    exact_match_modules: usize,
    mismatches_total: usize,
    missing_in_bcs: Vec<String>,
    missing_in_mv: Vec<String>,
    mismatches_sample: Vec<ModuleBytesMismatch>,
}

#[derive(Debug, Serialize)]
struct ModuleBytesMismatch {
    module: String,
    reason: String,
    mv_len: Option<usize>,
    bcs_len: Option<usize>,
    mv_sha256: Option<String>,
    bcs_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
struct CorpusRow {
    package_id: String,
    package_dir: String,
    local: LocalBytecodeCounts,
    local_vs_bcs: ModuleSetDiff,
    local_bytes_check: Option<LocalBytesCheck>,
    local_bytes_check_error: Option<String>,
    rpc: Option<SanityCounts>,
    rpc_vs_local: Option<ModuleSetDiff>,
    interface_compare: Option<InterfaceCompareSummary>,
    interface_compare_sample: Option<Vec<InterfaceCompareMismatch>>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CorpusSummary {
    total: usize,
    local_ok: usize,
    local_vs_bcs_module_match: usize,
    local_bytes_check_enabled: bool,
    local_bytes_ok: usize,
    local_bytes_mismatch_packages: usize,
    local_bytes_mismatches_total: usize,
    rpc_enabled: bool,
    rpc_ok: usize,
    rpc_module_match: usize,
    rpc_exposed_function_count_match: usize,
    interface_compare_enabled: bool,
    interface_ok: usize,
    interface_mismatch_packages: usize,
    interface_mismatches_total: usize,
    problems: usize,
    report_jsonl: String,
    index_jsonl: String,
    problems_jsonl: String,
    sample_ids: Option<String>,
    run_metadata_json: String,
}

#[derive(Debug, Copy, Clone)]
struct RetryConfig {
    retries: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl RetryConfig {
    fn from_args(args: &Args) -> Self {
        Self {
            retries: args.retries,
            initial_backoff: Duration::from_millis(args.retry_initial_ms),
            max_backoff: Duration::from_millis(args.retry_max_ms),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BytesInfo {
    len: usize,
    sha256: [u8; 32],
}

fn sha256_32(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..]);
    out
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn bytes_info(bytes: &[u8]) -> BytesInfo {
    BytesInfo {
        len: bytes.len(),
        sha256: sha256_32(bytes),
    }
}

fn bytes_info_sha256_hex(info: BytesInfo) -> String {
    bytes_to_hex(&info.sha256)
}

fn should_retry_error(error: &anyhow::Error) -> bool {
    let s = format!("{:#}", error);
    s.contains("Request rejected `429`")
        || s.contains("429")
        || s.to_ascii_lowercase().contains("too many")
        || s.to_ascii_lowercase().contains("timed out")
        || s.to_ascii_lowercase().contains("timeout")
        || s.to_ascii_lowercase().contains("connection")
        || s.to_ascii_lowercase().contains("transport")
}

async fn with_retries<T, F, Fut>(cfg: RetryConfig, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0usize;
    let mut backoff = cfg.initial_backoff;

    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= cfg.retries || !should_retry_error(&e) {
                    return Err(e);
                }
                attempt += 1;
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, cfg.max_backoff);
            }
        }
    }
}

fn canonicalize_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let old_map = std::mem::take(map);
            let mut entries: Vec<(String, Value)> = old_map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));

            for (_, v) in entries.iter_mut() {
                canonicalize_json_value(v);
            }

            for (k, v) in entries {
                map.insert(k, v);
            }
        }
        Value::Array(values) => {
            for v in values.iter_mut() {
                canonicalize_json_value(v);
            }
        }
        _ => {}
    }
}

fn bytes_to_hex_prefixed(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

#[derive(Debug, Serialize)]
struct GitMetadata {
    git_root: String,
    head: String,
    head_commit_time: Option<String>,
}

#[derive(Debug, Serialize)]
struct GitHeadMetadata {
    head: String,
    head_commit_time: Option<String>,
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

fn git_metadata_for_path(path: &Path) -> Option<GitMetadata> {
    let root = find_git_root(path)?;
    let head = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(o) } else { None })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())?;

    let head_commit_time = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["log", "-1", "--format=%cI"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(o) } else { None })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    Some(GitMetadata {
        git_root: root.display().to_string(),
        head,
        head_commit_time,
    })
}

#[derive(Debug, Serialize)]
struct RunMetadata {
    started_at_unix_seconds: u64,
    finished_at_unix_seconds: u64,
    argv: Vec<String>,
    rpc_url: String,
    bytecode_corpus_root: Option<String>,
    sui_packages_git: Option<GitMetadata>,
}

#[derive(Debug, Serialize)]
struct SubmissionSummary {
    tool: String,
    tool_version: String,
    started_at_unix_seconds: u64,
    finished_at_unix_seconds: u64,
    rpc_url: String,
    corpus_name: Option<String>,
    sui_packages_git: Option<GitHeadMetadata>,
    stats: CorpusSummaryStats,
}

#[derive(Debug, Serialize)]
struct CorpusSummaryStats {
    total: usize,
    local_ok: usize,
    local_vs_bcs_module_match: usize,
    local_bytes_check_enabled: bool,
    local_bytes_ok: usize,
    local_bytes_mismatch_packages: usize,
    local_bytes_mismatches_total: usize,
    rpc_enabled: bool,
    rpc_ok: usize,
    rpc_module_match: usize,
    rpc_exposed_function_count_match: usize,
    interface_compare_enabled: bool,
    interface_ok: usize,
    interface_mismatch_packages: usize,
    interface_mismatches_total: usize,
    problems: usize,
}

fn git_head_metadata_for_path(path: &Path) -> Option<GitHeadMetadata> {
    let meta = git_metadata_for_path(path)?;
    Some(GitHeadMetadata {
        head: meta.head,
        head_commit_time: meta.head_commit_time,
    })
}

fn normalize_address_str(addr: &str) -> Result<String> {
    let s = addr.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Err(anyhow!("empty address"));
    }
    let mut hex = s.to_ascii_lowercase();
    if hex.len() > 64 {
        return Err(anyhow!("address too long: {}", addr));
    }
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid hex address: {}", addr));
    }
    if hex.len() % 2 == 1 {
        hex = format!("0{}", hex);
    }
    Ok(format!("0x{:0>64}", hex))
}

fn module_self_address_hex(module: &CompiledModule) -> String {
    bytes_to_hex_prefixed(module.self_id().address().as_ref())
}

fn module_id_for_datatype_handle(
    module: &CompiledModule,
    datatype_handle_index: move_binary_format::file_format::DatatypeHandleIndex,
) -> BytecodeStructRefJson {
    let datatype_handle = module.datatype_handle_at(datatype_handle_index);
    let module_handle = module.module_handle_at(datatype_handle.module);
    let addr = module.address_identifier_at(module_handle.address);
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(datatype_handle.name).to_string();
    BytecodeStructRefJson {
        address: bytes_to_hex_prefixed(addr.as_ref()),
        module: module_name,
        name: struct_name,
    }
}

fn ability_set_to_strings(set: &AbilitySet) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Canonical order.
    if set.has_ability(Ability::Copy) {
        out.push("copy".to_string());
    }
    if set.has_ability(Ability::Drop) {
        out.push("drop".to_string());
    }
    if set.has_ability(Ability::Store) {
        out.push("store".to_string());
    }
    if set.has_ability(Ability::Key) {
        out.push("key".to_string());
    }
    out
}

fn visibility_to_string(v: Visibility) -> String {
    match v {
        Visibility::Public => "public".to_string(),
        Visibility::Friend => "friend".to_string(),
        Visibility::Private => "private".to_string(),
    }
}

fn signature_token_to_json(module: &CompiledModule, tok: &SignatureToken) -> Value {
    match tok {
        SignatureToken::Bool => serde_json::json!({"kind": "bool"}),
        SignatureToken::U8 => serde_json::json!({"kind": "u8"}),
        SignatureToken::U16 => serde_json::json!({"kind": "u16"}),
        SignatureToken::U32 => serde_json::json!({"kind": "u32"}),
        SignatureToken::U64 => serde_json::json!({"kind": "u64"}),
        SignatureToken::U128 => serde_json::json!({"kind": "u128"}),
        SignatureToken::U256 => serde_json::json!({"kind": "u256"}),
        SignatureToken::Address => serde_json::json!({"kind": "address"}),
        SignatureToken::Signer => serde_json::json!({"kind": "signer"}),
        SignatureToken::Vector(inner) => {
            serde_json::json!({"kind": "vector", "type": signature_token_to_json(module, inner)})
        }
        SignatureToken::Reference(inner) => {
            serde_json::json!({"kind": "ref", "mutable": false, "to": signature_token_to_json(module, inner)})
        }
        SignatureToken::MutableReference(inner) => {
            serde_json::json!({"kind": "ref", "mutable": true, "to": signature_token_to_json(module, inner)})
        }
        SignatureToken::TypeParameter(idx) => {
            serde_json::json!({"kind": "type_param", "index": idx})
        }
        SignatureToken::Datatype(idx) => {
            let tref = module_id_for_datatype_handle(module, *idx);
            serde_json::json!({"kind": "datatype", "address": tref.address, "module": tref.module, "name": tref.name, "type_args": []})
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, tys) = &**inst;
            let tref = module_id_for_datatype_handle(module, *idx);
            let args: Vec<Value> = tys
                .iter()
                .map(|t| signature_token_to_json(module, t))
                .collect();
            serde_json::json!({"kind": "datatype", "address": tref.address, "module": tref.module, "name": tref.name, "type_args": args})
        }
    }
}

fn rpc_visibility_to_string(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    match s {
        "Public" => Some("public".to_string()),
        "Friend" => Some("friend".to_string()),
        "Private" => Some("private".to_string()),
        _ => None,
    }
}

fn abilities_from_value(value: &Value) -> Vec<String> {
    // Handles shapes like:
    // - ["Store", "Key"]
    // - {"abilities": ["Store", "Key"]}
    // - {"constraints": {"abilities": [...]}}
    if let Some(arr) = value.as_array() {
        let mut out: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_ascii_lowercase())
            .collect();
        out.sort();
        out.dedup();
        return out;
    }
    if let Some(obj) = value.as_object() {
        if let Some(v) = obj.get("abilities") {
            return abilities_from_value(v);
        }
        if let Some(v) = obj.get("constraints") {
            return abilities_from_value(v);
        }
    }
    Vec::new()
}

fn rpc_type_to_canonical_json(v: &Value) -> Result<Value> {
    if let Some(s) = v.as_str() {
        let out = match s {
            "Bool" => serde_json::json!({"kind": "bool"}),
            "U8" => serde_json::json!({"kind": "u8"}),
            "U16" => serde_json::json!({"kind": "u16"}),
            "U32" => serde_json::json!({"kind": "u32"}),
            "U64" => serde_json::json!({"kind": "u64"}),
            "U128" => serde_json::json!({"kind": "u128"}),
            "U256" => serde_json::json!({"kind": "u256"}),
            "Address" => serde_json::json!({"kind": "address"}),
            "Signer" => serde_json::json!({"kind": "signer"}),
            other => return Err(anyhow!("unknown RPC primitive type string: {}", other)),
        };
        return Ok(out);
    }

    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("RPC type is not an object: {}", v))?;
    if obj.len() != 1 {
        return Err(anyhow!("RPC type expected single-key object: {}", v));
    }
    let (k, inner) = obj.iter().next().expect("len=1");
    let out = match k.as_str() {
        "Bool" => serde_json::json!({"kind": "bool"}),
        "U8" => serde_json::json!({"kind": "u8"}),
        "U16" => serde_json::json!({"kind": "u16"}),
        "U32" => serde_json::json!({"kind": "u32"}),
        "U64" => serde_json::json!({"kind": "u64"}),
        "U128" => serde_json::json!({"kind": "u128"}),
        "U256" => serde_json::json!({"kind": "u256"}),
        "Address" => serde_json::json!({"kind": "address"}),
        "Signer" => serde_json::json!({"kind": "signer"}),
        "Vector" => {
            serde_json::json!({"kind": "vector", "type": rpc_type_to_canonical_json(inner)?})
        }
        "Reference" => {
            serde_json::json!({"kind": "ref", "mutable": false, "to": rpc_type_to_canonical_json(inner)?})
        }
        "MutableReference" => {
            serde_json::json!({"kind": "ref", "mutable": true, "to": rpc_type_to_canonical_json(inner)?})
        }
        "TypeParameter" => {
            let idx = inner
                .as_u64()
                .ok_or_else(|| anyhow!("TypeParameter index is not u64: {}", inner))?;
            serde_json::json!({"kind": "type_param", "index": idx})
        }
        "Struct" => {
            let s = inner
                .as_object()
                .ok_or_else(|| anyhow!("Struct payload is not object: {}", inner))?;
            let addr = s
                .get("address")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Struct missing address: {}", inner))?;
            let module = s
                .get("module")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Struct missing module: {}", inner))?;
            let name = s
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Struct missing name: {}", inner))?;
            let args = s
                .get("typeArguments")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Struct missing typeArguments: {}", inner))?;
            let args_canon: Vec<Value> = args
                .iter()
                .map(rpc_type_to_canonical_json)
                .collect::<Result<_>>()?;
            serde_json::json!({
                "kind": "datatype",
                "address": normalize_address_str(addr)?,
                "module": module,
                "name": name,
                "type_args": args_canon,
            })
        }
        _ => return Err(anyhow!("unknown RPC type tag: {}", k)),
    };
    Ok(out)
}

fn bytecode_type_to_canonical_json(v: &Value) -> Result<Value> {
    // Ensure address normalization and stable shapes for comparison.
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("bytecode type is not object: {}", v))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("bytecode type missing kind: {}", v))?;
    match kind {
        "datatype" => {
            let addr = obj
                .get("address")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bytecode datatype missing address: {}", v))?;
            let module = obj
                .get("module")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bytecode datatype missing module: {}", v))?;
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bytecode datatype missing name: {}", v))?;
            let args = obj
                .get("type_args")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("bytecode datatype missing type_args: {}", v))?;
            let args_canon: Vec<Value> = args
                .iter()
                .map(bytecode_type_to_canonical_json)
                .collect::<Result<_>>()?;
            Ok(serde_json::json!({
                "kind": "datatype",
                "address": normalize_address_str(addr)?,
                "module": module,
                "name": name,
                "type_args": args_canon,
            }))
        }
        "vector" => {
            let inner = obj
                .get("type")
                .ok_or_else(|| anyhow!("vector missing type: {}", v))?;
            Ok(
                serde_json::json!({"kind": "vector", "type": bytecode_type_to_canonical_json(inner)?}),
            )
        }
        "ref" => {
            let mutable = obj
                .get("mutable")
                .and_then(Value::as_bool)
                .ok_or_else(|| anyhow!("ref missing mutable bool: {}", v))?;
            let inner = obj
                .get("to")
                .ok_or_else(|| anyhow!("ref missing to: {}", v))?;
            Ok(
                serde_json::json!({"kind":"ref","mutable":mutable,"to": bytecode_type_to_canonical_json(inner)?}),
            )
        }
        "type_param" => {
            let idx = obj
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("type_param missing numeric index: {}", v))?;
            Ok(serde_json::json!({"kind":"type_param","index": idx}))
        }
        "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address" | "signer" => {
            Ok(serde_json::json!({"kind": kind}))
        }
        _ => Err(anyhow!("unknown bytecode type kind: {}", kind)),
    }
}

fn build_bytecode_module_json(module: &CompiledModule) -> Result<BytecodeModuleJson> {
    let mut structs: BTreeMap<String, BytecodeStructJson> = BTreeMap::new();
    let mut functions: BTreeMap<String, BytecodeFunctionJson> = BTreeMap::new();

    for def in module.struct_defs() {
        let handle = module.datatype_handle_at(def.struct_handle);
        let name = module.identifier_at(handle.name).to_string();

        let type_params: Vec<BytecodeStructTypeParamJson> = handle
            .type_parameters
            .iter()
            .map(|tp| BytecodeStructTypeParamJson {
                constraints: ability_set_to_strings(&tp.constraints),
                is_phantom: tp.is_phantom,
            })
            .collect();

        let abilities = ability_set_to_strings(&handle.abilities);

        let mut fields: Vec<BytecodeFieldJson> = Vec::new();
        let mut is_native = false;
        match &def.field_information {
            StructFieldInformation::Declared(field_defs) => {
                for f in field_defs {
                    let field_name = module.identifier_at(f.name).to_string();
                    let field_ty = signature_token_to_json(module, &f.signature.0);
                    fields.push(BytecodeFieldJson {
                        name: field_name,
                        r#type: field_ty,
                    });
                }
            }
            StructFieldInformation::Native => {
                is_native = true;
            }
        }

        structs.insert(
            name,
            BytecodeStructJson {
                abilities,
                type_params,
                is_native,
                fields,
            },
        );
    }

    for def in module.function_defs() {
        let handle = module.function_handle_at(def.function);
        let name = module.identifier_at(handle.name).to_string();

        let params_sig = module.signature_at(handle.parameters);
        let returns_sig = module.signature_at(handle.return_);
        let params: Vec<Value> = params_sig
            .0
            .iter()
            .map(|t| signature_token_to_json(module, t))
            .collect();
        let returns: Vec<Value> = returns_sig
            .0
            .iter()
            .map(|t| signature_token_to_json(module, t))
            .collect();

        let type_params: Vec<BytecodeFunctionTypeParamJson> = handle
            .type_parameters
            .iter()
            .map(|c| BytecodeFunctionTypeParamJson {
                constraints: ability_set_to_strings(c),
            })
            .collect();

        let mut acquires: Vec<BytecodeStructRefJson> = Vec::new();
        for idx in def.acquires_global_resources.iter() {
            let sdef = module.struct_def_at(*idx);
            let sh = module.datatype_handle_at(sdef.struct_handle);
            acquires.push(BytecodeStructRefJson {
                address: module_self_address_hex(module),
                module: compiled_module_name(module),
                name: module.identifier_at(sh.name).to_string(),
            });
        }
        acquires.sort_by(|a, b| a.name.cmp(&b.name));

        functions.insert(
            name,
            BytecodeFunctionJson {
                visibility: visibility_to_string(def.visibility),
                is_entry: def.is_entry,
                is_native: def.code.is_none(),
                type_params,
                params,
                returns,
                acquires,
            },
        );
    }

    Ok(BytecodeModuleJson {
        address: module_self_address_hex(module),
        structs,
        functions,
    })
}

fn build_bytecode_interface_value_from_compiled_modules(
    package_id: &str,
    compiled_modules: &[CompiledModule],
) -> Result<(Vec<String>, Value)> {
    let mut module_map: BTreeMap<String, BytecodeModuleJson> = BTreeMap::new();
    for module in compiled_modules {
        let name = compiled_module_name(module);
        module_map.insert(name, build_bytecode_module_json(module)?);
    }

    let module_names: Vec<String> = module_map.keys().cloned().collect();
    let mut modules_value =
        serde_json::to_value(&module_map).context("serialize bytecode modules")?;
    canonicalize_json_value(&mut modules_value);

    let interface = BytecodePackageInterfaceJson {
        schema_version: 1,
        package_id: package_id.to_string(),
        module_names: module_names.clone(),
        modules: modules_value,
    };

    let mut interface_value =
        serde_json::to_value(interface).context("build bytecode interface JSON")?;
    canonicalize_json_value(&mut interface_value);
    Ok((module_names, interface_value))
}

struct InterfaceCompareOptions {
    max_mismatches: usize,
    include_values: bool,
}

fn compare_interface_rpc_vs_bytecode(
    _package_id: &str,
    rpc_interface_value: &Value,
    bytecode_interface_value: &Value,
    opts: InterfaceCompareOptions,
) -> (InterfaceCompareSummary, Vec<InterfaceCompareMismatch>) {
    let mut mismatches: Vec<InterfaceCompareMismatch> = Vec::new();
    let mut mismatch_count_total: usize = 0;

    let mut push_mismatch =
        |path: String, reason: String, rpc: Option<Value>, bytecode: Option<Value>| {
            mismatch_count_total += 1;
            if mismatches.len() < opts.max_mismatches {
                let (rpc, bytecode) = if opts.include_values {
                    (rpc, bytecode)
                } else {
                    (None, None)
                };
                mismatches.push(InterfaceCompareMismatch {
                    path,
                    reason,
                    rpc,
                    bytecode,
                });
            }
        };

    let empty_modules = serde_json::Map::new();
    let rpc_modules = rpc_interface_value
        .get("modules")
        .and_then(Value::as_object)
        .unwrap_or(&empty_modules);
    let byte_modules = bytecode_interface_value
        .get("modules")
        .and_then(Value::as_object)
        .unwrap_or(&empty_modules);

    let mut rpc_module_names: Vec<&String> = rpc_modules.keys().collect();
    rpc_module_names.sort();
    let mut byte_module_names: Vec<&String> = byte_modules.keys().collect();
    byte_module_names.sort();

    let rpc_set: HashSet<&str> = rpc_module_names.iter().map(|s| s.as_str()).collect();
    let byte_set: HashSet<&str> = byte_module_names.iter().map(|s| s.as_str()).collect();

    let modules_missing_in_bytecode: Vec<&str> = rpc_module_names
        .iter()
        .map(|s| s.as_str())
        .filter(|m| !byte_set.contains(m))
        .collect();
    for m in &modules_missing_in_bytecode {
        push_mismatch(
            format!("modules/{m}"),
            "module missing in bytecode".to_string(),
            rpc_modules.get(*m).cloned(),
            None,
        );
    }

    let modules_extra_in_bytecode: Vec<&str> = byte_module_names
        .iter()
        .map(|s| s.as_str())
        .filter(|m| !rpc_set.contains(m))
        .collect();
    for m in &modules_extra_in_bytecode {
        push_mismatch(
            format!("modules/{m}"),
            "extra module in bytecode".to_string(),
            None,
            byte_modules.get(*m).cloned(),
        );
    }

    let mut modules_compared = 0usize;
    let mut structs_compared = 0usize;
    let mut struct_mismatches = 0usize;
    let mut functions_compared = 0usize;
    let mut function_mismatches = 0usize;

    // Compare intersection modules.
    let mut intersection: Vec<&str> = rpc_module_names
        .iter()
        .map(|s| s.as_str())
        .filter(|m| byte_set.contains(*m))
        .collect();
    intersection.sort();

    for module_name in intersection {
        modules_compared += 1;

        let rpc_mod = rpc_modules.get(module_name).unwrap_or(&Value::Null);
        let byte_mod = byte_modules.get(module_name).unwrap_or(&Value::Null);

        // Structs
        let rpc_structs = get_object(rpc_mod, &["structs"])
            .cloned()
            .unwrap_or_default();
        let byte_structs = get_object(byte_mod, &["structs"])
            .cloned()
            .unwrap_or_default();

        let mut rpc_struct_names: Vec<String> = rpc_structs.keys().cloned().collect();
        rpc_struct_names.sort();
        let mut byte_struct_names: Vec<String> = byte_structs.keys().cloned().collect();
        byte_struct_names.sort();

        let byte_struct_set: HashSet<&str> = byte_struct_names.iter().map(|s| s.as_str()).collect();
        for sname in &rpc_struct_names {
            if !byte_struct_set.contains(sname.as_str()) {
                struct_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/structs/{sname}"),
                    "struct missing in bytecode".to_string(),
                    rpc_structs.get(sname).cloned(),
                    None,
                );
            }
        }

        for sname in &rpc_struct_names {
            let Some(rpc_struct) = rpc_structs.get(sname) else {
                continue;
            };
            let Some(byte_struct) = byte_structs.get(sname) else {
                continue;
            };
            structs_compared += 1;

            // Abilities
            let rpc_abilities = rpc_struct
                .get("abilities")
                .map(abilities_from_value)
                .unwrap_or_default();
            let byte_abilities = byte_struct
                .get("abilities")
                .map(abilities_from_value)
                .unwrap_or_default();
            if rpc_abilities != byte_abilities {
                struct_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/structs/{sname}/abilities"),
                    "abilities mismatch".to_string(),
                    rpc_struct.get("abilities").cloned(),
                    byte_struct.get("abilities").cloned(),
                );
            }

            // Type parameters
            let rpc_tps = rpc_struct
                .get("typeParameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let byte_tps = byte_struct
                .get("type_params")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rpc_tps.len() != byte_tps.len() {
                struct_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/structs/{sname}/type_params"),
                    format!(
                        "type param arity mismatch (rpc={} bytecode={})",
                        rpc_tps.len(),
                        byte_tps.len()
                    ),
                    rpc_struct.get("typeParameters").cloned(),
                    byte_struct.get("type_params").cloned(),
                );
            } else {
                for (i, (rtp, btp)) in rpc_tps.iter().zip(byte_tps.iter()).enumerate() {
                    let rpc_constraints = rtp
                        .get("constraints")
                        .map(abilities_from_value)
                        .unwrap_or_default();
                    let rpc_is_phantom = rtp
                        .get("isPhantom")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let byte_constraints = btp
                        .get("constraints")
                        .map(abilities_from_value)
                        .unwrap_or_default();
                    let byte_is_phantom = btp
                        .get("is_phantom")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if rpc_constraints != byte_constraints || rpc_is_phantom != byte_is_phantom {
                        struct_mismatches += 1;
                        push_mismatch(
                            format!("modules/{module_name}/structs/{sname}/type_params[{i}]"),
                            "struct type param mismatch".to_string(),
                            Some(
                                serde_json::json!({"constraints": rpc_constraints, "is_phantom": rpc_is_phantom}),
                            ),
                            Some(
                                serde_json::json!({"constraints": byte_constraints, "is_phantom": byte_is_phantom}),
                            ),
                        );
                    }
                }
            }

            // Fields
            let rpc_fields = rpc_struct
                .get("fields")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let byte_fields = byte_struct
                .get("fields")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let byte_is_native = byte_struct
                .get("is_native")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if byte_is_native && rpc_fields.is_empty() {
                // ok: native struct may have no declared fields in normalized view.
            } else if rpc_fields.len() != byte_fields.len() {
                struct_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/structs/{sname}/fields"),
                    format!(
                        "field count mismatch (rpc={} bytecode={})",
                        rpc_fields.len(),
                        byte_fields.len()
                    ),
                    rpc_struct.get("fields").cloned(),
                    byte_struct.get("fields").cloned(),
                );
            } else {
                for (i, (rf, bf)) in rpc_fields.iter().zip(byte_fields.iter()).enumerate() {
                    let rname = rf.get("name").and_then(Value::as_str).unwrap_or("");
                    let bname = bf.get("name").and_then(Value::as_str).unwrap_or("");
                    if rname != bname {
                        struct_mismatches += 1;
                        push_mismatch(
                            format!("modules/{module_name}/structs/{sname}/fields[{i}]/name"),
                            "field name mismatch".to_string(),
                            rf.get("name").cloned(),
                            bf.get("name").cloned(),
                        );
                        continue;
                    }
                    let rty = rf.get("type").unwrap_or(&Value::Null);
                    let bty = bf.get("type").unwrap_or(&Value::Null);
                    let rcanon = rpc_type_to_canonical_json(rty);
                    let bcanon = bytecode_type_to_canonical_json(bty);
                    match (rcanon, bcanon) {
                        (Ok(mut r), Ok(mut b)) => {
                            canonicalize_json_value(&mut r);
                            canonicalize_json_value(&mut b);
                            if r != b {
                                struct_mismatches += 1;
                                push_mismatch(
                                    format!(
                                        "modules/{module_name}/structs/{sname}/fields[{i}]/type"
                                    ),
                                    "field type mismatch".to_string(),
                                    Some(r),
                                    Some(b),
                                );
                            }
                        }
                        (Err(e), _) => {
                            struct_mismatches += 1;
                            push_mismatch(
                                format!("modules/{module_name}/structs/{sname}/fields[{i}]/type"),
                                format!("rpc type parse error: {:#}", e),
                                Some(rty.clone()),
                                None,
                            );
                        }
                        (_, Err(e)) => {
                            struct_mismatches += 1;
                            push_mismatch(
                                format!("modules/{module_name}/structs/{sname}/fields[{i}]/type"),
                                format!("bytecode type parse error: {:#}", e),
                                None,
                                Some(bty.clone()),
                            );
                        }
                    }
                }
            }
        }

        // Functions: compare only RPC exposedFunctions (RPC doesn't show private non-entry).
        let rpc_funcs = get_object(rpc_mod, &["exposedFunctions", "exposed_functions"])
            .cloned()
            .unwrap_or_default();
        let byte_funcs = get_object(byte_mod, &["functions"])
            .cloned()
            .unwrap_or_default();

        let mut rpc_func_names: Vec<String> = rpc_funcs.keys().cloned().collect();
        rpc_func_names.sort();

        for fname in &rpc_func_names {
            let Some(rpc_fun) = rpc_funcs.get(fname) else {
                continue;
            };
            let Some(byte_fun) = byte_funcs.get(fname) else {
                function_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/functions/{fname}"),
                    "function missing in bytecode".to_string(),
                    Some(rpc_fun.clone()),
                    None,
                );
                continue;
            };
            functions_compared += 1;

            let rpc_vis = rpc_fun
                .get("visibility")
                .and_then(rpc_visibility_to_string)
                .unwrap_or_else(|| "<unknown>".to_string());
            let byte_vis = byte_fun
                .get("visibility")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
                .to_string();
            if rpc_vis != byte_vis {
                function_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/functions/{fname}/visibility"),
                    "visibility mismatch".to_string(),
                    rpc_fun.get("visibility").cloned(),
                    byte_fun.get("visibility").cloned(),
                );
            }

            let rpc_entry = rpc_fun
                .get("isEntry")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let byte_entry = byte_fun
                .get("is_entry")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if rpc_entry != byte_entry {
                function_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/functions/{fname}/is_entry"),
                    "entry mismatch".to_string(),
                    rpc_fun.get("isEntry").cloned(),
                    byte_fun.get("is_entry").cloned(),
                );
            }

            // Type parameters (constraints only; RPC doesn't include phantom for functions).
            let rpc_tps = rpc_fun
                .get("typeParameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let byte_tps = byte_fun
                .get("type_params")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rpc_tps.len() != byte_tps.len() {
                function_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/functions/{fname}/type_params"),
                    format!(
                        "type param arity mismatch (rpc={} bytecode={})",
                        rpc_tps.len(),
                        byte_tps.len()
                    ),
                    rpc_fun.get("typeParameters").cloned(),
                    byte_fun.get("type_params").cloned(),
                );
            } else {
                for (i, (rtp, btp)) in rpc_tps.iter().zip(byte_tps.iter()).enumerate() {
                    let rpc_constraints = abilities_from_value(rtp);
                    let byte_constraints = btp
                        .get("constraints")
                        .map(abilities_from_value)
                        .unwrap_or_default();
                    if rpc_constraints != byte_constraints {
                        function_mismatches += 1;
                        push_mismatch(
                            format!("modules/{module_name}/functions/{fname}/type_params[{i}]"),
                            "function type param constraints mismatch".to_string(),
                            Some(serde_json::json!({"constraints": rpc_constraints})),
                            Some(serde_json::json!({"constraints": byte_constraints})),
                        );
                    }
                }
            }

            // Params and returns
            let rpc_params = rpc_fun
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let byte_params = byte_fun
                .get("params")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rpc_params.len() != byte_params.len() {
                function_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/functions/{fname}/params"),
                    format!(
                        "param count mismatch (rpc={} bytecode={})",
                        rpc_params.len(),
                        byte_params.len()
                    ),
                    rpc_fun.get("parameters").cloned(),
                    byte_fun.get("params").cloned(),
                );
            } else {
                for (i, (rp, bp)) in rpc_params.iter().zip(byte_params.iter()).enumerate() {
                    let rcanon = rpc_type_to_canonical_json(rp);
                    let bcanon = bytecode_type_to_canonical_json(bp);
                    match (rcanon, bcanon) {
                        (Ok(mut r), Ok(mut b)) => {
                            canonicalize_json_value(&mut r);
                            canonicalize_json_value(&mut b);
                            if r != b {
                                function_mismatches += 1;
                                push_mismatch(
                                    format!("modules/{module_name}/functions/{fname}/params[{i}]"),
                                    "param type mismatch".to_string(),
                                    Some(r),
                                    Some(b),
                                );
                            }
                        }
                        (Err(e), _) => {
                            function_mismatches += 1;
                            push_mismatch(
                                format!("modules/{module_name}/functions/{fname}/params[{i}]"),
                                format!("rpc type parse error: {:#}", e),
                                Some(rp.clone()),
                                None,
                            );
                        }
                        (_, Err(e)) => {
                            function_mismatches += 1;
                            push_mismatch(
                                format!("modules/{module_name}/functions/{fname}/params[{i}]"),
                                format!("bytecode type parse error: {:#}", e),
                                None,
                                Some(bp.clone()),
                            );
                        }
                    }
                }
            }

            let rpc_rets = rpc_fun
                .get("return")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let byte_rets = byte_fun
                .get("returns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rpc_rets.len() != byte_rets.len() {
                function_mismatches += 1;
                push_mismatch(
                    format!("modules/{module_name}/functions/{fname}/returns"),
                    format!(
                        "return count mismatch (rpc={} bytecode={})",
                        rpc_rets.len(),
                        byte_rets.len()
                    ),
                    rpc_fun.get("return").cloned(),
                    byte_fun.get("returns").cloned(),
                );
            } else {
                for (i, (rr, br)) in rpc_rets.iter().zip(byte_rets.iter()).enumerate() {
                    let rcanon = rpc_type_to_canonical_json(rr);
                    let bcanon = bytecode_type_to_canonical_json(br);
                    match (rcanon, bcanon) {
                        (Ok(mut r), Ok(mut b)) => {
                            canonicalize_json_value(&mut r);
                            canonicalize_json_value(&mut b);
                            if r != b {
                                function_mismatches += 1;
                                push_mismatch(
                                    format!("modules/{module_name}/functions/{fname}/returns[{i}]"),
                                    "return type mismatch".to_string(),
                                    Some(r),
                                    Some(b),
                                );
                            }
                        }
                        (Err(e), _) => {
                            function_mismatches += 1;
                            push_mismatch(
                                format!("modules/{module_name}/functions/{fname}/returns[{i}]"),
                                format!("rpc type parse error: {:#}", e),
                                Some(rr.clone()),
                                None,
                            );
                        }
                        (_, Err(e)) => {
                            function_mismatches += 1;
                            push_mismatch(
                                format!("modules/{module_name}/functions/{fname}/returns[{i}]"),
                                format!("bytecode type parse error: {:#}", e),
                                None,
                                Some(br.clone()),
                            );
                        }
                    }
                }
            }
        }
    }

    (
        InterfaceCompareSummary {
            modules_compared,
            modules_missing_in_bytecode: modules_missing_in_bytecode.len(),
            modules_extra_in_bytecode: modules_extra_in_bytecode.len(),
            structs_compared,
            struct_mismatches,
            functions_compared,
            function_mismatches,
            mismatches_total: mismatch_count_total,
        },
        mismatches,
    )
}

fn get_object<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a serde_json::Map<String, Value>> {
    for key in keys {
        if let Some(obj) = value.get(*key).and_then(Value::as_object) {
            return Some(obj);
        }
    }
    None
}

fn iter_ability_strings<'a>(value: &'a Value) -> Box<dyn Iterator<Item = &'a str> + 'a> {
    if let Some(arr) = value.as_array() {
        return Box::new(arr.iter().filter_map(|v| v.as_str()));
    }
    if let Some(obj) = value.as_object() {
        if let Some(arr) = obj.get("abilities").and_then(Value::as_array) {
            return Box::new(arr.iter().filter_map(|v| v.as_str()));
        }
    }
    Box::new(std::iter::empty())
}

fn struct_has_key(struct_def: &Value) -> bool {
    let Some(abilities_value) = struct_def.get("abilities") else {
        return false;
    };

    iter_ability_strings(abilities_value)
        .map(|s| s.to_ascii_lowercase())
        .any(|s| s == "key")
}

fn extract_sanity_counts(modules_value: &Value) -> SanityCounts {
    let mut structs = 0usize;
    let mut functions = 0usize;
    let mut key_structs = 0usize;

    let modules_obj = modules_value.as_object();
    let modules = modules_obj.map(|o| o.len()).unwrap_or(0);

    if let Some(modules_obj) = modules_obj {
        for (_module_name, module_def) in modules_obj {
            if let Some(structs_obj) = get_object(module_def, &["structs"]) {
                structs += structs_obj.len();
                for (_struct_name, struct_def) in structs_obj {
                    if struct_has_key(struct_def) {
                        key_structs += 1;
                    }
                }
            }

            if let Some(funcs_obj) = get_object(
                module_def,
                &["functions", "exposedFunctions", "exposed_functions"],
            ) {
                functions += funcs_obj.len();
            }
        }
    }

    SanityCounts {
        modules,
        structs,
        functions,
        key_structs,
    }
}

fn write_canonical_json(path: &Path, value: &Value) -> Result<()> {
    if path.as_os_str() == "-" {
        let stdout = io::stdout();
        let mut writer = BufWriter::new(stdout.lock());
        if let Err(e) = serde_json::to_writer_pretty(&mut writer, value) {
            if e.is_io() && e.io_error_kind() == Some(io::ErrorKind::BrokenPipe) {
                return Ok(());
            }
            return Err(e).context("serialize JSON");
        }
        writer.write_all(b"\n").ok();
        return Ok(());
    }

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).context("serialize JSON")?;
    writer.write_all(b"\n").ok();
    Ok(())
}

fn check_stability(interface_value: &Value) -> Result<()> {
    let s1 = serde_json::to_string_pretty(interface_value).context("serialize JSON")?;
    let mut v2: Value = serde_json::from_str(&s1).context("parse JSON")?;
    canonicalize_json_value(&mut v2);
    let s2 = serde_json::to_string_pretty(&v2).context("serialize JSON")?;
    if s1 != s2 {
        return Err(anyhow!("canonical JSON is not stable under roundtrip"));
    }
    Ok(())
}

fn collect_package_ids(args: &Args) -> Result<Vec<String>> {
    let mut ids = BTreeSet::<String>::new();

    for id in &args.package_id {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            ids.insert(trimmed.to_string());
        }
    }

    if let Some(path) = args.package_ids_file.as_ref() {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            ids.insert(line.to_string());
        }
    }

    if let Some(path) = args.mvr_catalog.as_ref() {
        let catalog_text =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let catalog: Value = serde_json::from_str(&catalog_text)
            .with_context(|| format!("parse {}", path.display()))?;
        let Some(names) = catalog.get("names").and_then(Value::as_array) else {
            return Err(anyhow!("mvr catalog missing 'names' array"));
        };

        let field = match args.mvr_network {
            MvrNetwork::Mainnet => "mainnet_package_info_id",
            MvrNetwork::Testnet => "testnet_package_info_id",
        };

        for item in names {
            if let Some(id) = item.get(field).and_then(Value::as_str) {
                let trimmed = id.trim();
                if !trimmed.is_empty() {
                    ids.insert(trimmed.to_string());
                }
            }
        }
    }

    let mut ids: Vec<String> = ids.into_iter().collect();
    if let Some(max) = args.max_packages {
        ids.truncate(max);
    }
    Ok(ids)
}

async fn resolve_package_address_from_package_info(
    client: Arc<sui_sdk::SuiClient>,
    package_info_id: ObjectID,
    retry: RetryConfig,
) -> Result<ObjectID> {
    let options = sui_sdk::rpc_types::SuiObjectDataOptions::new()
        .with_type()
        .with_content();

    let resp = with_retries(retry, || {
        let client = Arc::clone(&client);
        let options = options.clone();
        async move {
            client
                .read_api()
                .get_object_with_options(package_info_id, options)
                .await
                .with_context(|| format!("fetch object {}", package_info_id))
        }
    })
    .await?;

    let value = serde_json::to_value(&resp).context("serialize object response")?;
    let package_address = value
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.get("fields"))
        .and_then(|f| f.get("package_address"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!(
                "object {} missing content.fields.package_address",
                package_info_id
            )
        })?;

    ObjectID::from_str(package_address)
        .map_err(|e| anyhow!("invalid package_address {}: {}", package_address, e))
}

async fn fetch_bcs_module_names(
    client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<Vec<String>> {
    let options = sui_sdk::rpc_types::SuiObjectDataOptions::new().with_bcs();
    let resp = with_retries(retry, || {
        let client = Arc::clone(&client);
        let options = options.clone();
        async move {
            client
                .read_api()
                .get_object_with_options(package_id, options)
                .await
                .with_context(|| format!("fetch package bcs {}", package_id))
        }
    })
    .await?;

    let value = serde_json::to_value(&resp).context("serialize object response")?;
    let module_map = value
        .get("data")
        .and_then(|d| d.get("bcs"))
        .and_then(|b| b.get("moduleMap"))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("object {} missing data.bcs.moduleMap", package_id))?;

    let mut names: Vec<String> = module_map.keys().cloned().collect();
    names.sort();
    Ok(names)
}

async fn fetch_bcs_module_map_bytes(
    client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<Vec<(String, Vec<u8>)>> {
    let options = sui_sdk::rpc_types::SuiObjectDataOptions::new().with_bcs();
    let resp = with_retries(retry, || {
        let client = Arc::clone(&client);
        let options = options.clone();
        async move {
            client
                .read_api()
                .get_object_with_options(package_id, options)
                .await
                .with_context(|| format!("fetch package bcs {}", package_id))
        }
    })
    .await?;

    let value = serde_json::to_value(&resp).context("serialize object response")?;
    let module_map = value
        .get("data")
        .and_then(|d| d.get("bcs"))
        .and_then(|b| b.get("moduleMap"))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("object {} missing data.bcs.moduleMap", package_id))?;

    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(module_map.len());
    for (name, v) in module_map {
        let bytes: Vec<u8> = match v {
            Value::String(s) => base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .with_context(|| format!("base64 decode moduleMap[{}] for {}", name, package_id))?,
            Value::Array(arr) => {
                let mut b = Vec::with_capacity(arr.len());
                for x in arr {
                    let n = x
                        .as_u64()
                        .ok_or_else(|| anyhow!("moduleMap[{}] contains non-u64 byte", name))?;
                    if n > 255 {
                        return Err(anyhow!(
                            "moduleMap[{}] contains out-of-range byte {}",
                            name,
                            n
                        ));
                    }
                    b.push(n as u8);
                }
                b
            }
            _ => {
                return Err(anyhow!(
                    "moduleMap[{}] unexpected JSON type (expected string/array)",
                    name
                ))
            }
        };
        out.push((name.clone(), bytes));
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn bytecode_module_check(
    normalized_module_names: &[String],
    bcs_module_names: &[String],
) -> BytecodeModuleCheck {
    let normalized_set: HashSet<&str> =
        normalized_module_names.iter().map(|s| s.as_str()).collect();
    let bcs_set: HashSet<&str> = bcs_module_names.iter().map(|s| s.as_str()).collect();

    let mut missing_in_bcs: Vec<String> = normalized_module_names
        .iter()
        .filter(|name| !bcs_set.contains(name.as_str()))
        .cloned()
        .collect();
    missing_in_bcs.sort();

    let mut extra_in_bcs: Vec<String> = bcs_module_names
        .iter()
        .filter(|name| !normalized_set.contains(name.as_str()))
        .cloned()
        .collect();
    extra_in_bcs.sort();

    BytecodeModuleCheck {
        normalized_modules: normalized_module_names.len(),
        bcs_modules: bcs_module_names.len(),
        missing_in_bcs,
        extra_in_bcs,
    }
}

fn module_set_diff(left: &[String], right: &[String]) -> ModuleSetDiff {
    let left_set: HashSet<&str> = left.iter().map(|s| s.as_str()).collect();
    let right_set: HashSet<&str> = right.iter().map(|s| s.as_str()).collect();

    let mut missing_in_right: Vec<String> = left
        .iter()
        .filter(|name| !right_set.contains(name.as_str()))
        .cloned()
        .collect();
    missing_in_right.sort();

    let mut extra_in_right: Vec<String> = right
        .iter()
        .filter(|name| !left_set.contains(name.as_str()))
        .cloned()
        .collect();
    extra_in_right.sort();

    ModuleSetDiff {
        left_count: left.len(),
        right_count: right.len(),
        missing_in_right,
        extra_in_right,
    }
}

async fn build_interface_value_for_package(
    client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<(Vec<String>, Value)> {
    let modules = with_retries(retry, || {
        let client = Arc::clone(&client);
        async move {
            client
                .read_api()
                .get_normalized_move_modules_by_package(package_id)
                .await
                .with_context(|| format!("fetch normalized modules for {}", package_id))
        }
    })
    .await?;

    let mut module_names: Vec<String> = modules.keys().cloned().collect();
    module_names.sort();

    let mut modules_value =
        serde_json::to_value(&modules).context("serialize normalized modules")?;
    canonicalize_json_value(&mut modules_value);

    let interface = PackageInterfaceJson {
        schema_version: 1,
        package_id: package_id.to_string(),
        module_names: module_names.clone(),
        modules: modules_value,
    };

    let mut interface_value = serde_json::to_value(interface).context("build interface JSON")?;
    canonicalize_json_value(&mut interface_value);

    Ok((module_names, interface_value))
}

fn ability_set_has_key(set: &AbilitySet) -> bool {
    set.has_ability(Ability::Key)
}

fn analyze_compiled_module(module: &CompiledModule) -> LocalBytecodeCounts {
    let mut structs = 0usize;
    let mut key_structs = 0usize;

    let mut functions_total = 0usize;
    let mut functions_public = 0usize;
    let mut functions_friend = 0usize;
    let mut functions_private = 0usize;
    let mut functions_native = 0usize;

    let mut entry_functions = 0usize;
    let mut public_entry_functions = 0usize;
    let mut friend_entry_functions = 0usize;
    let mut private_entry_functions = 0usize;

    structs += module.struct_defs().len();

    for def in module.struct_defs() {
        let handle = module.datatype_handle_at(def.struct_handle);
        if ability_set_has_key(&handle.abilities) {
            key_structs += 1;
        }

        // Ensure we traverse field info once (some older modules may be native).
        match &def.field_information {
            StructFieldInformation::Declared(_fields) => {}
            StructFieldInformation::Native => {}
        }
    }

    functions_total += module.function_defs().len();
    for def in module.function_defs() {
        if def.code.is_none() {
            functions_native += 1;
        }

        match def.visibility {
            Visibility::Public => functions_public += 1,
            Visibility::Friend => functions_friend += 1,
            Visibility::Private => functions_private += 1,
        }

        if def.is_entry {
            entry_functions += 1;
            match def.visibility {
                Visibility::Public => public_entry_functions += 1,
                Visibility::Friend => friend_entry_functions += 1,
                Visibility::Private => private_entry_functions += 1,
            }
        }
    }

    LocalBytecodeCounts {
        modules: 1,
        structs,
        functions_total,
        functions_public,
        functions_friend,
        functions_private,
        functions_native,
        entry_functions,
        public_entry_functions,
        friend_entry_functions,
        private_entry_functions,
        key_structs,
    }
}

fn compiled_module_name(module: &CompiledModule) -> String {
    module.self_id().name().to_string()
}

fn read_package_id_from_metadata(package_dir: &Path) -> Result<String> {
    let metadata_path = package_dir.join("metadata.json");
    let metadata_text = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read {}", metadata_path.display()))?;
    let metadata: Value = serde_json::from_str(&metadata_text)
        .with_context(|| format!("parse {}", metadata_path.display()))?;
    Ok(metadata
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("metadata.json missing 'id'"))?
        .to_string())
}

fn fnv1a64(seed: u64, s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64 ^ seed;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn analyze_local_bytecode_package(
    package_dir: &Path,
) -> Result<(Vec<String>, LocalBytecodeCounts)> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut module_names: Vec<String> = Vec::new();
    let mut counts = LocalBytecodeCounts {
        modules: 0,
        structs: 0,
        functions_total: 0,
        functions_public: 0,
        functions_friend: 0,
        functions_private: 0,
        functions_native: 0,
        entry_functions: 0,
        public_entry_functions: 0,
        friend_entry_functions: 0,
        private_entry_functions: 0,
        key_structs: 0,
    };

    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let module = CompiledModule::deserialize_with_defaults(&bytes)
            .map_err(|e| anyhow!("deserialize {}: {}", path.display(), e))?;

        module_names.push(compiled_module_name(&module));
        let module_counts = analyze_compiled_module(&module);
        counts.modules += module_counts.modules;
        counts.structs += module_counts.structs;
        counts.functions_total += module_counts.functions_total;
        counts.functions_public += module_counts.functions_public;
        counts.functions_friend += module_counts.functions_friend;
        counts.functions_private += module_counts.functions_private;
        counts.functions_native += module_counts.functions_native;
        counts.entry_functions += module_counts.entry_functions;
        counts.public_entry_functions += module_counts.public_entry_functions;
        counts.friend_entry_functions += module_counts.friend_entry_functions;
        counts.private_entry_functions += module_counts.private_entry_functions;
        counts.key_structs += module_counts.key_structs;
    }

    Ok((module_names, counts))
}

fn list_local_module_names_only(package_dir: &Path) -> Result<Vec<String>> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad filename {}", path.display()))?
            .to_string();
        names.push(name);
    }
    if names.is_empty() {
        return Err(anyhow!("no .mv files found in {}", bytecode_dir.display()));
    }
    Ok(names)
}

fn read_local_compiled_modules(package_dir: &Path) -> Result<Vec<CompiledModule>> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    let mut modules: Vec<CompiledModule> = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let module = CompiledModule::deserialize_with_defaults(&bytes)
            .with_context(|| format!("deserialize {}", path.display()))?;
        modules.push(module);
    }
    if modules.is_empty() {
        return Err(anyhow!("no .mv files found in {}", bytecode_dir.display()));
    }
    Ok(modules)
}

fn read_local_bcs_module_names(package_dir: &Path) -> Result<Vec<String>> {
    let bcs_path = package_dir.join("bcs.json");
    let text =
        fs::read_to_string(&bcs_path).with_context(|| format!("read {}", bcs_path.display()))?;
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", bcs_path.display()))?;
    let module_map = v
        .get("moduleMap")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("bcs.json missing moduleMap"))?;
    let mut names: Vec<String> = module_map.keys().cloned().collect();
    names.sort();
    Ok(names)
}

fn decode_module_map_entry_bytes(module: &str, v: &Value) -> Result<Vec<u8>> {
    match v {
        Value::String(s) => base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .with_context(|| format!("base64 decode moduleMap[{}]", module)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for x in arr {
                let n = x
                    .as_u64()
                    .ok_or_else(|| anyhow!("moduleMap[{}] contains non-u64 byte", module))?;
                if n > 255 {
                    return Err(anyhow!(
                        "moduleMap[{}] contains out-of-range byte {}",
                        module,
                        n
                    ));
                }
                out.push(n as u8);
            }
            Ok(out)
        }
        _ => Err(anyhow!(
            "moduleMap[{}] unexpected JSON type (expected string/array)",
            module
        )),
    }
}

fn read_local_bcs_module_map_bytes_info(package_dir: &Path) -> Result<BTreeMap<String, BytesInfo>> {
    let bcs_path = package_dir.join("bcs.json");
    let text =
        fs::read_to_string(&bcs_path).with_context(|| format!("read {}", bcs_path.display()))?;
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", bcs_path.display()))?;
    let module_map = v
        .get("moduleMap")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("bcs.json missing moduleMap"))?;

    let mut out = BTreeMap::<String, BytesInfo>::new();
    for (name, entry) in module_map {
        let bytes = decode_module_map_entry_bytes(name, entry)
            .with_context(|| format!("decode bcs.json moduleMap[{}]", name))?;
        out.insert(name.clone(), bytes_info(&bytes));
    }
    Ok(out)
}

fn read_local_mv_bytes_info_map(package_dir: &Path) -> Result<BTreeMap<String, BytesInfo>> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    let mut out = BTreeMap::<String, BytesInfo>::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad filename {}", path.display()))?
            .to_string();
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        out.insert(name, bytes_info(&bytes));
    }
    if out.is_empty() {
        return Err(anyhow!("no .mv files found in {}", bytecode_dir.display()));
    }
    Ok(out)
}

fn local_bytes_check_for_package(
    package_dir: &Path,
    max_mismatches: usize,
) -> Result<LocalBytesCheck> {
    let mv = read_local_mv_bytes_info_map(package_dir)?;
    let bcs = read_local_bcs_module_map_bytes_info(package_dir)?;

    let mut all = BTreeSet::<String>::new();
    all.extend(mv.keys().cloned());
    all.extend(bcs.keys().cloned());

    let mut missing_in_bcs = Vec::<String>::new();
    let mut missing_in_mv = Vec::<String>::new();
    let mut mismatches_sample = Vec::<ModuleBytesMismatch>::new();
    let mut mismatches_total = 0usize;
    let mut exact_match_modules = 0usize;

    for module in all {
        let mv_info = mv.get(&module).copied();
        let bcs_info = bcs.get(&module).copied();

        match (mv_info, bcs_info) {
            (None, Some(bcs_info)) => {
                mismatches_total += 1;
                missing_in_mv.push(module.clone());
                if mismatches_sample.len() < max_mismatches {
                    mismatches_sample.push(ModuleBytesMismatch {
                        module,
                        reason: "missing_in_mv".to_string(),
                        mv_len: None,
                        bcs_len: Some(bcs_info.len),
                        mv_sha256: None,
                        bcs_sha256: Some(bytes_info_sha256_hex(bcs_info)),
                    });
                }
            }
            (Some(mv_info), None) => {
                mismatches_total += 1;
                missing_in_bcs.push(module.clone());
                if mismatches_sample.len() < max_mismatches {
                    mismatches_sample.push(ModuleBytesMismatch {
                        module,
                        reason: "missing_in_bcs".to_string(),
                        mv_len: Some(mv_info.len),
                        bcs_len: None,
                        mv_sha256: Some(bytes_info_sha256_hex(mv_info)),
                        bcs_sha256: None,
                    });
                }
            }
            (Some(mv_info), Some(bcs_info)) => {
                if mv_info.len == bcs_info.len && mv_info.sha256 == bcs_info.sha256 {
                    exact_match_modules += 1;
                    continue;
                }

                mismatches_total += 1;
                let reason = if mv_info.len != bcs_info.len {
                    "len_mismatch"
                } else {
                    "sha256_mismatch"
                };
                if mismatches_sample.len() < max_mismatches {
                    mismatches_sample.push(ModuleBytesMismatch {
                        module,
                        reason: reason.to_string(),
                        mv_len: Some(mv_info.len),
                        bcs_len: Some(bcs_info.len),
                        mv_sha256: Some(bytes_info_sha256_hex(mv_info)),
                        bcs_sha256: Some(bytes_info_sha256_hex(bcs_info)),
                    });
                }
            }
            (None, None) => {}
        }
    }

    Ok(LocalBytesCheck {
        mv_modules: mv.len(),
        bcs_modules: bcs.len(),
        exact_match_modules,
        mismatches_total,
        missing_in_bcs,
        missing_in_mv,
        mismatches_sample,
    })
}

fn collect_corpus_package_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut package_dirs: Vec<PathBuf> = Vec::new();

    let mut prefixes: Vec<_> = fs::read_dir(root)
        .with_context(|| format!("read {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", root.display()))?;
    prefixes.sort_by_key(|e| e.path());

    for prefix in prefixes {
        let prefix_path = prefix.path();
        if !prefix_path.is_dir() {
            continue;
        }

        let mut entries: Vec<_> = fs::read_dir(&prefix_path)
            .with_context(|| format!("read {}", prefix_path.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("list {}", prefix_path.display()))?;
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                package_dirs.push(path);
                continue;
            }
            if entry
                .file_type()
                .ok()
                .map(|t| t.is_symlink())
                .unwrap_or(false)
            {
                package_dirs.push(path);
            }
        }
    }

    Ok(package_dirs)
}

async fn run_corpus(args: &Args, client: Arc<sui_sdk::SuiClient>) -> Result<()> {
    let Some(root) = args.bytecode_corpus_root.as_ref() else {
        return Err(anyhow!("corpus mode requires --bytecode-corpus-root"));
    };
    let Some(out_dir) = args.out_dir.as_ref() else {
        return Err(anyhow!("corpus mode requires --out-dir"));
    };

    if args.emit_bytecode_json.is_some() {
        return Err(anyhow!(
            "--emit-bytecode-json is only valid for single-package mode"
        ));
    }
    if args.compare_bytecode_rpc || args.emit_compare_report.is_some() {
        return Err(anyhow!(
            "--compare-bytecode-rpc/--emit-compare-report are only valid for single-package mode"
        ));
    }
    if args.corpus_interface_compare && !args.corpus_rpc_compare {
        return Err(anyhow!(
            "--corpus-interface-compare requires --corpus-rpc-compare"
        ));
    }
    if args.corpus_module_names_only && (args.corpus_rpc_compare || args.corpus_interface_compare) {
        return Err(anyhow!(
            "--corpus-module-names-only is not compatible with --corpus-rpc-compare/--corpus-interface-compare"
        ));
    }

    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let run_started_at = now_unix_seconds();
    let argv: Vec<String> = std::env::args().collect();
    let sui_packages_git = git_metadata_for_path(root);

    let report_path = out_dir.join("corpus_report.jsonl");
    let problems_path = out_dir.join("problems.jsonl");
    let summary_path = out_dir.join("corpus_summary.json");
    let run_metadata_path = out_dir.join("run_metadata.json");
    let submission_summary_path = args.emit_submission_summary.clone();

    let index_path = args
        .corpus_index_jsonl
        .clone()
        .unwrap_or_else(|| out_dir.join("index.jsonl"));

    let sample_ids_path = args
        .corpus_sample_ids_out
        .clone()
        .unwrap_or_else(|| out_dir.join("sample_ids.txt"));

    // Discover package dirs and build a stable index (dedup by package_id).
    let package_dirs = collect_corpus_package_dirs(root)?;
    let mut seen = HashSet::<String>::new();
    let mut targets: Vec<(String, PathBuf)> = Vec::new();
    for dir in package_dirs {
        let id = match read_package_id_from_metadata(&dir) {
            Ok(id) => id,
            Err(_) => continue,
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        targets.push((id, dir));
    }
    targets.sort_by(|a, b| a.0.cmp(&b.0));

    // Write index.jsonl
    {
        let file = File::create(&index_path)
            .with_context(|| format!("create {}", index_path.display()))?;
        let mut writer = BufWriter::new(file);
        for (package_id, package_dir) in &targets {
            let row = CorpusIndexRow {
                package_id: package_id.clone(),
                package_dir: package_dir.display().to_string(),
            };
            serde_json::to_writer(&mut writer, &row).context("write index JSONL")?;
            writer.write_all(b"\n").ok();
        }
    }

    // Optionally restrict to ids from a file.
    if let Some(path) = args.corpus_ids_file.as_ref() {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let mut wanted = HashSet::<String>::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            wanted.insert(line.to_string());
        }

        let before = targets.len();
        let found_set: HashSet<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
        let mut missing: Vec<String> = wanted
            .iter()
            .filter(|id| !found_set.contains(id.as_str()))
            .cloned()
            .collect();
        missing.sort();

        targets.retain(|(id, _)| wanted.contains(id));
        if targets.is_empty() {
            return Err(anyhow!(
                "--corpus-ids-file selected 0 packages (wanted={}, missing={})",
                wanted.len(),
                missing.len()
            ));
        }

        if !missing.is_empty() {
            eprintln!(
                "corpus ids filter: selected {}/{} ({} missing; first missing: {})",
                targets.len(),
                before,
                missing.len(),
                missing.first().cloned().unwrap_or_default()
            );
        } else {
            eprintln!("corpus ids filter: selected {}/{}", targets.len(), before);
        }
    }

    // Apply sampling or max limit.
    if let Some(n) = args.corpus_sample {
        let seed = args.corpus_seed;
        let mut scored: Vec<(u64, String, PathBuf)> = targets
            .into_iter()
            .map(|(id, dir)| (fnv1a64(seed, &id), id, dir))
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(n);
        targets = scored.into_iter().map(|(_h, id, dir)| (id, dir)).collect();
        targets.sort_by(|a, b| a.0.cmp(&b.0));

        let mut ids_text = String::new();
        for (id, _) in &targets {
            ids_text.push_str(id);
            ids_text.push('\n');
        }
        fs::write(&sample_ids_path, ids_text)
            .with_context(|| format!("write {}", sample_ids_path.display()))?;
    } else if let Some(max) = args.max_packages {
        targets.truncate(max);
    }

    let retry = RetryConfig::from_args(args);
    let concurrency = args.concurrency.max(1);
    if args.corpus_rpc_compare && concurrency > 1 {
        eprintln!(
            "note: --corpus-rpc-compare may hit rate limits; consider --concurrency 1 (current={})",
            concurrency
        );
    }
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut join_set: tokio::task::JoinSet<Result<CorpusRow>> = tokio::task::JoinSet::new();

    for (package_id_str, package_dir) in targets {
        let client = Arc::clone(&client);
        let semaphore = Arc::clone(&semaphore);
        let retry_cfg = retry;
        let do_rpc = args.corpus_rpc_compare;
        let do_interface_compare = args.corpus_interface_compare;
        let module_names_only = args.corpus_module_names_only;
        let do_local_bytes_check = args.corpus_local_bytes_check;
        let local_bytes_max_mismatches = args.corpus_local_bytes_check_max_mismatches;
        let corpus_max_mismatches = args.corpus_interface_compare_max_mismatches;
        let corpus_include_values = args.corpus_interface_compare_include_values;
        let compare_opts = InterfaceCompareOptions {
            max_mismatches: corpus_max_mismatches,
            include_values: corpus_include_values,
        };

        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| anyhow!("semaphore closed: {}", e))?;

            let dir_str = package_dir.display().to_string();

            let (local_module_names, local_counts) = if module_names_only {
                match list_local_module_names_only(&package_dir) {
                    Ok(names) => {
                        let counts = LocalBytecodeCounts {
                            modules: names.len(),
                            structs: 0,
                            functions_total: 0,
                            functions_public: 0,
                            functions_friend: 0,
                            functions_private: 0,
                            functions_native: 0,
                            entry_functions: 0,
                            public_entry_functions: 0,
                            friend_entry_functions: 0,
                            private_entry_functions: 0,
                            key_structs: 0,
                        };
                        (names, counts)
                    }
                    Err(e) => {
                        return Ok(CorpusRow {
                            package_id: package_id_str,
                            package_dir: dir_str,
                            local: LocalBytecodeCounts {
                                modules: 0,
                                structs: 0,
                                functions_total: 0,
                                functions_public: 0,
                                functions_friend: 0,
                                functions_private: 0,
                                functions_native: 0,
                                entry_functions: 0,
                                public_entry_functions: 0,
                                friend_entry_functions: 0,
                                private_entry_functions: 0,
                                key_structs: 0,
                            },
                            local_vs_bcs: ModuleSetDiff {
                                left_count: 0,
                                right_count: 0,
                                missing_in_right: vec![],
                                extra_in_right: vec![],
                            },
                            local_bytes_check: None,
                            local_bytes_check_error: None,
                            rpc: None,
                            rpc_vs_local: None,
                            interface_compare: None,
                            interface_compare_sample: None,
                            error: Some(format!("local module name scan failed: {:#}", e)),
                        });
                    }
                }
            } else {
                match analyze_local_bytecode_package(&package_dir) {
                    Ok(v) => v,
                    Err(e) => {
                        return Ok(CorpusRow {
                            package_id: package_id_str,
                            package_dir: dir_str,
                            local: LocalBytecodeCounts {
                                modules: 0,
                                structs: 0,
                                functions_total: 0,
                                functions_public: 0,
                                functions_friend: 0,
                                functions_private: 0,
                                functions_native: 0,
                                entry_functions: 0,
                                public_entry_functions: 0,
                                friend_entry_functions: 0,
                                private_entry_functions: 0,
                                key_structs: 0,
                            },
                            local_vs_bcs: ModuleSetDiff {
                                left_count: 0,
                                right_count: 0,
                                missing_in_right: vec![],
                                extra_in_right: vec![],
                            },
                            local_bytes_check: None,
                            local_bytes_check_error: None,
                            rpc: None,
                            rpc_vs_local: None,
                            interface_compare: None,
                            interface_compare_sample: None,
                            error: Some(format!("local analysis failed: {:#}", e)),
                        });
                    }
                }
            };

            let bcs_module_names = match read_local_bcs_module_names(&package_dir) {
                Ok(v) => v,
                Err(e) => {
                    return Ok(CorpusRow {
                        package_id: package_id_str,
                        package_dir: dir_str,
                        local: local_counts,
                        local_vs_bcs: ModuleSetDiff {
                            left_count: 0,
                            right_count: 0,
                            missing_in_right: vec![],
                            extra_in_right: vec![],
                        },
                        local_bytes_check: None,
                        local_bytes_check_error: None,
                        rpc: None,
                        rpc_vs_local: None,
                        interface_compare: None,
                        interface_compare_sample: None,
                        error: Some(format!("read bcs.json failed: {:#}", e)),
                    });
                }
            };

            let local_vs_bcs = module_set_diff(&local_module_names, &bcs_module_names);
            let (local_bytes_check, local_bytes_check_error) = if do_local_bytes_check {
                match local_bytes_check_for_package(&package_dir, local_bytes_max_mismatches) {
                    Ok(check) => (Some(check), None),
                    Err(e) => (None, Some(format!("{:#}", e))),
                }
            } else {
                (None, None)
            };

            if !do_rpc {
                return Ok(CorpusRow {
                    package_id: package_id_str,
                    package_dir: dir_str,
                    local: local_counts,
                    local_vs_bcs,
                    local_bytes_check,
                    local_bytes_check_error,
                    rpc: None,
                    rpc_vs_local: None,
                    interface_compare: None,
                    interface_compare_sample: None,
                    error: None,
                });
            }

            let package_oid = ObjectID::from_str(&package_id_str).map_err(|e| {
                anyhow!("invalid package id from metadata {}: {}", package_id_str, e)
            })?;

            let (rpc_module_names, interface_value) =
                build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg)
                    .await?;
            let rpc_counts =
                extract_sanity_counts(interface_value.get("modules").unwrap_or(&Value::Null));
            let rpc_vs_local = module_set_diff(&rpc_module_names, &local_module_names);

            let (interface_compare, interface_compare_sample) = if do_interface_compare {
                match read_local_compiled_modules(&package_dir).and_then(|compiled| {
                    let (_names, bytecode_value) =
                        build_bytecode_interface_value_from_compiled_modules(
                            &package_id_str,
                            &compiled,
                        )?;
                    let (summary, mismatches) = compare_interface_rpc_vs_bytecode(
                        &package_id_str,
                        &interface_value,
                        &bytecode_value,
                        compare_opts,
                    );
                    Ok((summary, mismatches))
                }) {
                    Ok((summary, mismatches)) => {
                        let sample = if summary.mismatches_total == 0 {
                            None
                        } else {
                            Some(mismatches)
                        };
                        (Some(summary), sample)
                    }
                    Err(e) => {
                        return Ok(CorpusRow {
                            package_id: package_id_str,
                            package_dir: dir_str,
                            local: local_counts,
                            local_vs_bcs,
                            local_bytes_check,
                            local_bytes_check_error,
                            rpc: Some(rpc_counts),
                            rpc_vs_local: Some(rpc_vs_local),
                            interface_compare: None,
                            interface_compare_sample: None,
                            error: Some(format!("interface compare failed: {:#}", e)),
                        });
                    }
                }
            } else {
                (None, None)
            };

            Ok(CorpusRow {
                package_id: package_id_str,
                package_dir: dir_str,
                local: local_counts,
                local_vs_bcs,
                local_bytes_check,
                local_bytes_check_error,
                rpc: Some(rpc_counts),
                rpc_vs_local: Some(rpc_vs_local),
                interface_compare,
                interface_compare_sample,
                error: None,
            })
        });
    }

    let mut rows: Vec<CorpusRow> = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(row)) => rows.push(row),
            Ok(Err(e)) => rows.push(CorpusRow {
                package_id: "<join_error>".to_string(),
                package_dir: "<unknown>".to_string(),
                local: LocalBytecodeCounts {
                    modules: 0,
                    structs: 0,
                    functions_total: 0,
                    functions_public: 0,
                    functions_friend: 0,
                    functions_private: 0,
                    functions_native: 0,
                    entry_functions: 0,
                    public_entry_functions: 0,
                    friend_entry_functions: 0,
                    private_entry_functions: 0,
                    key_structs: 0,
                },
                local_vs_bcs: ModuleSetDiff {
                    left_count: 0,
                    right_count: 0,
                    missing_in_right: vec![],
                    extra_in_right: vec![],
                },
                local_bytes_check: None,
                local_bytes_check_error: None,
                rpc: None,
                rpc_vs_local: None,
                interface_compare: None,
                interface_compare_sample: None,
                error: Some(format!("{:#}", e)),
            }),
            Err(e) => rows.push(CorpusRow {
                package_id: "<panic>".to_string(),
                package_dir: "<unknown>".to_string(),
                local: LocalBytecodeCounts {
                    modules: 0,
                    structs: 0,
                    functions_total: 0,
                    functions_public: 0,
                    functions_friend: 0,
                    functions_private: 0,
                    functions_native: 0,
                    entry_functions: 0,
                    public_entry_functions: 0,
                    friend_entry_functions: 0,
                    private_entry_functions: 0,
                    key_structs: 0,
                },
                local_vs_bcs: ModuleSetDiff {
                    left_count: 0,
                    right_count: 0,
                    missing_in_right: vec![],
                    extra_in_right: vec![],
                },
                local_bytes_check: None,
                local_bytes_check_error: None,
                rpc: None,
                rpc_vs_local: None,
                interface_compare: None,
                interface_compare_sample: None,
                error: Some(format!("join error: {}", e)),
            }),
        }
    }

    rows.sort_by(|a, b| a.package_id.cmp(&b.package_id));

    let file =
        File::create(&report_path).with_context(|| format!("create {}", report_path.display()))?;
    let mut writer = BufWriter::new(file);

    let mut total = 0usize;
    let mut local_ok = 0usize;
    let mut bcs_module_match = 0usize;
    let mut local_bytes_ok = 0usize;
    let mut local_bytes_mismatch_packages = 0usize;
    let mut local_bytes_mismatches_total = 0usize;
    let mut rpc_ok = 0usize;
    let mut rpc_module_match = 0usize;
    let mut rpc_exposed_function_count_match = 0usize;
    let mut interface_ok = 0usize;
    let mut interface_mismatch_packages = 0usize;
    let mut interface_mismatches_total = 0usize;
    let mut problems = 0usize;

    for row in &rows {
        total += 1;
        if row.error.is_none() {
            local_ok += 1;
        }
        if row.local_vs_bcs.missing_in_right.is_empty()
            && row.local_vs_bcs.extra_in_right.is_empty()
        {
            bcs_module_match += 1;
        }
        if args.corpus_local_bytes_check {
            match row.local_bytes_check.as_ref() {
                Some(check) => {
                    local_bytes_mismatches_total += check.mismatches_total;
                    if check.mismatches_total == 0 && row.local_bytes_check_error.is_none() {
                        local_bytes_ok += 1;
                    } else {
                        local_bytes_mismatch_packages += 1;
                    }
                }
                None => local_bytes_mismatch_packages += 1,
            }
        }
        if let Some(rpc) = row.rpc.as_ref() {
            rpc_ok += 1;
            if let Some(diff) = row.rpc_vs_local.as_ref() {
                if diff.missing_in_right.is_empty() && diff.extra_in_right.is_empty() {
                    rpc_module_match += 1;
                }
            }
            let local = row.local;
            let expected_exposed =
                local.functions_public + local.functions_friend + local.private_entry_functions;
            if local.modules == rpc.modules
                && local.structs == rpc.structs
                && expected_exposed == rpc.functions
                && local.key_structs == rpc.key_structs
            {
                rpc_exposed_function_count_match += 1;
            }
        }

        if args.corpus_interface_compare {
            match row.interface_compare.as_ref() {
                Some(s) => {
                    interface_mismatches_total += s.mismatches_total;
                    if s.mismatches_total == 0 {
                        interface_ok += 1;
                    } else {
                        interface_mismatch_packages += 1;
                    }
                }
                None => interface_mismatch_packages += 1,
            }
        }

        serde_json::to_writer(&mut writer, row).context("write corpus JSONL")?;
        writer.write_all(b"\n").ok();
    }

    // Write problems.jsonl (subset of rows) and corpus_summary.json
    {
        let file = File::create(&problems_path)
            .with_context(|| format!("create {}", problems_path.display()))?;
        let mut writer = BufWriter::new(file);

        for row in &rows {
            let mut is_problem = row.error.is_some();
            if !row.local_vs_bcs.missing_in_right.is_empty()
                || !row.local_vs_bcs.extra_in_right.is_empty()
            {
                is_problem = true;
            }
            if args.corpus_local_bytes_check {
                if row.local_bytes_check_error.is_some() {
                    is_problem = true;
                }
                match row.local_bytes_check.as_ref() {
                    Some(check) => {
                        if check.mismatches_total != 0 {
                            is_problem = true;
                        }
                    }
                    None => is_problem = true,
                }
            }

            if args.corpus_rpc_compare {
                match row.rpc.as_ref() {
                    None => is_problem = true,
                    Some(rpc) => {
                        let local = row.local;
                        let expected_exposed = local.functions_public
                            + local.functions_friend
                            + local.private_entry_functions;
                        if local.modules != rpc.modules
                            || local.structs != rpc.structs
                            || local.key_structs != rpc.key_structs
                            || expected_exposed != rpc.functions
                        {
                            is_problem = true;
                        }
                        if let Some(diff) = row.rpc_vs_local.as_ref() {
                            if !diff.missing_in_right.is_empty() || !diff.extra_in_right.is_empty()
                            {
                                is_problem = true;
                            }
                        }
                    }
                }
            }

            if args.corpus_interface_compare {
                if let Some(s) = row.interface_compare.as_ref() {
                    if s.mismatches_total != 0 {
                        is_problem = true;
                    }
                } else {
                    is_problem = true;
                }
            }

            if is_problem {
                problems += 1;
                serde_json::to_writer(&mut writer, row).context("write problems JSONL")?;
                writer.write_all(b"\n").ok();
            }
        }
    }

    {
        let summary = CorpusSummary {
            total,
            local_ok,
            local_vs_bcs_module_match: bcs_module_match,
            local_bytes_check_enabled: args.corpus_local_bytes_check,
            local_bytes_ok,
            local_bytes_mismatch_packages,
            local_bytes_mismatches_total,
            rpc_enabled: args.corpus_rpc_compare,
            rpc_ok,
            rpc_module_match,
            rpc_exposed_function_count_match,
            interface_compare_enabled: args.corpus_interface_compare,
            interface_ok,
            interface_mismatch_packages,
            interface_mismatches_total,
            problems,
            report_jsonl: report_path.display().to_string(),
            index_jsonl: index_path.display().to_string(),
            problems_jsonl: problems_path.display().to_string(),
            sample_ids: args
                .corpus_sample
                .map(|_| sample_ids_path.display().to_string()),
            run_metadata_json: run_metadata_path.display().to_string(),
        };
        let file = File::create(&summary_path)
            .with_context(|| format!("create {}", summary_path.display()))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &summary).context("write corpus summary")?;
        writer.write_all(b"\n").ok();
    }

    {
        let meta = RunMetadata {
            started_at_unix_seconds: run_started_at,
            finished_at_unix_seconds: now_unix_seconds(),
            argv,
            rpc_url: args.rpc_url.clone(),
            bytecode_corpus_root: args
                .bytecode_corpus_root
                .as_ref()
                .map(|p| p.display().to_string()),
            sui_packages_git,
        };
        let file = File::create(&run_metadata_path)
            .with_context(|| format!("create {}", run_metadata_path.display()))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &meta).context("write run metadata")?;
        writer.write_all(b"\n").ok();
    }

    if let Some(path) = submission_summary_path.as_ref() {
        let corpus_name = args
            .bytecode_corpus_root
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

        let summary = SubmissionSummary {
            tool: "sui-move-interface-extractor".to_string(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_unix_seconds: run_started_at,
            finished_at_unix_seconds: now_unix_seconds(),
            rpc_url: args.rpc_url.clone(),
            corpus_name,
            sui_packages_git: git_head_metadata_for_path(
                args.bytecode_corpus_root
                    .as_deref()
                    .unwrap_or_else(|| Path::new(".")),
            ),
            stats: CorpusSummaryStats {
                total,
                local_ok,
                local_vs_bcs_module_match: bcs_module_match,
                local_bytes_check_enabled: args.corpus_local_bytes_check,
                local_bytes_ok,
                local_bytes_mismatch_packages,
                local_bytes_mismatches_total,
                rpc_enabled: args.corpus_rpc_compare,
                rpc_ok,
                rpc_module_match,
                rpc_exposed_function_count_match,
                interface_compare_enabled: args.corpus_interface_compare,
                interface_ok,
                interface_mismatch_packages,
                interface_mismatches_total,
                problems,
            },
        };

        let mut v = serde_json::to_value(summary).context("serialize submission summary")?;
        canonicalize_json_value(&mut v);
        write_canonical_json(path, &v)?;
    }

    eprintln!(
        "corpus done: total={} local_ok={} local_vs_bcs_module_match={} local_bytes_check_enabled={} local_bytes_ok={} local_bytes_mismatch_packages={} local_bytes_mismatches_total={} rpc_ok={} rpc_module_match={} rpc_exposed_function_count_match={} interface_ok={} interface_mismatch_packages={} interface_mismatches_total={} problems={} report={} index={} summary={} run_metadata={}",
        total,
        local_ok,
        bcs_module_match,
        args.corpus_local_bytes_check,
        local_bytes_ok,
        local_bytes_mismatch_packages,
        local_bytes_mismatches_total,
        rpc_ok,
        rpc_module_match,
        rpc_exposed_function_count_match,
        interface_ok,
        interface_mismatch_packages,
        interface_mismatches_total,
        problems,
        report_path.display(),
        index_path.display(),
        summary_path.display(),
        run_metadata_path.display()
    );

    if args.corpus_sample.is_some() {
        eprintln!("corpus sample ids written to {}", sample_ids_path.display());
    }

    Ok(())
}

async fn run_single(
    args: &Args,
    client: Arc<sui_sdk::SuiClient>,
    input_id_str: &str,
) -> Result<()> {
    let input_id = ObjectID::from_str(input_id_str)
        .map_err(|e| anyhow!("invalid --package-id {}: {}", input_id_str, e))?;

    let retry = RetryConfig::from_args(args);

    if args.emit_compare_report.is_some() && !args.compare_bytecode_rpc {
        return Err(anyhow!(
            "--emit-compare-report requires --compare-bytecode-rpc"
        ));
    }

    let (package_oid, module_names, interface_value) = match args.input_kind {
        InputKind::Package => {
            let (names, v) =
                build_interface_value_for_package(Arc::clone(&client), input_id, retry).await?;
            (input_id, names, v)
        }
        InputKind::PackageInfo => {
            let package_id =
                resolve_package_address_from_package_info(Arc::clone(&client), input_id, retry)
                    .await?;
            let (names, v) =
                build_interface_value_for_package(Arc::clone(&client), package_id, retry).await?;
            (package_id, names, v)
        }
        InputKind::Auto => {
            match build_interface_value_for_package(Arc::clone(&client), input_id, retry).await {
                Ok((names, v)) => (input_id, names, v),
                Err(_e) => {
                    let package_id = resolve_package_address_from_package_info(
                        Arc::clone(&client),
                        input_id,
                        retry,
                    )
                    .await?;
                    let (names, v) =
                        build_interface_value_for_package(Arc::clone(&client), package_id, retry)
                            .await?;
                    (package_id, names, v)
                }
            }
        }
    };

    if args.bytecode_check {
        let bcs_names = fetch_bcs_module_names(Arc::clone(&client), package_oid, retry).await?;
        let check = bytecode_module_check(&module_names, &bcs_names);
        eprintln!(
            "bytecode_check: normalized_modules={} bcs_modules={} missing_in_bcs={} extra_in_bcs={} ",
            check.normalized_modules,
            check.bcs_modules,
            check.missing_in_bcs.len(),
            check.extra_in_bcs.len()
        );
    }

    if args.check_stability {
        check_stability(&interface_value)?;
    }

    if args.sanity {
        let counts = extract_sanity_counts(interface_value.get("modules").unwrap_or(&Value::Null));
        eprintln!(
            "sanity: modules={} structs={} functions={} key_structs={}",
            counts.modules, counts.structs, counts.functions, counts.key_structs
        );
    }

    if let Some(path) = args.emit_json.as_ref() {
        write_canonical_json(path, &interface_value)?;
    }

    if args.emit_bytecode_json.is_some() || args.compare_bytecode_rpc {
        let module_map =
            fetch_bcs_module_map_bytes(Arc::clone(&client), package_oid, retry).await?;
        let mut compiled: Vec<CompiledModule> = Vec::with_capacity(module_map.len());
        for (name, bytes) in module_map {
            let module = CompiledModule::deserialize_with_defaults(&bytes)
                .with_context(|| format!("deserialize module {} for {}", name, package_oid))?;
            let self_name = compiled_module_name(&module);
            if self_name != name {
                return Err(anyhow!(
                    "module name mismatch for {}: moduleMap key={} compiled.self_id.name={}",
                    package_oid,
                    name,
                    self_name
                ));
            }
            compiled.push(module);
        }

        let (_names, bytecode_value) = build_bytecode_interface_value_from_compiled_modules(
            &package_oid.to_string(),
            &compiled,
        )?;

        if let Some(path) = args.emit_bytecode_json.as_ref() {
            write_canonical_json(path, &bytecode_value)?;
        }

        if args.sanity {
            let counts =
                extract_sanity_counts(bytecode_value.get("modules").unwrap_or(&Value::Null));
            eprintln!(
                "sanity(bytecode): modules={} structs={} functions={} key_structs={}",
                counts.modules, counts.structs, counts.functions, counts.key_structs
            );
        }

        if args.compare_bytecode_rpc {
            let (summary, mismatches) = compare_interface_rpc_vs_bytecode(
                &package_oid.to_string(),
                &interface_value,
                &bytecode_value,
                InterfaceCompareOptions {
                    max_mismatches: args.compare_max_mismatches,
                    include_values: args.emit_compare_report.is_some(),
                },
            );
            eprintln!(
                "interface_compare: modules_compared={} modules_missing_in_bytecode={} modules_extra_in_bytecode={} structs_compared={} struct_mismatches={} functions_compared={} function_mismatches={} mismatches_total={}",
                summary.modules_compared,
                summary.modules_missing_in_bytecode,
                summary.modules_extra_in_bytecode,
                summary.structs_compared,
                summary.struct_mismatches,
                summary.functions_compared,
                summary.function_mismatches,
                summary.mismatches_total
            );

            if let Some(path) = args.emit_compare_report.as_ref() {
                let report = InterfaceCompareReport {
                    package_id: package_oid.to_string(),
                    summary,
                    mismatches,
                };
                let mut report_value =
                    serde_json::to_value(report).context("serialize compare report")?;
                canonicalize_json_value(&mut report_value);
                write_canonical_json(path, &report_value)?;
            }
        }
    }

    let mut list_modules = args.list_modules;
    if !list_modules
        && args.emit_json.is_none()
        && args.emit_bytecode_json.is_none()
        && !args.compare_bytecode_rpc
        && args.emit_compare_report.is_none()
        && !args.sanity
    {
        list_modules = true;
    }

    if list_modules {
        println!("modules={} ", module_names.len());
        for name in module_names {
            println!("- {}", name);
        }
    }

    Ok(())
}

async fn run_single_local_bytecode_dir(args: &Args) -> Result<()> {
    let Some(dir) = args.bytecode_package_dir.as_ref() else {
        return Err(anyhow!("missing --bytecode-package-dir"));
    };

    if args.compare_bytecode_rpc || args.emit_compare_report.is_some() {
        return Err(anyhow!(
            "--compare-bytecode-rpc/--emit-compare-report require RPC; use --package-id mode"
        ));
    }

    let package_id = match read_package_id_from_metadata(dir) {
        Ok(id) => id,
        Err(_) => {
            let ids = collect_package_ids(args)?;
            if ids.len() != 1 {
                return Err(anyhow!(
                    "--bytecode-package-dir requires metadata.json with 'id' or exactly one --package-id"
                ));
            }
            ids[0].clone()
        }
    };

    let compiled = read_local_compiled_modules(dir)?;
    let (module_names, bytecode_value) =
        build_bytecode_interface_value_from_compiled_modules(&package_id, &compiled)?;

    if let Some(path) = args.emit_bytecode_json.as_ref() {
        write_canonical_json(path, &bytecode_value)?;
    }

    if args.sanity {
        let counts = extract_sanity_counts(bytecode_value.get("modules").unwrap_or(&Value::Null));
        eprintln!(
            "sanity(bytecode): modules={} structs={} functions={} key_structs={}",
            counts.modules, counts.structs, counts.functions, counts.key_structs
        );
    }

    let mut list_modules = args.list_modules;
    if !list_modules && args.emit_bytecode_json.is_none() && !args.sanity {
        list_modules = true;
    }
    if list_modules {
        println!("modules={} ", module_names.len());
        for name in module_names {
            println!("- {}", name);
        }
    }

    Ok(())
}

async fn run_batch(
    args: &Args,
    client: Arc<sui_sdk::SuiClient>,
    input_ids: Vec<String>,
) -> Result<()> {
    let Some(out_dir) = args.out_dir.as_ref() else {
        return Err(anyhow!("batch mode requires --out-dir"));
    };

    if args.emit_json.is_some() {
        return Err(anyhow!("--emit-json is only valid for single-package mode"));
    }

    if args.emit_bytecode_json.is_some() {
        return Err(anyhow!(
            "--emit-bytecode-json is only valid for single-package mode"
        ));
    }
    if args.compare_bytecode_rpc || args.emit_compare_report.is_some() {
        return Err(anyhow!(
            "--compare-bytecode-rpc/--emit-compare-report are only valid for single-package mode"
        ));
    }

    if args.list_modules {
        return Err(anyhow!(
            "--list-modules is only valid for single-package mode"
        ));
    }

    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let summary_path = args
        .summary_jsonl
        .clone()
        .unwrap_or_else(|| out_dir.join("summary.jsonl"));

    let concurrency = args.concurrency.max(1);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut join_set: tokio::task::JoinSet<Result<BatchSummaryRow>> = tokio::task::JoinSet::new();
    let retry = RetryConfig::from_args(args);

    for input_id_str in input_ids {
        let client = Arc::clone(&client);
        let semaphore = Arc::clone(&semaphore);
        let out_dir = out_dir.clone();
        let sanity_enabled = args.sanity;
        let check_stability_enabled = args.check_stability;
        let skip_existing = args.skip_existing;
        let input_kind = args.input_kind;
        let retry_cfg = retry;
        let bytecode_check_enabled = args.bytecode_check;

        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| anyhow!("semaphore closed: {}", e))?;

            let input_id_for_row = input_id_str.clone();

            let input_oid = match ObjectID::from_str(&input_id_str) {
                Ok(id) => id,
                Err(e) => {
                    return Ok(BatchSummaryRow {
                        input_id: input_id_for_row,
                        package_id: None,
                        resolved_from_package_info: false,
                        ok: false,
                        skipped: false,
                        output_path: None,
                        sanity: None,
                        bytecode: None,
                        error: Some(format!("invalid id: {}", e)),
                    });
                }
            };

            // Resolve to a package object id (optionally via PackageInfo).
            let mut resolved_from_package_info = false;
            let mut package_oid = input_oid;

            if matches!(input_kind, InputKind::PackageInfo) {
                match resolve_package_address_from_package_info(Arc::clone(&client), input_oid, retry_cfg).await {
                    Ok(resolved) => {
                        resolved_from_package_info = true;
                        package_oid = resolved;
                    }
                    Err(e) => {
                        return Ok(BatchSummaryRow {
                            input_id: input_id_for_row,
                            package_id: None,
                            resolved_from_package_info: true,
                            ok: false,
                            skipped: false,
                            output_path: None,
                            sanity: None,
                            bytecode: None,
                            error: Some(format!("{:#}", e)),
                        });
                    }
                }
            }

            let mut package_id_str = package_oid.to_string();
            let mut output_path = out_dir.join(format!("{}.json", package_id_str));

            if skip_existing && output_path.exists() {
                return Ok(BatchSummaryRow {
                    input_id: input_id_for_row,
                    package_id: Some(package_id_str),
                    resolved_from_package_info,
                    ok: true,
                    skipped: true,
                    output_path: Some(output_path.display().to_string()),
                    sanity: None,
                    bytecode: None,
                    error: None,
                });
            }

            let fetch_result: Result<(Vec<String>, Value)> = match input_kind {
                InputKind::Package => build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg).await,
                InputKind::PackageInfo => build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg).await,
                InputKind::Auto => {
                    // Try treating the input as a package first.
                    match build_interface_value_for_package(Arc::clone(&client), input_oid, retry_cfg).await {
                        Ok(v) => Ok(v),
                        Err(first_err) => {
                            // Fall back to resolving as PackageInfo.
                            let resolved_oid = match resolve_package_address_from_package_info(Arc::clone(&client), input_oid, retry_cfg).await {
                                Ok(resolved) => resolved,
                                Err(resolve_err) => {
                                    return Ok(BatchSummaryRow {
                                        input_id: input_id_for_row,
                                        package_id: None,
                                        resolved_from_package_info: false,
                                        ok: false,
                                        skipped: false,
                                        output_path: None,
                                        sanity: None,
                                        bytecode: None,
                                        error: Some(format!(
                                            "auto failed\n- as package: {:#}\n- as package-info: {:#}",
                                            first_err, resolve_err
                                        )),
                                    });
                                }
                            };

                            resolved_from_package_info = true;
                            package_oid = resolved_oid;
                            package_id_str = package_oid.to_string();
                            output_path = out_dir.join(format!("{}.json", package_id_str));

                            if skip_existing && output_path.exists() {
                                return Ok(BatchSummaryRow {
                                    input_id: input_id_for_row,
                                    package_id: Some(package_id_str),
                                    resolved_from_package_info,
                                    ok: true,
                                    skipped: true,
                                    output_path: Some(output_path.display().to_string()),
                                    sanity: None,
                                    bytecode: None,
                                    error: None,
                                });
                            }

                            build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg).await
                        }
                    }
                }
            };

            let (module_names, interface_value) = match fetch_result {
                Ok(v) => v,
                Err(e) => {
                    return Ok(BatchSummaryRow {
                        input_id: input_id_for_row,
                        package_id: Some(package_id_str),
                        resolved_from_package_info,
                        ok: false,
                        skipped: false,
                        output_path: Some(output_path.display().to_string()),
                        sanity: None,
                        bytecode: None,
                        error: Some(format!("{:#}", e)),
                    });
                }
            };

            if check_stability_enabled {
                if let Err(e) = check_stability(&interface_value) {
                    return Ok(BatchSummaryRow {
                        input_id: input_id_for_row,
                        package_id: Some(package_id_str),
                        resolved_from_package_info,
                        ok: false,
                        skipped: false,
                        output_path: Some(output_path.display().to_string()),
                        sanity: None,
                        bytecode: None,
                        error: Some(format!("{:#}", e)),
                    });
                }
            }

            let sanity = if sanity_enabled {
                Some(extract_sanity_counts(
                    interface_value.get("modules").unwrap_or(&Value::Null),
                ))
            } else {
                None
            };

            let bytecode = if bytecode_check_enabled {
                match fetch_bcs_module_names(Arc::clone(&client), package_oid, retry_cfg).await {
                    Ok(bcs_names) => Some(bytecode_module_check(&module_names, &bcs_names)),
                    Err(e) => {
                        return Ok(BatchSummaryRow {
                            input_id: input_id_for_row,
                            package_id: Some(package_id_str),
                            resolved_from_package_info,
                            ok: false,
                            skipped: false,
                            output_path: Some(output_path.display().to_string()),
                            sanity: None,
                            bytecode: None,
                            error: Some(format!("bytecode_check failed: {:#}", e)),
                        });
                    }
                }
            } else {
                None
            };

            if let Err(e) = write_canonical_json(&output_path, &interface_value) {
                return Ok(BatchSummaryRow {
                    input_id: input_id_for_row,
                    package_id: Some(package_id_str),
                    resolved_from_package_info,
                    ok: false,
                    skipped: false,
                    output_path: Some(output_path.display().to_string()),
                    sanity: None,
                    bytecode: None,
                    error: Some(format!("{:#}", e)),
                });
            }

            Ok(BatchSummaryRow {
                input_id: input_id_for_row,
                package_id: Some(package_id_str),
                resolved_from_package_info,
                ok: true,
                skipped: false,
                output_path: Some(output_path.display().to_string()),
                sanity,
                bytecode,
                error: None,
            })
        });
    }

    let mut rows: Vec<BatchSummaryRow> = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(row)) => rows.push(row),
            Ok(Err(e)) => rows.push(BatchSummaryRow {
                input_id: "<join_error>".to_string(),
                package_id: None,
                resolved_from_package_info: false,
                ok: false,
                skipped: false,
                output_path: None,
                sanity: None,
                bytecode: None,
                error: Some(format!("{:#}", e)),
            }),
            Err(e) => rows.push(BatchSummaryRow {
                input_id: "<panic>".to_string(),
                package_id: None,
                resolved_from_package_info: false,
                ok: false,
                skipped: false,
                output_path: None,
                sanity: None,
                bytecode: None,
                error: Some(format!("join error: {}", e)),
            }),
        }
    }

    rows.sort_by(|a, b| a.input_id.cmp(&b.input_id));

    let file = File::create(&summary_path)
        .with_context(|| format!("create {}", summary_path.display()))?;
    let mut writer = BufWriter::new(file);

    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for row in &rows {
        if row.ok {
            ok += 1;
        } else {
            failed += 1;
        }
        if row.skipped {
            skipped += 1;
        }

        serde_json::to_writer(&mut writer, row).context("write summary JSONL")?;
        writer.write_all(b"\n").ok();
    }

    eprintln!(
        "batch done: total={} ok={} failed={} skipped={} summary={} out_dir={}",
        rows.len(),
        ok,
        failed,
        skipped,
        summary_path.display(),
        out_dir.display()
    );

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.corpus_local_bytes_check && args.bytecode_corpus_root.is_none() {
        return Err(anyhow!(
            "--corpus-local-bytes-check requires --bytecode-corpus-root"
        ));
    }

    if args.emit_submission_summary.is_some() && args.bytecode_corpus_root.is_none() {
        return Err(anyhow!(
            "--emit-submission-summary is only valid in corpus mode (requires --bytecode-corpus-root)"
        ));
    }

    if args.bytecode_package_dir.is_some() {
        if args.out_dir.is_some() || args.bytecode_corpus_root.is_some() {
            return Err(anyhow!(
                "--bytecode-package-dir is single-package mode; do not use with --out-dir/--bytecode-corpus-root"
            ));
        }
        return run_single_local_bytecode_dir(&args).await;
    }

    if args.bytecode_corpus_root.is_some() {
        run_corpus(
            &args,
            Arc::new(SuiClientBuilder::default().build(&args.rpc_url).await?),
        )
        .await?;
        return Ok(());
    }

    let package_ids = collect_package_ids(&args)?;
    if package_ids.is_empty() {
        return Err(anyhow!(
            "no ids provided (use --package-id, --package-ids-file, or --mvr-catalog)"
        ));
    }

    let client = Arc::new(SuiClientBuilder::default().build(&args.rpc_url).await?);

    let is_batch = args.out_dir.is_some()
        || args.package_ids_file.is_some()
        || args.mvr_catalog.is_some()
        || package_ids.len() > 1;

    if is_batch {
        run_batch(&args, client, package_ids).await?;
        return Ok(());
    }

    let package_id = package_ids.first().expect("non-empty ids");
    run_single(&args, client, package_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_address_str_pads_to_32_bytes() {
        assert_eq!(
            normalize_address_str("0x2").unwrap(),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert_eq!(
            normalize_address_str("2").unwrap(),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_rpc_type_to_canonical_handles_string_primitives() {
        assert_eq!(
            rpc_type_to_canonical_json(&Value::String("U64".to_string())).unwrap(),
            serde_json::json!({"kind":"u64"})
        );
        assert_eq!(
            rpc_type_to_canonical_json(&Value::String("Address".to_string())).unwrap(),
            serde_json::json!({"kind":"address"})
        );
    }

    #[test]
    fn test_rpc_type_to_canonical_handles_struct_object() {
        let t = serde_json::json!({
            "Struct": {
                "address": "0x2",
                "module": "object",
                "name": "UID",
                "typeArguments": []
            }
        });
        let canon = rpc_type_to_canonical_json(&t).unwrap();
        assert_eq!(
            canon,
            serde_json::json!({
                "kind": "datatype",
                "address": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "module": "object",
                "name": "UID",
                "type_args": []
            })
        );
    }

    #[test]
    fn test_compare_interface_rpc_vs_bytecode_smoke_ok() {
        let rpc = serde_json::json!({
            "modules": {
                "m": {
                    "structs": {
                        "S": {
                            "abilities": { "abilities": ["Store"] },
                            "typeParameters": [],
                            "fields": [
                                {"name":"x", "type":"U64"}
                            ]
                        }
                    },
                    "exposedFunctions": {
                        "f": {
                            "visibility":"Public",
                            "isEntry": false,
                            "typeParameters": [],
                            "parameters": ["U64"],
                            "return": []
                        }
                    }
                }
            }
        });

        let bytecode = serde_json::json!({
            "modules": {
                "m": {
                    "address": "0x0000000000000000000000000000000000000000000000000000000000000001",
                    "structs": {
                        "S": {
                            "abilities": ["store"],
                            "type_params": [],
                            "is_native": false,
                            "fields": [{"name":"x", "type": {"kind":"u64"}}]
                        }
                    },
                    "functions": {
                        "f": {
                            "visibility": "public",
                            "is_entry": false,
                            "is_native": false,
                            "type_params": [],
                            "params": [{"kind":"u64"}],
                            "returns": [],
                            "acquires": []
                        }
                    }
                }
            }
        });

        let (summary, mismatches) = compare_interface_rpc_vs_bytecode(
            "0x1",
            &rpc,
            &bytecode,
            InterfaceCompareOptions {
                max_mismatches: 10,
                include_values: true,
            },
        );
        assert_eq!(summary.mismatches_total, 0, "{mismatches:#?}");
        assert!(mismatches.is_empty());
    }

    #[test]
    fn test_compare_interface_rpc_vs_bytecode_detects_type_mismatch() {
        let rpc = serde_json::json!({
            "modules": {
                "m": {
                    "structs": {
                        "S": {
                            "abilities": { "abilities": ["Store"] },
                            "typeParameters": [],
                            "fields": [{"name":"x", "type":"U64"}]
                        }
                    },
                    "exposedFunctions": {}
                }
            }
        });

        let bytecode = serde_json::json!({
            "modules": {
                "m": {
                    "address": "0x1",
                    "structs": {
                        "S": {
                            "abilities": ["store"],
                            "type_params": [],
                            "is_native": false,
                            "fields": [{"name":"x", "type": {"kind":"u128"}}]
                        }
                    },
                    "functions": {}
                }
            }
        });

        let (summary, mismatches) = compare_interface_rpc_vs_bytecode(
            "0x1",
            &rpc,
            &bytecode,
            InterfaceCompareOptions {
                max_mismatches: 10,
                include_values: false,
            },
        );
        assert!(summary.mismatches_total > 0);
        assert!(mismatches
            .iter()
            .any(|m| m.path.contains("/fields[0]/type")));
        assert!(mismatches
            .iter()
            .all(|m| m.rpc.is_none() && m.bytecode.is_none()));
    }

    #[test]
    fn test_decode_module_map_entry_bytes_base64_string() {
        let v = Value::String("AAEC".to_string());
        let bytes = decode_module_map_entry_bytes("m", &v).unwrap();
        assert_eq!(bytes, vec![0u8, 1u8, 2u8]);
    }

    #[test]
    fn test_decode_module_map_entry_bytes_array() {
        let v = serde_json::json!([0, 255, 1]);
        let bytes = decode_module_map_entry_bytes("m", &v).unwrap();
        assert_eq!(bytes, vec![0u8, 255u8, 1u8]);
    }

    #[test]
    fn test_decode_module_map_entry_bytes_array_rejects_out_of_range() {
        let v = serde_json::json!([256]);
        let err = decode_module_map_entry_bytes("m", &v).unwrap_err();
        assert!(format!("{:#}", err).contains("out-of-range"));
    }
}
