//! View command - inspect modules, objects, and session state

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde::Serialize;
use sui_sandbox_core::resolver::ModuleProvider;

use super::output::{format_address, format_module_interface};
use super::SandboxState;

#[derive(Parser, Debug)]
pub struct ViewCmd {
    #[command(subcommand)]
    pub target: ViewTarget,
}

#[derive(Subcommand, Debug)]
pub enum ViewTarget {
    /// View a module's interface (structs and functions)
    Module {
        /// Module path: "0xPKG::module" or "module" (uses last published)
        module: String,
    },
    /// View an object's contents (if loaded in session)
    Object {
        /// Object ID (0x...)
        object_id: String,
    },
    /// List all packages loaded in the session
    Packages,
    /// List all modules in a package
    Modules {
        /// Package address (0x...)
        package: String,
    },
}

impl ViewCmd {
    pub async fn execute(&self, state: &SandboxState, json_output: bool) -> Result<()> {
        match &self.target {
            ViewTarget::Module { module } => view_module(state, module, json_output),
            ViewTarget::Object { object_id } => view_object(state, object_id, json_output),
            ViewTarget::Packages => view_packages(state, json_output),
            ViewTarget::Modules { package } => view_modules(state, package, json_output),
        }
    }
}

fn view_module(state: &SandboxState, module_path: &str, json_output: bool) -> Result<()> {
    let (package, module_name) = parse_module_path(module_path, state)?;

    // Get module bytecode from resolver
    let module_id = move_core_types::language_storage::ModuleId::new(
        package,
        move_core_types::identifier::Identifier::new(module_name.clone())?,
    );

    let bytecode = state
        .resolver
        .get_module_bytes(&module_id)
        .ok_or_else(|| {
            anyhow!(
                "Module {}::{} not found in session",
                format_address(&package),
                module_name
            )
        })?
        .to_vec();

    let compiled = CompiledModule::deserialize_with_defaults(&bytecode)
        .context("Failed to deserialize module")?;

    if json_output {
        let info = extract_module_info(&compiled);
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("{}", format_module_interface(&compiled));
    }

    Ok(())
}

fn view_object(state: &SandboxState, object_id: &str, json_output: bool) -> Result<()> {
    let _addr = AccountAddress::from_hex_literal(object_id).context("Invalid object ID")?;

    // Check if object is in persisted state
    if let Some((type_tag, _bytes_b64)) = state.persisted.objects.get(object_id) {
        if json_output {
            #[derive(Serialize)]
            struct ObjectView {
                id: String,
                type_tag: String,
                in_session: bool,
            }

            let view = ObjectView {
                id: object_id.to_string(),
                type_tag: type_tag.clone(),
                in_session: true,
            };
            println!("{}", serde_json::to_string_pretty(&view)?);
        } else {
            println!("\x1b[1mObject:\x1b[0m {}", object_id);
            println!("\x1b[1mType:\x1b[0m {}", type_tag);
            println!("\x1b[1mStatus:\x1b[0m Loaded in session");
        }
    } else if json_output {
        #[derive(Serialize)]
        struct NotFound {
            id: String,
            in_session: bool,
            hint: String,
        }

        let view = NotFound {
            id: object_id.to_string(),
            in_session: false,
            hint: "Use 'sui-sandbox fetch object <ID>' to load from mainnet".to_string(),
        };
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        println!("\x1b[33mObject {} not found in session\x1b[0m", object_id);
        println!(
            "\nUse 'sui-sandbox fetch object {}' to load from mainnet",
            object_id
        );
    }

    Ok(())
}

