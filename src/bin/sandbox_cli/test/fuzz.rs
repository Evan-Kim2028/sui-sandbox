//! Fuzz testing CLI command.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;

use sui_sandbox_core::fuzz::{
    classify_params, ClassifiedFunction, FuzzConfig, FuzzReport, FuzzRunner, Outcome, ParamClass,
};
use sui_sandbox_core::shared::parsing::parse_type_tag_string;

use super::super::SandboxState;

#[derive(Parser, Debug)]
#[command(
    about = "Fuzz a Move function with randomly generated inputs",
    long_about = "Generates random valid inputs for a Move function's parameter types \
                  and executes it repeatedly against the local VM. Reports aborts, \
                  errors, gas exhaustion, and gas usage profiles.\n\n\
                  Phase 1 supports pure-argument-only functions (bool, integers, \
                  address, vectors, strings). Functions requiring object inputs \
                  are analyzed and reported as not yet fuzzable."
)]
pub struct FuzzCmd {
    /// Target: "0xPKG::module::function" or "0xPKG::module" (with --all-functions)
    pub target: String,

    /// Number of fuzz iterations
    #[arg(long, short = 'n', default_value = "100")]
    pub iterations: u64,

    /// Random seed for reproducibility (default: random)
    #[arg(long)]
    pub seed: Option<u64>,

    /// Sender address
    #[arg(long, default_value = "0x0")]
    pub sender: String,

    /// Gas budget per execution
    #[arg(long, default_value = "50000000000")]
    pub gas_budget: u64,

    /// Type arguments (e.g., "0x2::sui::SUI")
    #[arg(long = "type-arg", num_args(1..))]
    pub type_args: Vec<String>,

    /// Stop on first abort/error
    #[arg(long)]
    pub fail_fast: bool,

    /// Only analyze the function signature (don't execute)
    #[arg(long)]
    pub dry_run: bool,

    /// Fuzz all callable functions in the module
    #[arg(long)]
    pub all_functions: bool,

    /// Maximum vector length for generated vector inputs
    #[arg(long, default_value = "32")]
    pub max_vector_len: usize,
}

