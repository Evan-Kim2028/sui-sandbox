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

    /// Write Move source stub files to a directory (one .move file per module).
    /// These stubs allow the Move compiler to type-check imports from the package.
    /// Valid for single-package mode (`--package-id`) or local dir mode (`--bytecode-package-dir`).
    #[arg(long, value_name = "DIR")]
    pub emit_move_stubs: Option<PathBuf>,

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
    /// Fetch and replay mainnet transactions locally
    TxReplay(TxReplayArgs),
    /// Evaluate PTB construction and execution with self-healing simulation
    PtbEval(PtbEvalArgs),
    /// Interactive sandbox execution for LLM integration (JSON protocol)
    SandboxExec(SandboxExecArgs),
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

    // === v0.4.0 MM2 enhancements (now default) ===
    /// Use MM2-based static type checking (Phase 2). ON by default in v0.4.0.
    /// Provides better error messages and catches type errors earlier.
    /// Use --no-mm2 to disable and fall back to legacy bytecode analysis.
    #[arg(long, default_value_t = true)]
    pub use_mm2: bool,

    /// **DEPRECATED**: Disable MM2-based type checking (use legacy bytecode analysis).
    ///
    /// The legacy analyzer will be removed in v0.6.0. Migrate to MM2 (the default).
    #[deprecated(since = "0.5.0", note = "Legacy bytecode analyzer will be removed in v0.6.0")]
    #[arg(long, default_value_t = false)]
    pub no_mm2: bool,

    /// Stop after static type checking (no synthesis or execution).
    /// Useful for quickly validating type signatures without VM overhead.
    #[arg(long, default_value_t = false)]
    pub static_only: bool,

    /// Maximum depth for constructor chain resolution.
    /// Higher values allow more complex type construction but take longer.
    #[arg(long, default_value_t = 5)]
    pub max_chain_depth: usize,

    /// Use the new phase-based error taxonomy (E101-E502 codes). ON by default.
    /// Use --no-phase-errors for legacy A1-A5/B1-B2 failure stages.
    #[arg(long, default_value_t = true)]
    pub phase_errors: bool,

    /// **DEPRECATED**: Disable phase-based error taxonomy (use legacy A1-A5/B1-B2 stages).
    ///
    /// Legacy error stages will be removed in v0.6.0. Migrate to phase-based errors (the default).
    #[deprecated(since = "0.5.0", note = "Legacy A1-A5/B1-B2 error stages will be removed in v0.6.0")]
    #[arg(long, default_value_t = false)]
    pub no_phase_errors: bool,

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

    /// Use PTB (Programmable Transaction Block) execution mode for constructor chains.
    /// This executes multi-step constructor chains as PTB commands with proper result chaining.
    /// PTB mode uses SimulationEnvironment which is the canonical execution path.
    /// Enabled by default. Use --no-ptb for legacy VMHarness execution (provides tracing).
    #[arg(long, default_value_t = true)]
    pub use_ptb: bool,

    /// **DEPRECATED**: Disable PTB execution mode (use legacy VMHarness path).
    ///
    /// The legacy VMHarness path provides execution tracing but has inconsistent semantics.
    /// Only use for debugging or when execution tracing is required.
    /// Will be removed in v0.6.0 once SimulationEnvironment supports tracing.
    #[deprecated(since = "0.5.0", note = "Legacy VMHarness path will be removed in v0.6.0")]
    #[arg(long, default_value_t = false)]
    pub no_ptb: bool,

    // === Simulation Config Options ===
    /// Disable permissive crypto mocks (crypto operations may fail).
    /// By default, crypto natives (signature verify, etc.) always pass.
    #[arg(long, default_value_t = false)]
    pub strict_crypto: bool,

    /// Base timestamp for mock clock in milliseconds since epoch.
    /// Default: 1704067200000 (2024-01-01 00:00:00 UTC)
    #[arg(long, value_name = "MS")]
    pub clock_base_ms: Option<u64>,

    /// Seed for deterministic random number generation (hex string, up to 64 chars).
    /// Default: all zeros.
    #[arg(long, value_name = "HEX")]
    pub random_seed: Option<String>,
}

impl BenchmarkLocalArgs {
    /// Get effective MM2 setting (respects --no-mm2 override).
    pub fn effective_use_mm2(&self) -> bool {
        self.use_mm2 && !self.no_mm2
    }

    /// Get effective phase_errors setting (respects --no-phase-errors override).
    pub fn effective_phase_errors(&self) -> bool {
        self.phase_errors && !self.no_phase_errors
    }

    /// Get effective PTB setting (respects --no-ptb override).
    pub fn effective_use_ptb(&self) -> bool {
        self.use_ptb && !self.no_ptb
    }

    /// Build a SimulationConfig from the CLI arguments.
    pub fn simulation_config(&self) -> crate::benchmark::vm::SimulationConfig {
        let mut config = crate::benchmark::vm::SimulationConfig::default();

        // Apply strict crypto if requested
        if self.strict_crypto {
            config.mock_crypto_pass = false;
        }

        // Apply clock base if specified
        if let Some(ms) = self.clock_base_ms {
            config.clock_base_ms = ms;
        }

        // Apply random seed if specified (simple decimal parsing)
        if let Some(seed_str) = &self.random_seed {
            let mut seed = [0u8; 32];
            // Parse as decimal number and use it as first 8 bytes
            if let Ok(val) = seed_str.parse::<u64>() {
                seed[..8].copy_from_slice(&val.to_le_bytes());
            }
            config.random_seed = seed;
        }

        config
    }
}

