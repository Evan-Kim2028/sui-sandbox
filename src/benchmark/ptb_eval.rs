//! # PTB Evaluation Runner
//!
//! This module evaluates PTB construction and execution using the SimulationEnvironment.
//! It implements a self-healing workflow where errors are diagnosed and corrective
//! actions are taken automatically.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;

use crate::args::PtbEvalArgs;
use crate::benchmark::simulation::{SimulationEnvironment, SimulationError};
use crate::benchmark::tx_replay::{TransactionCache, CachedTransaction, build_address_aliases_for_test};

/// Result of evaluating a single PTB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtbEvalReport {
    /// Transaction digest.
    pub digest: String,

    /// Whether this is a framework-only transaction.
    pub is_framework_only: bool,

    /// Number of commands in the PTB.
    pub command_count: usize,

    /// Number of inputs to the PTB.
    pub input_count: usize,

    /// Final status after all retry attempts.
    pub status: EvalStatus,

    /// Number of retry attempts made.
    pub retry_count: usize,

    /// Self-healing actions taken.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub healing_actions: Vec<HealingAction>,

    /// Final error if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Error category if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
}

/// Evaluation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvalStatus {
    /// PTB executed successfully.
    Success,
    /// PTB failed after all retry attempts.
    Failed,
    /// PTB was skipped (filtered out).
    Skipped,
}

/// A self-healing action taken during evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealingAction {
    /// Type of action.
    pub action_type: String,
    /// Details about what was done.
    pub details: String,
    /// Whether the action succeeded.
    pub success: bool,
}

/// Summary statistics for an evaluation run.
#[derive(Debug, Default)]
pub struct EvalSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub framework_only_total: usize,
    pub framework_only_success: usize,
    pub third_party_total: usize,
    pub third_party_success: usize,
    pub error_categories: HashMap<String, usize>,
    pub healing_actions_taken: usize,
    pub healing_actions_successful: usize,
}

impl EvalSummary {
    pub fn add_report(&mut self, report: &PtbEvalReport) {
        self.total += 1;

        match report.status {
            EvalStatus::Success => {
                self.success += 1;
                if report.is_framework_only {
                    self.framework_only_total += 1;
                    self.framework_only_success += 1;
                } else {
                    self.third_party_total += 1;
                    self.third_party_success += 1;
                }
            }
            EvalStatus::Failed => {
                self.failed += 1;
                if report.is_framework_only {
                    self.framework_only_total += 1;
                } else {
                    self.third_party_total += 1;
                }
                if let Some(cat) = &report.error_category {
                    *self.error_categories.entry(cat.clone()).or_insert(0) += 1;
                }
            }
            EvalStatus::Skipped => {
                self.skipped += 1;
            }
        }

        for action in &report.healing_actions {
            self.healing_actions_taken += 1;
            if action.success {
                self.healing_actions_successful += 1;
            }
        }
    }

    pub fn print(&self) {
        println!("\n=== PTB Evaluation Summary ===");
        println!("Total evaluated: {}", self.total);
        println!("Success: {} ({:.1}%)", self.success,
            if self.total > 0 { self.success as f64 / self.total as f64 * 100.0 } else { 0.0 });
        println!("Failed: {} ({:.1}%)", self.failed,
            if self.total > 0 { self.failed as f64 / self.total as f64 * 100.0 } else { 0.0 });
        if self.skipped > 0 {
            println!("Skipped: {}", self.skipped);
        }

        println!("\n--- By Transaction Type ---");
        if self.framework_only_total > 0 {
            println!("Framework-only: {}/{} ({:.1}%)",
                self.framework_only_success, self.framework_only_total,
                self.framework_only_success as f64 / self.framework_only_total as f64 * 100.0);
        }
        if self.third_party_total > 0 {
            println!("Third-party: {}/{} ({:.1}%)",
                self.third_party_success, self.third_party_total,
                self.third_party_success as f64 / self.third_party_total as f64 * 100.0);
        }

        if !self.error_categories.is_empty() {
            println!("\n--- Error Categories ---");
            let mut sorted: Vec<_> = self.error_categories.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1));
            for (cat, count) in sorted.iter().take(10) {
                println!("  {}: {}", cat, count);
            }
        }

        if self.healing_actions_taken > 0 {
            println!("\n--- Self-Healing ---");
            println!("Actions taken: {}", self.healing_actions_taken);
            println!("Successful: {} ({:.1}%)", self.healing_actions_successful,
                self.healing_actions_successful as f64 / self.healing_actions_taken as f64 * 100.0);
        }
    }
}

/// Categorize an error for reporting.
fn categorize_error(error: &SimulationError) -> String {
    match error {
        SimulationError::MissingPackage { .. } => "MissingPackage".to_string(),
        SimulationError::MissingObject { .. } => "MissingObject".to_string(),
        SimulationError::TypeMismatch { .. } => "TypeMismatch".to_string(),
        SimulationError::ContractAbort { abort_code, .. } => format!("Abort({})", abort_code),
        SimulationError::DeserializationFailed { .. } => "DeserializationFailed".to_string(),
        SimulationError::ExecutionError { .. } => "ExecutionError".to_string(),
        SimulationError::SharedObjectLockConflict { .. } => "SharedObjectLockConflict".to_string(),
    }
}

