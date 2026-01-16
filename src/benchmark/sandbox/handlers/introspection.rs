//! Type introspection sandbox handlers.
//!
//! Handles list_functions, list_structs, get_function_info, find_constructors,
//! search_types, search_functions, get_struct_info, inspect_struct, and get_system_object_info.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;

/// List all functions in a module.
pub fn execute_list_functions(
    env: &SimulationEnvironment,
    module_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.list_functions(module_path) {
        Some(functions) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "functions": functions,
            "count": functions.len(),
        })),
        None => SandboxResponse::error(format!("Module not found: {}", module_path)),
    }
}

/// List all structs in a module.
pub fn execute_list_structs(
    env: &SimulationEnvironment,
    module_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.list_structs(module_path) {
        Some(structs) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "structs": structs,
            "count": structs.len(),
        })),
        None => SandboxResponse::error(format!("Module not found: {}", module_path)),
    }
}

/// Get detailed function information.
pub fn execute_get_function_info(
    env: &SimulationEnvironment,
    module_path: &str,
    function_name: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.get_function_info(module_path, function_name) {
        Some(info) => {
            SandboxResponse::success_with_data(serde_json::to_value(info).unwrap_or_default())
        }
        None => SandboxResponse::error(format!(
            "Function not found: {}::{}",
            module_path, function_name
        )),
    }
}

/// Find constructors for a type.
pub fn execute_find_constructors(
    env: &SimulationEnvironment,
    type_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    let constructors = env.find_constructors(type_path);
    SandboxResponse::success_with_data(serde_json::json!({
        "type": type_path,
        "constructors": constructors,
        "count": constructors.len(),
    }))
}

/// Search for types matching a pattern.
pub fn execute_search_types(
    env: &SimulationEnvironment,
    pattern: &str,
    ability_filter: Option<&str>,
    _verbose: bool,
) -> SandboxResponse {
    let results = env.search_types(pattern, ability_filter);
    SandboxResponse::success_with_data(serde_json::json!({
        "pattern": pattern,
        "ability_filter": ability_filter,
        "matches": results,
        "count": results.len(),
    }))
}

/// Search for functions matching a pattern.
pub fn execute_search_functions(
    env: &SimulationEnvironment,
    pattern: &str,
    entry_only: bool,
    _verbose: bool,
) -> SandboxResponse {
    let results = env.search_functions(pattern, entry_only);
    SandboxResponse::success_with_data(serde_json::json!({
        "pattern": pattern,
        "entry_only": entry_only,
        "matches": results,
        "count": results.len(),
    }))
}

/// Get struct type definition details.
pub fn execute_get_struct_info(
    env: &SimulationEnvironment,
    type_path: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.get_struct_info(type_path) {
        Some(info) => {
            SandboxResponse::success_with_data(serde_json::to_value(info).unwrap_or_default())
        }
        None => SandboxResponse::error(format!("Struct not found: {}", type_path)),
    }
}

/// Get struct definition(s) from loaded modules.
pub fn execute_inspect_struct(
    env: &SimulationEnvironment,
    package: &str,
    module: Option<&str>,
    struct_name: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Inspecting struct in package {} module {:?} name {:?}",
            package, module, struct_name
        );
    }

    match env.get_struct_definitions(package, module, struct_name) {
        Ok(definitions) => {
            // Convert StructDefinition to JSON-serializable format
            let json_defs: Vec<serde_json::Value> = definitions
                .into_iter()
                .map(|def| {
                    serde_json::json!({
                        "package": def.package,
                        "module": def.module,
                        "name": def.name,
                        "abilities": def.abilities,
                        "type_params": def.type_params.into_iter().map(|tp| serde_json::json!({
                            "name": tp.name,
                            "constraints": tp.constraints,
                        })).collect::<Vec<_>>(),
                        "fields": def.fields.into_iter().map(|f| serde_json::json!({
                            "name": f.name,
                            "field_type": f.field_type,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            SandboxResponse::success_with_data(serde_json::json!({
                "definitions": json_defs,
                "count": json_defs.len(),
            }))
        }
        Err(e) => SandboxResponse::error(format!("Failed to get struct definitions: {}", e)),
    }
}

/// Get system object information.
pub fn execute_get_system_object_info(object_name: &str, _verbose: bool) -> SandboxResponse {
    let info = match object_name.to_lowercase().as_str() {
        "clock" => serde_json::json!({
            "name": "Clock",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000006",
            "short_id": "0x6",
            "type": "0x2::clock::Clock",
            "is_shared": true,
            "description": "Global clock for timestamp access. Use Clock::timestamp_ms() to get current time.",
            "fields": [
                {"name": "id", "type": "UID"},
                {"name": "timestamp_ms", "type": "u64"}
            ],
            "common_usage": "&Clock as function parameter for time-dependent logic"
        }),
        "random" => serde_json::json!({
            "name": "Random",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000008",
            "short_id": "0x8",
            "type": "0x2::random::Random",
            "is_shared": true,
            "description": "On-chain randomness source.",
            "fields": [
                {"name": "id", "type": "UID"},
                {"name": "inner", "type": "Versioned"}
            ],
            "common_usage": "&Random as function parameter for randomness"
        }),
        "deny_list" => serde_json::json!({
            "name": "DenyList",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000403",
            "short_id": "0x403",
            "type": "0x2::deny_list::DenyList",
            "is_shared": true,
            "description": "Global deny list for regulated coins.",
            "common_usage": "&DenyList for checking/updating denied addresses"
        }),
        "system_state" => serde_json::json!({
            "name": "SuiSystemState",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000005",
            "short_id": "0x5",
            "type": "0x3::sui_system::SuiSystemState",
            "is_shared": true,
            "description": "Sui system state object (validators, epoch info, etc.).",
            "common_usage": "Used internally by system transactions"
        }),
        _ => {
            return SandboxResponse::error(format!(
                "Unknown system object: {}. Valid options: clock, random, deny_list, system_state",
                object_name
            ));
        }
    };

    SandboxResponse::success_with_data(info)
}
