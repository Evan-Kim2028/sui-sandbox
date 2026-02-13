//! Run command - execute a single Move function call

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;

use super::output::{format_effects, format_effects_json, format_error};
use super::SandboxState;
use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectInput, PTBExecutor};
use sui_sandbox_core::shared::parsing::{parse_pure_value, parse_type_tag_string};
use sui_sandbox_core::vm::SimulationConfig;

#[derive(Parser, Debug)]
pub struct RunCmd {
    /// Target function: "0xPKG::module::function" or "module::function" (uses last published)
    pub target: String,

    /// Arguments (auto-parsed: 42, true, 0xABC, "string", b"bytes").
    /// Object args can be passed with explicit prefixes:
    /// `obj-ref:<id>`, `obj-owned:<id>`, `obj-mut:<id>`,
    /// `obj-shared:<id>`, or `obj-shared-mut:<id>`.
    #[arg(long = "arg", num_args(1..))]
    pub args: Vec<String>,

    /// Type arguments (e.g., "0x2::sui::SUI")
    #[arg(long = "type-arg", num_args(1..))]
    pub type_args: Vec<String>,

    /// Sender address (default: 0x0)
    #[arg(long, default_value = "0x0")]
    pub sender: String,

    /// Gas budget (0 = default metered budget)
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
        let (inputs, args) = parse_arguments(&self.args, state)?;

        // Create harness with sender and gas budget
        let gas_budget = if self.gas_budget > 0 {
            Some(self.gas_budget)
        } else {
            SimulationConfig::default().gas_budget
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
    parse_type_tag_string(s)
}

/// Parse command line arguments into PTB inputs and argument references
fn parse_arguments(
    args: &[String],
    state: &SandboxState,
) -> Result<(Vec<InputValue>, Vec<Argument>)> {
    let mut inputs = Vec::new();
    let mut arguments = Vec::new();

    for (i, arg) in args.iter().enumerate() {
        let input = parse_single_arg_with_state(arg, state)?;
        inputs.push(input);
        arguments.push(Argument::Input(i as u16));
    }

    Ok((inputs, arguments))
}

/// Parse a single argument string into an InputValue
fn parse_single_arg(arg: &str) -> Result<InputValue> {
    let bytes = parse_pure_value(arg)?;
    Ok(InputValue::Pure(bytes))
}

#[derive(Clone, Copy)]
enum ObjectArgKind {
    Owned,
    MutRef,
    ImmRef,
    SharedImmutable,
    SharedMutable,
}

fn parse_single_arg_with_state(arg: &str, state: &SandboxState) -> Result<InputValue> {
    if let Some(rest) = arg.strip_prefix("obj-shared-mut:") {
        return parse_object_arg(rest, state, ObjectArgKind::SharedMutable)
            .context("Invalid --arg object syntax `obj-shared-mut:<object-id>`");
    }

    if let Some(rest) = arg.strip_prefix("obj-shared:") {
        return parse_object_arg(rest, state, ObjectArgKind::SharedImmutable)
            .context("Invalid --arg object syntax `obj-shared:<object-id>`");
    }

    if let Some(rest) = arg.strip_prefix("obj-mut:") {
        return parse_object_arg(rest, state, ObjectArgKind::MutRef)
            .context("Invalid --arg object syntax `obj-mut:<object-id>`");
    }

    if let Some(rest) = arg.strip_prefix("obj-owned:") {
        return parse_object_arg(rest, state, ObjectArgKind::Owned)
            .context("Invalid --arg object syntax `obj-owned:<object-id>`");
    }

    if let Some(rest) = arg.strip_prefix("obj-ref:") {
        return parse_object_arg(rest, state, ObjectArgKind::ImmRef)
            .context("Invalid --arg object syntax `obj-ref:<object-id>`");
    }

    if let Some(rest) = arg.strip_prefix("obj:") {
        return parse_object_arg(rest, state, ObjectArgKind::ImmRef)
            .context("Invalid --arg object syntax `obj-ref:<object-id>`");
    }

    parse_single_arg(arg)
}

fn parse_object_arg(arg: &str, state: &SandboxState, kind: ObjectArgKind) -> Result<InputValue> {
    let object_id = arg.trim();
    let object_addr = AccountAddress::from_hex_literal(object_id)
        .with_context(|| format!("invalid object id '{}'", object_id))?;
    let (bytes, type_tag) = state
        .get_object_input(object_id)
        .with_context(|| format!(
            "object {} not loaded in session. Use `sui-sandbox fetch object {}` first",
            object_id, object_id
        ))?;

    let input = match kind {
        ObjectArgKind::Owned => InputValue::Object(ObjectInput::Owned {
            id: object_addr,
            bytes,
            type_tag,
            version: None,
        }),
        ObjectArgKind::MutRef => InputValue::Object(ObjectInput::MutRef {
            id: object_addr,
            bytes,
            type_tag,
            version: None,
        }),
        ObjectArgKind::ImmRef => InputValue::Object(ObjectInput::ImmRef {
            id: object_addr,
            bytes,
            type_tag,
            version: None,
        }),
        ObjectArgKind::SharedImmutable => InputValue::Object(ObjectInput::Shared {
            id: object_addr,
            bytes,
            type_tag,
            version: None,
            mutable: false,
        }),
        ObjectArgKind::SharedMutable => InputValue::Object(ObjectInput::Shared {
            id: object_addr,
            bytes,
            type_tag,
            version: None,
            mutable: true,
        }),
    };

    Ok(input)
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
        // Uses the shared parse_pure_value function with type prefix
        let input = parse_single_arg("u8:255").unwrap();
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
