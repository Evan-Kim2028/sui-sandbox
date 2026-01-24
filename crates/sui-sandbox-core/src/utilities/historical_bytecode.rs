//! Historical Bytecode Resolution
//!
//! This module provides utilities for resolving bytecode at correct historical versions
//! using transaction effects data. When replaying historical transactions, the bytecode
//! version must match the objects being loaded.
//!
//! ## Key Concepts
//!
//! - **unchanged_consensus_objects**: Contains immutable packages with exact versions used
//! - **package_linkage**: Maps original_id → upgraded_id for package upgrades
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::HistoricalBytecodeResolver;
//!
//! let mut resolver = HistoricalBytecodeResolver::new(grpc_client, runtime);
//! let packages = resolver.resolve_all(&grpc_tx).await?;
//! ```

use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Result of resolving historical bytecode.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    /// Original package ID (what PTB references)
    pub original_id: String,
    /// Storage address where bytecode was fetched from (may differ for upgrades)
    pub storage_id: String,
    /// Version of the package at time of transaction
    pub version: u64,
    /// Module bytecode: (name, bytes)
    pub modules: Vec<(String, Vec<u8>)>,
    /// Linkage table entries from this package
    pub linkage: Vec<LinkageEntry>,
}

/// A linkage table entry mapping original → upgraded package.
#[derive(Debug, Clone)]
pub struct LinkageEntry {
    pub original_id: String,
    pub upgraded_id: String,
    pub upgraded_version: u64,
}

/// Configuration for historical bytecode resolution.
#[derive(Debug, Clone)]
pub struct ResolutionConfig {
    /// Maximum depth for transitive dependency resolution
    pub max_depth: usize,
    /// Skip framework packages (0x1, 0x2, 0x3)
    pub skip_framework: bool,
    /// Whether to follow linkage upgrades
    pub follow_upgrades: bool,
}

impl Default for ResolutionConfig {
    fn default() -> Self {
        Self {
            max_depth: 10,
            skip_framework: true,
            follow_upgrades: true,
        }
    }
}

/// Extract package versions from transaction effects.
///
/// This collects version information from:
/// - `unchanged_consensus_objects`: Immutable packages with exact versions
/// - `unchanged_loaded_runtime_objects`: Additional loaded objects
/// - Transaction inputs
///
/// Returns a map of object_id → version.
pub fn extract_package_versions_from_effects(
    unchanged_consensus_objects: &[(String, u64)],
    unchanged_loaded_runtime_objects: &[(String, u64)],
    changed_objects: &[(String, u64)],
) -> HashMap<String, u64> {
    let mut versions = HashMap::new();

    // unchanged_consensus_objects contains immutable packages
    for (id, ver) in unchanged_consensus_objects {
        versions.insert(normalize_id(id), *ver);
    }

    // unchanged_loaded_runtime_objects may include package dependencies
    for (id, ver) in unchanged_loaded_runtime_objects {
        versions.entry(normalize_id(id)).or_insert(*ver);
    }

    // changed_objects has INPUT versions (before tx)
    for (id, ver) in changed_objects {
        versions.entry(normalize_id(id)).or_insert(*ver);
    }

    versions
}

/// Extract package IDs referenced in MoveCall commands.
///
/// Returns a set of package IDs that need to be fetched.
pub fn extract_packages_from_commands(commands: &[crate::ptb::Command]) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();

    for cmd in commands {
        if let crate::ptb::Command::MoveCall { package, .. } = cmd {
            let pkg_id = format!("0x{}", hex::encode(package.as_ref()));
            packages.insert(normalize_id(&pkg_id));
        }
    }

    packages
}

/// Normalize a package/object ID to consistent hex format.
///
/// Converts "0xABC" or "ABC" to "0x0000...0abc" (64 hex chars after 0x).
pub fn normalize_id(id: &str) -> String {
    let id = id.trim();
    let hex_part = id.strip_prefix("0x").unwrap_or(id).to_lowercase();
    // Pad to 64 hex characters
    format!("0x{:0>64}", hex_part)
}

/// Check if an ID is a framework package (0x1, 0x2, 0x3).
pub fn is_framework_id(id: &str) -> bool {
    let normalized = normalize_id(id);
    matches!(
        normalized.as_str(),
        "0x0000000000000000000000000000000000000000000000000000000000000001"
            | "0x0000000000000000000000000000000000000000000000000000000000000002"
            | "0x0000000000000000000000000000000000000000000000000000000000000003"
    )
}

/// Build a map of original → upgraded package IDs from linkage tables.
///
/// This is useful for determining which storage address to fetch bytecode from.
pub fn build_upgrade_map(packages: &[ResolvedPackage]) -> BTreeMap<String, String> {
    let mut upgrades = BTreeMap::new();

    for pkg in packages {
        for linkage in &pkg.linkage {
            let orig = normalize_id(&linkage.original_id);
            let upgraded = normalize_id(&linkage.upgraded_id);
            if orig != upgraded {
                upgrades.insert(orig, upgraded);
            }
        }
    }

    upgrades
}

/// Extract dependencies from compiled module bytecode.
///
/// Parses the module handle table to find all package addresses referenced.
pub fn extract_dependencies_from_module(bytecode: &[u8]) -> Result<Vec<String>> {
    use move_binary_format::CompiledModule;

    let module = CompiledModule::deserialize_with_defaults(bytecode)
        .map_err(|e| anyhow!("Failed to deserialize module: {}", e))?;

    let mut deps = Vec::new();
    for handle in module.module_handles() {
        let addr = module.address_identifier_at(handle.address);
        let addr_str = addr.to_hex_literal();
        if !is_framework_id(&addr_str) {
            deps.push(normalize_id(&addr_str));
        }
    }

    Ok(deps)
}

/// Collect all dependencies from a set of modules.
pub fn collect_all_dependencies(modules: &[(String, Vec<u8>)]) -> BTreeSet<String> {
    let mut all_deps = BTreeSet::new();

    for (_name, bytecode) in modules {
        if let Ok(deps) = extract_dependencies_from_module(bytecode) {
            for dep in deps {
                all_deps.insert(dep);
            }
        }
    }

    all_deps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_id_full() {
        let id = "0x0000000000000000000000000000000000000000000000000000000000000002";
        assert_eq!(normalize_id(id), id);
    }

    #[test]
    fn test_normalize_id_short() {
        let id = "0x2";
        assert_eq!(
            normalize_id(id),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_normalize_id_no_prefix() {
        let id = "abc";
        assert_eq!(
            normalize_id(id),
            "0x0000000000000000000000000000000000000000000000000000000000000abc"
        );
    }

    #[test]
    fn test_is_framework_id() {
        assert!(is_framework_id("0x1"));
        assert!(is_framework_id("0x2"));
        assert!(is_framework_id("0x3"));
        assert!(!is_framework_id("0x4"));
        assert!(!is_framework_id("0x1eabed72"));
    }

    #[test]
    fn test_extract_package_versions() {
        let unchanged_consensus = vec![("0x1".to_string(), 1), ("0xabc".to_string(), 100)];
        let unchanged_runtime = vec![("0xdef".to_string(), 50)];
        let changed = vec![("0x123".to_string(), 10)];

        let versions = extract_package_versions_from_effects(
            &unchanged_consensus,
            &unchanged_runtime,
            &changed,
        );

        assert_eq!(versions.get(&normalize_id("0xabc")), Some(&100));
        assert_eq!(versions.get(&normalize_id("0xdef")), Some(&50));
        assert_eq!(versions.get(&normalize_id("0x123")), Some(&10));
    }
}
