//! Transaction Cache for Package/Object Lookup
//!
//! Provides cache-first lookups for packages and objects from previously cached
//! mainnet transactions. This avoids redundant network fetches when data is
//! already available locally.
//!
//! The cache reads from `.tx-cache/*.json` files which contain complete transaction
//! snapshots including all dependent packages and objects.
//!
//! # Usage
//!
//! ```ignore
//! let cache = TxCache::new(".tx-cache")?;
//!
//! // Look up a package (returns modules as base64 bytecode)
//! if let Some(modules) = cache.get_package("0x1234...")? {
//!     for (name, bytecode) in modules {
//!         println!("Found module: {}", name);
//!     }
//! }
//!
//! // Look up an object (returns BCS bytes as base64)
//! if let Some(bytes) = cache.get_object("0xabcd...")? {
//!     println!("Found object with {} bytes", bytes.len());
//! }
//! ```

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Transaction cache for efficient package/object lookup.
///
/// Scans cached transaction files and indexes packages/objects for fast lookup.
/// This is memory-efficient: we only load the index, not all bytecode.
#[allow(dead_code)]
pub struct TxCache {
    cache_dir: PathBuf,
    /// Maps package_id -> file path containing the package
    package_index: HashMap<String, PathBuf>,
    /// Maps object_id -> file path containing the object
    object_index: HashMap<String, PathBuf>,
}

/// Minimal structure for parsing cached transaction files.
#[derive(Debug, Deserialize)]
struct CachedTransaction {
    packages: HashMap<String, Vec<(String, String)>>, // package_id -> [(module_name, base64_bytes)]
    objects: HashMap<String, String>,                 // object_id -> base64_bytes
}

impl TxCache {
    /// Create a new cache from the given directory.
    ///
    /// This scans all `.json` files and builds an index of available packages/objects.
    /// The actual bytecode is loaded on-demand when requested.
    pub fn new<P: AsRef<Path>>(cache_dir: P) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();

        if !cache_dir.exists() {
            return Ok(Self {
                cache_dir,
                package_index: HashMap::new(),
                object_index: HashMap::new(),
            });
        }

        let mut package_index = HashMap::new();
        let mut object_index = HashMap::new();

        // Scan cache files and build index
        for entry in fs::read_dir(&cache_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }

            // Quick parse to extract just package/object IDs
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(cached) = serde_json::from_str::<CachedTransaction>(&content) {
                    // Index packages
                    for pkg_id in cached.packages.keys() {
                        let normalized = normalize_address(pkg_id);
                        package_index
                            .entry(normalized)
                            .or_insert_with(|| path.clone());
                    }

                    // Index objects
                    for obj_id in cached.objects.keys() {
                        let normalized = normalize_address(obj_id);
                        object_index
                            .entry(normalized)
                            .or_insert_with(|| path.clone());
                    }
                }
            }
        }

        Ok(Self {
            cache_dir,
            package_index,
            object_index,
        })
    }

    /// Create an empty cache (no directory).
    pub fn empty() -> Self {
        Self {
            cache_dir: PathBuf::new(),
            package_index: HashMap::new(),
            object_index: HashMap::new(),
        }
    }

    /// Check if the cache has any indexed data.
    pub fn is_empty(&self) -> bool {
        self.package_index.is_empty() && self.object_index.is_empty()
    }

    /// Get the number of indexed packages.
    pub fn package_count(&self) -> usize {
        self.package_index.len()
    }

    /// Get the number of indexed objects.
    pub fn object_count(&self) -> usize {
        self.object_index.len()
    }

    /// Check if a package is available in the cache.
    pub fn has_package(&self, package_id: &str) -> bool {
        let normalized = normalize_address(package_id);
        self.package_index.contains_key(&normalized)
    }

    /// Check if an object is available in the cache.
    pub fn has_object(&self, object_id: &str) -> bool {
        let normalized = normalize_address(object_id);
        self.object_index.contains_key(&normalized)
    }

    /// Get a package's modules from the cache.
    ///
    /// Returns a vector of (module_name, base64_bytecode) pairs, or None if not found.
    pub fn get_package(&self, package_id: &str) -> Result<Option<Vec<(String, String)>>> {
        let normalized = normalize_address(package_id);

        let path = match self.package_index.get(&normalized) {
            Some(p) => p,
            None => return Ok(None),
        };

        // Load and parse the file
        let content = fs::read_to_string(path)?;
        let cached: CachedTransaction = serde_json::from_str(&content)?;

        // Find the package (try both normalized and original forms)
        for (pkg_id, modules) in &cached.packages {
            if normalize_address(pkg_id) == normalized {
                return Ok(Some(modules.clone()));
            }
        }

        Ok(None)
    }

    /// Get a package's modules as decoded bytes.
    ///
    /// Returns a vector of (module_name, bytecode) pairs, or None if not found.
    pub fn get_package_bytes(&self, package_id: &str) -> Result<Option<Vec<(String, Vec<u8>)>>> {
        use base64::Engine;

        let modules = match self.get_package(package_id)? {
            Some(m) => m,
            None => return Ok(None),
        };

        let mut result = Vec::new();
        for (name, b64) in modules {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b64)
                .map_err(|e| anyhow!("Failed to decode module {}: {}", name, e))?;
            result.push((name, bytes));
        }

        Ok(Some(result))
    }

    /// Get an object's BCS bytes from the cache (base64 encoded).
    ///
    /// Returns the base64-encoded BCS bytes, or None if not found.
    pub fn get_object_base64(&self, object_id: &str) -> Result<Option<String>> {
        let normalized = normalize_address(object_id);

        let path = match self.object_index.get(&normalized) {
            Some(p) => p,
            None => return Ok(None),
        };

        // Load and parse the file
        let content = fs::read_to_string(path)?;
        let cached: CachedTransaction = serde_json::from_str(&content)?;

        // Find the object
        for (obj_id, bytes) in &cached.objects {
            if normalize_address(obj_id) == normalized {
                return Ok(Some(bytes.clone()));
            }
        }

        Ok(None)
    }

    /// Get an object's BCS bytes from the cache (decoded).
    ///
    /// Returns the decoded BCS bytes, or None if not found.
    pub fn get_object_bytes(&self, object_id: &str) -> Result<Option<Vec<u8>>> {
        use base64::Engine;

        let b64 = match self.get_object_base64(object_id)? {
            Some(b) => b,
            None => return Ok(None),
        };

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .map_err(|e| anyhow!("Failed to decode object {}: {}", object_id, e))?;

        Ok(Some(bytes))
    }

    /// List all indexed package IDs.
    pub fn list_packages(&self) -> Vec<&str> {
        self.package_index.keys().map(|s| s.as_str()).collect()
    }

    /// List all indexed object IDs.
    pub fn list_objects(&self) -> Vec<&str> {
        self.object_index.keys().map(|s| s.as_str()).collect()
    }
}

