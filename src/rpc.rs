//! Package data fetching via GraphQL.
//!
//! Provides functions for resolving package addresses, fetching BCS module bytes,
//! and building interface JSON from on-chain packages.
//!
//! ## Migration Note
//!
//! This module was migrated from JSON-RPC to GraphQL in v0.6.0. The function
//! signatures remain the same for backwards compatibility, but the `SuiClient`
//! parameter is now ignored in favor of direct GraphQL queries.
//!
//! For new code, prefer using `sui_transport::graphql::GraphQLClient` directly
//! (current state queries) or `sui_state_fetcher::HistoricalStateProvider` for
//! historical replay. `DataFetcher` is deprecated and kept for compatibility.

use crate::args::RetryConfig;
use crate::bytecode::build_bytecode_interface_value_from_compiled_modules;
use crate::graphql::GraphQLClient;
use crate::utils::with_retries;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;
use sui_sdk::types::base_types::ObjectID;

/// Resolve a PackageInfo object to its actual package address.
///
/// PackageInfo objects contain a `package_address` field that points to the
/// actual package. This function fetches the object and extracts that field.
///
/// Note: The `client` parameter is kept for backwards compatibility but is
/// no longer used. GraphQL is used internally instead.
pub async fn resolve_package_address_from_package_info(
    _client: Arc<sui_sdk::SuiClient>,
    package_info_id: ObjectID,
    retry: RetryConfig,
) -> Result<ObjectID> {
    let address = package_info_id.to_string();

    let obj = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let graphql = GraphQLClient::mainnet();
            let address = address.clone();
            async move {
                graphql
                    .fetch_object(&address)
                    .with_context(|| format!("fetch object {}", address))
            }
        },
    )
    .await?;

    // Extract package_address from the object's JSON content
    let content_json = obj.content_json.ok_or_else(|| {
        anyhow!(
            "object {} has no JSON content (may not be a PackageInfo object)",
            package_info_id
        )
    })?;

    let package_address = content_json
        .get("package_address")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!(
                "object {} missing package_address field (may not be a PackageInfo object)",
                package_info_id
            )
        })?;

    ObjectID::from_str(package_address)
        .map_err(|e| anyhow!("invalid package_address {}: {}", package_address, e))
}

/// Fetch the module names from a package.
///
/// Returns a sorted list of module names in the package.
///
/// Note: The `client` parameter is kept for backwards compatibility but is
/// no longer used. GraphQL is used internally instead.
pub async fn fetch_bcs_module_names(
    _client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<Vec<String>> {
    let address = package_id.to_string();

    let pkg = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let graphql = GraphQLClient::mainnet();
            let address = address.clone();
            async move {
                graphql
                    .fetch_package(&address)
                    .with_context(|| format!("fetch package {}", address))
            }
        },
    )
    .await?;

    let mut names: Vec<String> = pkg.modules.iter().map(|m| m.name.clone()).collect();
    names.sort();
    Ok(names)
}