impl FuzzCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        _verbose: bool,
    ) -> Result<()> {
        let sender =
            AccountAddress::from_hex_literal(&self.sender).context("Invalid sender address")?;

        let type_args = self
            .type_args
            .iter()
            .map(|s| parse_type_tag_string(s))
            .collect::<Result<Vec<_>>>()?;

        let seed = self.seed.unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
        });

        // Parse target
        let parts: Vec<&str> = self.target.split("::").collect();

        if self.all_functions || parts.len() == 2 {
            // Module-level fuzzing
            let (package, module_name) = if parts.len() == 2 {
                let pkg = AccountAddress::from_hex_literal(parts[0])
                    .context("Invalid package address")?;
                (pkg, parts[1].to_string())
            } else if parts.len() == 3 {
                // --all-functions with full target; use package::module
                let pkg = AccountAddress::from_hex_literal(parts[0])
                    .context("Invalid package address")?;
                (pkg, parts[1].to_string())
            } else {
                return Err(anyhow!(
                    "Invalid target. Expected '0xPKG::module' or '0xPKG::module::function'"
                ));
            };

            let module_path = format!("{}::{}", package.to_hex_literal(), module_name);
            let functions = state
                .resolver
                .list_functions(&module_path)
                .ok_or_else(|| anyhow!("Module '{}' not found", module_path))?;

            let mut reports = Vec::new();
            for func_name in &functions {
                // Check if callable
                if state
                    .resolver
                    .check_function_callable(&package, &module_name, func_name)
                    .is_err()
                {
                    continue;
                }

                let report = self.fuzz_single(
                    state,
                    package,
                    &module_name,
                    func_name,
                    sender,
                    &type_args,
                    seed,
                    json_output,
                )?;
                if let Some(r) = report {
                    reports.push(r);
                }
            }

            if json_output {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            }
            Ok(())
        } else if parts.len() == 3 {
            // Single function
            let package =
                AccountAddress::from_hex_literal(parts[0]).context("Invalid package address")?;
            let module_name = parts[1];
            let function_name = parts[2];

            state
                .resolver
                .check_function_callable(&package, module_name, function_name)?;

            self.fuzz_single(
                state,
                package,
                module_name,
                function_name,
                sender,
                &type_args,
                seed,
                json_output,
            )?;
            Ok(())
        } else {
            Err(anyhow!(
                "Invalid target format. Expected '0xPKG::module::function' or '0xPKG::module'"
            ))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fuzz_single(
        &self,
        state: &SandboxState,
        package: AccountAddress,
        module_name: &str,
        function_name: &str,
        sender: AccountAddress,
        type_args: &[move_core_types::language_storage::TypeTag],
        seed: u64,
        json_output: bool,
    ) -> Result<Option<FuzzReport>> {
        let target = format!(
            "{}::{}::{}",
            package.to_hex_literal(),
            module_name,
            function_name
        );

        // Get function signature to classify parameters
        let sig = state
            .resolver
            .get_function_signature(&package, module_name, function_name)
            .ok_or_else(|| anyhow!("Function '{}' not found", target))?;

        // We need the compiled module to classify params
        let compiled_module = state
            .resolver
            .get_module_by_addr_name(&package, module_name)
            .ok_or_else(|| anyhow!("Module '{}' not found", module_name))?;

        let classification = classify_params(compiled_module, &sig.parameter_types);

        if self.dry_run {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "target": target,
                        "classification": classification,
                        "verdict": if classification.is_fully_fuzzable { "FULLY_FUZZABLE" } else { "NOT_FUZZABLE" },
                    }))?
                );
            } else {
                print_dry_run(&target, &classification);
            }
            return Ok(None);
        }

        if !classification.is_fully_fuzzable {
            if !json_output {
                print_dry_run(&target, &classification);
                eprintln!(
                    "\nSkipping: {} has {} object parameter(s) not fuzzable in Phase 1",
                    target,
                    classification.object_count + classification.unfuzzable_count
                );
            }
            return Ok(None);
        }

        let config = FuzzConfig {
            iterations: self.iterations,
            seed,
            sender,
            gas_budget: self.gas_budget,
            type_args: type_args.to_vec(),
            fail_fast: self.fail_fast,
            max_vector_len: self.max_vector_len,
        };

        let runner = FuzzRunner::new(&state.resolver);
        let report = runner.run(
            package,
            module_name,
            function_name,
            &classification,
            &config,
        )?;

        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_report(&report);
        }

        Ok(Some(report))
    }
}

fn print_dry_run(target: &str, classification: &ClassifiedFunction) {
    println!("Function: {}", target);
    println!();
    println!("Parameters:");
    if classification.params.is_empty() {
        println!("  (none)");
    }
    for (i, (type_str, class)) in classification.params.iter().enumerate() {
        let status = match class {
            ParamClass::Pure { .. } => "Pure (fuzzable)",
            ParamClass::SystemInjected { .. } => "SystemInjected (auto-handled)",
            ParamClass::ObjectRef { mutable, .. } => {
                if *mutable {
                    "ObjectRef (mutable) — NOT fuzzable in Phase 1"
                } else {
                    "ObjectRef — NOT fuzzable in Phase 1"
                }
            }
            ParamClass::ObjectOwned { .. } => "ObjectOwned — NOT fuzzable in Phase 1",
            ParamClass::Unfuzzable { reason } => {
                println!("  [{i}] {type_str:30} -> Unfuzzable: {reason}");
                continue;
            }
        };
        println!("  [{i}] {type_str:30} -> {status}");
    }
    println!();
    if classification.is_fully_fuzzable {
        println!(
            "Verdict: FULLY FUZZABLE ({} pure, {} system-injected)",
            classification.pure_count, classification.system_count
        );
    } else {
        println!(
            "Verdict: NOT FUZZABLE — {} parameter(s) require object inputs (Phase 2)",
            classification.object_count + classification.unfuzzable_count
        );
        println!(
            "Fuzzable: {}/{} (pure: {}, system: {}, object: {}, unfuzzable: {})",
            classification.pure_count + classification.system_count,
            classification.params.len(),
            classification.pure_count,
            classification.system_count,
            classification.object_count,
            classification.unfuzzable_count,
        );
    }
}

