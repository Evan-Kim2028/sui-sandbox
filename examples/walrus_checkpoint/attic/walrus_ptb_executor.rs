//! PTB Execution Helper for Walrus Benchmark
//!
//! This module provides utilities to execute PTBs from Walrus checkpoint data

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use sui_sandbox_core::vm::{VMHarness, SimulationConfig};
use sui_sandbox_core::ptb::{PTBExecutor, InputValue, Command, Argument};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_types::base_types::ObjectID;
use std::collections::HashMap;

/// Parse PTB commands from JSON
pub fn parse_ptb_commands(ptb_json: &serde_json::Value) -> Result<Vec<Command>> {
    let commands_json = ptb_json
        .get("commands")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("No commands array in PTB"))?;

    let mut commands = Vec::new();

    for cmd_json in commands_json {
        if let Some(move_call) = cmd_json.get("MoveCall") {
            // Parse MoveCall command
            let package = parse_address(move_call.get("package").and_then(|p| p.as_str()).unwrap_or("0x2"))?;
            let module_name = move_call.get("module").and_then(|m| m.as_str()).unwrap_or("unknown");
            let function_name = move_call.get("function").and_then(|f| f.as_str()).unwrap_or("unknown");

            let module = Identifier::new(module_name)
                .map_err(|e| anyhow!("Invalid module name: {}", e))?;
            let function = Identifier::new(function_name)
                .map_err(|e| anyhow!("Invalid function name: {}", e))?;

            // Parse type arguments
            let type_args = if let Some(type_args_json) = move_call.get("type_arguments") {
                parse_type_arguments(type_args_json)?
            } else {
                vec![]
            };

            // Parse arguments
            let args = if let Some(args_json) = move_call.get("arguments") {
                parse_arguments(args_json)?
            } else {
                vec![]
            };

            commands.push(Command::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            });
        } else if let Some(transfer_objects) = cmd_json.get("TransferObjects") {
            // Parse TransferObjects command
            let objects = transfer_objects
                .get(0)
                .and_then(|o| o.as_array())
                .map(|arr| parse_arguments(&serde_json::Value::Array(arr.clone())))
                .transpose()?
                .unwrap_or_default();

            let address = transfer_objects
                .get(1)
                .map(|a| parse_argument(a))
                .transpose()?
                .unwrap_or(Argument::Input(0));

            commands.push(Command::TransferObjects {
                objects,
                address,
            });
        } else if let Some(split_coins) = cmd_json.get("SplitCoins") {
            // Parse SplitCoins command
            let coin = split_coins
                .get(0)
                .map(|c| parse_argument(c))
                .transpose()?
                .unwrap_or(Argument::GasCoin);

            let amounts = split_coins
                .get(1)
                .and_then(|a| a.as_array())
                .map(|arr| parse_arguments(&serde_json::Value::Array(arr.clone())))
                .transpose()?
                .unwrap_or_default();

            commands.push(Command::SplitCoins {
                coin,
                amounts,
            });
        } else if let Some(merge_coins) = cmd_json.get("MergeCoins") {
            // Parse MergeCoins command
            let destination = merge_coins
                .get(0)
                .map(|d| parse_argument(d))
                .transpose()?
                .unwrap_or(Argument::GasCoin);

            let sources = merge_coins
                .get(1)
                .and_then(|s| s.as_array())
                .map(|arr| parse_arguments(&serde_json::Value::Array(arr.clone())))
                .transpose()?
                .unwrap_or_default();

            commands.push(Command::MergeCoins {
                destination,
                sources,
            });
        }
        // Add more command types as needed
    }

    Ok(commands)
}

/// Parse an address from string
fn parse_address(addr_str: &str) -> Result<AccountAddress> {
    let addr_with_prefix = if addr_str.starts_with("0x") {
        addr_str.to_string()
    } else {
        format!("0x{}", addr_str)
    };

    AccountAddress::from_hex_literal(&addr_with_prefix)
        .map_err(|e| anyhow!("Failed to parse address '{}': {}", addr_str, e))
}

/// Parse type arguments
fn parse_type_arguments(type_args_json: &serde_json::Value) -> Result<Vec<TypeTag>> {
    let array = type_args_json
        .as_array()
        .ok_or_else(|| anyhow!("Type arguments must be array"))?;

    let mut type_args = Vec::new();
    for type_json in array {
        // Simplified type parsing - would need full implementation
        if let Some(type_str) = type_json.as_str() {
            // This is a simplification - would need proper type parsing
            type_args.push(TypeTag::Bool); // Placeholder
        }
    }

    Ok(type_args)
}

/// Parse PTB arguments
fn parse_arguments(args_json: &serde_json::Value) -> Result<Vec<Argument>> {
    let array = args_json
        .as_array()
        .ok_or_else(|| anyhow!("Arguments must be array"))?;

    array.iter().map(parse_argument).collect()
}

/// Parse a single argument
fn parse_argument(arg_json: &serde_json::Value) -> Result<Argument> {
    if let Some(input_idx) = arg_json.get("Input").and_then(|i| i.as_u64()) {
        Ok(Argument::Input(input_idx as u16))
    } else if let Some(result_idx) = arg_json.get("Result").and_then(|r| r.as_u64()) {
        Ok(Argument::Result(result_idx as u16))
    } else if let Some(nested_result) = arg_json.get("NestedResult") {
        let cmd_idx = nested_result.get(0).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let result_idx = nested_result.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        Ok(Argument::NestedResult(cmd_idx, result_idx))
    } else if arg_json.get("GasCoin").is_some() {
        Ok(Argument::GasCoin)
    } else {
        Err(anyhow!("Unknown argument type: {:?}", arg_json))
    }
}

/// Execute a PTB and return gas used
pub fn execute_ptb(
    commands: Vec<Command>,
    objects: Vec<sui_types::object::Object>,
    packages: &HashMap<String, Vec<(String, Vec<u8>)>>,
    sender: AccountAddress,
    gas_budget: u64,
) -> Result<(bool, u64)> {
    // Create module resolver with packages
    let mut resolver = LocalModuleResolver::empty();

    // Load packages into resolver
    for (pkg_id_str, modules) in packages {
        let pkg_addr = parse_address(pkg_id_str)?;
        for (module_name, module_bytes) in modules {
            // Parse module name
            if let Ok(mod_id) = Identifier::new(module_name) {
                resolver.add_module_bytes(pkg_addr, mod_id, module_bytes.clone())?;
            }
        }
    }

    // Create simulation config
    let config = SimulationConfig::default()
        .with_sender_address(sender.into())
        .with_gas_budget(Some(gas_budget));

    // Create VM harness
    let mut harness = VMHarness::new(resolver, config)?;

    // Create PTB executor
    let mut executor = PTBExecutor::new(&mut harness);

    // Add input objects
    for obj in objects {
        executor.add_input(InputValue::Object(Box::new(obj)));
    }

    // Execute commands
    match executor.execute(commands) {
        Ok(effects) => {
            // Get gas used from effects
            let gas_used = effects.gas_summary.computation_cost;
            Ok((true, gas_used))
        }
        Err(e) => {
            // Execution failed
            Err(e)
        }
    }
}
//! Moved to `examples/walrus_checkpoint/attic/` (superseded by the single-entry replay example).