#[derive(Debug, Parser)]
pub struct TxReplayArgs {
    /// Transaction digest to fetch and replay.
    #[arg(long, value_name = "DIGEST")]
    pub digest: Option<String>,

    /// Fetch and replay recent transactions from mainnet.
    #[arg(long, value_name = "COUNT")]
    pub recent: Option<usize>,

    /// RPC endpoint URL (defaults to mainnet).
    #[arg(long, value_name = "URL")]
    pub rpc_url: Option<String>,

    /// Use testnet instead of mainnet.
    #[arg(long, default_value_t = false)]
    pub testnet: bool,

    /// Output file for replay results (JSONL format).
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Path to bytecode corpus for loading packages.
    /// Required for replay to load the modules involved in the transaction.
    #[arg(long, value_name = "DIR")]
    pub bytecode_corpus: Option<PathBuf>,

    /// Show transaction summary only (don't attempt replay).
    #[arg(long, default_value_t = false)]
    pub summary_only: bool,

    /// Verbose output - show detailed command/effect information.
    #[arg(long, short = 'v', default_value_t = false)]
    pub verbose: bool,

    /// Fetch all input object data from RPC (enables detailed analysis).
    #[arg(long, default_value_t = false)]
    pub fetch_objects: bool,

    /// Attempt local replay and compare with on-chain effects.
    #[arg(long, default_value_t = false)]
    pub validate: bool,

    /// Execute full replay: fetch packages, load bytecode, execute PTB locally.
    /// Implies --fetch-objects and --validate.
    #[arg(long, default_value_t = false)]
    pub replay: bool,

    /// Only replay transactions that use framework packages only (0x1, 0x2, 0x3).
    /// Skips transactions that depend on third-party packages.
    #[arg(long, default_value_t = false)]
    pub framework_only: bool,

    // === Cache and Parallel Processing ===
    /// Cache directory for downloaded transactions.
    /// Transactions are saved as JSON files for faster iteration.
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Download transactions to cache without replaying.
    /// Use with --recent to download and cache transactions for later replay.
    #[arg(long, default_value_t = false)]
    pub download_only: bool,

    /// Replay from cache instead of fetching from RPC.
    /// Requires --cache-dir to be set.
    #[arg(long, default_value_t = false)]
    pub from_cache: bool,

    /// Number of parallel threads for replay (default: number of CPUs).
    #[arg(long, value_name = "N")]
    pub threads: Option<usize>,

    /// Run parallel replay on all cached transactions.
    /// Requires --cache-dir and --from-cache.
    #[arg(long, default_value_t = false)]
    pub parallel: bool,

    /// Clear the cache before downloading.
    #[arg(long, default_value_t = false)]
    pub clear_cache: bool,
}

#[derive(Debug, Parser)]
pub struct PtbEvalArgs {
    /// Cache directory containing transaction data (from tx-replay --download-only).
    #[arg(long, value_name = "DIR", default_value = ".tx-cache")]
    pub cache_dir: PathBuf,

    /// Output file for evaluation results (JSONL format).
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Maximum number of self-healing retry attempts per transaction.
    #[arg(long, default_value_t = 3)]
    pub max_retries: usize,

    /// Enable mainnet fetching for on-demand package/object loading.
    #[arg(long, default_value_t = false)]
    pub enable_fetching: bool,

    /// Only evaluate framework-only transactions (0x1, 0x2, 0x3).
    #[arg(long, default_value_t = false)]
    pub framework_only: bool,

    /// Only evaluate third-party transactions (exclude framework-only).
    #[arg(long, default_value_t = false)]
    pub third_party_only: bool,

    /// Limit evaluation to N transactions.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Verbose output - show detailed error information.
    #[arg(long, short = 'v', default_value_t = false)]
    pub verbose: bool,

    /// Show self-healing actions taken during evaluation.
    #[arg(long, default_value_t = false)]
    pub show_healing: bool,
}

#[derive(Debug, Parser)]
pub struct SandboxExecArgs {
    /// JSON file containing the execution request (or "-" for stdin).
    #[arg(long, value_name = "FILE", default_value = "-")]
    pub input: PathBuf,

    /// Output file for JSON response (or "-" for stdout).
    #[arg(long, value_name = "FILE", default_value = "-")]
    pub output: PathBuf,

    /// Enable mainnet fetching for on-demand package/object loading.
    #[arg(long, default_value_t = false)]
    pub enable_fetching: bool,

    /// Verbose output - show execution details to stderr.
    #[arg(long, short = 'v', default_value_t = false)]
    pub verbose: bool,

    /// Directory for compiled bytecode modules (persisted between calls).
    #[arg(long, value_name = "DIR")]
    pub bytecode_dir: Option<PathBuf>,

    /// Path to state file for persistent sandbox state between calls.
    /// If provided, the sandbox will load state from this file at startup
    /// and save state back after each request.
    #[arg(long, value_name = "FILE")]
    pub state_file: Option<PathBuf>,

    /// Disable automatic state persistence (still loads if state_file exists).
    /// Use this for read-only access to an existing state file.
    #[arg(long, default_value_t = false)]
    pub no_save_state: bool,

    /// Run in interactive mode (read JSON lines from stdin, write responses to stdout).
    /// Each line should be a complete JSON request. Responses are written as JSON lines.
    #[arg(long, default_value_t = false)]
    pub interactive: bool,
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
