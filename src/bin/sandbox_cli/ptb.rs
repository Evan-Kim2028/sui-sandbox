//! PTB command - execute Programmable Transaction Blocks from JSON specs

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

use super::output::{format_effects, format_effects_json, format_error};
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

/// JSON spec format for PTB
#[derive(Debug, Deserialize)]
pub struct PtbSpec {
    /// Input values
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    /// Commands to execute
    pub calls: Vec<CallSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum InputSpec {
    Pure(PureInput),
    Object(ObjectInputSpec),
}

#[derive(Debug, Deserialize)]
pub struct PureInput {
    #[serde(flatten)]
    pub value: PureValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PureValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    Bool(bool),
    Address(String),
    #[serde(rename = "vector_u8_utf8")]
    VectorU8Utf8(String),
    #[serde(rename = "vector_u8_hex")]
    VectorU8Hex(String),
    #[serde(rename = "vector_address")]
    VectorAddress(Vec<String>),
    #[serde(rename = "vector_u64")]
    VectorU64(Vec<u64>),
}

#[derive(Debug, Deserialize)]
pub struct ObjectInputSpec {
    #[serde(rename = "imm_or_owned_object")]
    pub imm_or_owned: Option<String>,
    #[serde(rename = "shared_object")]
    pub shared: Option<SharedObjectSpec>,
}

#[derive(Debug, Deserialize)]
pub struct SharedObjectSpec {
    pub id: String,
    pub mutable: bool,
}

#[derive(Debug, Deserialize)]
pub struct CallSpec {
    /// Target: "0xADDR::module::function"
    pub target: String,
    /// Type arguments
    #[serde(default)]
    pub type_args: Vec<String>,
    /// Arguments (references to inputs or results)
    #[serde(default)]
    pub args: Vec<ArgSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ArgSpec {
    Inline(InlineArg),
    Reference(ArgReference),
}

#[derive(Debug, Deserialize)]
pub struct InlineArg {
    #[serde(flatten)]
    pub value: PureValue,
}

#[derive(Debug, Deserialize)]
pub struct ArgReference {
    /// Input index
    pub input: Option<u16>,
    /// Result index from previous command
    pub result: Option<u16>,
    /// Nested result [cmd_index, result_index]
    pub nested_result: Option<[u16; 2]>,
    /// Gas coin reference
    pub gas_coin: Option<bool>,
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
        // Read spec
        let spec_value = self.read_spec_value()?;

        // Parse sender
        let sender =
            AccountAddress::from_hex_literal(&self.sender).context("Invalid sender address")?;
        state.set_last_sender(sender);

