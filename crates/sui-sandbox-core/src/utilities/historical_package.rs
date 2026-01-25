//! Historical Package Resolution Utility
//!
//! This module provides composable utilities for resolving packages at their
//! historical versions by following linkage tables. This is essential for
//! accurate transaction replay.
//!
//! ## The Problem
//!
//! On Sui, when a package is upgraded:
//! - The original address always contains v1 bytecode
//! - Upgraded bytecode lives at a different storage address
//! - Linkage tables map original_id → upgraded_id
//!
//! Without following linkage tables, you get bytecode with old constants
//! (e.g., CURRENT_VERSION=1) that don't match the on-chain state.
//!
//! ## Solution
//!
//! `HistoricalPackageResolver` handles:
//! 1. Linkage table processing to find upgraded storage addresses
//! 2. Transitive dependency resolution
//! 3. Caching to avoid re-fetching
//! 4. Framework package filtering
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::HistoricalPackageResolver;
//!
//! let mut resolver = HistoricalPackageResolver::new(grpc_client, runtime_handle);
//!
//! // Resolve packages from transaction effects
//! let packages = resolver.resolve_from_effects(
//!     &unchanged_consensus_objects,
//!     &initial_package_ids,
//! )?;
//!
//! // Get all compiled modules for the state reconstructor
//! for module in resolver.all_modules() {
//!     // ...
//! }
//! ```

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use base64::Engine;
use move_binary_format::CompiledModule;

use super::address::{is_framework_package, normalize_address};
use super::type_utils::extract_dependencies_from_bytecode;

/// Linkage entry from a package's metadata.
#[derive(Debug, Clone)]
pub struct PackageLinkage {
    /// Original package address (where types are defined)
    pub original_id: String,
    /// Upgraded package address (where current bytecode lives)
    pub upgraded_id: String,
    /// Version number
    pub version: u64,
}

/// A resolved package with its compiled modules.
#[derive(Debug)]
pub struct FetchedPackage {
    /// Storage address where this package was fetched from
    pub storage_id: String,
    /// Original address (may differ from storage_id for upgraded packages)
    pub original_id: Option<String>,
    /// Package version
    pub version: u64,
    /// Compiled modules
    pub modules: Vec<CompiledModule>,
    /// Raw module bytes (name -> bytes)
    pub raw_modules: Vec<(String, Vec<u8>)>,
    /// Linkage table entries
    pub linkage: Vec<PackageLinkage>,
}

/// Trait for fetching package data. Allows mocking in tests.
pub trait PackageFetcher: Send + Sync {
    /// Fetch a package at a specific version (or latest if None).
    fn fetch_package(
        &self,
        package_id: &str,
        version: Option<u64>,
    ) -> Result<Option<FetchedPackageData>>;
}

/// Raw package data from a fetcher.
#[derive(Debug, Clone)]
pub struct FetchedPackageData {
    /// Package storage address
    pub id: String,
    /// Package version
    pub version: u64,
    /// Module name -> raw bytecode
    pub modules: Vec<(String, Vec<u8>)>,
    /// Linkage table
    pub linkage: Vec<PackageLinkage>,
}

/// A simple callback-based package fetcher.
///
/// This allows callers to provide a closure that fetches packages synchronously,
/// avoiding the need for tokio as a dependency in the sandbox-core crate.
pub struct CallbackPackageFetcher<F>
where
    F: Fn(&str, Option<u64>) -> Result<Option<FetchedPackageData>>,
{
    fetch_fn: F,
}

impl<F> CallbackPackageFetcher<F>
where
    F: Fn(&str, Option<u64>) -> Result<Option<FetchedPackageData>>,
{
    /// Create a new callback-based fetcher.
    pub fn new(fetch_fn: F) -> Self {
        Self { fetch_fn }
    }
}

impl<F> PackageFetcher for CallbackPackageFetcher<F>
where
    F: Fn(&str, Option<u64>) -> Result<Option<FetchedPackageData>> + Send + Sync,
{
    fn fetch_package(
        &self,
        package_id: &str,
        version: Option<u64>,
    ) -> Result<Option<FetchedPackageData>> {
        (self.fetch_fn)(package_id, version)
    }
}