fn print_report(report: &FuzzReport) {
    println!("Fuzz target: {}", report.target);
    println!();
    println!("Parameters:");
    for (i, (type_str, class)) in report.classification.params.iter().enumerate() {
        let label = match class {
            ParamClass::Pure { .. } => "Pure",
            ParamClass::SystemInjected { .. } => "System",
            _ => "Other",
        };
        println!("  [{i}] {type_str:30} -> {label}");
    }
    println!();
    println!(
        "Results ({} iterations, seed: {}, {}ms):",
        report.completed_iterations, report.seed, report.elapsed_ms
    );

    let total = report.completed_iterations.max(1);
    let success_pct = report.outcomes.successes as f64 / total as f64 * 100.0;
    println!(
        "  Success:        {:>6} ({:.1}%)",
        report.outcomes.successes, success_pct
    );

    let abort_total: u64 = report.outcomes.aborts.iter().map(|a| a.count).sum();
    if abort_total > 0 {
        let abort_pct = abort_total as f64 / total as f64 * 100.0;
        println!("  Aborts:         {:>6} ({:.1}%)", abort_total, abort_pct);
        for abort in &report.outcomes.aborts {
            let loc = abort.location.as_deref().unwrap_or("unknown");
            println!(
                "    code {:>5}:    {:>6}  at {}",
                abort.code, abort.count, loc
            );
        }
    }

    if report.outcomes.gas_exhaustions > 0 {
        let gas_pct = report.outcomes.gas_exhaustions as f64 / total as f64 * 100.0;
        println!(
            "  Gas exhaustion: {:>6} ({:.1}%)",
            report.outcomes.gas_exhaustions, gas_pct
        );
    }

    let error_total: u64 = report.outcomes.errors.iter().map(|e| e.count).sum();
    if error_total > 0 {
        let err_pct = error_total as f64 / total as f64 * 100.0;
        println!("  Errors:         {:>6} ({:.1}%)", error_total, err_pct);
    }

    // Gas profile
    println!();
    println!("Gas profile:");
    println!(
        "  min: {}  max: {}  avg: {}  p50: {}  p99: {}",
        report.gas_profile.min,
        report.gas_profile.max,
        report.gas_profile.avg,
        report.gas_profile.p50,
        report.gas_profile.p99
    );
    if !report.gas_profile.max_input.is_empty() {
        println!(
            "  max gas input: [{}]",
            report.gas_profile.max_input.join(", ")
        );
    }

    // Interesting cases
    if !report.interesting_cases.is_empty() {
        println!();
        println!("Interesting cases:");
        for case in &report.interesting_cases {
            let outcome_str = match &case.outcome {
                Outcome::Abort { code, location } => {
                    let loc = location.as_deref().unwrap_or("");
                    format!("abort({code}) {loc}")
                }
                Outcome::Error { message } => {
                    let short = if message.len() > 80 {
                        format!("{}...", &message[..80])
                    } else {
                        message.clone()
                    };
                    format!("error: {short}")
                }
                Outcome::GasExhaustion => "gas exhaustion".into(),
                Outcome::Success => "success".into(),
            };
            println!(
                "  [iter {}] {} — inputs: [{}]",
                case.iteration,
                outcome_str,
                case.inputs_human.join(", ")
            );

            // Print reproduce command for abort/error cases
            if !matches!(case.outcome, Outcome::Success) {
                let args: Vec<String> = case
                    .inputs_human
                    .iter()
                    .map(|a| format!("--arg {a}"))
                    .collect();
                println!(
                    "    Reproduce: sui-sandbox run {} {}",
                    report.target,
                    args.join(" ")
                );
            }
        }
    }
}
