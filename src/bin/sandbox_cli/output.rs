//! Output formatting for sui-sandbox CLI
//!
//! Provides human-readable and JSON output formatting for all commands.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use move_binary_format::file_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde::Serialize;
use std::path::Path;

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
                out.push_str(&format!("  Command [{}]:\n", i));
                for (j, rv_bytes) in cmd_returns.iter().enumerate() {
                    let mut line = format!("    [{}] {}", j, STANDARD.encode(rv_bytes));
                    if let Some(type_tag) = effects
                        .return_type_tags
                        .get(i)
                        .and_then(|tags| tags.get(j))
                        .and_then(|tag| tag.as_ref())
                    {
                        line.push_str(&format!(" (type: {})", type_tag.to_canonical_string(true)));
                    }
                    out.push_str(&line);
                    out.push('\n');
                }
            }
        }
        out.push('\n');
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
        #[serde(skip_serializing_if = "Vec::is_empty")]
        return_values: Vec<Vec<String>>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        return_type_tags: Vec<Vec<Option<String>>>,
    }

    let return_values: Vec<Vec<String>> = effects
        .return_values
        .iter()
        .map(|cmd_returns| {
            cmd_returns
                .iter()
                .map(|bytes| STANDARD.encode(bytes))
                .collect()
        })
        .collect();

    let return_type_tags: Vec<Vec<Option<String>>> = effects
        .return_type_tags
        .iter()
        .map(|cmd_tags| {
            cmd_tags
                .iter()
                .map(|tag| tag.as_ref().map(|t| t.to_canonical_string(true)))
                .collect()
        })
        .collect();

    let json = EffectsJson {
        success: effects.success,
        error: effects.error.clone(),
        gas_used: effects.gas_used,
        created: effects.created.iter().map(format_address).collect(),
        mutated: effects.mutated.iter().map(format_address).collect(),
        deleted: effects.deleted.iter().map(format_address).collect(),
        events_count: effects.events.len(),
        return_values,
        return_type_tags,
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
pub fn print_status(state: &SandboxState, json_output: bool, state_file: Option<&Path>) {
    if json_output {
        #[derive(Serialize)]
        struct StatusJson {
            packages_loaded: usize,
            packages: Vec<PackageInfo>,
            objects_loaded: usize,
            modules_loaded: usize,
            dynamic_fields_loaded: usize,
            last_sender: Option<String>,
            rpc_url: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            state_file: Option<String>,
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
            objects_loaded: state.objects_count(),
            modules_loaded: state.modules_count(),
            dynamic_fields_loaded: state.dynamic_fields_count(),
            last_sender: state.last_sender_hex(),
            rpc_url: state.rpc_url.clone(),
            state_file: state_file.map(|p| p.display().to_string()),
        };

        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("\x1b[1mSui Sandbox Status\x1b[0m\n");
        println!("RPC URL: {}", state.rpc_url);
        if let Some(path) = state_file {
            println!("State file: {}", path.display());
        }
        println!(
            "Objects loaded: {} | Modules loaded: {} | Dynamic fields: {}",
            state.objects_count(),
            state.modules_count(),
            state.dynamic_fields_count()
        );

        if let Some(sender) = state.last_sender_hex() {
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

        if let Some(created) = state.metadata_created_at() {
            println!("\nSession created: {}", created);
        }
        if let Some(modified) = state.metadata_modified_at() {
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
        let mut out = format!("\x1b[31mError:\x1b[0m {}\n", error);
        let mut causes = error.chain().skip(1).peekable();
        if causes.peek().is_some() {
            out.push_str("Caused by:\n");
            for (idx, cause) in causes.enumerate() {
                out.push_str(&format!("  {}: {}\n", idx + 1, cause));
            }
        }
        out
    }
}

/// Structured debug diagnostics suitable for tooling and CI triage.
pub fn format_debug_diagnostic_json(
    command: &str,
    error: &anyhow::Error,
    category: Option<&str>,
    hints: Vec<String>,
) -> String {
    #[derive(Serialize)]
    struct DebugDiagnostic {
        command: String,
        category: String,
        error: String,
        causes: Vec<String>,
        hints: Vec<String>,
        timestamp_utc: String,
    }

    let causes: Vec<String> = error.chain().skip(1).map(|c| c.to_string()).collect();
    let inferred = category
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| classify_error(error));
    let payload = DebugDiagnostic {
        command: command.to_string(),
        category: inferred,
        error: error.to_string(),
        causes,
        hints,
        timestamp_utc: chrono::Utc::now().to_rfc3339(),
    };
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
}

/// Default hints for common operational failures.
pub fn default_diagnostic_hints(command: &str, error: &anyhow::Error) -> Vec<String> {
    let message = error.to_string().to_ascii_lowercase();
    let mut hints = Vec::new();

    if message.contains("not found in session") {
        hints.push(
            "Load required data first with `sui-sandbox fetch package|object ...`".to_string(),
        );
    }
    if message.contains("invalid")
        && (message.contains("digest") || message.contains("address") || message.contains("target"))
    {
        hints.push(
            "Re-run with a fully qualified value and check command help for expected formats"
                .to_string(),
        );
    }
    if message.contains("failed to fetch replay state")
        || message.contains("network")
        || message.contains("timeout")
    {
        hints.push(
            "Check network/rpc connectivity and retry with `--source grpc` or `--source walrus`"
                .to_string(),
        );
    }
    if looks_like_archive_runtime_gap(&message) {
        hints.push(format!(
            "Replay may be failing due to runtime-object gaps on archive endpoints (current: {}); try `SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443`",
            effective_grpc_endpoint_for_hints()
        ));
    }
    if message.contains("state file") {
        hints.push("Verify state-file path permissions, then retry the same command".to_string());
    }
    if command == "run-flow" {
        hints.push("Run with `--dry-run` to validate flow structure before execution".to_string());
    }
    if hints.is_empty() {
        hints.push("Retry with `--verbose` for additional execution details".to_string());
    }
    hints
}

fn effective_grpc_endpoint_for_hints() -> String {
    const MAINNET_ARCHIVE_GRPC: &str = "https://archive.mainnet.sui.io:443";
    std::env::var("SUI_GRPC_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| MAINNET_ARCHIVE_GRPC.to_string())
}

fn looks_like_archive_runtime_gap(message: &str) -> bool {
    (message.contains("contractabort") && message.contains("abort_code: 1"))
        || message.contains("unchanged_loaded_runtime_objects")
        || message.contains("missing runtime object")
        || message.contains("missing object input")
}

fn classify_error(error: &anyhow::Error) -> String {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("invalid") {
        "validation_error".to_string()
    } else if message.contains("timeout")
        || message.contains("network")
        || message.contains("fetch")
        || message.contains("grpc")
    {
        "network_error".to_string()
    } else if message.contains("permission") || message.contains("state file") {
        "io_error".to_string()
    } else {
        "execution_error".to_string()
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
    use anyhow::anyhow;

    #[test]
    fn test_format_address() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        assert_eq!(format_address(&addr), "0x0002");

        let addr = AccountAddress::from_hex_literal("0x123456").unwrap();
        assert_eq!(format_address(&addr), "0x123456");
    }

    #[test]
    fn test_default_diagnostic_hints_archive_runtime_gap() {
        let err = anyhow!(
            "ContractAbort {{ location: Undefined, abort_code: 1 }} (unchanged_loaded_runtime_objects missing)"
        );
        let hints = default_diagnostic_hints("replay", &err);
        assert!(hints.iter().any(|hint| {
            hint.contains("runtime-object gaps")
                && hint.contains("SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443")
        }));
    }
}
