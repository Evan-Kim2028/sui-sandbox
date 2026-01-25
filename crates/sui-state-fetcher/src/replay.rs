//! Replay utilities for executing transactions locally.
//!
//! This module provides helpers to convert [`ReplayState`] into formats
//! compatible with the VM and sandbox utilities.
//!
//! # Example: Basic Replay
//!
//! ```ignore
//! use sui_state_fetcher::{HistoricalStateProvider, replay};
//!
//! let provider = HistoricalStateProvider::mainnet().await?;
//! let state = provider.fetch_replay_state("8JTTa...").await?;
//!
//! // Convert to replay data format
//! let replay_data = replay::to_replay_data(&state);
//! let historical_versions = replay::get_historical_versions(&state);
//! ```
//!
//! # Example: With HistoricalStateReconstructor
//!
//! For best replay accuracy, use `HistoricalStateReconstructor` to patch
//! version fields in objects. This requires `sui-sandbox-core`.
//!
//! ```ignore
//! use sui_state_fetcher::{HistoricalStateProvider, replay};
//! use sui_sandbox_core::utilities::HistoricalStateReconstructor;
//! use sui_sandbox_core::resolver::LocalModuleResolver;
//!
//! // 1. Fetch state
//! let provider = HistoricalStateProvider::mainnet().await?;
//! let state = provider.fetch_replay_state("8JTTa...").await?;
//!
//! // 2. Build module resolver
//! let replay_data = replay::to_replay_data(&state);
//! let mut resolver = LocalModuleResolver::new();
//! for (pkg_id, modules_b64) in &replay_data.packages {
//!     // ... decode and load modules
//! }
//! resolver.load_sui_framework()?;
//!
//! // 3. Patch objects with HistoricalStateReconstructor
//! let raw_objects = replay::to_raw_objects(&state);
//! let mut reconstructor = HistoricalStateReconstructor::new();
//! reconstructor.set_timestamp(state.transaction.timestamp_ms.unwrap_or(0));
//! reconstructor.configure_from_modules(resolver.compiled_modules());
//! let reconstructed = reconstructor.reconstruct(&raw_objects, &replay_data.object_types);
//!
//! // 4. Use reconstructed.objects for VM execution
//! ```

use std::collections::HashMap;

use base64::Engine;
use move_core_types::account_address::AccountAddress;

use crate::types::{ObjectID, ReplayState};

/// Convert ReplayState to the data format expected by CachedTransaction.
///
/// Returns:
/// - `packages`: Map of package_id -> Vec<(module_name, bytecode_base64)>
/// - `objects`: Map of object_id -> bcs_base64
/// - `object_types`: Map of object_id -> type_string
/// - `object_versions`: Map of object_id -> version
/// - `linkage_upgrades`: Map of original_id -> upgraded_id
pub struct ReplayData {
    /// Package modules encoded as base64
    pub packages: HashMap<String, Vec<(String, String)>>,
    /// Object BCS encoded as base64
    pub objects: HashMap<String, String>,
    /// Object type strings
    pub object_types: HashMap<String, String>,
    /// Object versions
    pub object_versions: HashMap<String, u64>,
    /// Linkage upgrades (original_id -> upgraded_id)
    pub linkage_upgrades: HashMap<String, String>,
}

