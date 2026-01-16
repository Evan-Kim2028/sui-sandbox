//! Mainnet state import handlers.
//!
//! Handles import_package_from_mainnet, import_object_from_mainnet,
//! import_objects_from_mainnet, and import_object_at_version operations.
//!
//! These handlers allow the sandbox to fetch real mainnet state for
//! accurate simulation. They use the DataFetcher which provides
//! GraphQL-first fetching with JSON-RPC fallback for reliability.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use crate::data_fetcher::DataFetcher;

/// Create a DataFetcher for the specified network.
fn get_fetcher(network: Option<&str>) -> DataFetcher {
    match network.unwrap_or("mainnet") {
        "testnet" => DataFetcher::testnet(),
        _ => DataFetcher::mainnet(),
    }
}

/// Import a package from mainnet into the sandbox.
///
/// Fetches the package bytecode via GraphQL/JSON-RPC and deploys it
/// into the local simulation environment at the same address.
pub fn execute_import_package_from_mainnet(
    env: &mut SimulationEnvironment,
    package_id: &str,
    network: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Importing package {} from {}",
            package_id,
            network.unwrap_or("mainnet")
        );
    }

    let fetcher = get_fetcher(network);

    // Fetch package from mainnet
    let pkg = match fetcher.fetch_package(package_id) {
        Ok(pkg) => pkg,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Failed to fetch package {}: {}", package_id, e),
                "MainnetFetchError",
            );
        }
    };

    let source = pkg.source;
    let module_count = pkg.modules.len();

    if verbose {
        eprintln!("  Fetched {} modules via {:?}", module_count, source);
    }

    // Deploy modules into sandbox at the original mainnet address
    let modules: Vec<(String, Vec<u8>)> = pkg
        .modules
        .into_iter()
        .map(|m| (m.name, m.bytecode))
        .collect();

    match env.deploy_package_at_address(package_id, modules) {
        Ok(addr) => {
            if verbose {
                eprintln!("  Deployed package at {}", addr.to_hex_literal());
            }
            SandboxResponse::success_with_data(serde_json::json!({
                "package_id": addr.to_hex_literal(),
                "module_count": module_count,
                "source": format!("{:?}", source),
                "network": network.unwrap_or("mainnet"),
            }))
        }
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to deploy package: {}", e),
            "PackageDeployError",
        ),
    }
}

/// Import an object from mainnet into the sandbox.
///
/// Fetches the object's current state (type, fields, ownership) and
/// loads it into the sandbox at the same address.
pub fn execute_import_object_from_mainnet(
    env: &mut SimulationEnvironment,
    object_id: &str,
    network: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Importing object {} from {}",
            object_id,
            network.unwrap_or("mainnet")
        );
    }

    let fetcher = get_fetcher(network);

    // Fetch object from mainnet
    let obj = match fetcher.fetch_object(object_id) {
        Ok(obj) => obj,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Failed to fetch object {}: {}", object_id, e),
                "MainnetFetchError",
            );
        }
    };

    if verbose {
        eprintln!(
            "  Fetched object via {:?}: type={:?}, version={}",
            obj.source, obj.type_string, obj.version
        );
    }

    // Check if we have BCS bytes
    let bcs_bytes = match obj.bcs_bytes {
        Some(bytes) => bytes,
        None => {
            return SandboxResponse::error_with_category(
                format!("Object {} has no BCS bytes available", object_id),
                "MissingBcsError",
            );
        }
    };

    // Load into sandbox
    match env.load_cached_object_with_type(
        object_id,
        bcs_bytes,
        obj.type_string.as_deref(),
        obj.is_shared,
    ) {
        Ok(id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": id.to_hex_literal(),
            "type": obj.type_string,
            "version": obj.version,
            "is_shared": obj.is_shared,
            "is_immutable": obj.is_immutable,
            "source": format!("{:?}", obj.source),
            "network": network.unwrap_or("mainnet"),
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to load object into sandbox: {}", e),
            "ObjectLoadError",
        ),
    }
}

