//! VM Integration utilities.
//!
//! Helper functions for loading [`ReplayState`] into a VM harness.
//! These are provided as standalone functions to avoid circular dependencies
//! with `sui-sandbox-core`.
//!
//! # Example
//!
//! ```ignore
//! use sui_state_fetcher::{HistoricalStateProvider, ReplayState};
//! use sui_state_fetcher::vm_integration::load_packages_into_resolver;
//! use sui_sandbox_core::resolver::LocalModuleResolver;
//!
//! let provider = HistoricalStateProvider::mainnet().await?;
//! let state = provider.fetch_replay_state(digest).await?;
//!
//! // Load packages into resolver
//! let mut resolver = LocalModuleResolver::new();
//! load_packages_into_resolver(&mut resolver, &state)?;
//!
//! // Create VM and execute
//! let harness = VMHarness::new(&resolver, config)?;
//! ```

use std::collections::HashMap;

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;

use crate::types::{PackageData, ReplayState, VersionedObject};

/// Load packages from ReplayState into a LocalModuleResolver.
///
/// This handles package upgrade linkage by setting up address aliases
/// when a package has been upgraded (original_id differs from address).
pub fn prepare_packages_for_resolver(
    state: &ReplayState,
) -> Vec<(
    AccountAddress,
    Vec<(String, Vec<u8>)>,
    Option<AccountAddress>,
)> {
    let mut result = Vec::new();

    for (addr, pkg) in &state.packages {
        // If this is an upgraded package, we need to set up aliasing
        // The bytecode address (in modules) might differ from the storage address
        let target_addr = if pkg.original_id.is_some() && pkg.original_id != Some(*addr) {
            // This is an upgraded package - modules are at original_id but stored at addr
            Some(*addr)
        } else {
            None
        };

        result.push((*addr, pkg.modules.clone(), target_addr));
    }

    result
}

/// Extract objects in a format suitable for preloading into VM storage.
///
/// Returns a map of object_id -> (type_tag, bcs_bytes) for all objects.
pub fn prepare_objects_for_vm(
    state: &ReplayState,
) -> HashMap<AccountAddress, (Option<String>, Vec<u8>)> {
    state
        .objects
        .iter()
        .map(|(id, obj)| (*id, (obj.type_tag.clone(), obj.bcs_bytes.clone())))
        .collect()
}

/// Extract dynamic field children in a format suitable for preloading.
///
/// This is a helper for when you have parent->child relationships and need
/// to format them for `VMHarness::preload_dynamic_fields`.
///
/// Note: This requires knowing the parent-child relationships, which are
/// embedded in the dynamic field wrapper objects. The caller should parse
/// these from the object BCS data.
pub fn extract_dynamic_field_info(obj: &VersionedObject) -> Option<DynamicFieldChild> {
    // Dynamic field wrapper objects have a specific structure:
    // struct Field<Name, Value> { id: UID, name: Name, value: Value }
    // The parent is encoded in the UID derivation.
    //
    // For now, we return None and let the VM's on-demand fetcher handle this.
    // A more complete implementation would parse the BCS to extract parent info.
    let _ = obj; // Suppress unused warning
    None
}

/// Information about a dynamic field child object.
#[derive(Debug, Clone)]
pub struct DynamicFieldChild {
    /// Parent object ID.
    pub parent_id: AccountAddress,
    /// Child object ID.
    pub child_id: AccountAddress,
    /// Type tag of the stored value.
    pub type_tag: TypeTag,
    /// BCS bytes of the stored value.
    pub value_bytes: Vec<u8>,
}

/// Create address aliases from package linkage tables.
///
/// When packages are upgraded, their type references use the original_id
/// but bytecode is stored at the new address. This function extracts
/// the mapping needed to rewrite addresses during execution.
pub fn create_address_aliases(
    packages: &HashMap<AccountAddress, PackageData>,
) -> HashMap<AccountAddress, AccountAddress> {
    let mut aliases = HashMap::new();

    for pkg in packages.values() {
        // Add linkage table entries (dependency rewrites)
        for (original, upgraded) in &pkg.linkage {
            if original != upgraded {
                aliases.insert(*original, *upgraded);
            }
        }

        // Add self-upgrade alias if applicable
        if let Some(original_id) = pkg.original_id {
            if original_id != pkg.address {
                aliases.insert(original_id, pkg.address);
            }
        }
    }

    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_address_aliases_empty() {
        let packages = HashMap::new();
        let aliases = create_address_aliases(&packages);
        assert!(aliases.is_empty());
    }

    #[test]
    fn test_prepare_packages_for_resolver() {
        use crate::types::PackageData;

        let addr = AccountAddress::new([1u8; 32]);
        let mut packages = HashMap::new();
        packages.insert(
            addr,
            PackageData {
                address: addr,
                version: 1,
                modules: vec![("test".to_string(), vec![1, 2, 3])],
                linkage: HashMap::new(),
                original_id: None,
            },
        );

        let state = ReplayState {
            transaction: sui_sandbox_types::FetchedTransaction {
                digest: sui_sandbox_types::TransactionDigest::new("test"),
                sender: AccountAddress::ZERO,
                gas_budget: 0,
                gas_price: 0,
                commands: vec![],
                inputs: vec![],
                effects: None,
                timestamp_ms: None,
                checkpoint: None,
            },
            objects: HashMap::new(),
            packages,
            protocol_version: 0,
            epoch: 0,
            checkpoint: None,
        };

        let result = prepare_packages_for_resolver(&state);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, addr);
        assert!(result[0].2.is_none()); // No upgrade, no target alias
    }
}