/// Helper function to create FetchedPackageData from a GrpcObject.
/// This is provided as a utility for callers who are using gRPC.
pub fn grpc_object_to_package_data(
    package_id: &str,
    obj: &sui_transport::grpc::GrpcObject,
) -> Option<FetchedPackageData> {
    obj.package_modules.as_ref().map(|modules| {
        let linkage = obj
            .package_linkage
            .as_ref()
            .map(|links| {
                links
                    .iter()
                    .map(|l| PackageLinkage {
                        original_id: l.original_id.clone(),
                        upgraded_id: l.upgraded_id.clone(),
                        version: l.upgraded_version,
                    })
                    .collect()
            })
            .unwrap_or_default();

        FetchedPackageData {
            id: package_id.to_string(),
            version: obj.version,
            modules: modules.clone(),
            linkage,
        }
    })
}

/// Configuration for package resolution.
#[derive(Debug, Clone)]
pub struct PackageResolutionConfig {
    /// Maximum dependency depth to resolve
    pub max_depth: usize,
    /// Whether to skip framework packages (0x1, 0x2, 0x3)
    pub skip_framework: bool,
}

impl Default for PackageResolutionConfig {
    fn default() -> Self {
        Self {
            max_depth: 10,
            skip_framework: true,
        }
    }
}

/// Historical package resolver that follows linkage tables.
///
/// This resolves packages at their correct historical versions by:
/// 1. Processing linkage tables to find upgraded package addresses
/// 2. Detecting self-upgrades (package's own linkage pointing to upgraded storage)
/// 3. Fetching bytecode from the correct storage addresses
/// 4. Handling transitive dependencies
///
/// ## Self-Upgrade Detection
///
/// On Sui, when a package is upgraded, the original address always contains v1 bytecode.
/// A package's linkage table may contain a self-reference entry where:
/// - `original_id == this_package's_address`
/// - `upgraded_id` is different (points to the new storage address)
///
/// When this is detected, the resolver fetches from the upgraded address but stores
/// the modules at the original address (what PTB references).
pub struct HistoricalPackageResolver<F: PackageFetcher> {
    fetcher: F,
    config: PackageResolutionConfig,
    /// Cache: storage_id -> FetchedPackage
    package_cache: HashMap<String, FetchedPackage>,
    /// Linkage upgrades: original_id -> upgraded_id
    linkage_upgrades: HashMap<String, String>,
    /// Reverse linkage: upgraded_id -> original_id
    linkage_originals: HashMap<String, String>,
    /// Known historical versions: package_id -> version
    historical_versions: HashMap<String, u64>,
}

