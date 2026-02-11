//! Fuzzing execution loop.
//!
//! Runs N iterations of random input generation + VM execution,
//! collecting outcomes and gas statistics.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;

use crate::ptb::{Argument, Command, InputValue, PTBExecutor};
use crate::resolver::LocalModuleResolver;
use crate::vm::{SimulationConfig, VMHarness};

use super::classifier::{ClassifiedFunction, ParamClass, PureType};
use super::report::*;
use super::value_gen::ValueGenerator;

/// Configuration for a fuzz run.
pub struct FuzzConfig {
    /// Number of iterations to run.
    pub iterations: u64,
    /// Random seed for reproducibility.
    pub seed: u64,
    /// Sender address for transactions.
    pub sender: AccountAddress,
    /// Gas budget per execution.
    pub gas_budget: u64,
    /// Type arguments for generic functions.
    pub type_args: Vec<TypeTag>,
    /// Stop on first abort/error.
    pub fail_fast: bool,
    /// Maximum vector length for generated inputs.
    pub max_vector_len: usize,
}

/// Runs fuzz iterations against the local Move VM.
pub struct FuzzRunner<'a> {
    resolver: &'a LocalModuleResolver,
}

impl<'a> FuzzRunner<'a> {
    pub fn new(resolver: &'a LocalModuleResolver) -> Self {
        Self { resolver }
    }

