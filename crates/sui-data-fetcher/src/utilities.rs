//! Data helper utilities for gRPC responses.
//!
//! This module provides utilities for aggregating and working with data from
//! gRPC responses. These are **data helpers** that collect and structure data,
//! distinct from infrastructure workarounds (see `sui_sandbox_core::utilities`).
//!
//! ## What Belongs Here
//!
//! - Aggregating data from gRPC transaction responses
//! - gRPC client initialization helpers
//! - Data extraction from gRPC types
//!
//! ## What Does NOT Belong Here
//!
//! - Object patching (use `sui_sandbox_core::utilities::GenericObjectPatcher`)
//! - Address normalization (use `sui_sandbox_core::utilities::normalize_address`)
//! - VM/resolver setup (use example-specific code)

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::grpc::{GrpcClient, GrpcInput, GrpcTransaction};

/// Create a Tokio runtime and connect to Surflux gRPC.
///
/// Reads `SURFLUX_API_KEY` from environment (usually via .env file).
/// Returns both the runtime (for blocking operations) and the connected client.
///
/// # Example
///
/// ```ignore
/// use sui_data_fetcher::utilities::create_grpc_client;
///
/// let (rt, grpc) = create_grpc_client()?;
/// let tx = rt.block_on(async { grpc.get_transaction(digest).await })?;
/// ```
pub fn create_grpc_client() -> Result<(tokio::runtime::Runtime, GrpcClient)> {
    let rt = tokio::runtime::Runtime::new()?;

    let api_key = std::env::var("SURFLUX_API_KEY")
        .map_err(|_| anyhow!("SURFLUX_API_KEY not set in environment. Add it to .env file."))?;

    let grpc = rt.block_on(async {
        GrpcClient::with_api_key("https://grpc.surflux.dev:443", Some(api_key)).await
    })?;

    Ok((rt, grpc))
}

/// Collect historical object versions from a gRPC transaction.
///
/// Aggregates version information from multiple sources in the gRPC response:
/// - `unchanged_loaded_runtime_objects`: Objects read but not modified
/// - `changed_objects`: Objects modified (provides INPUT versions before tx)
/// - `unchanged_consensus_objects`: Actual consensus versions for shared objects
/// - Transaction inputs: Object, SharedObject, and Receiving inputs
///
/// Returns a map from object ID (hex string) to version number.
///
/// # Example
///
/// ```ignore
/// use sui_data_fetcher::utilities::collect_historical_versions;
///
/// let versions = collect_historical_versions(&grpc_tx);
/// for (obj_id, version) in &versions {
///     println!("Object {} at version {}", obj_id, version);
/// }
/// ```
pub fn collect_historical_versions(grpc_tx: &GrpcTransaction) -> HashMap<String, u64> {
    let mut versions: HashMap<String, u64> = HashMap::new();

    // From unchanged_loaded_runtime_objects
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        versions.insert(id.clone(), *ver);
    }

    // From changed_objects (these give us INPUT versions)
    for (id, ver) in &grpc_tx.changed_objects {
        versions.insert(id.clone(), *ver);
    }

    // From unchanged_consensus_objects (actual consensus versions for shared objects)
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        versions.insert(id.clone(), *ver);
    }

    // From transaction inputs
    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                versions.insert(object_id.clone(), *version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                versions.insert(object_id.clone(), *initial_version);
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                versions.insert(object_id.clone(), *version);
            }
            _ => {}
        }
    }

    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_historical_versions_empty() {
        let grpc_tx = GrpcTransaction {
            digest: "test".to_string(),
            sender: "0x1".to_string(),
            timestamp_ms: None,
            checkpoint: None,
            gas_budget: None,
            gas_price: None,
            inputs: vec![],
            commands: vec![],
            status: None,
            execution_error: None,
            unchanged_loaded_runtime_objects: vec![],
            unchanged_consensus_objects: vec![],
            changed_objects: vec![],
            created_objects: vec![],
        };

        let versions = collect_historical_versions(&grpc_tx);
        assert!(versions.is_empty());
    }

    #[test]
    fn test_collect_historical_versions_aggregates() {
        let grpc_tx = GrpcTransaction {
            digest: "test".to_string(),
            sender: "0x1".to_string(),
            timestamp_ms: None,
            checkpoint: None,
            gas_budget: None,
            gas_price: None,
            inputs: vec![GrpcInput::Object {
                object_id: "0xaaa".to_string(),
                version: 10,
                digest: "d1".to_string(),
            }],
            commands: vec![],
            status: None,
            execution_error: None,
            unchanged_loaded_runtime_objects: vec![("0xbbb".to_string(), 20)],
            unchanged_consensus_objects: vec![("0xccc".to_string(), 30)],
            changed_objects: vec![("0xddd".to_string(), 40)],
            created_objects: vec![],
        };

        let versions = collect_historical_versions(&grpc_tx);
        assert_eq!(versions.get("0xaaa"), Some(&10));
        assert_eq!(versions.get("0xbbb"), Some(&20));
        assert_eq!(versions.get("0xccc"), Some(&30));
        assert_eq!(versions.get("0xddd"), Some(&40));
    }
}
