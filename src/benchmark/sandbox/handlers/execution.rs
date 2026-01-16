//! Execution sandbox handlers.
//!
//! Handles execute_ptb, validate_ptb, and call_function operations.

use crate::benchmark::sandbox::types::{
    CommandReturnValues, EventResponse, ObjectEffectResponse, PtbArg, PtbCommand, PtbInput,
    SandboxResponse, TransactionEffectsResponse,
};
use crate::benchmark::simulation::SimulationEnvironment;
use move_core_types::account_address::AccountAddress;

use super::encoding::encode_pure_value;

/// Convert a PtbArg to the internal Argument type.
fn convert_ptb_arg(arg: &PtbArg) -> crate::benchmark::ptb::Argument {
    match arg {
        PtbArg::Input(idx) => crate::benchmark::ptb::Argument::Input(*idx as u16),
        PtbArg::Result { cmd, idx } => {
            // Use NestedResult for multi-value access, Result for single-value (idx=0)
            if *idx == 0 {
                crate::benchmark::ptb::Argument::Result(*cmd as u16)
            } else {
                crate::benchmark::ptb::Argument::NestedResult(*cmd as u16, *idx as u16)
            }
        }
    }
}

/// Execute a PTB with the given commands.
pub fn execute_ptb_command(
    env: &mut SimulationEnvironment,
    inputs: &[PtbInput],
    commands: &[PtbCommand],
    verbose: bool,
) -> SandboxResponse {
    use crate::benchmark::ptb::{Argument, Command as RealPtbCommand, InputValue};
    use move_core_types::identifier::Identifier;
    use move_core_types::language_storage::TypeTag;

    if verbose {
        eprintln!(
            "Executing PTB with {} inputs and {} commands",
            inputs.len(),
            commands.len()
        );
    }

    // Track gas budget for simulation
    let mut _gas_budget: u64 = 50_000_000; // Default 50M MIST

    // Convert inputs
    let mut real_inputs: Vec<InputValue> = Vec::new();
    for input in inputs {
        match input {
            PtbInput::Pure { value, value_type } => match encode_pure_value(value, value_type) {
                Ok(bytes) => real_inputs.push(InputValue::Pure(bytes)),
                Err(e) => return SandboxResponse::error(format!("Failed to encode input: {}", e)),
            },
            PtbInput::Object { object_id, mode } => {
                match env.get_object_for_ptb_with_mode(object_id, mode.as_deref()) {
                    Ok(obj) => real_inputs.push(InputValue::Object(obj)),
                    Err(e) => {
                        return SandboxResponse::error(format!(
                            "Failed to get object {}: {}",
                            object_id, e
                        ))
                    }
                }
            }
            PtbInput::Gas { budget } => {
                _gas_budget = *budget;
                // Gas coin is a special input - create a SUI coin with the budget
                match env.create_gas_coin(*budget) {
                    Ok(obj) => real_inputs.push(InputValue::Object(obj)),
                    Err(e) => {
                        return SandboxResponse::error(format!("Failed to create gas coin: {}", e))
                    }
                }
            }
            PtbInput::Witness { witness_type } => {
                if verbose {
                    eprintln!("Synthesizing witness for type: {}", witness_type);
                }
                let witness_bytes = vec![1u8];
                real_inputs.push(InputValue::Pure(witness_bytes));
            }
        }
    }

    // Convert commands
    let mut real_commands: Vec<RealPtbCommand> = Vec::new();
    for cmd in commands {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => {
                let pkg_addr = match AccountAddress::from_hex_literal(package) {
                    Ok(a) => a,
                    Err(e) => {
                        return SandboxResponse::error(format!("Invalid package address: {}", e))
                    }
                };

                let module_id = match Identifier::new(module.as_str()) {
                    Ok(id) => id,
                    Err(e) => return SandboxResponse::error(format!("Invalid module name: {}", e)),
                };

                let function_id = match Identifier::new(function.as_str()) {
                    Ok(id) => id,
                    Err(e) => {
                        return SandboxResponse::error(format!("Invalid function name: {}", e))
                    }
                };

                // Parse type args from strings
                let parsed_type_args: Vec<TypeTag> = match type_args
                    .iter()
                    .map(|s| crate::benchmark::tx_replay::parse_type_tag(s))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(tags) => tags,
                    Err(e) => {
                        return SandboxResponse::error(format!("Invalid type argument: {}", e))
                    }
                };

                let converted_args: Vec<Argument> = args.iter().map(convert_ptb_arg).collect();

                real_commands.push(RealPtbCommand::MoveCall {
                    package: pkg_addr,
                    module: module_id,
                    function: function_id,
                    type_args: parsed_type_args,
                    args: converted_args,
                });
            }
            PtbCommand::TransferObjects { objects, recipient } => {
                real_commands.push(RealPtbCommand::TransferObjects {
                    objects: objects.iter().map(convert_ptb_arg).collect(),
                    address: convert_ptb_arg(recipient),
                });
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                real_commands.push(RealPtbCommand::SplitCoins {
                    coin: convert_ptb_arg(coin),
                    amounts: amounts.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::MergeCoins { target, sources } => {
                real_commands.push(RealPtbCommand::MergeCoins {
                    destination: convert_ptb_arg(target),
                    sources: sources.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::MakeMoveVec {
                element_type,
                elements,
            } => {
                let type_tag = if let Some(type_str) = element_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => Some(tag),
                        Err(e) => {
                            return SandboxResponse::error(format!("Invalid element type: {}", e))
                        }
                    }
                } else {
                    None
                };
                real_commands.push(RealPtbCommand::MakeMoveVec {
                    type_tag,
                    elements: elements.iter().map(convert_ptb_arg).collect(),
                });
            }
            PtbCommand::Publish {
                modules,
                dependencies,
            } => {
                use base64::Engine;
                // Decode base64 modules
                let mut decoded_modules = Vec::new();
                for (i, b64) in modules.iter().enumerate() {
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => decoded_modules.push(bytes),
                        Err(e) => {
                            return SandboxResponse::error(format!(
                                "Invalid base64 in module {}: {}",
                                i, e
                            ))
                        }
                    }
                }

                // Parse dependency IDs
                let mut dep_ids = Vec::new();
                for dep in dependencies {
                    match AccountAddress::from_hex_literal(dep) {
                        Ok(addr) => dep_ids.push(addr),
                        Err(e) => {
                            return SandboxResponse::error(format!(
                                "Invalid dependency ID '{}': {}",
                                dep, e
                            ))
                        }
                    }
                }

                real_commands.push(RealPtbCommand::Publish {
                    modules: decoded_modules,
                    dep_ids,
                });
            }
            PtbCommand::Upgrade {
                modules,
                package,
                ticket,
            } => {
                use base64::Engine;
                // Decode base64 modules
                let mut decoded_modules = Vec::new();
                for (i, b64) in modules.iter().enumerate() {
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => decoded_modules.push(bytes),
                        Err(e) => {
                            return SandboxResponse::error(format!(
                                "Invalid base64 in module {}: {}",
                                i, e
                            ))
                        }
                    }
                }

                // Parse package ID
                let pkg_id = match AccountAddress::from_hex_literal(package) {
                    Ok(addr) => addr,
                    Err(e) => return SandboxResponse::error(format!("Invalid package ID: {}", e)),
                };

                real_commands.push(RealPtbCommand::Upgrade {
                    modules: decoded_modules,
                    package: pkg_id,
                    ticket: convert_ptb_arg(ticket),
                });
            }
            PtbCommand::Receive {
                object_id,
                object_type,
            } => {
                // Parse object ID
                let obj_id = match AccountAddress::from_hex_literal(object_id) {
                    Ok(addr) => addr,
                    Err(e) => return SandboxResponse::error(format!("Invalid object ID: {}", e)),
                };

                // Parse type if provided
                let type_tag = if let Some(type_str) = object_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => Some(tag),
                        Err(e) => {
                            return SandboxResponse::error(format!("Invalid object type: {}", e))
                        }
                    }
                } else {
                    None
                };

                real_commands.push(RealPtbCommand::Receive {
                    object_id: obj_id,
                    object_type: type_tag,
                });
            }
        }
    }

    // Execute
    let result = env.execute_ptb(real_inputs, real_commands);

    if result.success {
        // Build enhanced response with full effects
        if let Some(ref effects) = result.effects {
            let effects_response = build_effects_response(effects);
            let events_response = build_events_response(&effects.events);
            SandboxResponse::success_with_effects(
                effects_response,
                events_response,
                effects.gas_used,
            )
        } else {
            SandboxResponse::success_with_data(serde_json::json!({
                "status": "success",
            }))
        }
    } else if let Some(ref error) = result.error {
        let mut response = match error {
            crate::benchmark::simulation::SimulationError::ContractAbort {
                abort_code,
                module,
                function,
                ..
            } => SandboxResponse::abort(
                *abort_code,
                Some(format!("{}::{}", module, function)),
                format!(
                    "Contract abort in {}::{} with code {}",
                    module, function, abort_code
                ),
            ),
            _ => SandboxResponse::error_with_category(
                error.to_string(),
                categorize_simulation_error(error),
            ),
        };
        // Add command failure context
        response.failed_command_index = result.failed_command_index;
        response.failed_command_description = result.failed_command_description.clone();
        response.commands_succeeded = if result.commands_succeeded > 0 {
            Some(result.commands_succeeded)
        } else {
            None
        };
        response
    } else {
        let mut response = SandboxResponse::error("Unknown execution error");
        response.failed_command_index = result.failed_command_index;
        response.failed_command_description = result.failed_command_description.clone();
        response.commands_succeeded = if result.commands_succeeded > 0 {
            Some(result.commands_succeeded)
        } else {
            None
        };
        response
    }
}

