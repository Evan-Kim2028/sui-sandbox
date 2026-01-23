//! Run command - execute a single Move function call

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;

use super::output::{format_effects, format_effects_json, format_error};
use super::SandboxState;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, PTBExecutor};

#[derive(Parser, Debug)]
pub struct RunCmd {
    /// Target function: "0xPKG::module::function" or "module::function" (uses last published)
    pub target: String,

    /// Arguments (auto-parsed: 42, true, 0xABC, "string", b"bytes")
    #[arg(long = "arg", num_args(1..))]
    pub args: Vec<String>,

    /// Type arguments (e.g., "0x2::sui::SUI")
    #[arg(long = "type-arg", num_args(1..))]
    pub type_args: Vec<String>,

    /// Sender address (default: 0x0)
    #[arg(long, default_value = "0x0")]
    pub sender: String,

    /// Gas budget (0 = unlimited)
    #[arg(long, default_value = "0")]
    pub gas_budget: u64,
}

impl RunCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let result = self.execute_inner(state).await;

        match result {
            Ok(effects) => {
                if json_output {
                    println!("{}", format_effects_json(&effects));
                } else {
                    println!("{}", format_effects(&effects, verbose));
                }

                if effects.success {
                    Ok(())
                } else {
                    Err(anyhow!(effects
                        .error
                        .unwrap_or_else(|| "Execution failed".to_string())))
                }
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                Err(e)
            }
        }
    }

    async fn execute_inner(
        &self,
        state: &mut SandboxState,
    ) -> Result<sui_sandbox_core::ptb::TransactionEffects> {
        // Parse target
        let (package, module, function) = parse_target(&self.target, state)?;

        // Parse sender
        let sender =
            AccountAddress::from_hex_literal(&self.sender).context("Invalid sender address")?;
        state.set_last_sender(sender);

        // Parse type arguments
        let type_args = self
            .type_args
            .iter()
            .map(|s| parse_type_tag(s))
            .collect::<Result<Vec<_>>>()?;

        // Parse arguments and build inputs
        let (inputs, args) = parse_arguments(&self.args)?;

        // Create harness with sender and gas budget
        let gas_budget = if self.gas_budget > 0 {
            Some(self.gas_budget)
        } else {
            None
        };
        let mut harness = state.create_harness_with_sender(sender, gas_budget)?;

        // Create executor
        let mut executor = PTBExecutor::new(&mut harness);

        // Add inputs
        for input in &inputs {
            executor.add_input(input.clone());
        }

        // Build MoveCall command
        let command = Command::MoveCall {
            package,
            module: Identifier::new(module).context("Invalid module name")?,
            function: Identifier::new(function).context("Invalid function name")?,
            type_args,
            args,
        };

        // Execute
        let effects = executor.execute_commands(&[command])?;

        Ok(effects)
    }
}

/// Parse a target string like "0xPKG::module::function" or "module::function"
fn parse_target(target: &str, state: &SandboxState) -> Result<(AccountAddress, String, String)> {
    let parts: Vec<&str> = target.split("::").collect();

    match parts.len() {
        2 => {
            // module::function - use last published package
            let package = state.last_published().ok_or_else(|| {
                anyhow!("No package published. Use full target format: 0xPKG::module::function")
            })?;
            Ok((package, parts[0].to_string(), parts[1].to_string()))
        }
        3 => {
            // 0xPKG::module::function
            let package = AccountAddress::from_hex_literal(parts[0])
                .context("Invalid package address in target")?;
            Ok((package, parts[1].to_string(), parts[2].to_string()))
        }
        _ => Err(anyhow!(
            "Invalid target format. Expected '0xPKG::module::function' or 'module::function'"
        )),
    }
}

/// Parse a type tag string
fn parse_type_tag(s: &str) -> Result<TypeTag> {
    sui_sandbox_core::types::parse_type_tag(s)
        .map_err(|e| anyhow!("Invalid type tag '{}': {}", s, e))
}

/// Parse command line arguments into PTB inputs and argument references
fn parse_arguments(args: &[String]) -> Result<(Vec<InputValue>, Vec<Argument>)> {
    let mut inputs = Vec::new();
    let mut arguments = Vec::new();

    for (i, arg) in args.iter().enumerate() {
        let input = parse_single_arg(arg)?;
        inputs.push(input);
        arguments.push(Argument::Input(i as u16));
    }

    Ok((inputs, arguments))
}