/// Fetch the module bytecode from a package.
///
/// Returns a sorted list of (module_name, bytecode_bytes) pairs.
///
/// Note: The `client` parameter is kept for backwards compatibility but is
/// no longer used. GraphQL is used internally instead.
pub async fn fetch_bcs_module_map_bytes(
    _client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<Vec<(String, Vec<u8>)>> {
    use base64::Engine;
    let address = package_id.to_string();

    let pkg = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let graphql = GraphQLClient::mainnet();
            let address = address.clone();
            async move {
                graphql
                    .fetch_package(&address)
                    .with_context(|| format!("fetch package {}", address))
            }
        },
    )
    .await?;

    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(pkg.modules.len());
    for module in pkg.modules {
        let bytecode_b64 = module.bytecode_base64.ok_or_else(|| {
            anyhow!(
                "module {} in package {} has no bytecode",
                module.name,
                package_id
            )
        })?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&bytecode_b64)
            .with_context(|| {
                format!(
                    "base64 decode module {} bytecode for {}",
                    module.name, package_id
                )
            })?;

        out.push((module.name, bytes));
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Build an interface JSON value for a package.
///
/// This fetches the package bytecode via GraphQL and builds the interface
/// from the compiled modules. The resulting interface is bytecode-derived
/// and contains structs, functions, and their signatures.
///
/// Note: The `client` parameter is kept for backwards compatibility but is
/// no longer used. GraphQL is used internally instead.
///
/// ## Migration Note
///
/// Prior to v0.6.0, this function used `get_normalized_move_modules_by_package`
/// from the JSON-RPC API, which returned a different format. The new implementation
/// uses bytecode-derived interfaces which have the same information but in a
/// different schema. The key differences:
///
/// - Type parameters use `constraints` instead of `abilities` for generic constraints
/// - Field types use the bytecode representation format
///
/// For most use cases (checking module structure, finding functions/structs),
/// the interface provides equivalent information.
pub async fn build_interface_value_for_package(
    _client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<(Vec<String>, Value)> {
    use base64::Engine;
    let address = package_id.to_string();

    let pkg = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let graphql = GraphQLClient::mainnet();
            let address = address.clone();
            async move {
                graphql
                    .fetch_package(&address)
                    .with_context(|| format!("fetch package {}", address))
            }
        },
    )
    .await?;

    // Deserialize all modules to CompiledModule
    let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
    for module in &pkg.modules {
        let bytecode_b64 = module.bytecode_base64.as_ref().ok_or_else(|| {
            anyhow!(
                "module {} in package {} has no bytecode",
                module.name,
                package_id
            )
        })?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bytecode_b64)
            .with_context(|| {
                format!(
                    "base64 decode module {} bytecode for {}",
                    module.name, package_id
                )
            })?;

        let compiled =
            move_binary_format::file_format::CompiledModule::deserialize_with_defaults(&bytes)
                .with_context(|| {
                    format!("deserialize module {} for {}", module.name, package_id)
                })?;

        compiled_modules.push(compiled);
    }

    // Build interface from compiled modules
    build_bytecode_interface_value_from_compiled_modules(&address, &compiled_modules)
}

// ============================================================================
// New GraphQL-native functions (for new code)
// ============================================================================

/// Fetch package module names using GraphQL directly.
///
/// This is the preferred way to fetch module names for new code.
pub fn fetch_module_names_graphql(package_id: &str) -> Result<Vec<String>> {
    let graphql = GraphQLClient::mainnet();
    let pkg = graphql.fetch_package(package_id)?;

    let mut names: Vec<String> = pkg.modules.iter().map(|m| m.name.clone()).collect();
    names.sort();
    Ok(names)
}

/// Fetch package module bytecode using GraphQL directly.
///
/// This is the preferred way to fetch module bytecode for new code.
/// Returns a sorted list of (module_name, bytecode_bytes) pairs.
pub fn fetch_module_bytecode_graphql(package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
    use base64::Engine;
    let graphql = GraphQLClient::mainnet();
    let pkg = graphql.fetch_package(package_id)?;

    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(pkg.modules.len());
    for module in pkg.modules {
        let bytecode_b64 = module
            .bytecode_base64
            .ok_or_else(|| anyhow!("module {} has no bytecode", module.name))?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&bytecode_b64)
            .with_context(|| format!("base64 decode module {} bytecode", module.name))?;

        out.push((module.name, bytes));
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Build a bytecode-derived interface JSON for a package using GraphQL.
///
/// This is the preferred way to build interfaces for new code.
pub fn build_interface_graphql(package_id: &str) -> Result<(Vec<String>, Value)> {
    use base64::Engine;
    let graphql = GraphQLClient::mainnet();
    let pkg = graphql.fetch_package(package_id)?;

    // Deserialize all modules
    let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
    for module in &pkg.modules {
        let bytecode_b64 = module
            .bytecode_base64
            .as_ref()
            .ok_or_else(|| anyhow!("module {} has no bytecode", module.name))?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bytecode_b64)
            .with_context(|| format!("base64 decode module {} bytecode", module.name))?;

        let compiled =
            move_binary_format::file_format::CompiledModule::deserialize_with_defaults(&bytes)
                .with_context(|| format!("deserialize module {}", module.name))?;

        compiled_modules.push(compiled);
    }

    build_bytecode_interface_value_from_compiled_modules(package_id, &compiled_modules)
}
