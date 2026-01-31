//! Output formatting for sui-sandbox CLI
//!
//! Provides human-readable and JSON output formatting for all commands.

use move_binary_format::file_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde::Serialize;

use super::SandboxState;
use sui_sandbox_core::ptb::TransactionEffects;

/// Format transaction effects for display
pub fn format_effects(effects: &TransactionEffects, verbose: bool) -> String {
    let mut out = String::new();

    if effects.success {
        out.push_str("\x1b[32m✓ Transaction executed successfully\x1b[0m\n\n");
    } else {
        out.push_str(&format!(
            "\x1b[31m✗ Transaction failed: {}\x1b[0m\n\n",
            effects.error.as_deref().unwrap_or("unknown error")
        ));
    }

    // Gas usage
    if effects.gas_used > 0 {
        out.push_str(&format!("Gas used: {} units\n\n", effects.gas_used));
    }

    // Created objects
    if !effects.created.is_empty() {
        out.push_str("\x1b[1mCreated Objects:\x1b[0m\n");
        for id in &effects.created {
            out.push_str(&format!("  \x1b[36m{}\x1b[0m\n", format_address(id)));
        }
        out.push('\n');
    }

    // Mutated objects
    if !effects.mutated.is_empty() {
        out.push_str("\x1b[1mMutated Objects:\x1b[0m\n");
        for id in &effects.mutated {
            out.push_str(&format!("  \x1b[33m{}\x1b[0m\n", format_address(id)));
        }
        out.push('\n');
    }

    // Deleted objects
    if !effects.deleted.is_empty() {
        out.push_str("\x1b[1mDeleted Objects:\x1b[0m\n");
        for id in &effects.deleted {
            out.push_str(&format!("  \x1b[31m{}\x1b[0m\n", format_address(id)));
        }
        out.push('\n');
    }

    // Events
    if !effects.events.is_empty() {
        out.push_str(&format!(
            "\x1b[1mEvents:\x1b[0m {} emitted\n",
            effects.events.len()
        ));
        if verbose {
            for (i, event) in effects.events.iter().enumerate() {
                out.push_str(&format!("  [{}] {}\n", i, event.type_tag));
            }
        }
        out.push('\n');
    }

    // Return values (if any and verbose)
    if verbose && !effects.return_values.is_empty() {
        out.push_str("\x1b[1mReturn Values:\x1b[0m\n");
        for (i, cmd_returns) in effects.return_values.iter().enumerate() {
            if !cmd_returns.is_empty() {
                out.push_str(&format!(
                    "  Command [{}]: {} return value(s)\n",
                    i,
                    cmd_returns.len()
                ));
            }
        }
    }

    out
}

/// Format effects as JSON
pub fn format_effects_json(effects: &TransactionEffects) -> String {
    #[derive(Serialize)]
    struct EffectsJson {
        success: bool,
        error: Option<String>,
        gas_used: u64,
        created: Vec<String>,
        mutated: Vec<String>,
        deleted: Vec<String>,
        events_count: usize,
    }

    let json = EffectsJson {
        success: effects.success,
        error: effects.error.clone(),
        gas_used: effects.gas_used,
        created: effects.created.iter().map(format_address).collect(),
        mutated: effects.mutated.iter().map(format_address).collect(),
        deleted: effects.deleted.iter().map(format_address).collect(),
        events_count: effects.events.len(),
    };

    serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".to_string())
}

/// Format a publish result
pub fn format_publish_result(
    package_address: AccountAddress,
    modules: &[(String, Vec<u8>)],
    json_output: bool,
) -> String {
    if json_output {
        #[derive(Serialize)]
        struct PublishResult {
            package_address: String,
            modules: Vec<String>,
        }

        let result = PublishResult {
            package_address: format_address(&package_address),
            modules: modules.iter().map(|(name, _)| name.clone()).collect(),
        };
        serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
    } else {
        let mut out = String::new();
        out.push_str("\x1b[32m✓ Package published successfully\x1b[0m\n\n");
        out.push_str(&format!(
            "\x1b[1mPackage Address:\x1b[0m \x1b[36m{}\x1b[0m\n\n",
            format_address(&package_address)
        ));
        out.push_str("\x1b[1mModules:\x1b[0m\n");
        for (name, bytes) in modules {
            out.push_str(&format!("  {} ({} bytes)\n", name, bytes.len()));
        }
        out
    }
}

