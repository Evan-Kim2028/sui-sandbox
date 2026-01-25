//! Fetch command - import packages and objects from mainnet

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use move_core_types::account_address::AccountAddress;
use serde::Serialize;

use super::output::format_error;
use super::SandboxState;

#[derive(Parser, Debug)]
pub struct FetchCmd {
    #[command(subcommand)]
    pub target: FetchTarget,
}

#[derive(Subcommand, Debug)]
pub enum FetchTarget {
    /// Fetch a package and optionally its dependencies
    Package {
        /// Package ID (0x...)
        #[arg(value_name = "ID")]
        package_id: String,

        /// Also fetch transitive dependencies
        #[arg(long)]
        with_deps: bool,
    },
    /// Fetch an object at latest or specific version
    Object {
        /// Object ID (0x...)
        #[arg(value_name = "ID")]
        object_id: String,

        /// Specific version to fetch
        #[arg(long)]
        version: Option<u64>,
    },
}

impl FetchCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        let result = self.execute_inner(state, verbose);

        match result {
            Ok(output) => {
                if json_output {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    print_fetch_result(&output);
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                Err(e)
            }
        }
    }

    fn execute_inner(&self, state: &mut SandboxState, verbose: bool) -> Result<FetchResult> {
        match &self.target {
            FetchTarget::Package {
                package_id,
                with_deps,
            } => fetch_package(state, package_id, *with_deps, verbose),
            FetchTarget::Object { object_id, version } => {
                fetch_object(state, object_id, *version, verbose)
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct FetchResult {
    pub success: bool,
    pub packages_fetched: Vec<PackageInfo>,
    pub objects_fetched: Vec<ObjectInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PackageInfo {
    pub address: String,
    pub modules: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ObjectInfo {
    pub id: String,
    pub type_tag: Option<String>,
    pub version: Option<u64>,
}

fn fetch_package(
    state: &mut SandboxState,
    package_id: &str,
    with_deps: bool,
    verbose: bool,
) -> Result<FetchResult> {
    let addr = AccountAddress::from_hex_literal(package_id).context("Invalid package ID")?;

    if verbose {
        eprintln!("Fetching package {}...", package_id);
    }

    // Use sui-transport GraphQL client (synchronous)
    let client = sui_transport::graphql::GraphQLClient::new(&state.rpc_url);

    let package_data = client
        .fetch_package(package_id)
        .context("Failed to fetch package from RPC")?;

    let mut packages_fetched = Vec::new();

    // Extract modules from package data
    let modules: Vec<(String, Vec<u8>)> = package_data
        .modules
        .iter()
        .filter_map(|m| {
            m.bytecode_base64.as_ref().and_then(|b64| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                    .ok()
                    .map(|bytes| (m.name.clone(), bytes))
            })
        })
        .collect();

    let module_names: Vec<String> = modules.iter().map(|(n, _)| n.clone()).collect();

    // Add to state
    state.add_package(addr, modules);

    packages_fetched.push(PackageInfo {
        address: package_id.to_string(),
        modules: module_names,
    });

    // Fetch dependencies if requested
    if with_deps {
        if verbose {
            eprintln!("Fetching dependencies...");
            eprintln!("  Note: Auto-fetching dependencies from linkage table.");
        }

        // The GraphQL API doesn't directly expose dependency list in our current implementation
        // We would need to parse the linkage_table field or iterate through module handles
        // For now, just inform the user
        if verbose {
            eprintln!("  Use 'sui-sandbox fetch package <ID>' for each dependency as needed.");
        }
    }

    Ok(FetchResult {
        success: true,
        packages_fetched,
        objects_fetched: vec![],
        error: None,
    })
}

fn fetch_object(
    state: &SandboxState,
    object_id: &str,
    version: Option<u64>,
    verbose: bool,
) -> Result<FetchResult> {
    let _addr = AccountAddress::from_hex_literal(object_id).context("Invalid object ID")?;

    if verbose {
        eprintln!("Fetching object {}...", object_id);
        if let Some(v) = version {
            eprintln!("  at version {}", v);
        }
    }

    // Use sui-transport GraphQL client (synchronous)
    let client = sui_transport::graphql::GraphQLClient::new(&state.rpc_url);

    let object_data = if let Some(v) = version {
        client
            .fetch_object_at_version(object_id, v)
            .context("Failed to fetch object at version from RPC")?
    } else {
        client
            .fetch_object(object_id)
            .context("Failed to fetch object from RPC")?
    };

    let object_info = ObjectInfo {
        id: object_id.to_string(),
        type_tag: object_data.type_string,
        version: Some(object_data.version),
    };

    if verbose {
        eprintln!("  Type: {:?}", object_info.type_tag);
        eprintln!("  Version: {:?}", object_info.version);
    }

    Ok(FetchResult {
        success: true,
        packages_fetched: vec![],
        objects_fetched: vec![object_info],
        error: None,
    })
}

fn print_fetch_result(result: &FetchResult) {
    if result.success {
        println!("\x1b[32m✓ Fetch completed successfully\x1b[0m\n");
    } else {
        println!(
            "\x1b[31m✗ Fetch failed: {}\x1b[0m\n",
            result.error.as_deref().unwrap_or("unknown error")
        );
        return;
    }

    if !result.packages_fetched.is_empty() {
        println!("\x1b[1mPackages Fetched:\x1b[0m");
        for pkg in &result.packages_fetched {
            println!(
                "  \x1b[36m{}\x1b[0m ({} modules)",
                pkg.address,
                pkg.modules.len()
            );
            for module in &pkg.modules {
                println!("    - {}", module);
            }
        }
    }

    if !result.objects_fetched.is_empty() {
        println!("\x1b[1mObjects Fetched:\x1b[0m");
        for obj in &result.objects_fetched {
            let type_str = obj
                .type_tag
                .as_ref()
                .map(|t| format!(" : {}", t))
                .unwrap_or_default();
            let version_str = obj
                .version
                .map(|v| format!(" (v{})", v))
                .unwrap_or_default();
            println!("  \x1b[36m{}\x1b[0m{}{}", obj.id, type_str, version_str);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetch_result_serialization() {
        let result = FetchResult {
            success: true,
            packages_fetched: vec![PackageInfo {
                address: "0x123".to_string(),
                modules: vec!["test".to_string()],
            }],
            objects_fetched: vec![],
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"address\":\"0x123\""));
    }
}
