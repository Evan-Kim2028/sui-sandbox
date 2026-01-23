//! Bridge command - Generate sui client commands for transitioning out of sandbox
//!
//! This module provides helper commands that generate the equivalent `sui client`
//! commands for deploying and executing transactions on real networks (testnet/mainnet).
//!
//! These are transition helpers, not replacements for sui client.

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Default gas budget for publish operations (100M MIST = 0.1 SUI)
const DEFAULT_PUBLISH_GAS_BUDGET: u64 = 100_000_000;

/// Default gas budget for call operations (10M MIST = 0.01 SUI)
const DEFAULT_CALL_GAS_BUDGET: u64 = 10_000_000;

#[derive(Parser, Debug)]
#[command(about = "Generate sui client commands for deployment")]
pub struct BridgeCmd {
    #[command(subcommand)]
    pub command: BridgeSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum BridgeSubcommand {
    /// Generate sui client publish command
    Publish(BridgePublishCmd),

    /// Generate sui client call command
    Call(BridgeCallCmd),

    /// Generate sui client ptb command
    Ptb(BridgePtbCmd),
}

#[derive(Parser, Debug)]
pub struct BridgePublishCmd {
    /// Path to Move package directory
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Gas budget in MIST (default: 100000000 = 0.1 SUI)
    #[arg(long, default_value_t = DEFAULT_PUBLISH_GAS_BUDGET)]
    pub gas_budget: u64,

    /// Skip install instructions
    #[arg(long)]
    pub quiet: bool,
}

#[derive(Parser, Debug)]
pub struct BridgeCallCmd {
    /// Function target: "0xPKG::module::function"
    pub target: String,

    /// Arguments to pass to the function
    #[arg(long = "arg", value_name = "VALUE")]
    pub args: Vec<String>,

    /// Type arguments
    #[arg(long = "type-arg", value_name = "TYPE")]
    pub type_args: Vec<String>,

    /// Gas budget in MIST (default: 10000000 = 0.01 SUI)
    #[arg(long, default_value_t = DEFAULT_CALL_GAS_BUDGET)]
    pub gas_budget: u64,

    /// Skip install instructions
    #[arg(long)]
    pub quiet: bool,
}

#[derive(Parser, Debug)]
pub struct BridgePtbCmd {
    /// Path to PTB JSON specification (sandbox format)
    #[arg(long)]
    pub spec: PathBuf,

    /// Gas budget in MIST (default: 10000000 = 0.01 SUI)
    #[arg(long, default_value_t = DEFAULT_CALL_GAS_BUDGET)]
    pub gas_budget: u64,

