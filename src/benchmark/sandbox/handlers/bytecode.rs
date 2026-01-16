//! Bytecode analysis sandbox handlers.
//!
//! Handles disassemble_function, disassemble_module, module_summary, and get_module_dependencies.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use crate::utils::format_address_short;
use move_core_types::account_address::AccountAddress;

/// Normalize address to short form for display.
fn normalize_address(addr: &AccountAddress) -> String {
    format_address_short(addr)
}

/// Disassemble a function to bytecode.
pub fn execute_disassemble_function(
    env: &SimulationEnvironment,
    module_path: &str,
    function_name: &str,
    _verbose: bool,
) -> SandboxResponse {
    match env.disassemble_function(module_path, function_name) {
        Some(disasm) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "function": function_name,
            "disassembly": disasm,
        })),
        None => SandboxResponse::error(format!(
            "Function not found or cannot disassemble: {}::{}",
            module_path, function_name
        )),
    }
}

/// Get module dependency graph.
pub fn execute_get_module_dependencies(
    env: &SimulationEnvironment,
    module_path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting dependencies for module: {}", module_path);
    }

    // Parse module path
    let parts: Vec<&str> = module_path.split("::").collect();
    if parts.len() != 2 {
        return SandboxResponse::error_with_category(
            format!(
                "Invalid module path: {}. Expected format: '0x2::module'",
                module_path
            ),
            "ParseError".to_string(),
        );
    }

    let address = match AccountAddress::from_hex_literal(parts[0]) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid address in module path: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let module_name = parts[1];

    // Get dependencies from resolver
    match env.get_module_dependencies(&address, module_name) {
        Ok(deps) => {
            let dep_list: Vec<String> = deps
                .iter()
                .map(|(addr, name)| format!("{}::{}", normalize_address(addr), name))
                .collect();
            SandboxResponse::success_with_data(serde_json::json!({
                "module": module_path,
                "dependencies": dep_list,
                "count": dep_list.len(),
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to get dependencies: {}", e),
            "ModuleNotFound".to_string(),
        ),
    }
}

/// Disassemble an entire module's bytecode.
pub fn execute_disassemble_module(
    env: &SimulationEnvironment,
    module_path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Disassembling module: {}", module_path);
    }

    // Parse module path
    let parts: Vec<&str> = module_path.split("::").collect();
    if parts.len() != 2 {
        return SandboxResponse::error_with_category(
            format!(
                "Invalid module path: {}. Expected format: '0x2::module'",
                module_path
            ),
            "ParseError".to_string(),
        );
    }

    let address = match AccountAddress::from_hex_literal(parts[0]) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid address in module path: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let module_name = parts[1];

    match env.disassemble_module(&address, module_name) {
        Ok(disasm) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "disassembly": disasm,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to disassemble module: {}", e),
            "ModuleNotFound".to_string(),
        ),
    }
}

/// Get human-readable module summary.
pub fn execute_module_summary(
    env: &SimulationEnvironment,
    module_path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting summary for module: {}", module_path);
    }

    // Parse module path
    let parts: Vec<&str> = module_path.split("::").collect();
    if parts.len() != 2 {
        return SandboxResponse::error_with_category(
            format!(
                "Invalid module path: {}. Expected format: '0x2::module'",
                module_path
            ),
            "ParseError".to_string(),
        );
    }

    let address = match AccountAddress::from_hex_literal(parts[0]) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Invalid address in module path: {}", e),
                "ParseError".to_string(),
            );
        }
    };

    let module_name = parts[1];

    match env.get_module_summary(&address, module_name) {
        Ok(summary) => SandboxResponse::success_with_data(serde_json::json!({
            "module": module_path,
            "summary": summary,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to get module summary: {}", e),
            "ModuleNotFound".to_string(),
        ),
    }
}