/// Convert ReplayState to ReplayData format compatible with existing replay utilities.
pub fn to_replay_data(state: &ReplayState) -> ReplayData {
    let engine = base64::engine::general_purpose::STANDARD;

    // Convert packages
    let mut packages = HashMap::new();
    let mut linkage_upgrades = HashMap::new();

    for (addr, pkg) in &state.packages {
        let pkg_id = format!("0x{}", hex::encode(addr.as_ref()));

        // Convert modules to base64
        let modules_b64: Vec<(String, String)> = pkg
            .modules
            .iter()
            .map(|(name, bytes)| (name.clone(), engine.encode(bytes)))
            .collect();

        packages.insert(pkg_id.clone(), modules_b64);

        // Collect linkage upgrades
        for (orig, upgraded) in &pkg.linkage {
            let orig_str = format!("0x{}", hex::encode(orig.as_ref()));
            let upgraded_str = format!("0x{}", hex::encode(upgraded.as_ref()));
            if orig_str != upgraded_str {
                linkage_upgrades.insert(orig_str, upgraded_str);
            }
        }
    }

    // Convert objects
    let mut objects = HashMap::new();
    let mut object_types = HashMap::new();
    let mut object_versions = HashMap::new();

    for (id, obj) in &state.objects {
        let id_str = format!("0x{}", hex::encode(id.as_ref()));

        objects.insert(id_str.clone(), engine.encode(&obj.bcs_bytes));
        object_versions.insert(id_str.clone(), obj.version);

        if let Some(ref type_tag) = obj.type_tag {
            object_types.insert(id_str, type_tag.clone());
        }
    }

    ReplayData {
        packages,
        objects,
        object_types,
        object_versions,
        linkage_upgrades,
    }
}

/// Get historical versions map from ReplayState.
///
/// Returns a map of object_id (hex string) -> version that can be used
/// with existing replay utilities.
pub fn get_historical_versions(state: &ReplayState) -> HashMap<String, u64> {
    state
        .objects
        .iter()
        .map(|(id, obj)| {
            let id_str = format!("0x{}", hex::encode(id.as_ref()));
            (id_str, obj.version)
        })
        .collect()
}

/// Build address aliases from package linkage tables.
///
/// Returns a map of storage_address -> bytecode_address for upgraded packages.
/// This is needed because upgraded packages have bytecode that references
/// their original address, but are stored at a different address.
pub fn build_address_aliases(state: &ReplayState) -> HashMap<AccountAddress, AccountAddress> {
    let mut aliases = HashMap::new();

    for (storage_addr, pkg) in &state.packages {
        // Check if this package has an original_id different from its storage address
        if let Some(original_id) = &pkg.original_id {
            if original_id != storage_addr {
                // The bytecode uses original_id, but the package is stored at storage_addr
                aliases.insert(*storage_addr, *original_id);
            }
        }

        // Also check linkage table
        for (orig, upgraded) in &pkg.linkage {
            if orig != upgraded {
                // When we load the upgraded package, its bytecode references the original
                aliases.insert(*upgraded, *orig);
            }
        }
    }

    aliases
}

/// Convert ReplayState objects to raw BCS bytes for patching.
///
/// This is useful for integration with `HistoricalStateReconstructor`
/// which expects `HashMap<String, Vec<u8>>` for patching.
pub fn to_raw_objects(state: &ReplayState) -> HashMap<String, Vec<u8>> {
    state
        .objects
        .iter()
        .map(|(id, obj)| {
            let id_str = format!("0x{}", hex::encode(id.as_ref()));
            (id_str, obj.bcs_bytes.clone())
        })
        .collect()
}

/// Extract object ID as hex string.
pub fn object_id_to_string(id: &ObjectID) -> String {
    format!("0x{}", hex::encode(id.as_ref()))
}

/// Parse object ID from hex string.
pub fn parse_object_id(id_str: &str) -> Option<ObjectID> {
    let hex_str = id_str.strip_prefix("0x").unwrap_or(id_str);
    let padded = format!("{:0>64}", hex_str);
    let bytes = hex::decode(&padded).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(AccountAddress::new(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_object_id() {
        let id = parse_object_id("0x2").unwrap();
        assert_eq!(
            &format!("0x{}", hex::encode(id.as_ref())),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );

        let full = "0x0000000000000000000000000000000000000000000000000000000000000002";
        let id2 = parse_object_id(full).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn test_object_id_to_string() {
        let id = parse_object_id("0x2").unwrap();
        let s = object_id_to_string(&id);
        assert_eq!(
            s,
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }
}
