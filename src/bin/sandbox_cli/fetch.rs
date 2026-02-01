//! Fetch command - import packages and objects from mainnet

use anyhow::{Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use move_core_types::account_address::AccountAddress;
use serde::Serialize;

use super::network::{cache_dir, infer_network, resolve_graphql_endpoint};
use super::output::format_error;
use super::state::ObjectMetadata;
use super::SandboxState;
use std::collections::HashMap;
use sui_state_fetcher::types::{PackageData, VersionedObject};
use sui_state_fetcher::{HistoricalStateProvider, VersionedCache};
use sui_transport::graphql::GraphQLClient;
use sui_transport::graphql::ObjectOwner;

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
    /// Ingest packages from Walrus checkpoints into the local index
    Checkpoints {
        /// Start checkpoint (inclusive)
        #[arg(value_name = "START")]
        start: u64,

        /// End checkpoint (inclusive)
        #[arg(value_name = "END")]
        end: u64,

        /// Number of concurrent checkpoint fetches (default: 4)
        #[arg(long, default_value = "4")]
        concurrency: usize,
    },
}

impl FetchCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        // Handle checkpoints separately since it's async and doesn't use sandbox state
        if let FetchTarget::Checkpoints {
            start,
            end,
            concurrency,
        } = &self.target
        {
            return self
                .execute_checkpoints(*start, *end, *concurrency, json_output, verbose)
                .await;
        }

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

    async fn execute_checkpoints(
        &self,
        start: u64,
        end: u64,
        concurrency: usize,
        json_output: bool,
        verbose: bool,
    ) -> Result<()> {
        if verbose {
            eprintln!(
                "Ingesting packages from checkpoints {}..{} (concurrency: {})",
                start, end, concurrency
            );
        }

        let provider = HistoricalStateProvider::mainnet().await?;

        let ingested = provider
            .ingest_packages_from_checkpoint_range(start, end, concurrency)
            .await?;

        let result = IngestResult {
            success: true,
            start_checkpoint: start,
            end_checkpoint: end,
            packages_ingested: ingested,
            error: None,
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            println!(
                "\x1b[32m✓ Ingested {} packages from checkpoints {}..{}\x1b[0m",
                ingested, start, end
            );
        }

        Ok(())
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
            FetchTarget::Checkpoints { .. } => {
                unreachable!("Checkpoints handled in execute()")
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

#[derive(Debug, Serialize)]
pub struct IngestResult {
    pub success: bool,
    pub start_checkpoint: u64,
    pub end_checkpoint: u64,
    pub packages_ingested: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
    let client = GraphQLClient::new(&graphql_endpoint);
    let network = infer_network(&state.rpc_url, &graphql_endpoint);
    let cache = VersionedCache::with_storage(cache_dir(&network))?;

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
    state.add_package(addr, modules.clone());

    packages_fetched.push(PackageInfo {
        address: package_id.to_string(),
        modules: module_names,
    });

    // Populate the shared versioned cache for MCP parity.
    let pkg = PackageData {
        address: addr,
        version: package_data.version,
        modules,
        linkage: HashMap::new(),
        original_id: None,
    };
    cache.put_package(pkg);
    let _ = cache.flush();

    // Fetch dependencies if requested
    if with_deps {
        if verbose {
            eprintln!("Fetching dependencies (closure)...");
        }
        let fetched =
            fetch_dependency_closure(state, &client, &cache, verbose, &mut packages_fetched)?;
        if verbose && fetched > 0 {
            eprintln!("  ✓ fetched {} dependency packages", fetched);
        }
    }

    Ok(FetchResult {
        success: true,
        packages_fetched,
        objects_fetched: vec![],
        error: None,
    })
}

fn fetch_dependency_closure(
    state: &mut SandboxState,
    client: &GraphQLClient,
    cache: &VersionedCache,
    verbose: bool,
    packages_fetched: &mut Vec<PackageInfo>,
) -> Result<usize> {
    use std::collections::BTreeSet;
    const MAX_ROUNDS: usize = 8;

    let mut fetched = 0usize;
    let mut seen: BTreeSet<AccountAddress> = BTreeSet::new();

    for _ in 0..MAX_ROUNDS {
        let missing = state.resolver.get_missing_dependencies();
        let pending: Vec<AccountAddress> = missing
            .into_iter()
            .filter(|addr| !seen.contains(addr))
            .collect();
        if pending.is_empty() {
            break;
        }
        for addr in pending {
            seen.insert(addr);
            let addr_hex = addr.to_hex_literal();
            if verbose {
                eprintln!("  fetching {}", addr_hex);
            }
            let pkg = match client.fetch_package(&addr_hex) {
                Ok(p) => p,
                Err(err) => {
                    if verbose {
                        eprintln!("  failed to fetch {}: {}", addr_hex, err);
                    }
                    continue;
                }
            };

            let mut modules: Vec<(String, Vec<u8>)> = Vec::new();
            let mut module_names = Vec::new();
            for module in pkg.modules {
                if let Some(b64) = module.bytecode_base64 {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                        module_names.push(module.name.clone());
                        modules.push((module.name, bytes));
                    }
                }
            }
            if modules.is_empty() {
                if verbose {
                    eprintln!("  no modules for {}", addr_hex);
                }
                continue;
            }

            state.add_package(addr, modules.clone());
            packages_fetched.push(PackageInfo {
                address: addr_hex.clone(),
                modules: module_names,
            });
            let pkg_data = PackageData {
                address: addr,
                version: pkg.version,
                modules,
                linkage: HashMap::new(),
                original_id: None,
            };
            cache.put_package(pkg_data);
            let _ = cache.flush();
            fetched += 1;
        }
    }

    Ok(fetched)
}

fn fetch_object(
    state: &mut SandboxState,
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
    let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
    let client = sui_transport::graphql::GraphQLClient::new(&graphql_endpoint);
    let network = infer_network(&state.rpc_url, &graphql_endpoint);
    let cache = VersionedCache::with_storage(cache_dir(&network))?;

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
        type_tag: object_data.type_string.clone(),
        version: Some(object_data.version),
    };

    if verbose {
        eprintln!("  Type: {:?}", object_info.type_tag);
        eprintln!("  Version: {:?}", object_info.version);
    }

    if let Some(bcs_base64) = &object_data.bcs_base64 {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bcs_base64)
            .context("Failed to decode object BCS")?;
        let (is_shared, is_immutable, owner) = match &object_data.owner {
            ObjectOwner::Shared { .. } => (true, false, Some("shared".to_string())),
            ObjectOwner::Immutable => (false, true, Some("immutable".to_string())),
            ObjectOwner::Address(addr) => (false, false, Some(format!("address:{}", addr))),
            ObjectOwner::Parent(parent) => (false, false, Some(format!("object:{}", parent))),
            ObjectOwner::Unknown => (false, false, None),
        };

        if let Some(type_tag) = object_data.type_string.as_deref() {
            let meta = ObjectMetadata {
                version: object_data.version,
                is_shared,
                is_immutable,
                owner: owner.clone(),
            };
            state.add_object_with_metadata(object_id, Some(type_tag), bcs_base64, meta)?;
        } else if verbose {
            eprintln!("  Warning: object missing type tag; not loaded into session");
        }

        let id = AccountAddress::from_hex_literal(object_id)?;
        let versioned = VersionedObject {
            id,
            version: object_data.version,
            digest: object_data.digest.clone(),
            type_tag: object_data.type_string.clone(),
            bcs_bytes: bytes,
            is_shared,
            is_immutable,
        };
        cache.put_object(versioned);
        let _ = cache.flush();
    } else if verbose {
        eprintln!("  Warning: object has no BCS payload; not loaded into session");
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