    /// Run the fuzzer against a target function.
    ///
    /// Returns a complete FuzzReport with outcomes, gas profile, and interesting cases.
    pub fn run(
        &self,
        package: AccountAddress,
        module_name: &str,
        function_name: &str,
        classification: &ClassifiedFunction,
        config: &FuzzConfig,
    ) -> Result<FuzzReport> {
        let target = format!(
            "{}::{}::{}",
            package.to_hex_literal(),
            module_name,
            function_name
        );

        // Collect the pure parameter types (in order), skipping system-injected ones
        let pure_params: Vec<(usize, &PureType)> = classification
            .params
            .iter()
            .enumerate()
            .filter_map(|(i, (_, class))| {
                if let ParamClass::Pure { pure_type } = class {
                    Some((i, pure_type))
                } else {
                    None
                }
            })
            .collect();

        let mut gen = ValueGenerator::new(config.seed, config.max_vector_len);
        let mut successes = 0u64;
        let mut gas_exhaustions = 0u64;
        let mut abort_map: HashMap<u64, AbortInfo> = HashMap::new();
        let mut error_map: HashMap<String, u64> = HashMap::new();
        let mut gas_values: Vec<u64> = Vec::with_capacity(config.iterations as usize);
        let mut max_gas_input: Vec<String> = Vec::new();
        let mut max_gas_value = 0u64;
        let mut interesting_cases: Vec<InterestingCase> = Vec::new();
        let mut completed = 0u64;

        let module_ident = Identifier::new(module_name)
            .map_err(|e| anyhow!("Invalid module name '{}': {}", module_name, e))?;
        let function_ident = Identifier::new(function_name)
            .map_err(|e| anyhow!("Invalid function name '{}': {}", function_name, e))?;

        let start = Instant::now();

        for iteration in 0..config.iterations {
            // Generate random pure inputs
            let mut input_values: Vec<InputValue> = Vec::new();
            let mut input_human: Vec<String> = Vec::new();
            let mut input_bcs_hex: Vec<String> = Vec::new();

            for (_param_idx, pure_type) in &pure_params {
                let bcs_bytes = gen.generate(pure_type);
                input_human.push(ValueGenerator::format_value(pure_type, &bcs_bytes));
                input_bcs_hex.push(hex::encode(&bcs_bytes));
                input_values.push(InputValue::Pure(bcs_bytes));
            }

            // Create fresh VM harness per iteration
            let sim_config = SimulationConfig {
                sender_address: config.sender.into(),
                gas_budget: Some(config.gas_budget),
                deterministic_random: true,
                mock_crypto_pass: true,
                ..Default::default()
            };
            let mut harness = match VMHarness::with_config(self.resolver, false, sim_config) {
                Ok(h) => h,
                Err(e) => {
                    return Err(anyhow!("Failed to create VM harness: {}", e));
                }
            };

            // Create executor and add inputs
            let mut executor = PTBExecutor::new(&mut harness);
            for input in &input_values {
                executor.add_input(input.clone());
            }

            // Build MoveCall command
            let args: Vec<Argument> = (0..input_values.len())
                .map(|i| Argument::Input(i as u16))
                .collect();
            let command = Command::MoveCall {
                package,
                module: module_ident.clone(),
                function: function_ident.clone(),
                type_args: config.type_args.clone(),
                args,
            };

            // Execute
            let effects = executor.execute_commands(&[command]);

            // Classify outcome
            let (outcome, gas_used) = match effects {
                Ok(effects) => {
                    let gas = effects.gas_used;
                    if effects.success {
                        (Outcome::Success, gas)
                    } else {
                        let err_msg = effects.error.unwrap_or_default();
                        classify_error(&err_msg, gas)
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    classify_error(&err_msg, 0)
                }
            };

            // Track gas
            gas_values.push(gas_used);
            if gas_used > max_gas_value {
                max_gas_value = gas_used;
                max_gas_input = input_human.clone();
            }

            // Update counters
            match &outcome {
                Outcome::Success => {
                    successes += 1;
                }
                Outcome::Abort { code, location } => {
                    let entry = abort_map.entry(*code).or_insert_with(|| AbortInfo {
                        code: *code,
                        location: location.clone(),
                        count: 0,
                        sample_inputs: input_human.clone(),
                        sample_inputs_bcs: input_bcs_hex.clone(),
                    });
                    entry.count += 1;

                    // Record as interesting (first occurrence of this abort code)
                    if entry.count == 1 {
                        interesting_cases.push(InterestingCase {
                            iteration,
                            outcome: outcome.clone(),
                            inputs_human: input_human.clone(),
                            inputs_bcs_hex: input_bcs_hex.clone(),
                            gas_used,
                        });
                    }
                }
                Outcome::Error { message } => {
                    let key = truncate_error(message);
                    let count = error_map.entry(key.clone()).or_insert(0);
                    *count += 1;

                    // Record first occurrence
                    if *count == 1 {
                        interesting_cases.push(InterestingCase {
                            iteration,
                            outcome: outcome.clone(),
                            inputs_human: input_human.clone(),
                            inputs_bcs_hex: input_bcs_hex.clone(),
                            gas_used,
                        });
                    }
                }
                Outcome::GasExhaustion => {
                    gas_exhaustions += 1;
                    if gas_exhaustions == 1 {
                        interesting_cases.push(InterestingCase {
                            iteration,
                            outcome: outcome.clone(),
                            inputs_human: input_human.clone(),
                            inputs_bcs_hex: input_bcs_hex.clone(),
                            gas_used,
                        });
                    }
                }
            }

            completed = iteration + 1;

            // Fail-fast check
            if config.fail_fast && !matches!(outcome, Outcome::Success) {
                break;
            }
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Build abort list sorted by code
        let mut aborts: Vec<AbortInfo> = abort_map.into_values().collect();
        aborts.sort_by_key(|a| a.code);

        // Build error list sorted by count (descending)
        let mut errors: Vec<ErrorInfo> = error_map
            .into_iter()
            .map(|(message, count)| ErrorInfo { message, count })
            .collect();
        errors.sort_by(|a, b| b.count.cmp(&a.count));

        let gas_profile = GasProfile::from_values(&mut gas_values, max_gas_input);

        Ok(FuzzReport {
            target,
            total_iterations: config.iterations,
            completed_iterations: completed,
            seed: config.seed,
            elapsed_ms,
            classification: classification.clone(),
            outcomes: FuzzOutcomeSummary {
                successes,
                gas_exhaustions,
                aborts,
                errors,
            },
            gas_profile,
            interesting_cases,
        })
    }
}

/// Parse an error message to extract abort code and location.
fn classify_error(err_msg: &str, gas: u64) -> (Outcome, u64) {
    // Check for gas exhaustion
    if err_msg.contains("OutOfGas")
        || err_msg.contains("out of gas")
        || err_msg.contains("OUT_OF_GAS")
    {
        return (Outcome::GasExhaustion, gas);
    }

    // Try to extract abort code
    // Common patterns: "ABORTED with code 42", "Move abort: 42", "abort(42)"
    if let Some(code) = extract_abort_code(err_msg) {
        let location = extract_abort_location(err_msg);
        return (Outcome::Abort { code, location }, gas);
    }

    (
        Outcome::Error {
            message: err_msg.to_string(),
        },
        gas,
    )
}

/// Extract abort code from an error message.
fn extract_abort_code(msg: &str) -> Option<u64> {
    // Pattern: "with code N" or "abort code N"
    for pattern in &["with code ", "abort code ", "abort(", "ABORTED("] {
        if let Some(pos) = msg.find(pattern) {
            let after = &msg[pos + pattern.len()..];
            let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = num_str.parse::<u64>() {
                return Some(code);
            }
        }
    }

    // Pattern: "MoveAbort(..., N)" — common in Sui VM errors
    if let Some(pos) = msg.find("MoveAbort") {
        // Find the last number before the closing paren
        if let Some(paren_end) = msg[pos..].find(')') {
            let inside = &msg[pos..pos + paren_end];
            // Split by comma, take last part
            if let Some(last) = inside.rsplit(',').next() {
                let num_str: String = last.trim().chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(code) = num_str.parse::<u64>() {
                    return Some(code);
                }
            }
        }
    }

    None
}

/// Extract the abort location (module path) from an error message.
fn extract_abort_location(msg: &str) -> Option<String> {
    // Pattern: "in module 0x...::name::func" or similar
    // Look for "0x" followed by "::module::function" pattern
    if let Some(pos) = msg.find("0x") {
        let after = &msg[pos..];
        // Take characters that look like a module path
        let path: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == ':' || *c == '_' || *c == 'x')
            .collect();
        if path.contains("::") {
            return Some(path);
        }
    }
    None
}

/// Truncate an error message for grouping (first 200 chars).
fn truncate_error(msg: &str) -> String {
    if msg.len() > 200 {
        format!("{}...", &msg[..200])
    } else {
        msg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_abort_code_with_code() {
        assert_eq!(extract_abort_code("ABORTED with code 42"), Some(42));
    }

    #[test]
    fn test_extract_abort_code_move_abort() {
        assert_eq!(
            extract_abort_code("MoveAbort(0x1::vector, 1) in command 0"),
            Some(1)
        );
    }

    #[test]
    fn test_extract_abort_code_none() {
        assert_eq!(extract_abort_code("some random error"), None);
    }

    #[test]
    fn test_classify_error_gas() {
        let (outcome, _) = classify_error("OutOfGas in function call", 0);
        assert!(matches!(outcome, Outcome::GasExhaustion));
    }

    #[test]
    fn test_classify_error_abort() {
        let (outcome, _) = classify_error("ABORTED with code 5 in 0x2::math", 100);
        match outcome {
            Outcome::Abort { code, .. } => assert_eq!(code, 5),
            _ => panic!("Expected Abort"),
        }
    }

    #[test]
    fn test_truncate_error_short() {
        assert_eq!(truncate_error("short error"), "short error");
    }

    #[test]
    fn test_truncate_error_long() {
        let long = "x".repeat(300);
        let truncated = truncate_error(&long);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.len(), 203);
    }
}