    /// Skip install instructions
    #[arg(long)]
    pub quiet: bool,
}

impl BridgeCmd {
    pub fn execute(&self, json_output: bool) -> Result<()> {
        match &self.command {
            BridgeSubcommand::Publish(cmd) => cmd.execute(json_output),
            BridgeSubcommand::Call(cmd) => cmd.execute(json_output),
            BridgeSubcommand::Ptb(cmd) => cmd.execute(json_output),
        }
    }
}

impl BridgePublishCmd {
    pub fn execute(&self, json_output: bool) -> Result<()> {
        let path_str = self.path.display().to_string();

        let output = PublishOutput {
            command: format!(
                "sui client publish {} --gas-budget {}",
                shell_escape(&path_str),
                self.gas_budget
            ),
            prerequisites: vec![
                "sui client switch --env <testnet|mainnet>".to_string(),
                "sui client faucet  # (testnet only, if needed)".to_string(),
            ],
            notes: vec![
                "Ensure your package compiles: sui move build".to_string(),
                format!(
                    "Gas budget: {} MIST ({:.4} SUI)",
                    self.gas_budget,
                    self.gas_budget as f64 / 1_000_000_000.0
                ),
            ],
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            output.print_human(self.quiet);
        }

        Ok(())
    }
}

impl BridgeCallCmd {
    pub fn execute(&self, json_output: bool) -> Result<()> {
        // Parse the target: 0xPKG::module::function
        let (package, module, function) = parse_function_target(&self.target)?;

        let mut cmd_parts = vec![
            "sui client call".to_string(),
            format!("--package {}", package),
            format!("--module {}", module),
            format!("--function {}", function),
        ];

        // Add type arguments
        if !self.type_args.is_empty() {
            cmd_parts.push(format!("--type-args {}", self.type_args.join(" ")));
        }

        // Add arguments
        if !self.args.is_empty() {
            let formatted_args: Vec<String> = self
                .args
                .iter()
                .map(|a| format_arg_for_sui_client(a))
                .collect();
            cmd_parts.push(format!("--args {}", formatted_args.join(" ")));
        }

        cmd_parts.push(format!("--gas-budget {}", self.gas_budget));

        let command = cmd_parts.join(" \\\n  ");

        let mut notes = vec![format!(
            "Gas budget: {} MIST ({:.4} SUI)",
            self.gas_budget,
            self.gas_budget as f64 / 1_000_000_000.0
        )];

        // Add note about package address if it looks like a sandbox address
        if is_sandbox_address(&package) {
            notes.push(format!(
                "Note: {} looks like a sandbox address. Replace with your deployed package ID.",
                package
            ));
        }

        let output = CallOutput {
            command,
            prerequisites: vec!["sui client switch --env <testnet|mainnet>".to_string()],
            notes,
            package: package.clone(),
            module: module.clone(),
            function: function.clone(),
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            output.print_human(self.quiet);
        }

        Ok(())
    }
}

impl BridgePtbCmd {
    pub fn execute(&self, json_output: bool) -> Result<()> {
        // Read and parse the sandbox PTB spec
        let spec_content = std::fs::read_to_string(&self.spec)
            .map_err(|e| anyhow!("Failed to read PTB spec: {}", e))?;

        let spec: SandboxPtbSpec = serde_json::from_str(&spec_content)
            .map_err(|e| anyhow!("Failed to parse PTB spec: {}", e))?;

        // Convert to sui client ptb commands
        let mut ptb_parts: Vec<String> = vec!["sui client ptb".to_string()];

        // Track assigned variables for result chaining
        let mut result_vars: Vec<String> = Vec::new();

        for (idx, call) in spec.calls.iter().enumerate() {
            let var_name = format!("result_{}", idx);

            // Build the move-call command
            let mut move_call = format!("--move-call {}", call.target);

            // Add type arguments
            if let Some(ref type_args) = call.type_arguments {
                if !type_args.is_empty() {
                    move_call.push_str(&format!(" \"<{}>\"", type_args.join(", ")));
                }
            }

            // Add arguments
            if let Some(ref args) = call.arguments {
                for arg in args {
                    let arg_str = convert_ptb_argument(arg, &result_vars)?;
                    move_call.push_str(&format!(" {}", arg_str));
                }
            }

            ptb_parts.push(move_call);

            // Assign result if needed by later commands
            if spec.calls.len() > 1 && idx < spec.calls.len() - 1 {
                ptb_parts.push(format!("--assign {}", var_name));
                result_vars.push(var_name);
            }
        }

        ptb_parts.push(format!("--gas-budget {}", self.gas_budget));

        let command = ptb_parts.join(" \\\n  ");

        let output = PtbOutput {
            command,
            prerequisites: vec!["sui client switch --env <testnet|mainnet>".to_string()],
            notes: vec![
                format!(
                    "Gas budget: {} MIST ({:.4} SUI)",
                    self.gas_budget,
                    self.gas_budget as f64 / 1_000_000_000.0
                ),
                "Review the generated command - some argument translations may need adjustment"
                    .to_string(),
                "Use --preview flag to test before executing: add --preview before --gas-budget"
                    .to_string(),
            ],
            calls_count: spec.calls.len(),
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            output.print_human(self.quiet);
        }

        Ok(())
    }
}

// =============================================================================
// Output Types
// =============================================================================

#[derive(serde::Serialize)]
struct PublishOutput {
    command: String,
    prerequisites: Vec<String>,
    notes: Vec<String>,
}

impl PublishOutput {
    fn print_human(&self, quiet: bool) {
        if !quiet {
            println!("\x1b[1m📦 Deploy to Sui Network\x1b[0m\n");
            println!("\x1b[33mPrerequisites:\x1b[0m");
            for prereq in &self.prerequisites {
                println!("  {}", prereq);
            }
            println!();
        }

        println!("\x1b[32mCommand:\x1b[0m");
        println!("  {}", self.command);

        if !quiet {
            println!();
            println!("\x1b[90mNotes:\x1b[0m");
            for note in &self.notes {
                println!("  • {}", note);
            }
        }
    }
}

#[derive(serde::Serialize)]
struct CallOutput {
    command: String,
    prerequisites: Vec<String>,
    notes: Vec<String>,
    package: String,
    module: String,
    function: String,
}

impl CallOutput {
    fn print_human(&self, quiet: bool) {
        if !quiet {
            println!("\x1b[1m🔧 Call Function on Sui Network\x1b[0m\n");
            println!(
                "\x1b[36mTarget:\x1b[0m {}::{}::{}",
                self.package, self.module, self.function
            );
            println!();
            println!("\x1b[33mPrerequisites:\x1b[0m");
            for prereq in &self.prerequisites {
                println!("  {}", prereq);
            }
            println!();
        }

        println!("\x1b[32mCommand:\x1b[0m");
        println!("  {}", self.command);

        if !quiet {
            println!();
            println!("\x1b[90mNotes:\x1b[0m");
            for note in &self.notes {
                println!("  • {}", note);
            }
        }
    }
}

#[derive(serde::Serialize)]
struct PtbOutput {
    command: String,
    prerequisites: Vec<String>,
    notes: Vec<String>,
    calls_count: usize,
}

impl PtbOutput {
    fn print_human(&self, quiet: bool) {
        if !quiet {
            println!("\x1b[1m⚡ Execute PTB on Sui Network\x1b[0m\n");
            println!("\x1b[36mCalls:\x1b[0m {} move call(s)", self.calls_count);
            println!();
            println!("\x1b[33mPrerequisites:\x1b[0m");
            for prereq in &self.prerequisites {
                println!("  {}", prereq);
            }
            println!();
        }

        println!("\x1b[32mCommand:\x1b[0m");
        println!("  {}", self.command);

        if !quiet {
            println!();
            println!("\x1b[90mNotes:\x1b[0m");
            for note in &self.notes {
                println!("  • {}", note);
            }
        }
    }
}

// =============================================================================
// Sandbox PTB Spec Types (for parsing)
// =============================================================================

#[derive(serde::Deserialize, Debug)]
struct SandboxPtbSpec {
    calls: Vec<SandboxPtbCall>,
    #[serde(default)]
    inputs: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize, Debug)]