/// Normalize an address to a consistent format.
///
/// Uses the shared implementation from `crate::cache::normalize_address`.
fn normalize_address(addr: &str) -> String {
    crate::cache::normalize_address(addr)
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_normalize_address() {
        assert_eq!(
            normalize_address("0x2"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert_eq!(
            normalize_address("2"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert_eq!(
            normalize_address("0x0000000000000000000000000000000000000000000000000000000000000002"),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_empty_cache() {
        let cache = TxCache::empty();
        assert!(cache.is_empty());
        assert_eq!(cache.package_count(), 0);
        assert_eq!(cache.object_count(), 0);
        assert!(!cache.has_package("0x2"));
    }

    #[test]
    fn test_cache_from_directory() {
        // This test requires the actual cache directory
        let cache = TxCache::new(".tx-cache");
        match cache {
            Ok(c) => {
                println!(
                    "Cache loaded: {} packages, {} objects",
                    c.package_count(),
                    c.object_count()
                );
            }
            Err(e) => {
                println!("No cache available: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod integration_tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_data_fetcher_cache_first() {
        use crate::data_fetcher::DataFetcher;

        // Create fetcher with cache
        let fetcher = DataFetcher::mainnet().with_cache_optional(".tx-cache");

        if !fetcher.has_cache() {
            println!("No cache available, skipping test");
            return;
        }

        // Use the new CacheStats structure
        let stats = fetcher.cache_stats().unwrap();
        println!(
            "Cache: {} packages, {} objects, {} transactions",
            stats.package_count, stats.object_count, stats.transaction_count
        );

        // Try to fetch a package that should be in cache
        // Use a known DeFi package from our cached transactions
        let test_pkg = "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270";

        match fetcher.fetch_package(test_pkg) {
            Ok(pkg) => {
                println!("Fetched package: {} modules", pkg.modules.len());
                println!("Source: {:?}", pkg.source);

                // Should be from cache!
                assert_eq!(
                    pkg.source,
                    crate::data_fetcher::DataSource::Cache,
                    "Package should come from cache"
                );

                // Verify modules are valid
                assert!(!pkg.modules.is_empty(), "Package should have modules");
                for m in &pkg.modules {
                    assert!(!m.name.is_empty(), "Module should have name");
                    assert!(!m.bytecode.is_empty(), "Module should have bytecode");
                }

                println!("✓ Cache-first lookup working!");
            }
            Err(e) => {
                println!("Failed to fetch: {}", e);
                // This is OK if the package isn't in cache
            }
        }
    }
}
