//! Historical package version finder.
//!
//! When replaying historical transactions, we need bytecode that was active at transaction time.
//! The problem is that transaction effects don't directly track which package versions were used.
//!
//! This module solves this by:
//! 1. Reading the Version object's value from historical state (e.g., value = 8)
//! 2. Iterating through package versions to find one where CURRENT_VERSION matches
//! 3. Returning that package version for use in replay
//!
//! ## Example
//!
//! ```ignore
//! use sui_sandbox_core::utilities::HistoricalVersionFinder;
//!
//! let finder = HistoricalVersionFinder::new(grpc_client);
//!
//! // Find package version where CURRENT_VERSION = 8
//! let version = finder.find_package_version_for_constant(
//!     "0xefe8b36d...",  // package ID
//!     8,                // target CURRENT_VERSION value
//! ).await?;
//!
//! // Now fetch that specific package version
//! let package = grpc.get_object_at_version(&package_id, Some(version)).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use move_binary_format::CompiledModule;
use parking_lot::RwLock;

use super::version_utils::detect_version_constants;

/// Result of finding a historical package version.
#[derive(Debug, Clone)]
pub struct VersionFindResult {
    /// The package version that has the matching CURRENT_VERSION constant
    pub package_version: u64,
    /// The detected CURRENT_VERSION constant value
    pub detected_constant: u64,
    /// Number of versions searched before finding
    pub versions_searched: usize,
}

/// Cache for package version -> CURRENT_VERSION mappings.
///
/// This avoids repeatedly fetching and parsing the same package versions.
#[derive(Debug, Default)]
pub struct VersionConstantCache {
    /// Map from (package_id, package_version) to detected CURRENT_VERSION constant
    cache: RwLock<HashMap<(String, u64), Option<u64>>>,
}

impl VersionConstantCache {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Get cached constant for a package version.
    pub fn get(&self, package_id: &str, version: u64) -> Option<Option<u64>> {
        self.cache.read().get(&(package_id.to_string(), version)).copied()
    }

    /// Store constant for a package version.
    pub fn insert(&self, package_id: &str, version: u64, constant: Option<u64>) {
        self.cache.write().insert((package_id.to_string(), version), constant);
    }

    /// Get all cached entries for a package.
    pub fn get_all_for_package(&self, package_id: &str) -> Vec<(u64, Option<u64>)> {
        self.cache
            .read()
            .iter()
            .filter(|((pid, _), _)| pid == package_id)
            .map(|((_, ver), constant)| (*ver, *constant))
            .collect()
    }
}

/// Extracts CURRENT_VERSION constant from compiled modules.
///
/// This reuses the existing `detect_version_constants` infrastructure.
pub fn extract_version_constant_from_modules(modules: &[CompiledModule]) -> Option<u64> {
    let versions = detect_version_constants(modules.iter());

    // Return the highest version found (protocols typically use one version constant)
    versions.values().max().copied()
}

/// Extracts CURRENT_VERSION constant from raw module bytecode.
pub fn extract_version_constant_from_bytecode(module_bytes: &[(String, Vec<u8>)]) -> Option<u64> {
    let modules: Vec<CompiledModule> = module_bytes
        .iter()
        .filter_map(|(_, bytes)| CompiledModule::deserialize_with_defaults(bytes).ok())
        .collect();

    extract_version_constant_from_modules(&modules)
}

/// Strategy for searching package versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStrategy {
    /// Search from version 1 upward (default, finds earliest matching version)
    Ascending,
    /// Search from latest version downward (faster for recent transactions)
    Descending,
    /// Binary search (fastest for large version ranges)
    Binary,
}

impl Default for SearchStrategy {
    fn default() -> Self {
        Self::Ascending
    }
}

/// Configuration for version finding.
#[derive(Debug, Clone)]
pub struct VersionFinderConfig {
    /// Maximum number of versions to search
    pub max_versions_to_search: usize,
    /// Search strategy
    pub strategy: SearchStrategy,
    /// Whether to cache results
    pub use_cache: bool,
}

impl Default for VersionFinderConfig {
    fn default() -> Self {
        Self {
            max_versions_to_search: 50,
            strategy: SearchStrategy::Ascending,
            use_cache: true,
        }
    }
}

/// Async trait for fetching package modules at a specific version.
#[async_trait::async_trait]
pub trait PackageModuleFetcher: Send + Sync {
    /// Fetch modules for a package at a specific version.
    /// Returns None if the version doesn't exist.
    async fn fetch_modules_at_version(
        &self,
        package_id: &str,
        version: u64,
    ) -> anyhow::Result<Option<Vec<(String, Vec<u8>)>>>;

