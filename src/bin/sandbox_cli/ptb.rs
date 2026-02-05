//! PTB command - execute Programmable Transaction Blocks from JSON specs

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use std::path::PathBuf;

use super::output::{format_effects, format_effects_json, format_error};
use super::ptb_spec::{read_ptb_spec, ArgReference, ArgSpec, InputSpec, PtbSpec, PureValue};
use super::SandboxState;
use sui_sandbox_core::ptb::{
    validate_ptb, Argument, Command, InputValue, ObjectInput, PTBExecutor, ValidationResult,
};
use sui_sandbox_core::shared::parsing::parse_type_tag_string;

#[derive(Parser, Debug)]
pub struct PtbCmd {
    /// JSON spec file (use '-' for stdin)
    #[arg(long)]
    pub spec: PathBuf,

    /// Sender address
    #[arg(long)]
    pub sender: String,

    /// Gas budget (default: 10_000_000)
    #[arg(long, default_value = "10000000")]
    pub gas_budget: u64,
}

impl PtbCmd {
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
        // Parse sender
        let sender =
            AccountAddress::from_hex_literal(&self.sender).context("Invalid sender address")?;
        state.set_last_sender(sender);

        let spec = read_ptb_spec(&self.spec, true)?;
        let (inputs, commands) = convert_spec(&spec, state)?;
        let validation = validate_ptb(&commands, inputs.len());
        if !validation.valid {
            return Err(anyhow!(format_validation_errors(&validation, &spec)));
        }

        // Create harness with sender and gas budget
        let mut harness = state.create_harness_with_sender(sender, Some(self.gas_budget))?;

        // Create executor
        let mut executor = PTBExecutor::new(&mut harness);

        // Add inputs
        for input in &inputs {
            executor.add_input(input.clone());
        }

        // Execute
        let effects = executor.execute_commands(&commands)?;

        Ok(effects)
    }
}

/// Convert a PtbSpec to PTB inputs and commands
fn convert_spec(spec: &PtbSpec, state: &SandboxState) -> Result<(Vec<InputValue>, Vec<Command>)> {
    let mut inputs = Vec::new();
    let mut commands = Vec::new();

    // Convert explicit inputs
    for (idx, input_spec) in spec.inputs.iter().enumerate() {
        let input = convert_input_spec(input_spec, state)
            .with_context(|| format!("Input {} (spec.inputs[{}])", idx, idx))?;
        inputs.push(input);
    }

    // Track the next available input index for inline args
    let mut next_input_idx = inputs.len() as u16;

    // Convert commands
    for (call_idx, call) in spec.calls.iter().enumerate() {
        let (package, module, function) = parse_target(&call.target)
            .with_context(|| format!("Call {} target '{}'", call_idx, call.target))?;

        let type_args = call
            .type_args
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                parse_type_tag(s)
                    .with_context(|| format!("Call {} type_args[{}] '{}'", call_idx, idx, s))
            })
            .collect::<Result<Vec<TypeTag>>>()?;

        let mut args = Vec::new();
        for (arg_idx, arg_spec) in call.args.iter().enumerate() {
            match arg_spec {
                ArgSpec::Inline(inline) => {
                    // Add inline value as new input
                    let inline_value = convert_pure_value(&inline.value).with_context(|| {
                        format!("Call {} arg {} (inline value)", call_idx, arg_idx)
                    })?;
                    inputs.push(inline_value);
                    args.push(Argument::Input(next_input_idx));
                    next_input_idx += 1;
                }
                ArgSpec::Reference(reference) => {
                    let arg = convert_arg_reference(reference).with_context(|| {
                        format!("Call {} arg {} (reference)", call_idx, arg_idx)
                    })?;
                    args.push(arg);
                }
            }
        }

        commands.push(Command::MoveCall {
            package,
            module: Identifier::new(module).context("Invalid module name")?,
            function: Identifier::new(function).context("Invalid function name")?,
            type_args,
            args,
        });
    }

    Ok((inputs, commands))
}

fn format_validation_errors(validation: &ValidationResult, spec: &PtbSpec) -> String {
    let mut lines = Vec::with_capacity(validation.errors.len() + 2);
    lines.push("PTB validation failed:".to_string());
    for error in &validation.errors {
        let target = spec
            .calls
            .get(error.command_index)
            .map(|call| call.target.as_str())
            .unwrap_or("<unknown>");
        lines.push(format!(
            "- command {} ({}): {}",
            error.command_index, target, error.message
        ));
    }
    lines.push("Hint: command indices map to the order in the \"calls\" array.".to_string());
    lines.join("\n")
}

fn convert_input_spec(spec: &InputSpec, state: &SandboxState) -> Result<InputValue> {
    match spec {
        InputSpec::Pure(pure) => convert_pure_value(&pure.value),
        InputSpec::Object(obj) => {
            if let Some(id) = &obj.imm_or_owned {
                let addr = AccountAddress::from_hex_literal(id).context("Invalid object ID")?;
                let (bytes, type_tag) = state.get_object_input(id)?;
                Ok(InputValue::Object(ObjectInput::Owned {
                    id: addr,
                    bytes,
                    type_tag,
                    version: None,
                }))
            } else if let Some(shared) = &obj.shared {
                let _ = shared.mutable;
                let addr = AccountAddress::from_hex_literal(&shared.id)
                    .context("Invalid shared object ID")?;
                let (bytes, type_tag) = state.get_object_input(&shared.id)?;
                Ok(InputValue::Object(ObjectInput::Shared {
                    id: addr,
                    bytes,
                    type_tag,
                    version: None,
                    mutable: shared.mutable,
                }))
            } else {
                Err(anyhow!(
                    "Object input must specify imm_or_owned_object or shared_object"
                ))
            }
        }
    }
}