/// Evaluate a single cached transaction.
fn evaluate_transaction(
    cached: &CachedTransaction,
    env: &mut SimulationEnvironment,
    max_retries: usize,
    verbose: bool,
    show_healing: bool,
) -> PtbEvalReport {
    let digest = cached.transaction.digest.0.clone();
    let is_framework_only = cached.transaction.uses_only_framework();
    let command_count = cached.transaction.commands.len();
    let input_count = cached.transaction.inputs.len();

    let mut healing_actions = Vec::new();
    let mut last_error: Option<SimulationError> = None;
    let mut retry_count = 0;

    // Load cached packages into the environment
    for (pkg_id, _) in &cached.packages {
        if let Some(modules) = cached.get_package_modules(pkg_id) {
            if let Err(e) = env.deploy_package(modules) {
                if verbose {
                    eprintln!("Warning: Failed to load package {}: {}", pkg_id, e);
                }
            }
        }
    }

    // Load cached objects into the environment
    // This is critical - we need the actual object state from mainnet
    match env.load_cached_objects(&cached.objects) {
        Ok(count) => {
            if verbose && count > 0 {
                eprintln!("Loaded {} cached objects", count);
            }
        }
        Err(e) => {
            if verbose {
                eprintln!("Warning: Failed to load cached objects: {}", e);
            }
        }
    }

    // Set the sender address from the original transaction
    // This is critical for authorization checks (tx_context.sender() comparisons)
    env.set_sender(cached.transaction.sender);
    if verbose {
        eprintln!("Set sender to {}", cached.transaction.sender.to_hex_literal());
    }

    // Set the timestamp from the original transaction if available
    if let Some(ts) = cached.transaction.timestamp_ms {
        env.set_timestamp_ms(ts);
        if verbose {
            eprintln!("Set timestamp to {}", ts);
        }
    }

    // Build address aliases for package upgrades
    let address_aliases = build_address_aliases_for_test(cached);

    // Convert to PTB commands
    let ptb_result = cached.transaction.to_ptb_commands_with_objects_and_aliases(
        &cached.objects,
        &address_aliases,
    );

    let (inputs, commands) = match ptb_result {
        Ok(ic) => ic,
        Err(e) => {
            return PtbEvalReport {
                digest,
                is_framework_only,
                command_count,
                input_count,
                status: EvalStatus::Failed,
                retry_count: 0,
                healing_actions,
                error: Some(format!("Conversion failed: {}", e)),
                error_category: Some("ConversionError".to_string()),
            };
        }
    };

    // Debug: show PTB structure
    if verbose {
        eprintln!("PTB has {} inputs, {} commands", inputs.len(), commands.len());
        for (i, input) in inputs.iter().enumerate() {
            match input {
                crate::benchmark::ptb::InputValue::Pure(bytes) => {
                    eprintln!("  Input {}: Pure({} bytes)", i, bytes.len());
                }
                crate::benchmark::ptb::InputValue::Object(obj) => {
                    eprintln!("  Input {}: Object {} ({} bytes)", i, obj.id().to_hex_literal(), obj.bytes().len());
                }
            }
        }
    }

    // Try to execute with retries
    for attempt in 0..=max_retries {
        retry_count = attempt;

        let result = env.execute_ptb(inputs.clone(), commands.clone());

        if result.success {
            return PtbEvalReport {
                digest,
                is_framework_only,
                command_count,
                input_count,
                status: EvalStatus::Success,
                retry_count,
                healing_actions,
                error: None,
                error_category: None,
            };
        }

        // Analyze the error and try to heal
        if let Some(ref error) = result.error {
            last_error = Some(error.clone());

            if verbose {
                eprintln!("  Execution failed: {}", error);
                if let Some(ref raw) = result.raw_error {
                    eprintln!("  Raw error: {}", raw);
                }
            }

            if attempt < max_retries {
                // Try to take corrective action
                let action = try_heal(env, error, show_healing);
                if let Some(action) = action {
                    healing_actions.push(action);
                } else {
                    // No healing action possible, stop retrying
                    break;
                }
            }
        } else {
            // No structured error, stop retrying
            break;
        }
    }

    // Failed after all retries
    let (error_str, error_category) = if let Some(err) = &last_error {
        (Some(err.to_string()), Some(categorize_error(err)))
    } else {
        (None, None)
    };

    PtbEvalReport {
        digest,
        is_framework_only,
        command_count,
        input_count,
        status: EvalStatus::Failed,
        retry_count,
        healing_actions,
        error: error_str,
        error_category,
    }
}