impl<F: PackageFetcher> HistoricalPackageResolver<F> {
    /// Create a new resolver with custom fetcher.
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            config: PackageResolutionConfig::default(),
            package_cache: HashMap::new(),
            linkage_upgrades: HashMap::new(),
            linkage_originals: HashMap::new(),
            historical_versions: HashMap::new(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(fetcher: F, config: PackageResolutionConfig) -> Self {
        Self {
            fetcher,
            config,
            package_cache: HashMap::new(),
            linkage_upgrades: HashMap::new(),
            linkage_originals: HashMap::new(),
            historical_versions: HashMap::new(),
        }
    }

    /// Set known historical versions for packages.
    pub fn set_historical_versions(&mut self, versions: HashMap<String, u64>) {
        self.historical_versions = versions;
    }

    /// Resolve packages starting from initial package IDs.
    ///
    /// This follows linkage tables to fetch upgraded bytecode and resolves
    /// all transitive dependencies. Key features:
    ///
    /// 1. **Self-upgrade detection**: Checks if a package's linkage table contains
    ///    a self-reference where `original_id == pkg_id` but `upgraded_id` differs.
    ///    When detected, fetches from the upgraded storage address.
    ///
    /// 2. **Post-processing re-fetch**: After initial resolution, re-fetches any
    ///    packages that were fetched with v1 bytecode before their upgrade was
    ///    discovered via another package's linkage table.
    pub fn resolve_packages(&mut self, initial_ids: &[String]) -> Result<()> {
        let mut to_fetch: HashSet<String> = initial_ids.iter().cloned().collect();
        let mut fetched: HashSet<String> = HashSet::new();

        for _depth in 0..self.config.max_depth {
            if to_fetch.is_empty() {
                break;
            }

            let mut next_deps: HashSet<String> = HashSet::new();

            for pkg_id in to_fetch.iter() {
                let pkg_id_normalized = normalize_address(pkg_id);

                if fetched.contains(&pkg_id_normalized) {
                    continue;
                }

                if self.config.skip_framework && is_framework_package(&pkg_id_normalized) {
                    fetched.insert(pkg_id_normalized);
                    continue;
                }

                // Check if we already know this package has been upgraded
                let known_upgrade = self.linkage_upgrades.get(&pkg_id_normalized).cloned();
                let (fetch_id, fetch_id_normalized) = if let Some(upgraded_id) = known_upgrade {
                    // Already know this package is upgraded - fetch from upgraded address
                    (upgraded_id.clone(), upgraded_id)
                } else {
                    (pkg_id.clone(), pkg_id_normalized.clone())
                };

                // Get historical version if known
                let version = self.historical_versions.get(&fetch_id).copied();

                match self.fetcher.fetch_package(&fetch_id, version)? {
                    Some(data) => {
                        // Check for self-upgrade: package's own linkage pointing to upgraded storage
                        let mut self_upgrade: Option<String> = None;
                        for linkage in &data.linkage {
                            let orig = normalize_address(&linkage.original_id);
                            let upgraded = normalize_address(&linkage.upgraded_id);

                            // Self-upgrade detection: original_id == this package but upgraded_id differs
                            if orig == fetch_id_normalized && orig != upgraded {
                                self_upgrade = Some(upgraded.clone());
                                self.linkage_upgrades.insert(orig.clone(), upgraded.clone());
                                self.linkage_originals
                                    .insert(upgraded.clone(), orig.clone());
                                break;
                            }
                        }

                        // If we discovered a self-upgrade, re-fetch from upgraded address
                        if let Some(upgraded_addr) = self_upgrade {
                            let upgrade_version =
                                self.historical_versions.get(&upgraded_addr).copied();
                            if let Some(upgraded_data) = self
                                .fetcher
                                .fetch_package(&upgraded_addr, upgrade_version)?
                            {
                                // Process and store upgraded package at original address
                                self.process_and_store_package(
                                    &pkg_id_normalized,
                                    Some(upgraded_addr.clone()),
                                    upgraded_data,
                                    &mut next_deps,
                                    &fetched,
                                );
                                fetched.insert(pkg_id_normalized.clone());
                                fetched.insert(upgraded_addr);
                                fetched.insert(fetch_id_normalized);
                                continue;
                            }
                        }

                        // Process linkage table for other packages
                        for linkage in &data.linkage {
                            if self.config.skip_framework
                                && is_framework_package(&linkage.original_id)
                            {
                                continue;
                            }

                            let orig = normalize_address(&linkage.original_id);
                            let upgraded = normalize_address(&linkage.upgraded_id);

                            if orig != upgraded {
                                self.linkage_upgrades.insert(orig.clone(), upgraded.clone());
                                self.linkage_originals
                                    .insert(upgraded.clone(), orig.clone());

                                // Queue upgraded package for fetching
                                if !fetched.contains(&upgraded)
                                    && !self.package_cache.contains_key(&upgraded)
                                {
                                    next_deps.insert(upgraded);
                                }
                            }
                        }

                        // Determine storage key - use original address if this is an upgraded fetch
                        let storage_key = if pkg_id_normalized != fetch_id_normalized {
                            pkg_id_normalized.clone()
                        } else if let Some(original) =
                            self.linkage_originals.get(&pkg_id_normalized)
                        {
                            original.clone()
                        } else {
                            pkg_id_normalized.clone()
                        };

                        // Process and store package
                        self.process_and_store_package(
                            &storage_key,
                            if storage_key != fetch_id_normalized {
                                Some(fetch_id_normalized.clone())
                            } else {
                                None
                            },
                            data,
                            &mut next_deps,
                            &fetched,
                        );

                        fetched.insert(storage_key.clone());
                        fetched.insert(fetch_id_normalized);
                        if pkg_id_normalized != storage_key {
                            fetched.insert(pkg_id_normalized);
                        }
                    }
                    None => {
                        // Package not found, mark as visited to avoid retrying
                        fetched.insert(pkg_id_normalized);
                        fetched.insert(fetch_id_normalized);
                    }
                }
            }

            to_fetch = next_deps;
        }

        // Post-processing: Re-fetch packages that were fetched with v1 bytecode
        // before their upgrade was discovered via another package's linkage
        self.refetch_upgraded_packages()?;

        Ok(())
    }

    /// Process package data and store in cache.
    fn process_and_store_package(
        &mut self,
        storage_key: &str,
        upgraded_from: Option<String>,
        data: FetchedPackageData,
        next_deps: &mut HashSet<String>,
        fetched: &HashSet<String>,
    ) {
        let mut compiled_modules = Vec::new();
        let mut raw_modules = Vec::new();

        for (name, bytecode) in &data.modules {
            raw_modules.push((name.clone(), bytecode.clone()));

            if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
                // Extract dependencies from bytecode
                let deps = extract_dependencies_from_bytecode(bytecode);
                for dep in deps {
                    let dep_normalized = normalize_address(&dep);
                    // Use upgraded version if known
                    let actual_dep = self
                        .linkage_upgrades
                        .get(&dep_normalized)
                        .cloned()
                        .unwrap_or(dep_normalized);

                    if !fetched.contains(&actual_dep)
                        && !self.package_cache.contains_key(&actual_dep)
                    {
                        next_deps.insert(actual_dep);
                    }
                }

                compiled_modules.push(module);
            }
        }

        let package = FetchedPackage {
            storage_id: storage_key.to_string(),
            original_id: upgraded_from,
            version: data.version,
            modules: compiled_modules,
            raw_modules,
            linkage: data.linkage,
        };

        self.package_cache.insert(storage_key.to_string(), package);
    }