/// Format module interface for display
pub fn format_module_interface(module: &CompiledModule) -> String {
    let mut out = String::new();

    let module_id = module.self_id();
    out.push_str(&format!(
        "\x1b[1mModule:\x1b[0m {}::{}\n\n",
        format_address(module_id.address()),
        module_id.name()
    ));

    // Structs
    if !module.struct_defs().is_empty() {
        out.push_str("\x1b[1mStructs:\x1b[0m\n");
        for struct_def in module.struct_defs() {
            let struct_handle = module.datatype_handle_at(struct_def.struct_handle);
            let name = module.identifier_at(struct_handle.name);

            // Get abilities
            let abilities = struct_handle.abilities;
            let ability_strs: Vec<&str> = [
                (abilities.has_copy(), "copy"),
                (abilities.has_drop(), "drop"),
                (abilities.has_store(), "store"),
                (abilities.has_key(), "key"),
            ]
            .iter()
            .filter_map(|(has, name)| if *has { Some(*name) } else { None })
            .collect();

            let abilities_str = if ability_strs.is_empty() {
                String::new()
            } else {
                format!(" has {}", ability_strs.join(", "))
            };

            out.push_str(&format!(
                "  \x1b[33mstruct\x1b[0m {}{}\n",
                name, abilities_str
            ));
        }
        out.push('\n');
    }

    // Functions
    let public_fns: Vec<_> = module
        .function_defs()
        .iter()
        .enumerate()
        .filter(|(_, def)| {
            matches!(
                def.visibility,
                move_binary_format::file_format::Visibility::Public
            )
        })
        .collect();

    if !public_fns.is_empty() {
        out.push_str("\x1b[1mPublic Functions:\x1b[0m\n");
        for (_, func_def) in public_fns {
            let func_handle = module.function_handle_at(func_def.function);
            let name = module.identifier_at(func_handle.name);

            let entry_str = if func_def.is_entry { " entry" } else { "" };

            out.push_str(&format!(
                "  \x1b[34mpublic{}\x1b[0m fun {}(...)\n",
                entry_str, name
            ));
        }
    }

    out
}

/// Format session status
pub fn print_status(state: &SandboxState, json_output: bool) {
    if json_output {
        #[derive(Serialize)]
        struct StatusJson {
            packages_loaded: usize,
            packages: Vec<PackageInfo>,
            last_sender: Option<String>,
            rpc_url: String,
        }

        #[derive(Serialize)]
        struct PackageInfo {
            address: String,
            modules: Vec<String>,
        }

        let packages: Vec<PackageInfo> = state
            .loaded_packages()
            .iter()
            .map(|addr| PackageInfo {
                address: addr.clone(),
                modules: state.get_package_modules(addr).unwrap_or_default(),
            })
            .collect();

        let status = StatusJson {
            packages_loaded: packages.len(),
            packages,
            last_sender: state.persisted.last_sender.clone(),
            rpc_url: state.rpc_url.clone(),
        };

        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("\x1b[1mSui Sandbox Status\x1b[0m\n");
        println!("RPC URL: {}", state.rpc_url);

        if let Some(sender) = &state.persisted.last_sender {
            println!("Last sender: {}", sender);
        }

        let packages = state.loaded_packages();
        if packages.is_empty() {
            println!("\nNo packages loaded (framework always available)");
        } else {
            println!("\n\x1b[1mLoaded Packages:\x1b[0m");
            for addr in packages {
                let modules = state.get_package_modules(&addr).unwrap_or_default();
                println!("  {} ({} modules)", addr, modules.len());
                for module in modules {
                    println!("    - {}", module);
                }
            }
        }

        if let Some(created) = &state.persisted.metadata.created_at {
            println!("\nSession created: {}", created);
        }
        if let Some(modified) = &state.persisted.metadata.last_modified {
            println!("Last modified: {}", modified);
        }
    }
}

/// Format an error for display
pub fn format_error(error: &anyhow::Error, json_output: bool) -> String {
    if json_output {
        #[derive(Serialize)]
        struct ErrorJson {
            error: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            cause: Option<String>,
        }

        let err = ErrorJson {
            error: error.to_string(),
            cause: error.source().map(|e| e.to_string()),
        };
        serde_json::to_string_pretty(&err).unwrap_or_else(|_| "{}".to_string())
    } else {
        format!("\x1b[31mError:\x1b[0m {}\n", error)
    }
}

/// Format an address for display (shortened form)
pub fn format_address(addr: &AccountAddress) -> String {
    let hex = hex::encode(addr);
    // Remove leading zeros but keep at least 4 chars
    let trimmed = hex.trim_start_matches('0');
    if trimmed.len() < 4 {
        format!("0x{:0>4}", trimmed)
    } else {
        format!("0x{}", trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_address() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert_eq!(format_address(&addr), "0x0002");

        let addr = AccountAddress::from_hex_literal("0x123456").unwrap();
        assert_eq!(format_address(&addr), "0x123456");
    }
}