/// Attempt to heal an error by taking corrective action.
fn try_heal(
    env: &mut SimulationEnvironment,
    error: &SimulationError,
    show_healing: bool,
) -> Option<HealingAction> {
    match error {
        SimulationError::MissingPackage { address, module: _, .. } => {
            if show_healing {
                eprintln!("  Healing: Attempting to deploy missing package {}", address);
            }

            // Try to fetch from mainnet
            match env.deploy_package_from_mainnet(address) {
                Ok(_) => {
                    if show_healing {
                        eprintln!("  Healing: Successfully deployed {}", address);
                    }
                    Some(HealingAction {
                        action_type: "deploy_package".to_string(),
                        details: format!("Deployed {} from mainnet", address),
                        success: true,
                    })
                }
                Err(e) => {
                    if show_healing {
                        eprintln!("  Healing: Failed to deploy {}: {}", address, e);
                    }
                    Some(HealingAction {
                        action_type: "deploy_package".to_string(),
                        details: format!("Failed to deploy {}: {}", address, e),
                        success: false,
                    })
                }
            }
        }
        SimulationError::MissingObject { id, expected_type: _, .. } => {
            if show_healing {
                eprintln!("  Healing: Attempting to fetch missing object {}", id);
            }

            // Try to fetch from mainnet
            match env.fetch_object_from_mainnet(id) {
                Ok(_) => {
                    if show_healing {
                        eprintln!("  Healing: Successfully fetched object {}", id);
                    }
                    Some(HealingAction {
                        action_type: "fetch_object".to_string(),
                        details: format!("Fetched {} from mainnet", id),
                        success: true,
                    })
                }
                Err(e) => {
                    if show_healing {
                        eprintln!("  Healing: Failed to fetch {}: {}", id, e);
                    }
                    Some(HealingAction {
                        action_type: "fetch_object".to_string(),
                        details: format!("Failed to fetch {}: {}", id, e),
                        success: false,
                    })
                }
            }
        }
        SimulationError::ContractAbort { .. } |
        SimulationError::TypeMismatch { .. } |
        SimulationError::DeserializationFailed { .. } |
        SimulationError::ExecutionError { .. } |
        SimulationError::SharedObjectLockConflict { .. } => {
            // These errors typically can't be healed automatically
            // They require changes to the object state or transaction structure
            None
        }
    }
}

/// Run the PTB evaluation.
pub fn run_ptb_eval(args: &PtbEvalArgs) -> Result<()> {
    // Check cache directory
    if !args.cache_dir.exists() {
        return Err(anyhow!(
            "Cache directory not found: {}. Run 'tx-replay --download-only' first.",
            args.cache_dir.display()
        ));
    }

    // Load cached transactions
    let cache = TransactionCache::new(&args.cache_dir)?;
    let digests = cache.list()?;

    if digests.is_empty() {
        return Err(anyhow!("No transactions found in cache"));
    }

    println!("Found {} cached transactions", digests.len());

    // Create output file if specified
    let mut output_file = match &args.output {
        Some(p) => Some(std::fs::File::create(p)?),
        None => None,
    };

    // Print fetching status
    if args.enable_fetching {
        println!("Mainnet fetching enabled for self-healing");
    }

    let mut summary = EvalSummary::default();

    // Determine limit
    let limit = args.limit.unwrap_or(digests.len());

    // Track load failures
    let mut load_failures = 0usize;

    // Evaluate transactions
    for (i, digest) in digests.iter().take(limit).enumerate() {
        let cached = match cache.load(digest) {
            Ok(c) => c,
            Err(e) => {
                load_failures += 1;
                if args.verbose {
                    eprintln!("  Warning: Failed to load cached transaction {}: {}", digest, e);
                }
                continue;
            }
        };

        let is_framework_only = cached.transaction.uses_only_framework();

        // Apply filters
        if args.framework_only && !is_framework_only {
            continue;
        }
        if args.third_party_only && is_framework_only {
            continue;
        }

        if args.verbose {
            println!("\n[{}/{}] Evaluating {}...", i + 1, limit.min(digests.len()), digest);
        }

        // Create fresh environment for each transaction
        let mut env = SimulationEnvironment::new()?;
        if args.enable_fetching {
            env = env.with_mainnet_fetching();
        }

        let report = evaluate_transaction(
            &cached,
            &mut env,
            args.max_retries,
            args.verbose,
            args.show_healing,
        );

        // Update summary
        summary.add_report(&report);

        // Write to output file
        if let Some(ref mut file) = output_file {
            let json = serde_json::to_string(&report)?;
            writeln!(file, "{}", json)?;
        }

        // Progress indicator
        if !args.verbose && (i + 1) % 10 == 0 {
            print!("\rEvaluated {}/{} transactions...", i + 1, limit.min(digests.len()));
            std::io::stdout().flush()?;
        }
    }

    if !args.verbose {
        println!(); // Clear progress line
    }

    // Print load failure summary if any
    if load_failures > 0 {
        println!("\nWarning: {} transaction(s) failed to load from cache", load_failures);
    }

    // Print summary
    summary.print();

    if let Some(ref path) = args.output {
        println!("\nResults written to: {}", path.display());
    }

    Ok(())
}