    /// Get the latest version of a package.
    async fn get_latest_version(&self, package_id: &str) -> anyhow::Result<Option<u64>>;
}

/// Historical version finder that searches for package versions matching a target constant.
pub struct HistoricalVersionFinder<F: PackageModuleFetcher> {
    fetcher: F,
    cache: Arc<VersionConstantCache>,
    config: VersionFinderConfig,
}

impl<F: PackageModuleFetcher> HistoricalVersionFinder<F> {
    /// Create a new finder with default configuration.
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            cache: Arc::new(VersionConstantCache::new()),
            config: VersionFinderConfig::default(),
        }
    }

    /// Create a new finder with custom configuration.
    pub fn with_config(fetcher: F, config: VersionFinderConfig) -> Self {
        Self {
            fetcher,
            cache: Arc::new(VersionConstantCache::new()),
            config,
        }
    }

    /// Create a new finder with a shared cache.
    pub fn with_cache(fetcher: F, cache: Arc<VersionConstantCache>) -> Self {
        Self {
            fetcher,
            cache,
            config: VersionFinderConfig::default(),
        }
    }

    /// Find the package version where CURRENT_VERSION equals the target value.
    ///
    /// # Arguments
    /// * `package_id` - The package ID (hex string)
    /// * `target_constant` - The CURRENT_VERSION value to match (from Version object)
    ///
    /// # Returns
    /// * `Ok(Some(result))` - Found matching version
    /// * `Ok(None)` - No matching version found within search limits
    /// * `Err(e)` - Error during search
    pub async fn find_package_version_for_constant(
        &self,
        package_id: &str,
        target_constant: u64,
    ) -> anyhow::Result<Option<VersionFindResult>> {
        // First check cache for already-found mappings
        if self.config.use_cache {
            let cached = self.cache.get_all_for_package(package_id);
            for (version, constant) in cached {
                if constant == Some(target_constant) {
                    return Ok(Some(VersionFindResult {
                        package_version: version,
                        detected_constant: target_constant,
                        versions_searched: 0,
                    }));
                }
            }
        }

        match self.config.strategy {
            SearchStrategy::Ascending => {
                self.search_ascending(package_id, target_constant).await
            }
            SearchStrategy::Descending => {
                self.search_descending(package_id, target_constant).await
            }
            SearchStrategy::Binary => {
                self.search_binary(package_id, target_constant).await
            }
        }
    }

    /// Search from version 1 upward.
    async fn search_ascending(
        &self,
        package_id: &str,
        target_constant: u64,
    ) -> anyhow::Result<Option<VersionFindResult>> {
        for version in 1..=self.config.max_versions_to_search as u64 {
            if let Some(result) = self.check_version(package_id, version, target_constant, version as usize).await? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    /// Search from latest version downward.
    async fn search_descending(
        &self,
        package_id: &str,
        target_constant: u64,
    ) -> anyhow::Result<Option<VersionFindResult>> {
        let latest = self.fetcher.get_latest_version(package_id).await?;
        let Some(latest_version) = latest else {
            return Ok(None);
        };

        let start = latest_version;
        let end = start.saturating_sub(self.config.max_versions_to_search as u64).max(1);

        let mut searched = 0;
        for version in (end..=start).rev() {
            searched += 1;
            if let Some(result) = self.check_version(package_id, version, target_constant, searched).await? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    /// Binary search for the matching version.
    ///
    /// This assumes CURRENT_VERSION is monotonically increasing across package versions,
    /// which is true for well-designed protocols.
    async fn search_binary(
        &self,
        package_id: &str,
        target_constant: u64,
    ) -> anyhow::Result<Option<VersionFindResult>> {
        let latest = self.fetcher.get_latest_version(package_id).await?;
        let Some(latest_version) = latest else {
            return Ok(None);
        };

        let mut low = 1u64;
        let mut high = latest_version;
        let mut searched = 0;

        while low <= high && searched < self.config.max_versions_to_search {
            let mid = low + (high - low) / 2;
            searched += 1;

            let modules = self.fetcher.fetch_modules_at_version(package_id, mid).await?;
            let constant = modules.as_ref().and_then(|m| extract_version_constant_from_bytecode(m));

            // Cache the result
            if self.config.use_cache {
                self.cache.insert(package_id, mid, constant);
            }

            match constant {
                Some(c) if c == target_constant => {
                    return Ok(Some(VersionFindResult {
                        package_version: mid,
                        detected_constant: target_constant,
                        versions_searched: searched,
                    }));
                }
                Some(c) if c < target_constant => {
                    // Need higher version
                    low = mid + 1;
                }
                Some(_) => {
                    // Need lower version
                    if mid == 0 {
                        break;
                    }
                    high = mid - 1;
                }
                None => {
                    // Version doesn't exist or has no constant, try lower
                    if mid == 0 {
                        break;
                    }
                    high = mid - 1;
                }
            }
        }

        // Binary search didn't find exact match, do linear search in remaining range
        for version in low..=high {
            if searched >= self.config.max_versions_to_search {
                break;
            }
            if let Some(result) = self.check_version(package_id, version, target_constant, searched).await? {
                return Ok(Some(result));
            }
            searched += 1;
        }

        Ok(None)
    }

    /// Check if a specific version has the target constant.
    async fn check_version(
        &self,
        package_id: &str,
        version: u64,
        target_constant: u64,
        versions_searched: usize,
    ) -> anyhow::Result<Option<VersionFindResult>> {
        // Check cache first
        if self.config.use_cache {
            if let Some(cached_constant) = self.cache.get(package_id, version) {
                if cached_constant == Some(target_constant) {
                    return Ok(Some(VersionFindResult {
                        package_version: version,
                        detected_constant: target_constant,
                        versions_searched,
                    }));
                }
                // Cached but doesn't match
                return Ok(None);
            }
        }

        // Fetch and check
        let modules = self.fetcher.fetch_modules_at_version(package_id, version).await?;
        let constant = modules.as_ref().and_then(|m| extract_version_constant_from_bytecode(m));

        // Cache the result
        if self.config.use_cache {
            self.cache.insert(package_id, version, constant);
        }

        if constant == Some(target_constant) {
            return Ok(Some(VersionFindResult {
                package_version: version,
                detected_constant: target_constant,
                versions_searched,
            }));
        }

        Ok(None)
    }

    /// Get the shared cache.
    pub fn cache(&self) -> &Arc<VersionConstantCache> {
        &self.cache
    }
}

/// Adapter to use sui_transport::grpc::GrpcClient with HistoricalVersionFinder.
///
/// This is defined as a separate struct to avoid direct dependency coupling.
/// Users can implement PackageModuleFetcher for their own clients.
pub struct GrpcPackageFetcher<C> {
    client: C,
}

impl<C> GrpcPackageFetcher<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

/// Trait for gRPC-like clients that can fetch objects.
/// This allows the finder to work with any client that implements these methods.
#[async_trait::async_trait]
pub trait GrpcLikeClient: Send + Sync {
    /// Fetch object at a specific version.
    async fn get_object_at_version(
        &self,
        object_id: &str,
        version: Option<u64>,
    ) -> anyhow::Result<Option<GrpcObjectResult>>;

    /// Fetch latest object.
    async fn get_object(&self, object_id: &str) -> anyhow::Result<Option<GrpcObjectResult>>;
}

/// Minimal object result for version finding.
pub struct GrpcObjectResult {
    pub version: u64,
    pub package_modules: Option<Vec<(String, Vec<u8>)>>,
}

#[async_trait::async_trait]
impl<C: GrpcLikeClient> PackageModuleFetcher for GrpcPackageFetcher<C> {
    async fn fetch_modules_at_version(
        &self,
        package_id: &str,
        version: u64,
    ) -> anyhow::Result<Option<Vec<(String, Vec<u8>)>>> {
        let result = self.client.get_object_at_version(package_id, Some(version)).await?;
        Ok(result.and_then(|r| r.package_modules))
    }

    async fn get_latest_version(&self, package_id: &str) -> anyhow::Result<Option<u64>> {
        let result = self.client.get_object(package_id).await?;
        Ok(result.map(|r| r.version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_constant_cache() {
        let cache = VersionConstantCache::new();

        cache.insert("0xabc", 1, Some(5));
        cache.insert("0xabc", 2, Some(10));
        cache.insert("0xdef", 1, Some(3));

        assert_eq!(cache.get("0xabc", 1), Some(Some(5)));
        assert_eq!(cache.get("0xabc", 2), Some(Some(10)));
        assert_eq!(cache.get("0xabc", 3), None);

        let all = cache.get_all_for_package("0xabc");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_config_defaults() {
        let config = VersionFinderConfig::default();
        assert_eq!(config.max_versions_to_search, 50);
        assert_eq!(config.strategy, SearchStrategy::Ascending);
        assert!(config.use_cache);
    }
}