        // Convert spec to PTB inputs and commands
        let (inputs, commands, spec_for_errors) = if spec_value.get("commands").is_some() {
            let (inputs, commands) = convert_mcp_spec(&spec_value, state)?;
            (inputs, commands, None)
        } else {
            let spec: PtbSpec =
                serde_json::from_value(spec_value).context("Failed to parse CLI PTB spec")?;
            let (inputs, commands) = convert_spec(&spec, state)?;
            (inputs, commands, Some(spec))
        };
        let validation = validate_ptb(&commands, inputs.len());
        if !validation.valid {
            if let Some(spec) = spec_for_errors.as_ref() {
                return Err(anyhow!(format_validation_errors(&validation, spec)));
            }
            return Err(anyhow!(format_validation_errors_generic(&validation)));
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

    fn read_spec_value(&self) -> Result<Value> {
        let json_str = if self.spec.as_os_str() == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            std::fs::read_to_string(&self.spec)
                .with_context(|| format!("Failed to read spec file: {}", self.spec.display()))?
        };

        serde_json::from_str(&json_str).context("Failed to parse PTB spec JSON")
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

fn convert_mcp_spec(
    value: &Value,
    state: &SandboxState,
) -> Result<(Vec<InputValue>, Vec<Command>)> {
    let inputs_value = value
        .get("inputs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let commands_value = value
        .get("commands")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if commands_value.is_empty() {
        return Err(anyhow!("MCP PTB spec missing 'commands' array"));
    }

    let mut inputs = Vec::new();
    for (idx, input_value) in inputs_value.iter().enumerate() {
        let input =
            parse_mcp_input(input_value, state).with_context(|| format!("MCP inputs[{}]", idx))?;
        inputs.push(input);
    }

    let mut commands = Vec::new();
    for (idx, cmd_value) in commands_value.iter().enumerate() {
        let cmd = parse_mcp_command(cmd_value).with_context(|| format!("MCP commands[{}]", idx))?;
        commands.push(cmd);
    }

    Ok((inputs, commands))
}

fn parse_mcp_input(value: &Value, state: &SandboxState) -> Result<InputValue> {
    if let Some(inner) = value.get("Pure") {
        return parse_mcp_pure_input(inner);
    }
    if let Some(inner) = value.get("Object") {
        return parse_mcp_object_input(inner, state);
    }

    if let Some(kind) = value.get("kind").and_then(|v| v.as_str()) {
        if kind.eq_ignore_ascii_case("pure") {
            return parse_mcp_pure_input(value);
        }
        return parse_mcp_object_input(value, state);
    }

    if value.get("object_id").is_some() || value.get("object_ref").is_some() {
        return parse_mcp_object_input(value, state);
    }

    parse_mcp_pure_input(value)
}

fn parse_mcp_pure_input(value: &Value) -> Result<InputValue> {
    let type_hint = value
        .get("type")
        .or_else(|| value.get("value_type"))
        .and_then(|v| v.as_str());
    let raw_value = value.get("value").unwrap_or(value);
    let bytes = sui_sandbox_core::shared::parsing::parse_pure_from_json(raw_value, type_hint)?;
    Ok(InputValue::Pure(bytes))
}

fn parse_mcp_object_input(value: &Value, state: &SandboxState) -> Result<InputValue> {
    let object_id = value
        .get("object_id")
        .or_else(|| value.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let object_ref = value
        .get("object_ref")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let object_id = match (object_id, object_ref) {
        (Some(id), _) if !id.is_empty() => id,
        (None, Some(_)) => {
            return Err(anyhow!(
                "object_ref is not supported in CLI PTB; use object_id"
            ))
        }
        _ => return Err(anyhow!("Object input missing object_id")),
    };

    let (bytes, type_tag) = state.get_object_input(&object_id)?;
    let addr = AccountAddress::from_hex_literal(&object_id).context("Invalid object_id")?;
    let mode = value
        .get("mode")
        .or_else(|| value.get("kind"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let mutable = value
        .get("mutable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if mode.as_deref() == Some("shared") {
        Ok(InputValue::Object(ObjectInput::Shared {
            id: addr,
            bytes,
            type_tag,
            version: None,
            mutable,
        }))
    } else {
        Ok(InputValue::Object(ObjectInput::Owned {
            id: addr,
            bytes,
            type_tag,
            version: None,
        }))
    }
}

fn parse_mcp_command(value: &Value) -> Result<Command> {
    if let Some(inner) = value.get("MoveCall") {
        return parse_mcp_kind_command("MoveCall", inner);
    }
    if let Some(inner) = value.get("SplitCoins") {
        return parse_mcp_kind_command("SplitCoins", inner);
    }
    if let Some(inner) = value.get("MergeCoins") {
        return parse_mcp_kind_command("MergeCoins", inner);
    }
    if let Some(inner) = value.get("TransferObjects") {
        return parse_mcp_kind_command("TransferObjects", inner);
    }
    if let Some(inner) = value.get("MakeMoveVec") {
        return parse_mcp_kind_command("MakeMoveVec", inner);
    }
    if let Some(inner) = value.get("Publish") {
        return parse_mcp_kind_command("Publish", inner);
    }
    if let Some(inner) = value.get("Upgrade") {
        return parse_mcp_kind_command("Upgrade", inner);
    }
    if let Some(inner) = value.get("Receive") {
        return parse_mcp_kind_command("Receive", inner);
    }

    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Command missing kind"))?;
    parse_mcp_kind_command(kind, value)
}

fn parse_mcp_kind_command(kind: &str, value: &Value) -> Result<Command> {
    match kind {
        "MoveCall" | "move_call" => {
            let package = value
                .get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("MoveCall requires package"))?;
            let module = value
                .get("module")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("MoveCall requires module"))?;
            let function = value
                .get("function")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("MoveCall requires function"))?;
            let type_args = value
                .get("type_args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let args = parse_mcp_args(value.get("args"))?;
            build_move_call_command(package, module, function, &type_args, args)
        }
        "SplitCoins" | "split_coins" => {
            let coin = parse_mcp_arg(value.get("coin"))?;
            let amounts = parse_mcp_args(value.get("amounts"))?;
            Ok(Command::SplitCoins { coin, amounts })
        }
        "MergeCoins" | "merge_coins" => {
            let destination = parse_mcp_arg(value.get("destination"))?;
            let sources = parse_mcp_args(value.get("sources"))?;
            Ok(Command::MergeCoins {
                destination,
                sources,
            })
        }
        "TransferObjects" | "transfer_objects" => {
            let objects = parse_mcp_args(value.get("objects"))?;
            let address = parse_mcp_arg(value.get("address"))?;
            Ok(Command::TransferObjects { objects, address })
        }
        "MakeMoveVec" | "make_move_vec" => {
            let elements = parse_mcp_args(value.get("elements"))?;
            let type_tag = value
                .get("type_arg")
                .and_then(|v| v.as_str())
                .map(parse_type_tag)
                .transpose()
                .map_err(|e| anyhow!("Invalid type_arg: {}", e))?;
            Ok(Command::MakeMoveVec { type_tag, elements })
        }
        "Publish" | "publish" => {
            let modules = value
                .get("modules")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(decode_b64_opt)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let deps = value
                .get("dependencies")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| AccountAddress::from_hex_literal(s).ok())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(Command::Publish {
                modules,
                dep_ids: deps,
            })
        }
        "Upgrade" | "upgrade" => {
            let modules = value
                .get("modules")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(decode_b64_opt)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let package = value
                .get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Upgrade requires package"))?;
            let package = AccountAddress::from_hex_literal(package)
                .context("Invalid upgrade package address")?;
            let ticket = parse_mcp_arg(value.get("ticket"))?;
            Ok(Command::Upgrade {
                modules,
                package,
                ticket,
            })
        }
        "Receive" | "receive" => {
            let object_id = value
                .get("object_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("Receive requires object_id"))?;
            let addr =
                AccountAddress::from_hex_literal(object_id).context("Invalid receive object_id")?;
            let object_type = value
                .get("object_type")
                .and_then(|v| v.as_str())
                .map(parse_type_tag)
                .transpose()
                .map_err(|e| anyhow!("Invalid object_type: {}", e))?;
            Ok(Command::Receive {
                object_id: addr,
                object_type,
            })
        }
        _ => Err(anyhow!("Unknown command kind: {}", kind)),
    }
}

fn build_move_call_command(
    package: &str,
    module: &str,
    function: &str,
    type_args: &[String],
    args: Vec<Argument>,
) -> Result<Command> {
    let pkg_addr = AccountAddress::from_hex_literal(package)
        .with_context(|| format!("Invalid package address: {}", package))?;
    let module_id = Identifier::new(module).context("Invalid module name")?;
    let function_id = Identifier::new(function).context("Invalid function name")?;
    let parsed_type_args: Vec<TypeTag> = type_args
        .iter()
        .map(|s| parse_type_tag(s))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Command::MoveCall {
        package: pkg_addr,
        module: module_id,
        function: function_id,
        type_args: parsed_type_args,
        args,
    })
}

fn parse_mcp_arg(value: Option<&Value>) -> Result<Argument> {
    match value {
        Some(v) => parse_mcp_single_arg(v),
        None => Err(anyhow!("Argument missing")),
    }
}

fn parse_mcp_args(value: Option<&Value>) -> Result<Vec<Argument>> {
    let arr = value
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("args must be an array"))?;
    let mut args = Vec::new();
    for v in arr {
        args.push(parse_mcp_single_arg(v)?);
    }
    Ok(args)
}

fn parse_mcp_single_arg(value: &Value) -> Result<Argument> {
    if let Some(arg) = parse_mcp_arg_reference(value)? {
        return Ok(arg);
    }
    Err(anyhow!("Arguments must reference inputs/results"))
}

fn parse_mcp_arg_reference(value: &Value) -> Result<Option<Argument>> {
    if let Some(input_idx) = value.get("input").and_then(|v| v.as_u64()) {
        return Ok(Some(Argument::Input(input_idx as u16)));
    }
    if let Some(result_idx) = value.get("result").and_then(|v| v.as_u64()) {
        return Ok(Some(Argument::Result(result_idx as u16)));
    }
    if let Some(nested) = value.get("nested_result").and_then(|v| v.as_array()) {
        if nested.len() == 2 {
            if let (Some(a), Some(b)) = (nested[0].as_u64(), nested[1].as_u64()) {
                return Ok(Some(Argument::NestedResult(a as u16, b as u16)));
            }
        }
    }
    if value.get("gas_coin").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(Some(Argument::Input(0)));
    }
    if let Some(kind) = value.get("kind").and_then(|v| v.as_str()) {
        match kind {
            "Input" | "input" => {
                let idx = value
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("Input arg requires index"))?;
                return Ok(Some(Argument::Input(idx as u16)));
            }
            "Result" | "result" => {
                if let Some(idx) = value.get("index").and_then(|v| v.as_u64()) {
                    return Ok(Some(Argument::Result(idx as u16)));
                }
                if let Some(res) = value.get("Result") {
                    if let Some(cmd) = res.get("cmd").and_then(|v| v.as_u64()) {
                        let idx = res.get("idx").and_then(|v| v.as_u64()).unwrap_or(0);
                        if idx == 0 {
                            return Ok(Some(Argument::Result(cmd as u16)));
                        }
                        return Ok(Some(Argument::NestedResult(cmd as u16, idx as u16)));
                    }
                }
            }
            "NestedResult" | "nested_result" => {
                let idx = value
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("NestedResult requires index"))?;
                let nested_idx = value
                    .get("nested_index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("NestedResult requires nested_index"))?;
                return Ok(Some(Argument::NestedResult(idx as u16, nested_idx as u16)));
            }
            _ => {}
        }
    }
    if let Some(res) = value.get("Result") {
        if let Some(cmd) = res.get("cmd").and_then(|v| v.as_u64()) {
            let idx = res.get("idx").and_then(|v| v.as_u64()).unwrap_or(0);
            if idx == 0 {
                return Ok(Some(Argument::Result(cmd as u16)));
            }
            return Ok(Some(Argument::NestedResult(cmd as u16, idx as u16)));
        }
    }
    if let Some(input_val) = value.get("Input") {
        if let Some(idx) = input_val.as_u64() {
            return Ok(Some(Argument::Input(idx as u16)));
        }
        if let Some(idx) = input_val.get("index").and_then(|v| v.as_u64()) {
            return Ok(Some(Argument::Input(idx as u16)));
        }
    }
    Ok(None)
}

fn decode_b64_opt(value: &str) -> Option<Vec<u8>> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, value).ok()
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

fn format_validation_errors_generic(validation: &ValidationResult) -> String {
    let mut lines = Vec::with_capacity(validation.errors.len() + 2);
    lines.push("PTB validation failed:".to_string());
    for error in &validation.errors {
        lines.push(format!(
            "- command {}: {}",
            error.command_index, error.message
        ));
    }
    lines.push("Hint: command indices map to the order in the \"commands\" array.".to_string());
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

    #[test]
    fn test_convert_mcp_spec_move_call() {
        let value = serde_json::json!({
            "inputs": [
                {"kind": "pure", "value": 42, "type": "u64"}
            ],
            "commands": [
                {
                    "kind": "move_call",
                    "package": "0x2",
                    "module": "coin",
                    "function": "zero",
                    "type_args": ["0x2::sui::SUI"],
                    "args": []
                }
            ]
        });

        let state = SandboxState::new("https://fullnode.mainnet.sui.io:443").unwrap();
        let (inputs, commands) = convert_mcp_spec(&value, &state).unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(commands.len(), 1);
    }
}