    /// Re-fetch packages that were fetched with v1 bytecode before their upgrade was discovered.
    fn refetch_upgraded_packages(&mut self) -> Result<()> {
        // Collect packages that need re-fetching
        let to_refetch: Vec<(String, String)> = self
            .linkage_upgrades
            .iter()
            .filter(|(original, upgraded)| {
                // Check if we have the original package but with v1 bytecode
                // (indicated by not having the upgraded package in cache)
                self.package_cache.contains_key(*original)
                    && !self.package_cache.contains_key(*upgraded)
            })
            .map(|(o, u)| (o.clone(), u.clone()))
            .collect();

        for (original_id, upgraded_id) in to_refetch {
            let version = self.historical_versions.get(&upgraded_id).copied();
            if let Some(data) = self.fetcher.fetch_package(&upgraded_id, version)? {
                // Parse modules
                let mut compiled_modules = Vec::new();
                let mut raw_modules = Vec::new();

                for (name, bytecode) in &data.modules {
                    raw_modules.push((name.clone(), bytecode.clone()));
                    if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
                        compiled_modules.push(module);
                    }
                }

                let package = FetchedPackage {
                    storage_id: original_id.clone(),
                    original_id: Some(upgraded_id),
                    version: data.version,
                    modules: compiled_modules,
                    raw_modules,
                    linkage: data.linkage,
                };

                // Replace v1 bytecode with upgraded bytecode
                self.package_cache.insert(original_id, package);
            }
        }

        Ok(())
    }

    /// Get all resolved packages.
    pub fn packages(&self) -> impl Iterator<Item = &FetchedPackage> {
        self.package_cache.values()
    }

    /// Get all compiled modules from resolved packages.
    pub fn all_modules(&self) -> impl Iterator<Item = &CompiledModule> {
        self.package_cache.values().flat_map(|p| p.modules.iter())
    }

    /// Get all raw modules as (package_id, module_name, bytecode).
    pub fn all_raw_modules(&self) -> impl Iterator<Item = (&str, &str, &[u8])> {
        self.package_cache.values().flat_map(|p| {
            let pkg_id = p.storage_id.as_str();
            p.raw_modules
                .iter()
                .map(move |(name, bytes)| (pkg_id, name.as_str(), bytes.as_slice()))
        })
    }

    /// Get packages as base64-encoded modules for CachedTransaction.
    pub fn packages_as_base64(&self) -> HashMap<String, Vec<(String, String)>> {
        self.package_cache
            .iter()
            .map(|(pkg_id, pkg)| {
                let modules_b64: Vec<(String, String)> = pkg
                    .raw_modules
                    .iter()
                    .map(|(name, bytes)| {
                        (
                            name.clone(),
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                        )
                    })
                    .collect();
                (pkg_id.clone(), modules_b64)
            })
            .collect()
    }

    /// Get linkage upgrades map (original_id -> upgraded_id).
    pub fn linkage_upgrades(&self) -> &HashMap<String, String> {
        &self.linkage_upgrades
    }

    /// Check if an original package has been upgraded.
    pub fn get_upgraded_id(&self, original_id: &str) -> Option<&str> {
        self.linkage_upgrades
            .get(&normalize_address(original_id))
            .map(|s| s.as_str())
    }

    /// Get a specific package by ID.
    pub fn get_package(&self, package_id: &str) -> Option<&FetchedPackage> {
        self.package_cache.get(&normalize_address(package_id))
    }

    /// Get number of resolved packages.
    pub fn package_count(&self) -> usize {
        self.package_cache.len()
    }

    /// Get number of linkage upgrades discovered.
    pub fn upgrade_count(&self) -> usize {
        self.linkage_upgrades.len()
    }

    /// Clear the package cache.
    pub fn clear(&mut self) {
        self.package_cache.clear();
        self.linkage_upgrades.clear();
        self.linkage_originals.clear();
    }

    /// Get the reverse linkage map (upgraded_id -> original_id).
    pub fn linkage_originals(&self) -> &HashMap<String, String> {
        &self.linkage_originals
    }
}

