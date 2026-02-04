//! Shared fetch utilities for transaction replay.
//!
//! This module provides reusable functions for fetching packages and objects
//! during transaction replay, used by both CLI and MCP server.

use std::collections::HashMap;

use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use sui_sandbox_types::env_bool;
use sui_transport::grpc::GrpcClient;

use crate::types::PackageData;
use crate::HistoricalStateProvider;

/// Result of building package aliases.
#[derive(Debug, Default)]
pub struct PackageAliases {
    /// Maps storage address -> runtime address for upgraded packages.
    pub aliases: HashMap<AccountAddress, AccountAddress>,
    /// Maps package address -> version.
    pub versions: HashMap<AccountAddress, u64>,
    /// Maps runtime address -> storage address for linkage resolution.
    pub linkage_upgrades: HashMap<AccountAddress, AccountAddress>,
}

/// Build alias maps from fetched packages.
///
/// This function builds three maps:
/// 1. `aliases`: storage_id -> runtime_id (for resolving type tags)
/// 2. `versions`: package_id -> version (for version tracking)
/// 3. `linkage_upgrades`: runtime_id -> storage_id (for linkage resolution)
///
/// If a provider is given and checkpoint is None, it also fetches upgrade history
/// from GraphQL to populate aliases for all known upgrades.
pub fn build_aliases(
    packages: &HashMap<AccountAddress, PackageData>,
    provider: Option<&HistoricalStateProvider>,
    checkpoint: Option<u64>,
) -> PackageAliases {
    let mut result = PackageAliases::default();

    for pkg in packages.values() {
        let runtime = pkg.original_id.unwrap_or(pkg.address);
        if runtime != pkg.address {
            result.aliases.insert(pkg.address, runtime);
            result.linkage_upgrades.insert(runtime, pkg.address);
        }
        result.versions.insert(pkg.address, pkg.version);
        for (runtime_dep, storage_dep) in &pkg.linkage {
            if runtime_dep != storage_dep {
                result.aliases.insert(*storage_dep, *runtime_dep);
            }
        }
    }

    if let Some(provider) = provider {
        if checkpoint.is_some() {
            // Avoid pulling future upgrade aliases when replaying historical checkpoints.
            return result;
        }
        let gql = provider.graphql();
        for pkg in packages.values() {
            let runtime = pkg.original_id.unwrap_or(pkg.address);
            let runtime_hex = runtime.to_hex_literal();
            if let Ok(upgrades) = gql.get_package_upgrades(&runtime_hex) {
                for (addr, ver) in upgrades {
                    if let Ok(storage_addr) = AccountAddress::from_hex_literal(&addr) {
                        if storage_addr != runtime {
                            result.aliases.insert(storage_addr, runtime);
                        }
                        result.versions.insert(storage_addr, ver);
                    }
                }
            }
        }
    }

    if env_bool("SUI_DEBUG_LINKAGE") {
        eprintln!(
            "[linkage] alias_count={} version_count={} linkage_upgrade_count={}",
            result.aliases.len(),
            result.versions.len(),
            result.linkage_upgrades.len()
        );
        for (storage, runtime) in &result.aliases {
            eprintln!(
                "[linkage] alias storage={} -> runtime={}",
                storage.to_hex_literal(),
                runtime.to_hex_literal()
            );
        }
    }

    result
}

