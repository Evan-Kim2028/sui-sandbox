//! Module-related sandbox handlers.
//!
//! Handles load_module, compile_move, and list_modules operations.

use crate::benchmark::package_builder::PackageBuilder;
use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use std::path::Path;

/// Load a compiled Move module from bytecode file(s).
pub fn execute_load_module(
    env: &mut SimulationEnvironment,
    bytecode_path: &str,
    module_name: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    let path = Path::new(bytecode_path);

    if !path.exists() {
        return SandboxResponse::error(format!("Bytecode path does not exist: {}", bytecode_path));
    }

    // Load .mv files from directory
    let mut modules: Vec<(String, Vec<u8>)> = Vec::new();

    if path.is_dir() {
        // Read all .mv files in directory
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.extension().is_some_and(|e| e == "mv") {
                        let name = file_path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();

                        // Apply module name filter if specified
                        if let Some(filter) = module_name {
                            if !name.contains(filter) {
                                continue;
                            }
                        }

                        match std::fs::read(&file_path) {
                            Ok(bytes) => {
                                if verbose {
                                    eprintln!("Loading module: {} ({} bytes)", name, bytes.len());
                                }
                                modules.push((name, bytes));
                            }
                            Err(e) => {
                                return SandboxResponse::error(format!(
                                    "Failed to read {}: {}",
                                    file_path.display(),
                                    e
                                ));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                return SandboxResponse::error(format!("Failed to read directory: {}", e));
            }
        }
    } else {
        // Single file
        match std::fs::read(path) {
            Ok(bytes) => {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                modules.push((name, bytes));
            }
            Err(e) => {
                return SandboxResponse::error(format!("Failed to read file: {}", e));
            }
        }
    }

    if modules.is_empty() {
        return SandboxResponse::error("No .mv files found in bytecode path");
    }

    // Deploy to environment
    match env.deploy_package(modules.clone()) {
        Ok(address) => SandboxResponse::success_with_data(serde_json::json!({
            "package_address": address.to_hex_literal(),
            "modules_loaded": modules.len(),
            "module_names": modules.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to deploy package: {}", e),
            "DeploymentError",
        ),
    }
}

/// Compile Move source code to bytecode.
pub fn execute_compile_move(
    env: &mut SimulationEnvironment,
    package_name: &str,
    module_name: &str,
    source: &str,
    verbose: bool,
) -> SandboxResponse {
    // Create package builder with temp directory
    let builder = match PackageBuilder::new_temp() {
        Ok(b) => b,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Failed to create package builder: {}", e),
                "CompilationError".to_string(),
            )
        }
    };

    if verbose {
        eprintln!("Compiling module {}::{}", package_name, module_name);
    }

    // Build the package
    match builder.build_from_source(package_name, module_name, source) {
        Ok(result) => {
            if !result.success || result.modules.is_empty() {
                return SandboxResponse::error_with_category(
                    format!("Compilation failed: {}", result.diagnostics),
                    "CompilationError".to_string(),
                );
            }
            // Deploy the compiled modules directly
            match env.deploy_package(result.modules.clone()) {
                Ok(address) => SandboxResponse::success_with_data(serde_json::json!({
                    "package_address": address.to_hex_literal(),
                    "modules_loaded": result.modules.len(),
                    "module_names": result.modules.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                })),
                Err(e) => SandboxResponse::error_with_category(
                    format!("Failed to deploy package: {}", e),
                    "DeploymentError".to_string(),
                ),
            }
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Compilation failed: {}", e),
            "CompilationError".to_string(),
        ),
    }
}

/// List all loaded modules.
pub fn execute_list_modules(env: &SimulationEnvironment, _verbose: bool) -> SandboxResponse {
    let modules = env.list_modules();
    SandboxResponse::success_with_data(serde_json::json!({
        "modules": modules,
        "count": modules.len(),
    }))
}