/// Convenience type alias for a resolver with a callback fetcher.
pub type CallbackResolver<F> = HistoricalPackageResolver<CallbackPackageFetcher<F>>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock fetcher for testing
    struct MockFetcher {
        packages: HashMap<String, FetchedPackageData>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                packages: HashMap::new(),
            }
        }

        fn add_package(&mut self, id: &str, version: u64, linkage: Vec<PackageLinkage>) {
            let normalized = normalize_address(id);
            self.packages.insert(
                normalized.clone(),
                FetchedPackageData {
                    id: normalized,
                    version,
                    modules: vec![("test".to_string(), vec![])],
                    linkage,
                },
            );
        }
    }

    impl PackageFetcher for MockFetcher {
        fn fetch_package(
            &self,
            package_id: &str,
            _version: Option<u64>,
        ) -> Result<Option<FetchedPackageData>> {
            let normalized = normalize_address(package_id);
            Ok(self.packages.get(&normalized).cloned())
        }
    }

    #[test]
    fn test_resolver_creation() {
        let fetcher = MockFetcher::new();
        let resolver = HistoricalPackageResolver::new(fetcher);
        assert_eq!(resolver.package_count(), 0);
    }

    #[test]
    fn test_resolve_simple_package() {
        let mut fetcher = MockFetcher::new();
        fetcher.add_package("0xabc123", 1, vec![]);

        let mut resolver = HistoricalPackageResolver::new(fetcher);
        resolver
            .resolve_packages(&["0xabc123".to_string()])
            .unwrap();

        assert_eq!(resolver.package_count(), 1);
    }

    #[test]
    fn test_linkage_upgrade_tracking() {
        let mut fetcher = MockFetcher::new();

        // Package A has linkage to upgraded B
        fetcher.add_package(
            "0xaaa",
            1,
            vec![PackageLinkage {
                original_id: "0xbbb".to_string(),
                upgraded_id: "0xccc".to_string(),
                version: 2,
            }],
        );
        fetcher.add_package("0xccc", 2, vec![]);

        let mut resolver = HistoricalPackageResolver::new(fetcher);
        resolver.resolve_packages(&["0xaaa".to_string()]).unwrap();

        // Should have discovered the upgrade
        assert_eq!(resolver.upgrade_count(), 1);
        let upgraded = resolver.get_upgraded_id("0xbbb");
        assert!(upgraded.is_some());
    }

    #[test]
    fn test_skip_framework() {
        let fetcher = MockFetcher::new();
        let mut resolver = HistoricalPackageResolver::new(fetcher);

        // Framework packages should be skipped
        resolver
            .resolve_packages(&["0x1".to_string(), "0x2".to_string()])
            .unwrap();
        assert_eq!(resolver.package_count(), 0);
    }

    #[test]
    fn test_config_max_depth() {
        let fetcher = MockFetcher::new();
        let config = PackageResolutionConfig {
            max_depth: 5,
            skip_framework: true,
        };
        let resolver = HistoricalPackageResolver::with_config(fetcher, config);
        assert_eq!(resolver.config.max_depth, 5);
    }
}
