use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Copy, Clone, ValueEnum)]
pub enum MvrNetwork {
    Mainnet,
    Testnet,
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub enum InputKind {
    /// Treat inputs as package object IDs.
    Package,
    /// Treat inputs as `::package_info::PackageInfo` object IDs (resolve `package_address`).
    PackageInfo,
    /// Try as package ID first; on failure, try resolving as PackageInfo.
    Auto,
}

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// On-chain id (0x...). Can be provided multiple times.
    #[arg(long, value_name = "ID")]
    pub package_id: Vec<String>,

    /// Read additional ids from a file (1 id per line; '#' comments allowed).
    #[arg(long, value_name = "PATH")]
    pub package_ids_file: Option<PathBuf>,

    /// Read ids from an MVR catalog.json (uses *package_info_id fields).
    #[arg(long, value_name = "PATH")]
    pub mvr_catalog: Option<PathBuf>,

    /// Which MVR catalog id field to use.
    #[arg(long, value_enum, default_value_t = MvrNetwork::Mainnet)]
    pub mvr_network: MvrNetwork,

    /// How to interpret input ids.
    #[arg(long, value_enum, default_value_t = InputKind::Auto)]
    pub input_kind: InputKind,

    /// Root of a local bytecode corpus (expects `0x??/<id>/bytecode_modules/*.mv` style dirs).
    ///
    /// Example: `<sui-packages-checkout>/packages/mainnet_most_used`
    #[arg(long, value_name = "DIR")]
    pub bytecode_corpus_root: Option<PathBuf>,

    /// In corpus mode, also fetch RPC-normalized modules and compare counts/module names.
    #[arg(long, default_value_t = false)]
    pub corpus_rpc_compare: bool,

    /// In corpus mode, do a rigorous field-by-field compare between RPC normalized interface and
    /// bytecode-derived interface (types, fields, params/returns, etc).
    /// Requires `--corpus-rpc-compare`.
    #[arg(long, default_value_t = false)]
    pub corpus_interface_compare: bool,

    /// In corpus mode with `--corpus-interface-compare`, capture up to N mismatch samples per package
    /// to make failures actionable. `0` keeps only summary counts.
    #[arg(long, default_value_t = 10)]
    pub corpus_interface_compare_max_mismatches: usize,

    /// In corpus mode with `--corpus-interface-compare`, include RPC/bytecode values in mismatch samples.
    /// This can make outputs much larger; off by default.
    #[arg(long, default_value_t = false)]
    pub corpus_interface_compare_include_values: bool,

    /// In corpus mode, skip deserializing `.mv` bytecode and only derive module names from filenames.
    /// This is faster for very large corpora but disables struct/function/key counts.
    /// Not compatible with `--corpus-rpc-compare` or `--corpus-interface-compare`.
    #[arg(long, default_value_t = false)]
    pub corpus_module_names_only: bool,

    /// In corpus mode, write an index JSONL (defaults to <out-dir>/index.jsonl).
    #[arg(long, value_name = "PATH")]
    pub corpus_index_jsonl: Option<PathBuf>,

    /// In corpus mode, restrict analysis to package ids from a file (1 id per line; '#' comments allowed).
    /// This is useful for re-running a deterministic sample or focusing only on problematic ids.
    #[arg(long, value_name = "PATH")]
    pub corpus_ids_file: Option<PathBuf>,

    /// In corpus mode, select a deterministic sample of N packages.
    #[arg(long, value_name = "N")]
    pub corpus_sample: Option<usize>,

    /// Seed used for deterministic corpus sampling.
    #[arg(long, default_value_t = 0)]
    pub corpus_seed: u64,

    /// In corpus mode with --corpus-sample, write sampled package ids (defaults to <out-dir>/sample_ids.txt).
    #[arg(long, value_name = "PATH")]
    pub corpus_sample_ids_out: Option<PathBuf>,

    /// In corpus mode, verify local `.mv` bytes exactly match `bcs.json` moduleMap bytes.
    ///
    /// This is a strong local integrity check that does not require RPC.
    #[arg(long, default_value_t = false)]
    pub corpus_local_bytes_check: bool,

    /// In corpus mode with `--corpus-local-bytes-check`, capture up to N mismatch samples per package.
    #[arg(long, default_value_t = 10)]
    pub corpus_local_bytes_check_max_mismatches: usize,

    /// In corpus mode, write a sanitized “submission summary” JSON (no local filesystem paths).
    /// This is intended to be safe to share publicly (e.g., in a paper or benchmark repo).
    #[arg(long, value_name = "PATH")]
    pub emit_submission_summary: Option<PathBuf>,

    /// RPC URL (default: mainnet fullnode)
    #[arg(long, default_value = "https://fullnode.mainnet.sui.io:443")]
    pub rpc_url: String,

    /// Write canonical interface JSON to a file path (use '-' for stdout). Only valid for single-package mode.
    #[arg(long, value_name = "PATH")]
    pub emit_json: Option<PathBuf>,

    /// Write bytecode-derived canonical interface JSON to a file path (use '-' for stdout).
    /// Valid for single-package mode (`--package-id`) or local dir mode (`--bytecode-package-dir`).
    #[arg(long, value_name = "PATH")]
    pub emit_bytecode_json: Option<PathBuf>,