/// Parse a single argument string into an InputValue
fn parse_single_arg(arg: &str) -> Result<InputValue> {
    let arg = arg.trim();

    // Boolean
    if arg == "true" {
        return Ok(InputValue::Pure(bcs::to_bytes(&true)?));
    }
    if arg == "false" {
        return Ok(InputValue::Pure(bcs::to_bytes(&false)?));
    }

    // Address (0x prefixed)
    if arg.starts_with("0x") || arg.starts_with("0X") {
        // Try as address first
        if let Ok(addr) = AccountAddress::from_hex_literal(arg) {
            return Ok(InputValue::Pure(bcs::to_bytes(&addr)?));
        }
    }

    // String (quoted)
    if (arg.starts_with('"') && arg.ends_with('"'))
        || (arg.starts_with('\'') && arg.ends_with('\''))
    {
        let s = &arg[1..arg.len() - 1];
        return Ok(InputValue::Pure(bcs::to_bytes(&s.as_bytes().to_vec())?));
    }

    // Byte vector (b"..." or x"...")
    if arg.starts_with("b\"") && arg.ends_with('"') {
        let s = &arg[2..arg.len() - 1];
        return Ok(InputValue::Pure(bcs::to_bytes(&s.as_bytes().to_vec())?));
    }
    if arg.starts_with("x\"") && arg.ends_with('"') {
        let hex_str = &arg[2..arg.len() - 1];
        let bytes = hex::decode(hex_str).context("Invalid hex in x\"...\"")?;
        return Ok(InputValue::Pure(bcs::to_bytes(&bytes)?));
    }

    // Vector of addresses ([@0x1, @0x2])
    if arg.starts_with("[@") && arg.ends_with(']') {
        let inner = &arg[1..arg.len() - 1];
        let addrs: Result<Vec<AccountAddress>> = inner
            .split(',')
            .map(|s| {
                let s = s.trim().trim_start_matches('@');
                AccountAddress::from_hex_literal(s).context("Invalid address in vector")
            })
            .collect();
        return Ok(InputValue::Pure(bcs::to_bytes(&addrs?)?));
    }

    // Try as u64
    if let Ok(n) = arg.parse::<u64>() {
        return Ok(InputValue::Pure(bcs::to_bytes(&n)?));
    }

    // Try as u128 (for large numbers)
    if let Ok(n) = arg.parse::<u128>() {
        return Ok(InputValue::Pure(bcs::to_bytes(&n)?));
    }

    // Try as i64 (negative numbers)
    if let Ok(n) = arg.parse::<i64>() {
        // Convert to u64 representation (two's complement for Move's unsigned types)
        let u = n as u64;
        return Ok(InputValue::Pure(bcs::to_bytes(&u)?));
    }

    // Explicit type annotations: u8:42, u16:1000, etc.
    if let Some((type_prefix, value)) = arg.split_once(':') {
        return parse_typed_value(type_prefix, value);
    }

    Err(anyhow!(
        "Could not parse argument '{}'. Supported formats: numbers, true/false, \"string\", 0xADDRESS, u8:N, u64:N, etc.",
        arg
    ))
}

/// Parse a typed value like "u8:42" or "u256:123"
fn parse_typed_value(type_prefix: &str, value: &str) -> Result<InputValue> {
    match type_prefix {
        "u8" => {
            let n: u8 = value.parse().context("Invalid u8 value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&n)?))
        }
        "u16" => {
            let n: u16 = value.parse().context("Invalid u16 value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&n)?))
        }
        "u32" => {
            let n: u32 = value.parse().context("Invalid u32 value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&n)?))
        }
        "u64" => {
            let n: u64 = value.parse().context("Invalid u64 value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&n)?))
        }
        "u128" => {
            let n: u128 = value.parse().context("Invalid u128 value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&n)?))
        }
        "u256" => {
            // Parse as big integer string
            let n = move_core_types::u256::U256::from_str_radix(value, 10)
                .context("Invalid u256 value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&n)?))
        }
        "bool" => {
            let b: bool = value.parse().context("Invalid bool value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&b)?))
        }
        "address" => {
            let addr = AccountAddress::from_hex_literal(value).context("Invalid address value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&addr)?))
        }
        "string" | "utf8" => {
            Ok(InputValue::Pure(bcs::to_bytes(&value.as_bytes().to_vec())?))
        }
        "hex" => {
            let bytes = hex::decode(value).context("Invalid hex value")?;
            Ok(InputValue::Pure(bcs::to_bytes(&bytes)?))
        }
        _ => Err(anyhow!("Unknown type prefix '{}'. Supported: u8, u16, u32, u64, u128, u256, bool, address, string, hex", type_prefix)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_arg_bool() {
        let input = parse_single_arg("true").unwrap();
        if let InputValue::Pure(bytes) = input {
            let val: bool = bcs::from_bytes(&bytes).unwrap();
            assert!(val);
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_parse_single_arg_number() {
        let input = parse_single_arg("42").unwrap();
        if let InputValue::Pure(bytes) = input {
            let val: u64 = bcs::from_bytes(&bytes).unwrap();
            assert_eq!(val, 42);
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_parse_single_arg_address() {
        let input = parse_single_arg("0x123").unwrap();
        if let InputValue::Pure(bytes) = input {
            let val: AccountAddress = bcs::from_bytes(&bytes).unwrap();
            assert_eq!(val, AccountAddress::from_hex_literal("0x123").unwrap());
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_parse_single_arg_string() {
        let input = parse_single_arg("\"hello\"").unwrap();
        if let InputValue::Pure(bytes) = input {
            let val: Vec<u8> = bcs::from_bytes(&bytes).unwrap();
            assert_eq!(val, b"hello".to_vec());
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_parse_typed_value_u8() {
        let input = parse_typed_value("u8", "255").unwrap();
        if let InputValue::Pure(bytes) = input {
            let val: u8 = bcs::from_bytes(&bytes).unwrap();
            assert_eq!(val, 255);
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_parse_target_full() {
        // This test doesn't need state since we're using full format
        let target = "0x123::mymodule::myfunction";
        let parts: Vec<&str> = target.split("::").collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "0x123");
        assert_eq!(parts[1], "mymodule");
        assert_eq!(parts[2], "myfunction");
    }

    #[test]
    fn test_parse_type_tag() {
        let tag = parse_type_tag("0x2::sui::SUI").unwrap();
        assert!(tag.to_string().contains("SUI"));
    }
}