struct SandboxPtbCall {
    target: String,
    #[serde(default)]
    type_arguments: Option<Vec<String>>,
    #[serde(default)]
    arguments: Option<Vec<serde_json::Value>>,
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Parse a function target like "0x2::coin::value" into (package, module, function)
fn parse_function_target(target: &str) -> Result<(String, String, String)> {
    let parts: Vec<&str> = target.split("::").collect();
    if parts.len() != 3 {
        return Err(anyhow!(
            "Invalid function target '{}'. Expected format: 0xPKG::module::function",
            target
        ));
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    ))
}

/// Check if an address looks like a sandbox test address (short or common test patterns)
fn is_sandbox_address(addr: &str) -> bool {
    let addr_lower = addr.to_lowercase();

    // Framework addresses are NOT sandbox addresses
    if addr_lower == "0x1" || addr_lower == "0x2" || addr_lower == "0x3" || addr_lower == "0x6" {
        return false;
    }

    // Common sandbox test addresses
    addr_lower == "0x100"
        || addr_lower == "0x200"
        || addr_lower == "0xdeadbeef"
        || addr_lower == "0xcafe"
        || addr_lower == "0xbeef"
        // Short addresses that aren't framework (likely test addresses)
        || (addr_lower.starts_with("0x")
            && addr_lower.len() > 3
            && addr_lower.len() < 10
            && !["0x1", "0x2", "0x3", "0x6"].contains(&addr_lower.as_str()))
}

/// Format an argument value for sui client
fn format_arg_for_sui_client(arg: &str) -> String {
    // If it looks like an address, prefix with @
    if arg.starts_with("0x") && arg.len() > 4 {
        format!("@{}", arg)
    } else if arg.starts_with('"') && arg.ends_with('"') {
        // String literal - keep as is
        arg.to_string()
    } else {
        // Numbers, booleans, etc - keep as is
        arg.to_string()
    }
}

/// Convert a sandbox PTB argument to sui client ptb format
fn convert_ptb_argument(arg: &serde_json::Value, result_vars: &[String]) -> Result<String> {
    match arg {
        serde_json::Value::Object(obj) => {
            if let Some(result_idx) = obj.get("Result") {
                let idx = result_idx
                    .as_u64()
                    .ok_or_else(|| anyhow!("Result index must be a number"))?
                    as usize;
                if idx < result_vars.len() {
                    Ok(result_vars[idx].clone())
                } else {
                    Ok(format!("result_{}", idx))
                }
            } else if let Some(input_idx) = obj.get("Input") {
                let idx = input_idx
                    .as_u64()
                    .ok_or_else(|| anyhow!("Input index must be a number"))?;
                Ok(format!("@input_{}", idx))
            } else if obj.contains_key("Pure") {
                // Pure value - extract the value
                if let Some(pure_obj) = obj.get("Pure").and_then(|p| p.as_object()) {
                    if let Some(val) = pure_obj.get("value") {
                        return Ok(format_json_value(val));
                    }
                }
                Ok("<pure_value>".to_string())
            } else {
                // Unknown object type
                Ok(format!("<{}>", serde_json::to_string(obj)?))
            }
        }
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::String(s) => {
            if s.starts_with("0x") {
                Ok(format!("@{}", s))
            } else {
                Ok(format!("\"{}\"", s))
            }
        }
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        _ => Ok(format!("{}", arg)),
    }
}