/// Fetch a child object (e.g., dynamic field) with version constraints.
///
/// Tries multiple sources in order:
/// 1. Local cache (versioned)
/// 2. GraphQL at checkpoint (if provided)
/// 3. gRPC latest
/// 4. GraphQL latest
///
/// Returns (type_tag, bcs_bytes, version) if found and version <= max_version.
pub fn fetch_child_object(
    provider: &HistoricalStateProvider,
    child_id: AccountAddress,
    checkpoint: Option<u64>,
    max_version: u64,
) -> Option<(TypeTag, Vec<u8>, u64)> {
    let debug_df = env_bool("SUI_DEBUG_DF_FETCH");
    let strict_checkpoint = checkpoint.is_some() && env_bool("SUI_DF_STRICT_CHECKPOINT");
    let cache = provider.cache();
    let mut best: Option<(TypeTag, Vec<u8>, u64)> = None;

    // Try cache first
    for ver in cache.get_object_versions(&child_id) {
        if ver > max_version {
            continue;
        }
        if let Some(obj) = cache.get_object(&child_id, ver) {
            if let Some(type_str) = obj.type_tag {
                if let Some(tag) = sui_sandbox_types::parse_type_tag(&type_str) {
                    best = Some((tag, obj.bcs_bytes, obj.version));
                }
            }
        }
    }
    if let Some(hit) = best {
        if debug_df {
            eprintln!(
                "[df_fetch] cache versioned child={} version={}",
                child_id.to_hex_literal(),
                hit.2
            );
        }
        return Some(hit);
    }

    let gql = provider.graphql();
    let id_str = child_id.to_hex_literal();

    // Try GraphQL at checkpoint
    if let Some(cp) = checkpoint {
        if let Ok(obj) = gql.fetch_object_at_checkpoint(&id_str, cp) {
            if obj.version <= max_version {
                if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64) {
                        if let Some(tag) = sui_sandbox_types::parse_type_tag(&type_str) {
                            if debug_df {
                                eprintln!(
                                    "[df_fetch] checkpoint child={} version={}",
                                    id_str, obj.version
                                );
                            }
                            return Some((tag, bytes, obj.version));
                        }
                    }
                }
            }
        }
    }

    if strict_checkpoint {
        if debug_df {
            eprintln!(
                "[df_fetch] strict checkpoint skip latest child={}",
                child_id.to_hex_literal()
            );
        }
        return None;
    }

    // Try gRPC latest
    if let Some((tag, bytes, version)) = fetch_object_via_grpc(provider, &id_str, None) {
        if version <= max_version {
            if debug_df {
                eprintln!(
                    "[df_fetch] grpc latest child={} version={}",
                    id_str, version
                );
            }
            return Some((tag, bytes, version));
        }
    }

    // Try GraphQL latest
    if let Ok(obj) = gql.fetch_object(&id_str) {
        if obj.version <= max_version {
            if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64) {
                    if let Some(tag) = sui_sandbox_types::parse_type_tag(&type_str) {
                        if debug_df {
                            eprintln!("[df_fetch] latest child={} version={}", id_str, obj.version);
                        }
                        return Some((tag, bytes, obj.version));
                    }
                }
            }
        }
    }

    None
}

/// Fetch an object via gRPC.
///
/// Returns (type_tag, bcs_bytes, version) if successful.
pub fn fetch_object_via_grpc(
    provider: &HistoricalStateProvider,
    object_id: &str,
    version: Option<u64>,
) -> Option<(TypeTag, Vec<u8>, u64)> {
    let endpoint = provider.grpc_endpoint().to_string();
    let fut = async {
        let client = GrpcClient::new(&endpoint).await.ok()?;
        client
            .get_object_at_version(object_id, version)
            .await
            .ok()
            .flatten()
    };
    let grpc_obj = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(fut))
    } else {
        let rt = tokio::runtime::Runtime::new().ok()?;
        rt.block_on(fut)
    }?;

    let bcs_bytes = grpc_obj.bcs?;
    let type_str = grpc_obj.type_string?;
    let tag = sui_sandbox_types::parse_type_tag(&type_str)?;
    Some((tag, bcs_bytes, grpc_obj.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_aliases_empty() {
        let packages = HashMap::new();
        let result = build_aliases(&packages, None, None);
        assert!(result.aliases.is_empty());
        assert!(result.versions.is_empty());
        assert!(result.linkage_upgrades.is_empty());
    }

    #[test]
    fn test_build_aliases_simple() {
        let mut packages = HashMap::new();
        let storage_addr = AccountAddress::from_hex_literal("0xabc").unwrap();
        let runtime_addr = AccountAddress::from_hex_literal("0x2").unwrap();

        packages.insert(
            storage_addr,
            PackageData {
                address: storage_addr,
                version: 5,
                original_id: Some(runtime_addr),
                modules: vec![],
                linkage: HashMap::new(),
            },
        );

        let result = build_aliases(&packages, None, None);

        assert_eq!(result.aliases.get(&storage_addr), Some(&runtime_addr));
        assert_eq!(result.versions.get(&storage_addr), Some(&5));
        assert_eq!(
            result.linkage_upgrades.get(&runtime_addr),
            Some(&storage_addr)
        );
    }
}