fn view_packages(state: &SandboxState, json_output: bool) -> Result<()> {
    let packages = state.loaded_packages();

    if json_output {
        #[derive(Serialize)]
        struct PackagesView {
            count: usize,
            packages: Vec<PackageEntry>,
        }

        #[derive(Serialize)]
        struct PackageEntry {
            address: String,
            modules: Vec<String>,
        }

        let entries: Vec<PackageEntry> = packages
            .iter()
            .map(|addr| PackageEntry {
                address: addr.clone(),
                modules: state.get_package_modules(addr).unwrap_or_default(),
            })
            .collect();

        let view = PackagesView {
            count: packages.len(),
            packages: entries,
        };
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else if packages.is_empty() {
        println!("\x1b[33mNo user packages loaded\x1b[0m");
        println!("\nFramework packages (0x1, 0x2, 0x3) are always available.");
        println!("Use 'sui-sandbox publish' or 'sui-sandbox fetch package' to load packages.");
    } else {
        println!("\x1b[1mLoaded Packages:\x1b[0m\n");
        for addr in &packages {
            let modules = state.get_package_modules(addr).unwrap_or_default();
            println!("  \x1b[36m{}\x1b[0m ({} modules)", addr, modules.len());
            for module in &modules {
                println!("    - {}", module);
            }
        }
        println!("\n\x1b[90mNote: Framework packages (0x1, 0x2, 0x3) not listed but always available\x1b[0m");
    }

    Ok(())
}

fn view_modules(state: &SandboxState, package: &str, json_output: bool) -> Result<()> {
    let addr = AccountAddress::from_hex_literal(package).context("Invalid package address")?;

    // Try to get modules from the resolver
    let modules = get_package_modules_from_resolver(&state.resolver, addr)?;

    if json_output {
        #[derive(Serialize)]
        struct ModulesView {
            package: String,
            modules: Vec<ModuleEntry>,
        }

        #[derive(Serialize)]
        struct ModuleEntry {
            name: String,
            structs_count: usize,
            functions_count: usize,
            public_functions_count: usize,
        }

        let entries: Vec<ModuleEntry> = modules
            .iter()
            .map(|m| {
                let public_count = m
                    .function_defs()
                    .iter()
                    .filter(|f| {
                        matches!(
                            f.visibility,
                            move_binary_format::file_format::Visibility::Public
                        )
                    })
                    .count();

                ModuleEntry {
                    name: m.self_id().name().to_string(),
                    structs_count: m.struct_defs().len(),
                    functions_count: m.function_defs().len(),
                    public_functions_count: public_count,
                }
            })
            .collect();

        let view = ModulesView {
            package: package.to_string(),
            modules: entries,
        };
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        println!("\x1b[1mPackage:\x1b[0m {}\n", package);

        if modules.is_empty() {
            println!("\x1b[33mNo modules found\x1b[0m");
        } else {
            println!("\x1b[1mModules:\x1b[0m");
            for m in &modules {
                let module_id = m.self_id();
                let name = module_id.name().to_string();
                let struct_count = m.struct_defs().len();
                let fn_count = m.function_defs().len();
                let public_count = m
                    .function_defs()
                    .iter()
                    .filter(|f| {
                        matches!(
                            f.visibility,
                            move_binary_format::file_format::Visibility::Public
                        )
                    })
                    .count();

                println!(
                    "  \x1b[33m{}\x1b[0m - {} structs, {} functions ({} public)",
                    name, struct_count, fn_count, public_count
                );
            }
        }
    }

    Ok(())
}

/// Parse a module path like "0xPKG::module" or "module"
fn parse_module_path(path: &str, state: &SandboxState) -> Result<(AccountAddress, String)> {
    let parts: Vec<&str> = path.split("::").collect();

    match parts.len() {
        1 => {
            // Just module name - use last published package
            let package = state.last_published().ok_or_else(|| {
                anyhow!("No package published. Use full module path: 0xPKG::module")
            })?;
            Ok((package, parts[0].to_string()))
        }
        2 => {
            // 0xPKG::module
            let package =
                AccountAddress::from_hex_literal(parts[0]).context("Invalid package address")?;
            Ok((package, parts[1].to_string()))
        }
        _ => Err(anyhow!(
            "Invalid module path. Expected '0xPKG::module' or 'module'"
        )),
    }
}

/// Extract module info for JSON output
#[derive(Serialize)]
struct ModuleInfo {
    address: String,
    name: String,
    structs: Vec<StructInfo>,
    functions: Vec<FunctionInfo>,
}

#[derive(Serialize)]
struct StructInfo {
    name: String,
    abilities: Vec<String>,
    type_params: usize,
}

#[derive(Serialize)]
struct FunctionInfo {
    name: String,
    visibility: String,
    is_entry: bool,
    type_params: usize,
    params: usize,
    returns: usize,
}

fn extract_module_info(module: &CompiledModule) -> ModuleInfo {
    let module_id = module.self_id();

    let structs: Vec<StructInfo> = module
        .struct_defs()
        .iter()
        .map(|def| {
            let handle = module.datatype_handle_at(def.struct_handle);
            let name = module.identifier_at(handle.name).to_string();
            let abilities = handle.abilities;

            let mut ability_strs = Vec::new();
            if abilities.has_copy() {
                ability_strs.push("copy".to_string());
            }
            if abilities.has_drop() {
                ability_strs.push("drop".to_string());
            }
            if abilities.has_store() {
                ability_strs.push("store".to_string());
            }
            if abilities.has_key() {
                ability_strs.push("key".to_string());
            }

            StructInfo {
                name,
                abilities: ability_strs,
                type_params: handle.type_parameters.len(),
            }
        })
        .collect();

    let functions: Vec<FunctionInfo> = module
        .function_defs()
        .iter()
        .map(|def| {
            let handle = module.function_handle_at(def.function);
            let name = module.identifier_at(handle.name).to_string();

            let visibility = match def.visibility {
                move_binary_format::file_format::Visibility::Public => "public",
                move_binary_format::file_format::Visibility::Friend => "friend",
                move_binary_format::file_format::Visibility::Private => "private",
            };

            let params_sig = module.signature_at(handle.parameters);
            let returns_sig = module.signature_at(handle.return_);

            FunctionInfo {
                name,
                visibility: visibility.to_string(),
                is_entry: def.is_entry,
                type_params: handle.type_parameters.len(),
                params: params_sig.0.len(),
                returns: returns_sig.0.len(),
            }
        })
        .collect();

    ModuleInfo {
        address: format_address(module_id.address()),
        name: module_id.name().to_string(),
        structs,
        functions,
    }
}

/// Get all modules from a package via the resolver
fn get_package_modules_from_resolver(
    resolver: &sui_sandbox_core::resolver::LocalModuleResolver,
    package: AccountAddress,
) -> Result<Vec<CompiledModule>> {
    use move_core_types::resolver::ModuleResolver;

    let mut modules = Vec::new();

    // Get module names from the package
    let module_names = resolver.get_package_modules(&package);
    for name in module_names {
        let module_id = move_core_types::language_storage::ModuleId::new(
            package,
            move_core_types::identifier::Identifier::new(name)?,
        );

        if let Some(bytecode) = resolver.get_module(&module_id)? {
            if let Ok(compiled) = CompiledModule::deserialize_with_defaults(&bytecode) {
                modules.push(compiled);
            }
        }
    }

    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_module_path_full() {
        // Create a minimal state for testing
        let state = SandboxState::new("https://test.rpc").unwrap();

        let (addr, module) = parse_module_path("0x2::coin", &state).unwrap();
        assert_eq!(addr, AccountAddress::from_hex_literal("0x2").unwrap());
        assert_eq!(module, "coin");
    }

    #[test]
    fn test_parse_module_path_short_no_package() {
        let state = SandboxState::new("https://test.rpc").unwrap();

        // Should fail because no package is published
        let result = parse_module_path("mymodule", &state);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_module_info() {
        // This would require a valid CompiledModule, which is complex to create
        // In practice, this is tested via integration tests
    }
}