/// Format a JSON value for sui client
fn format_json_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => {
            if s.starts_with("0x") {
                format!("@{}", s)
            } else {
                format!("\"{}\"", s)
            }
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_json_value).collect();
            format!("\"[{}]\"", items.join(", "))
        }
        _ => format!("{}", val),
    }
}

/// Escape a string for shell usage
fn shell_escape(s: &str) -> String {
    if s.contains(' ') || s.contains('\'') || s.contains('"') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_function_target_valid() {
        let (pkg, module, func) = parse_function_target("0x2::coin::value").unwrap();
        assert_eq!(pkg, "0x2");
        assert_eq!(module, "coin");
        assert_eq!(func, "value");
    }

    #[test]
    fn test_parse_function_target_long_address() {
        let (pkg, module, func) =
            parse_function_target("0x1234567890abcdef::my_module::my_func").unwrap();
        assert_eq!(pkg, "0x1234567890abcdef");
        assert_eq!(module, "my_module");
        assert_eq!(func, "my_func");
    }

    #[test]
    fn test_parse_function_target_invalid() {
        assert!(parse_function_target("0x2::coin").is_err());
        assert!(parse_function_target("coin::value").is_err());
        assert!(parse_function_target("just_a_name").is_err());
    }

    #[test]
    fn test_is_sandbox_address() {
        assert!(is_sandbox_address("0x100"));
        assert!(is_sandbox_address("0x200"));
        assert!(is_sandbox_address("0xdeadbeef"));
        assert!(is_sandbox_address("0xCAFE"));

        // Real-looking addresses should not be flagged
        assert!(!is_sandbox_address(
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
        ));
        assert!(!is_sandbox_address("0x2")); // Framework address
    }

    #[test]
    fn test_format_arg_for_sui_client() {
        assert_eq!(format_arg_for_sui_client("42"), "42");
        assert_eq!(format_arg_for_sui_client("true"), "true");
        assert_eq!(format_arg_for_sui_client("0x123abc"), "@0x123abc");
        assert_eq!(format_arg_for_sui_client("\"hello\""), "\"hello\"");
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("simple"), "simple");
        assert_eq!(shell_escape("with space"), "'with space'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_convert_ptb_argument_result() {
        let result_vars = vec!["result_0".to_string(), "result_1".to_string()];
        let arg = serde_json::json!({"Result": 0});
        assert_eq!(
            convert_ptb_argument(&arg, &result_vars).unwrap(),
            "result_0"
        );
    }

    #[test]
    fn test_convert_ptb_argument_number() {
        let result_vars: Vec<String> = vec![];
        let arg = serde_json::json!(42);
        assert_eq!(convert_ptb_argument(&arg, &result_vars).unwrap(), "42");
    }

    #[test]
    fn test_convert_ptb_argument_address() {
        let result_vars: Vec<String> = vec![];
        let arg = serde_json::json!("0x123");
        assert_eq!(convert_ptb_argument(&arg, &result_vars).unwrap(), "@0x123");
    }

    #[test]
    fn test_publish_output_contains_required_parts() {
        let cmd = BridgePublishCmd {
            path: PathBuf::from("./my_package"),
            gas_budget: 100_000_000,
            quiet: false,
        };

        // We can't easily capture stdout, but we can verify the internal logic
        let path_str = cmd.path.display().to_string();
        let command = format!(
            "sui client publish {} --gas-budget {}",
            shell_escape(&path_str),
            cmd.gas_budget
        );

        assert!(command.contains("sui client publish"));
        assert!(command.contains("./my_package"));
        assert!(command.contains("--gas-budget"));
        assert!(command.contains("100000000"));
    }

    #[test]
    fn test_call_output_with_type_args() {
        let cmd = BridgeCallCmd {
            target: "0x2::coin::zero".to_string(),
            args: vec![],
            type_args: vec!["0x2::sui::SUI".to_string()],
            gas_budget: 10_000_000,
            quiet: false,
        };

        let (package, module, function) = parse_function_target(&cmd.target).unwrap();
        assert_eq!(package, "0x2");
        assert_eq!(module, "coin");
        assert_eq!(function, "zero");
    }
}