/// Import multiple objects from mainnet in a batch.
pub fn execute_import_objects_from_mainnet(
    env: &mut SimulationEnvironment,
    object_ids: &[String],
    network: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Importing {} objects from {}",
            object_ids.len(),
            network.unwrap_or("mainnet")
        );
    }

    let fetcher = get_fetcher(network);
    let mut imported = Vec::new();
    let mut failed = Vec::new();

    for object_id in object_ids {
        // Fetch object
        let obj = match fetcher.fetch_object(object_id) {
            Ok(obj) => obj,
            Err(e) => {
                failed.push(serde_json::json!({
                    "object_id": object_id,
                    "error": e.to_string(),
                    "stage": "fetch",
                }));
                continue;
            }
        };

        // Check BCS bytes
        let bcs_bytes = match obj.bcs_bytes {
            Some(bytes) => bytes,
            None => {
                failed.push(serde_json::json!({
                    "object_id": object_id,
                    "error": "No BCS bytes available",
                    "stage": "fetch",
                }));
                continue;
            }
        };

        // Load into sandbox
        match env.load_cached_object_with_type(
            object_id,
            bcs_bytes,
            obj.type_string.as_deref(),
            obj.is_shared,
        ) {
            Ok(id) => {
                if verbose {
                    eprintln!("  Imported {}", id.to_hex_literal());
                }
                imported.push(serde_json::json!({
                    "object_id": id.to_hex_literal(),
                    "type": obj.type_string,
                    "is_shared": obj.is_shared,
                }));
            }
            Err(e) => {
                failed.push(serde_json::json!({
                    "object_id": object_id,
                    "error": e.to_string(),
                    "stage": "load",
                }));
            }
        }
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "imported": imported,
        "imported_count": imported.len(),
        "failed": failed,
        "failed_count": failed.len(),
        "network": network.unwrap_or("mainnet"),
    }))
}

/// Import an object at a specific historical version.
pub fn execute_import_object_at_version(
    env: &mut SimulationEnvironment,
    object_id: &str,
    version: u64,
    network: Option<&str>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!(
            "Importing object {}@{} from {}",
            object_id,
            version,
            network.unwrap_or("mainnet")
        );
    }

    let fetcher = get_fetcher(network);

    // Fetch object at specific version
    let obj = match fetcher.fetch_object_at_version(object_id, version) {
        Ok(obj) => obj,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Failed to fetch object {}@{}: {}", object_id, version, e),
                "MainnetFetchError",
            );
        }
    };

    if verbose {
        eprintln!(
            "  Fetched object via {:?}: type={:?}",
            obj.source, obj.type_string
        );
    }

    // Check BCS bytes
    let bcs_bytes = match obj.bcs_bytes {
        Some(bytes) => bytes,
        None => {
            return SandboxResponse::error_with_category(
                format!(
                    "Object {}@{} has no BCS bytes available",
                    object_id, version
                ),
                "MissingBcsError",
            );
        }
    };

    // Load into sandbox
    match env.load_cached_object_with_type(
        object_id,
        bcs_bytes,
        obj.type_string.as_deref(),
        obj.is_shared,
    ) {
        Ok(id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": id.to_hex_literal(),
            "type": obj.type_string,
            "version": version,
            "is_shared": obj.is_shared,
            "is_immutable": obj.is_immutable,
            "source": format!("{:?}", obj.source),
            "network": network.unwrap_or("mainnet"),
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to load object into sandbox: {}", e),
            "ObjectLoadError",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_fetcher_mainnet() {
        let fetcher = get_fetcher(None);
        // Just verify it creates without panic
        drop(fetcher);
    }

    #[test]
    fn test_get_fetcher_testnet() {
        let fetcher = get_fetcher(Some("testnet"));
        drop(fetcher);
    }
}