fn convert_pure_value(value: &PureValue) -> Result<InputValue> {
    let bytes = match value {
        PureValue::U8(n) => bcs::to_bytes(n)?,
        PureValue::U16(n) => bcs::to_bytes(n)?,
        PureValue::U32(n) => bcs::to_bytes(n)?,
        PureValue::U64(n) => bcs::to_bytes(n)?,
        PureValue::U128(n) => bcs::to_bytes(n)?,
        PureValue::Bool(b) => bcs::to_bytes(b)?,
        PureValue::Address(s) => {
            let addr = normalize_address(s)?;
            bcs::to_bytes(&addr)?
        }
        PureValue::VectorU8Utf8(s) => bcs::to_bytes(&s.as_bytes().to_vec())?,
        PureValue::VectorU8Hex(s) => {
            let s = s.strip_prefix("0x").unwrap_or(s);
            let bytes = hex::decode(s).context("Invalid hex in vector_u8_hex")?;
            bcs::to_bytes(&bytes)?
        }
        PureValue::VectorAddress(addrs) => {
            let addresses: Result<Vec<AccountAddress>> =
                addrs.iter().map(|s| normalize_address(s)).collect();
            bcs::to_bytes(&addresses?)?
        }
        PureValue::VectorU64(nums) => bcs::to_bytes(nums)?,
    };

    Ok(InputValue::Pure(bytes))
}

fn convert_arg_reference(reference: &ArgReference) -> Result<Argument> {
    if let Some(idx) = reference.input {
        Ok(Argument::Input(idx))
    } else if let Some(idx) = reference.result {
        Ok(Argument::Result(idx))
    } else if let Some([cmd, res]) = reference.nested_result {
        Ok(Argument::NestedResult(cmd, res))
    } else if reference.gas_coin == Some(true) {
        // Gas coin is typically input 0 in our system
        Ok(Argument::Input(0))
    } else {
        Err(anyhow!("Invalid argument reference"))
    }
}

fn parse_target(target: &str) -> Result<(AccountAddress, String, String)> {
    let parts: Vec<&str> = target.split("::").collect();
    if parts.len() != 3 {
        return Err(anyhow!(
            "Invalid target '{}'. Expected '0xADDR::module::function'",
            target
        ));
    }

    let package = AccountAddress::from_hex_literal(parts[0]).context("Invalid package address")?;

    Ok((package, parts[1].to_string(), parts[2].to_string()))
}

fn parse_type_tag(s: &str) -> Result<TypeTag> {
    parse_type_tag_string(s)
}

fn normalize_address(s: &str) -> Result<AccountAddress> {
    let s = s.trim().to_lowercase();
    let h = s.strip_prefix("0x").unwrap_or(&s);
    if h.is_empty() {
        return Ok(AccountAddress::ZERO);
    }
    if h.len() > 64 {
        return Err(anyhow!("Address too long: {}", s));
    }
    let padded = format!("0x{:0>64}", h);
    AccountAddress::from_hex_literal(&padded).context("Invalid address")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_target() {
        let (pkg, module, func) = parse_target("0x2::coin::split").unwrap();
        assert_eq!(pkg, AccountAddress::from_hex_literal("0x2").unwrap());
        assert_eq!(module, "coin");
        assert_eq!(func, "split");
    }

    #[test]
    fn test_parse_target_invalid() {
        assert!(parse_target("invalid").is_err());
        assert!(parse_target("0x2::coin").is_err());
    }

    #[test]
    fn test_convert_pure_value_u64() {
        let value = PureValue::U64(42);
        let input = convert_pure_value(&value).unwrap();
        if let InputValue::Pure(bytes) = input {
            let n: u64 = bcs::from_bytes(&bytes).unwrap();
            assert_eq!(n, 42);
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_convert_pure_value_address() {
        let value = PureValue::Address("0x2".to_string());
        let input = convert_pure_value(&value).unwrap();
        if let InputValue::Pure(bytes) = input {
            let addr: AccountAddress = bcs::from_bytes(&bytes).unwrap();
            assert_eq!(addr, AccountAddress::from_hex_literal("0x2").unwrap());
        } else {
            panic!("Expected Pure input");
        }
    }

    #[test]
    fn test_normalize_address() {
        assert_eq!(
            normalize_address("0x2").unwrap(),
            AccountAddress::from_hex_literal("0x2").unwrap()
        );
        assert_eq!(
            normalize_address("2").unwrap(),
            AccountAddress::from_hex_literal("0x2").unwrap()
        );
    }

    #[test]
    fn test_deserialize_spec() {
        let json = r#"{
            "calls": [
                {
                    "target": "0x2::coin::split",
                    "type_args": ["0x2::sui::SUI"],
                    "args": [
                        {"u64": 1000}
                    ]
                }
            ]
        }"#;

        let spec: PtbSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.calls.len(), 1);
        assert_eq!(spec.calls[0].target, "0x2::coin::split");
    }

    #[test]
    fn test_deserialize_spec_with_inputs() {
        let json = r#"{
            "inputs": [
                {"u64": 42},
                {"address": "0x123"}
            ],
            "calls": [
                {
                    "target": "0x2::test::func",
                    "args": [
                        {"input": 0},
                        {"input": 1}
                    ]
                }
            ]
        }"#;

        let spec: PtbSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.inputs.len(), 2);
        assert_eq!(spec.calls.len(), 1);
    }
}