    /// Compare RPC normalized interface vs bytecode-derived interface for the package id.
    /// Use `--emit-compare-report` to write mismatch details.
    #[arg(long, default_value_t = false)]
    pub compare_bytecode_rpc: bool,

    /// Write a comparison report (JSON) showing mismatch counts and a sample of mismatches.
    /// Only valid for single-package mode with `--compare-bytecode-rpc`.
    #[arg(long, value_name = "PATH")]
    pub emit_compare_report: Option<PathBuf>,

    /// Maximum mismatches to include in `--emit-compare-report` (counts still include all mismatches).
    #[arg(long, default_value_t = 200)]
    pub compare_max_mismatches: usize,

    /// Local bytecode artifact directory (expects `bytecode_modules/*.mv`, optional `metadata.json`).
    /// Enables bytecode-first single-package mode without RPC.
    #[arg(long, value_name = "DIR")]
    pub bytecode_package_dir: Option<PathBuf>,

    /// Output directory for batch/corpus mode.
    #[arg(long, value_name = "DIR")]
    pub out_dir: Option<PathBuf>,

    /// Write a batch summary as JSONL (defaults to <out-dir>/summary.jsonl).
    #[arg(long, value_name = "PATH")]
    pub summary_jsonl: Option<PathBuf>,

    /// Limit the number of packages processed (after dedup/sort).
    #[arg(long, value_name = "N")]
    pub max_packages: Option<usize>,

    /// Max concurrent RPC fetches in batch/corpus mode.
    #[arg(long, default_value_t = 2)]
    pub concurrency: usize,

    /// Number of retries for retryable RPC failures (e.g., 429 rate limiting).
    #[arg(long, default_value_t = 8)]
    pub retries: usize,

    /// Initial retry backoff in milliseconds.
    #[arg(long, default_value_t = 250)]
    pub retry_initial_ms: u64,

    /// Maximum retry backoff in milliseconds.
    #[arg(long, default_value_t = 5000)]
    pub retry_max_ms: u64,

    /// Skip packages whose output JSON already exists in --out-dir.
    #[arg(long, default_value_t = false)]
    pub skip_existing: bool,

    /// Print basic counts (modules/structs/functions/key_structs)
    #[arg(long, default_value_t = false)]
    pub sanity: bool,

    /// Print module names (single-package mode)
    #[arg(long, default_value_t = false)]
    pub list_modules: bool,

    /// Verify canonical JSON is stable under serialize/parse/serialize
    #[arg(long, default_value_t = false)]
    pub check_stability: bool,

    /// Compare normalized module list to on-chain MovePackage `bcs.moduleMap` keys.
    #[arg(long, default_value_t = false)]
    pub bytecode_check: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    BenchmarkLocal(BenchmarkLocalArgs),
}

#[derive(Debug, Parser)]
pub struct BenchmarkLocalArgs {
    /// Path to the directory containing target .mv files.
    #[arg(long, value_name = "DIR")]
    pub target_corpus: PathBuf,

    /// Path to the JSONL output report.
    #[arg(long, value_name = "FILE")]
    pub output: PathBuf,

    /// Skip Tier B execution (faster, but potentially more false positives).
    #[arg(long, default_value_t = false)]
    pub tier_a_only: bool,

    /// Use a restricted set of mock objects for Tier B.
    #[arg(long, default_value_t = false)]
    pub restricted_state: bool,

    // === v0.4.0 MM2 enhancements ===

    /// Enable MM2-based static type checking (Phase 2).
    /// This provides better error messages and catches type errors earlier.
    #[arg(long, default_value_t = false)]
    pub use_mm2: bool,

    /// Stop after static type checking (no synthesis or execution).
    /// Useful for quickly validating type signatures without VM overhead.
    /// Requires --use-mm2.
    #[arg(long, default_value_t = false)]
    pub static_only: bool,

    /// Maximum depth for constructor chain resolution.
    /// Higher values allow more complex type construction but take longer.
    #[arg(long, default_value_t = 5)]
    pub max_chain_depth: usize,

    /// Use the new phase-based error taxonomy (E101-E502 codes).
    /// This replaces the legacy A1-A5/B1-B2 failure stages.
    #[arg(long, default_value_t = false)]
    pub phase_errors: bool,

    /// Output error distribution by phase instead of pass rates.
    /// Groups failures by Resolution/TypeCheck/Synthesis/Execution/Validation.
    #[arg(long, default_value_t = false)]
    pub error_distribution: bool,

    /// Filter to only test functions matching this pattern (regex).
    #[arg(long, value_name = "PATTERN")]
    pub function_filter: Option<String>,

    /// Filter to only test modules matching this pattern (regex).
    #[arg(long, value_name = "PATTERN")]
    pub module_filter: Option<String>,
}

#[derive(Debug, Copy, Clone)]
pub struct RetryConfig {
    pub retries: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl RetryConfig {
    pub fn from_args(args: &Args) -> Self {
        Self {
            retries: args.retries,
            initial_backoff: Duration::from_millis(args.retry_initial_ms),
            max_backoff: Duration::from_millis(args.retry_max_ms),
        }
    }
}