/// Validate a PTB without executing it.
pub fn execute_validate_ptb(
    env: &SimulationEnvironment,
    inputs: &[PtbInput],
    commands: &[PtbCommand],
    verbose: bool,
) -> SandboxResponse {
    use move_core_types::identifier::Identifier;

    if verbose {
        eprintln!(
            "Validating PTB with {} inputs and {} commands",
            inputs.len(),
            commands.len()
        );
    }

    let mut validation_errors: Vec<String> = Vec::new();
    let mut input_types: Vec<serde_json::Value> = Vec::new();
    let mut command_info: Vec<serde_json::Value> = Vec::new();
    let mut command_result_types: Vec<Vec<String>> = Vec::new();
    let mut input_type_map: Vec<Option<String>> = Vec::new();

    // Validate inputs
    for (i, input) in inputs.iter().enumerate() {
        match input {
            PtbInput::Pure { value, value_type } => match encode_pure_value(value, value_type) {
                Ok(bytes) => {
                    input_type_map.push(Some(value_type.clone()));
                    input_types.push(serde_json::json!({
                        "index": i,
                        "type": "pure",
                        "value_type": value_type,
                        "bytes_len": bytes.len(),
                        "valid": true
                    }));
                }
                Err(e) => {
                    input_type_map.push(None);
                    validation_errors
                        .push(format!("Input {}: Failed to encode pure value: {}", i, e));
                    input_types.push(serde_json::json!({
                        "index": i,
                        "type": "pure",
                        "value_type": value_type,
                        "valid": false,
                        "error": e.to_string()
                    }));
                }
            },
            PtbInput::Object { object_id, mode } => {
                match env.get_object_for_ptb_with_mode(object_id, mode.as_deref()) {
                    Ok(obj) => {
                        let type_str = obj.type_tag().map(|t| format!("{}", t));
                        input_type_map.push(type_str.clone());
                        input_types.push(serde_json::json!({
                            "index": i,
                            "type": "object",
                            "object_id": object_id,
                            "mode": mode,
                            "object_type": type_str,
                            "valid": true
                        }));
                    }
                    Err(e) => {
                        input_type_map.push(None);
                        validation_errors
                            .push(format!("Input {}: Object not found or invalid: {}", i, e));
                        input_types.push(serde_json::json!({
                            "index": i,
                            "type": "object",
                            "object_id": object_id,
                            "mode": mode,
                            "valid": false,
                            "error": e.to_string()
                        }));
                    }
                }
            }
            PtbInput::Gas { budget } => {
                input_type_map.push(Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()));
                input_types.push(serde_json::json!({
                    "index": i,
                    "type": "gas",
                    "budget": budget,
                    "valid": true
                }));
            }
            PtbInput::Witness { witness_type } => {
                input_type_map.push(Some(witness_type.clone()));
                input_types.push(serde_json::json!({
                    "index": i,
                    "type": "witness",
                    "witness_type": witness_type,
                    "valid": true
                }));
            }
        }
    }

    // Validate commands
    for (i, cmd) in commands.iter().enumerate() {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => {
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                // Validate package address
                if let Err(e) = AccountAddress::from_hex_literal(package) {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid package address: {}", e));
                }

                // Validate module identifier
                if let Err(e) = Identifier::new(module.as_str()) {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid module name: {}", e));
                }

                // Validate function identifier
                if let Err(e) = Identifier::new(function.as_str()) {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid function name: {}", e));
                }

                // Validate type arguments
                let mut parsed_type_args: Vec<String> = Vec::new();
                for (j, type_str) in type_args.iter().enumerate() {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(tag) => parsed_type_args.push(format!("{}", tag)),
                        Err(e) => {
                            cmd_valid = false;
                            cmd_errors.push(format!("Type arg {}: {}", j, e));
                        }
                    }
                }

                // Deep function validation
                let module_path = format!("{}::{}", package, module);
                let mut function_info_json = serde_json::json!(null);
                let mut expected_params = 0usize;
                let mut expected_type_args = 0usize;
                let mut is_entry = false;
                let mut is_public = false;
                let mut param_types: Vec<String> = Vec::new();
                let mut return_types: Vec<String> = Vec::new();

                match env.get_function_info(&module_path, function) {
                    Some(info) => {
                        function_info_json = info.clone();

                        if let Some(params) = info.get("params").and_then(|p| p.as_array()) {
                            param_types = params
                                .iter()
                                .filter_map(|p| p.as_str().map(|s| s.to_string()))
                                .collect();
                            expected_params = param_types
                                .iter()
                                .filter(|p| !p.contains("TxContext"))
                                .count();
                        }

                        if let Some(type_params) =
                            info.get("type_params").and_then(|p| p.as_array())
                        {
                            expected_type_args = type_params.len();
                        }

                        is_entry = info
                            .get("is_entry")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        is_public =
                            info.get("visibility").and_then(|v| v.as_str()) == Some("public");

                        if let Some(returns) = info.get("returns").and_then(|r| r.as_array()) {
                            return_types = returns
                                .iter()
                                .filter_map(|r| r.as_str().map(|s| s.to_string()))
                                .collect();
                        }

                        if !is_entry && !is_public {
                            cmd_valid = false;
                            cmd_errors.push(format!(
                                "Function '{}' is private and cannot be called via PTB",
                                function
                            ));
                        }

                        if args.len() != expected_params {
                            cmd_valid = false;
                            cmd_errors.push(format!(
                                "Argument count mismatch: provided {} args, function expects {} (excluding TxContext)",
                                args.len(), expected_params
                            ));
                        }

                        if type_args.len() != expected_type_args {
                            cmd_valid = false;
                            cmd_errors.push(format!(
                                "Type argument count mismatch: provided {} type args, function expects {}",
                                type_args.len(), expected_type_args
                            ));
                        }
                    }
                    None => {
                        cmd_valid = false;
                        cmd_errors.push(format!(
                            "Function '{}::{}' not found in module",
                            module, function
                        ));
                    }
                }

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                command_result_types.push(return_types.clone());

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "MoveCall",
                    "package": package,
                    "module": module,
                    "function": function,
                    "type_args": parsed_type_args,
                    "arg_count": args.len(),
                    "function_exists": function_info_json != serde_json::json!(null),
                    "function_signature": {
                        "expected_params": expected_params,
                        "expected_type_args": expected_type_args,
                        "is_entry": is_entry,
                        "is_public": is_public,
                        "param_types": param_types,
                        "return_types": &command_result_types[i]
                    },
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
            PtbCommand::TransferObjects { objects, .. } => {
                command_result_types.push(Vec::new());
                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "TransferObjects",
                    "object_count": objects.len(),
                    "valid": true
                }));
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                let coin_type = match coin {
                    PtbArg::Input(idx) if *idx < input_type_map.len() => {
                        input_type_map[*idx].clone()
                    }
                    PtbArg::Result { cmd, idx } if *cmd < command_result_types.len() => {
                        command_result_types[*cmd].get(*idx).cloned()
                    }
                    _ => None,
                };
                let result_type = coin_type
                    .map(|t| format!("vector<{}>", t))
                    .unwrap_or_else(|| "vector<Coin>".to_string());
                command_result_types.push(vec![result_type]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "SplitCoins",
                    "amount_count": amounts.len(),
                    "valid": true
                }));
            }
            PtbCommand::MergeCoins { sources, .. } => {
                command_result_types.push(Vec::new());
                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "MergeCoins",
                    "source_count": sources.len(),
                    "valid": true
                }));
            }
            PtbCommand::MakeMoveVec {
                element_type,
                elements,
            } => {
                let type_valid = if let Some(type_str) = element_type {
                    match crate::benchmark::tx_replay::parse_type_tag(type_str) {
                        Ok(_) => true,
                        Err(e) => {
                            validation_errors
                                .push(format!("Command {}: Invalid element type: {}", i, e));
                            false
                        }
                    }
                } else {
                    true
                };

                let result_type = element_type
                    .as_ref()
                    .map(|t| format!("vector<{}>", t))
                    .unwrap_or_else(|| "vector<?>".to_string());
                command_result_types.push(vec![result_type]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "MakeMoveVec",
                    "element_type": element_type,
                    "element_count": elements.len(),
                    "valid": type_valid
                }));
            }
            PtbCommand::Publish {
                modules,
                dependencies,
            } => {
                use base64::Engine;
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                for (j, b64) in modules.iter().enumerate() {
                    if base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .is_err()
                    {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid base64 in module {}", j));
                    }
                }

                for dep in dependencies {
                    if AccountAddress::from_hex_literal(dep).is_err() {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid dependency ID: {}", dep));
                    }
                }

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                command_result_types.push(vec!["0x2::package::UpgradeCap".to_string()]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "Publish",
                    "module_count": modules.len(),
                    "dependency_count": dependencies.len(),
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
            PtbCommand::Upgrade {
                modules, package, ..
            } => {
                use base64::Engine;
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                for (j, b64) in modules.iter().enumerate() {
                    if base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .is_err()
                    {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid base64 in module {}", j));
                    }
                }

                if AccountAddress::from_hex_literal(package).is_err() {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid package ID: {}", package));
                }

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                command_result_types.push(vec!["0x2::package::UpgradeReceipt".to_string()]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "Upgrade",
                    "module_count": modules.len(),
                    "package": package,
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
            PtbCommand::Receive {
                object_id,
                object_type,
            } => {
                let mut cmd_valid = true;
                let mut cmd_errors: Vec<String> = Vec::new();

                if AccountAddress::from_hex_literal(object_id).is_err() {
                    cmd_valid = false;
                    cmd_errors.push(format!("Invalid object ID: {}", object_id));
                }

                let result_type = if let Some(type_str) = object_type {
                    if crate::benchmark::tx_replay::parse_type_tag(type_str).is_err() {
                        cmd_valid = false;
                        cmd_errors.push(format!("Invalid object type: {}", type_str));
                        "?".to_string()
                    } else {
                        type_str.clone()
                    }
                } else {
                    "?".to_string()
                };

                if !cmd_errors.is_empty() {
                    validation_errors
                        .extend(cmd_errors.iter().map(|e| format!("Command {}: {}", i, e)));
                }

                command_result_types.push(vec![result_type]);

                command_info.push(serde_json::json!({
                    "index": i,
                    "type": "Receive",
                    "object_id": object_id,
                    "object_type": object_type,
                    "valid": cmd_valid,
                    "errors": cmd_errors
                }));
            }
        }
    }

    // Validate argument references
    let validate_arg_reference = |arg: &PtbArg, cmd_idx: usize, errors: &mut Vec<String>| match arg
    {
        PtbArg::Input(idx) => {
            if *idx >= inputs.len() {
                errors.push(format!(
                    "Command {}: Input({}) references non-existent input (only {} inputs)",
                    cmd_idx,
                    idx,
                    inputs.len()
                ));
            }
        }
        PtbArg::Result {
            cmd: result_cmd,
            idx: result_idx,
        } => {
            if *result_cmd >= cmd_idx {
                errors.push(format!(
                    "Command {}: Result(cmd={}, idx={}) references command {} which hasn't executed yet (forward reference)",
                    cmd_idx, result_cmd, result_idx, result_cmd
                ));
            } else if *result_cmd < command_result_types.len() {
                let result_count = command_result_types[*result_cmd].len();
                if *result_idx >= result_count {
                    errors.push(format!(
                        "Command {}: Result(cmd={}, idx={}) references return index {} but command {} only returns {} values",
                        cmd_idx, result_cmd, result_idx, result_idx, result_cmd, result_count
                    ));
                }
            }
        }
    };

    for (i, cmd) in commands.iter().enumerate() {
        let mut ref_errors: Vec<String> = Vec::new();
        match cmd {
            PtbCommand::MoveCall { args, .. } => {
                for arg in args {
                    validate_arg_reference(arg, i, &mut ref_errors);
                }
            }
            PtbCommand::TransferObjects { objects, recipient } => {
                for obj in objects {
                    validate_arg_reference(obj, i, &mut ref_errors);
                }
                validate_arg_reference(recipient, i, &mut ref_errors);
            }
            PtbCommand::SplitCoins { coin, amounts } => {
                validate_arg_reference(coin, i, &mut ref_errors);
                for amt in amounts {
                    validate_arg_reference(amt, i, &mut ref_errors);
                }
            }
            PtbCommand::MergeCoins { target, sources } => {
                validate_arg_reference(target, i, &mut ref_errors);
                for src in sources {
                    validate_arg_reference(src, i, &mut ref_errors);
                }
            }
            PtbCommand::MakeMoveVec { elements, .. } => {
                for elem in elements {
                    validate_arg_reference(elem, i, &mut ref_errors);
                }
            }
            _ => {}
        }
        validation_errors.extend(ref_errors);
    }

    // Build response
    let is_valid = validation_errors.is_empty();

    let result_type_info: Vec<serde_json::Value> = command_result_types
        .iter()
        .enumerate()
        .map(|(idx, types)| {
            serde_json::json!({
                "command_index": idx,
                "return_types": types
            })
        })
        .collect();

    if is_valid {
        SandboxResponse::success_with_data(serde_json::json!({
            "valid": true,
            "input_count": inputs.len(),
            "command_count": commands.len(),
            "inputs": input_types,
            "commands": command_info,
            "result_types": result_type_info
        }))
    } else {
        SandboxResponse::success_with_data(serde_json::json!({
            "valid": false,
            "error_count": validation_errors.len(),
            "errors": validation_errors,
            "inputs": input_types,
            "commands": command_info,
            "result_types": result_type_info
        }))
    }
}

/// Call a specific Move function directly.
pub fn execute_call_function(
    env: &mut SimulationEnvironment,
    package: &str,
    module: &str,
    function: &str,
    type_args: &[String],
    args: &[serde_json::Value],
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Calling {}::{}::{}", package, module, function);
    }

    match env.call_function(package, module, function, type_args, args) {
        Ok(result) => SandboxResponse::success_with_data(serde_json::json!({
            "return_values": result.return_values,
            "gas_used": result.gas_used,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Function call failed: {}", e),
            "ExecutionError",
        ),
    }
}

/// Build transaction effects response from internal effects.
fn build_effects_response(
    effects: &crate::benchmark::ptb::TransactionEffects,
) -> TransactionEffectsResponse {
    use crate::benchmark::ptb::{ObjectChange, Owner};

    let owner_to_string = |owner: &Owner| -> String {
        match owner {
            Owner::Address(addr) => format!("address:{}", addr.to_hex_literal()),
            Owner::Shared => "shared".to_string(),
            Owner::Immutable => "immutable".to_string(),
        }
    };

    let mut created = Vec::new();
    let mut mutated = Vec::new();
    let mut deleted = Vec::new();
    let mut wrapped = Vec::new();
    let mut unwrapped = Vec::new();

    let type_to_string =
        |t: &Option<move_core_types::language_storage::TypeTag>| -> Option<String> {
            t.as_ref().map(|tag| format!("{}", tag))
        };

    for change in &effects.object_changes {
        match change {
            ObjectChange::Created {
                id,
                owner,
                object_type,
            } => {
                created.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 1,
                });
            }
            ObjectChange::Mutated {
                id,
                owner,
                object_type,
            } => {
                mutated.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 2,
                });
            }
            ObjectChange::Deleted { id, .. } => {
                deleted.push(id.to_hex_literal());
            }
            ObjectChange::Wrapped { id, .. } => {
                wrapped.push(id.to_hex_literal());
            }
            ObjectChange::Unwrapped {
                id,
                owner,
                object_type,
            } => {
                unwrapped.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: owner_to_string(owner),
                    version: 1,
                });
            }
            ObjectChange::Transferred {
                id,
                recipient,
                object_type,
                ..
            } => {
                mutated.push(ObjectEffectResponse {
                    id: id.to_hex_literal(),
                    object_type: type_to_string(object_type),
                    owner: format!("address:{}", recipient.to_hex_literal()),
                    version: 2,
                });
            }
        }
    }

    // Fallback to simple lists if no object_changes
    if created.is_empty() && !effects.created.is_empty() {
        created = effects
            .created
            .iter()
            .map(|id| ObjectEffectResponse {
                id: id.to_hex_literal(),
                object_type: None,
                owner: "unknown".to_string(),
                version: 1,
            })
            .collect();
    }
    if mutated.is_empty() && !effects.mutated.is_empty() {
        mutated = effects
            .mutated
            .iter()
            .map(|id| ObjectEffectResponse {
                id: id.to_hex_literal(),
                object_type: None,
                owner: "unknown".to_string(),
                version: 2,
            })
            .collect();
    }
    if deleted.is_empty() && !effects.deleted.is_empty() {
        deleted = effects
            .deleted
            .iter()
            .map(|id| id.to_hex_literal())
            .collect();
    }
    if wrapped.is_empty() && !effects.wrapped.is_empty() {
        wrapped = effects
            .wrapped
            .iter()
            .map(|id| id.to_hex_literal())
            .collect();
    }
    if unwrapped.is_empty() && !effects.unwrapped.is_empty() {
        unwrapped = effects
            .unwrapped
            .iter()
            .map(|id| ObjectEffectResponse {
                id: id.to_hex_literal(),
                object_type: None,
                owner: "unknown".to_string(),
                version: 1,
            })
            .collect();
    }

    let return_values: Option<Vec<CommandReturnValues>> = if effects.return_values.is_empty() {
        None
    } else {
        let values: Vec<CommandReturnValues> = effects
            .return_values
            .iter()
            .enumerate()
            .map(|(i, vals)| CommandReturnValues {
                command_index: i,
                values: vals.iter().map(hex::encode).collect(),
                count: vals.len(),
            })
            .collect();
        Some(values)
    };

    TransactionEffectsResponse {
        created,
        mutated,
        deleted,
        wrapped,
        unwrapped,
        return_values,
    }
}

/// Build events response from emitted events.
fn build_events_response(events: &[crate::benchmark::natives::EmittedEvent]) -> Vec<EventResponse> {
    events
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect()
}

/// Categorize simulation error for response.
fn categorize_simulation_error(error: &crate::benchmark::simulation::SimulationError) -> String {
    use crate::benchmark::simulation::SimulationError;
    match error {
        SimulationError::ContractAbort { .. } => "ContractAbort".to_string(),
        SimulationError::MissingPackage { .. } => "MissingPackage".to_string(),
        SimulationError::MissingObject { .. } => "MissingObject".to_string(),
        SimulationError::ExecutionError { .. } => "ExecutionError".to_string(),
        _ => "UnknownError".to_string(),
    }
}
