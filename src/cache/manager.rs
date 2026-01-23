//! Unified Cache Manager implementation.
//!
//! Provides read/write access to cached packages, objects, and transactions
//! with consistent address normalization and metadata tracking.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::index::{CachedObjectEntry, CachedPackageEntry};
use super::normalize::normalize_address;
use super::CacheStats;

/// Unified cache manager for packages, objects, and transactions.
///
/// This manager provides:
/// - In-memory indices for fast lookups (packages and objects)
/// - Disk-backed storage for complete transaction data
/// - Write-through caching for network fetches
/// - Consistent address normalization
/// - Metadata tracking (version, type info)
pub struct CacheManager {
    /// Cache directory path
    cache_dir: PathBuf,
    /// Package index: normalized_address -> entry
    package_index: HashMap<String, CachedPackageEntry>,
    /// Object index: normalized_address -> entry
    object_index: HashMap<String, CachedObjectEntry>,
    /// Whether the cache is read-only (no writes)
    read_only: bool,
}

/// Serialized cache transaction format.
///
/// This is the canonical format for cached transaction files.
/// All other cache systems should use this structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTransactionData {
    /// Transaction metadata
    pub transaction: serde_json::Value,
    /// Packages: package_id -> [(module_name, base64_bytecode)]
    pub packages: HashMap<String, Vec<(String, String)>>,
    /// Objects: object_id -> base64_bcs_bytes
    pub objects: HashMap<String, String>,
    /// Object types: object_id -> type_string
    #[serde(default)]
    pub object_types: HashMap<String, String>,
    /// Object versions: object_id -> version
    #[serde(default)]
    pub object_versions: HashMap<String, u64>,
    /// Package versions: package_id -> version
    #[serde(default)]
    pub package_versions: HashMap<String, u64>,
    /// Cache timestamp (unix seconds)
    pub cached_at: u64,
}

/// Result of a package lookup.
#[derive(Debug, Clone)]
pub struct CachedPackage {
    /// Normalized package address
    pub address: String,
    /// Package version
    pub version: u64,
    /// Modules: (name, bytecode)
    pub modules: Vec<(String, Vec<u8>)>,
}

/// Result of an object lookup.
#[derive(Debug, Clone)]
pub struct CachedObject {
    /// Normalized object address
    pub address: String,
    /// Object version
    pub version: u64,
    /// Type tag string
    pub type_tag: Option<String>,
    /// BCS-serialized bytes
    pub bcs_bytes: Vec<u8>,
    /// Whether this object is shared
    pub is_shared: bool,
    /// Whether this object is immutable
    pub is_immutable: bool,
}

impl CacheManager {
    /// Create a new cache manager for the given directory.
    ///
    /// This scans all `.json` files in the directory and builds an in-memory index
    /// of available packages and objects. The actual bytecode is loaded on-demand.
    pub fn new<P: AsRef<Path>>(cache_dir: P) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir)?;
        }

        let mut manager = Self {
            cache_dir,
            package_index: HashMap::new(),
            object_index: HashMap::new(),
            read_only: false,
        };

        // Build index from existing files
        manager.rebuild_index()?;

        Ok(manager)
    }

    /// Create a read-only cache (no writes allowed).
    pub fn read_only<P: AsRef<Path>>(cache_dir: P) -> Result<Self> {
        let mut manager = Self::new(cache_dir)?;
        manager.read_only = true;
        Ok(manager)
    }

    /// Create an empty cache (for testing or when no cache directory exists).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            cache_dir: PathBuf::new(),
            package_index: HashMap::new(),
            object_index: HashMap::new(),
            read_only: true,
        }
    }

    /// Rebuild the in-memory index from disk.
    fn rebuild_index(&mut self) -> Result<()> {
        self.package_index.clear();
        self.object_index.clear();

        if !self.cache_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }

            // Parse the file to extract package/object IDs
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(cached) = serde_json::from_str::<CachedTransactionData>(&content) {
                    self.index_transaction_data(&cached, &path);
                }
            }
        }

        Ok(())
    }

    /// Index a transaction's packages and objects.
    fn index_transaction_data(&mut self, data: &CachedTransactionData, source_file: &Path) {
        // Index packages
        for (pkg_id, modules) in &data.packages {
            let normalized = normalize_address(pkg_id);
            let version = data.package_versions.get(pkg_id).copied().unwrap_or(1);

            let module_names: Vec<String> = modules.iter().map(|(name, _)| name.clone()).collect();

            let entry = CachedPackageEntry::new(
                normalized.clone(),
                version,
                module_names,
                source_file.to_path_buf(),
            );

            // Keep the entry with highest version
            self.package_index
                .entry(normalized)
                .and_modify(|existing| {
                    if entry.version > existing.version {
                        *existing = entry.clone();
                    }
                })
                .or_insert(entry);
        }

        // Index objects
        for obj_id in data.objects.keys() {
            let normalized = normalize_address(obj_id);
            let version = data.object_versions.get(obj_id).copied().unwrap_or(0);
            let type_tag = data.object_types.get(obj_id).cloned();

            let entry = CachedObjectEntry::new(
                normalized.clone(),
                version,
                type_tag,
                source_file.to_path_buf(),
            );

            // Keep the entry with highest version
            self.object_index
                .entry(normalized)
                .and_modify(|existing| {
                    if entry.version > existing.version {
                        *existing = entry.clone();
                    }
                })
                .or_insert(entry);
        }
    }

    // ========== Read Operations ==========

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

    /// Get a package from the cache.
    ///
    /// Returns the package with all modules and bytecode, or None if not found.
    pub fn get_package(&self, package_id: &str) -> Result<Option<CachedPackage>> {
        use base64::Engine;

        let normalized = normalize_address(package_id);

        let entry = match self.package_index.get(&normalized) {
            Some(e) => e,
            None => return Ok(None),
        };

        // Load the source file
        let content = fs::read_to_string(&entry.source_file)?;
        let data: CachedTransactionData = serde_json::from_str(&content)?;

        // Find the package (try both normalized and original)
        for (pkg_id, modules) in &data.packages {
            if normalize_address(pkg_id) == normalized {
                let decoded_modules: Vec<(String, Vec<u8>)> = modules
                    .iter()
                    .filter_map(|(name, b64)| {
                        base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .ok()
                            .map(|bytes| (name.clone(), bytes))
                    })
                    .collect();

                return Ok(Some(CachedPackage {
                    address: normalized,
                    version: entry.version,
                    modules: decoded_modules,
                }));
            }
        }

        Ok(None)
    }

    /// Get an object from the cache.
    ///
    /// Returns the object with BCS bytes and metadata, or None if not found.
    pub fn get_object(&self, object_id: &str) -> Result<Option<CachedObject>> {
        use base64::Engine;

        let normalized = normalize_address(object_id);

        let entry = match self.object_index.get(&normalized) {
            Some(e) => e,
            None => return Ok(None),
        };

        // Load the source file
        let content = fs::read_to_string(&entry.source_file)?;
        let data: CachedTransactionData = serde_json::from_str(&content)?;

        // Find the object
        for (obj_id, b64) in &data.objects {
            if normalize_address(obj_id) == normalized {
                let bcs_bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| anyhow!("Failed to decode object {}: {}", object_id, e))?;

                return Ok(Some(CachedObject {
                    address: normalized,
                    version: entry.version,
                    type_tag: entry.type_tag.clone(),
                    bcs_bytes,
                    is_shared: entry.is_shared,
                    is_immutable: entry.is_immutable,
                }));
            }
        }

        Ok(None)
    }

    /// Get package entry metadata without loading bytecode.
    pub fn get_package_entry(&self, package_id: &str) -> Option<&CachedPackageEntry> {
        let normalized = normalize_address(package_id);
        self.package_index.get(&normalized)
    }

    /// Get object entry metadata without loading bytes.
    pub fn get_object_entry(&self, object_id: &str) -> Option<&CachedObjectEntry> {
        let normalized = normalize_address(object_id);
        self.object_index.get(&normalized)
    }

    // ========== Write Operations ==========

    /// Put a package into the cache.
    ///
    /// This creates a new cache file or updates an existing one.
    pub fn put_package(
        &mut self,
        package_id: &str,
        version: u64,
        modules: Vec<(String, Vec<u8>)>,
    ) -> Result<()> {
        if self.read_only {
            return Err(anyhow!("Cache is read-only"));
        }

        use base64::Engine;

        let normalized = normalize_address(package_id);

        // Check if we already have this version
        if let Some(existing) = self.package_index.get(&normalized) {
            if existing.version >= version {
                return Ok(()); // Already have same or newer version
            }
        }

        // Encode modules to base64
        let encoded_modules: Vec<(String, String)> = modules
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                )
            })
            .collect();

        // Create or load cache file
        let cache_file = self
            .cache_dir
            .join(format!("pkg_{}.json", &normalized[2..10]));

        let mut data = if cache_file.exists() {
            let content = fs::read_to_string(&cache_file)?;
            serde_json::from_str::<CachedTransactionData>(&content).unwrap_or_else(|_| {
                CachedTransactionData {
                    transaction: serde_json::Value::Null,
                    packages: HashMap::new(),
                    objects: HashMap::new(),
                    object_types: HashMap::new(),
                    object_versions: HashMap::new(),
                    package_versions: HashMap::new(),
                    cached_at: 0,
                }
            })
        } else {
            CachedTransactionData {
                transaction: serde_json::Value::Null,
                packages: HashMap::new(),
                objects: HashMap::new(),
                object_types: HashMap::new(),
                object_versions: HashMap::new(),
                package_versions: HashMap::new(),
                cached_at: 0,
            }
        };

        // Add the package
        let module_names: Vec<String> = encoded_modules.iter().map(|(n, _)| n.clone()).collect();
        data.packages.insert(normalized.clone(), encoded_modules);
        data.package_versions.insert(normalized.clone(), version);
        data.cached_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Save to disk
        let content = serde_json::to_string_pretty(&data)?;
        fs::write(&cache_file, content)?;

        // Update index
        self.package_index.insert(
            normalized.clone(),
            CachedPackageEntry::new(normalized, version, module_names, cache_file),
        );

        Ok(())
    }

    /// Put an object into the cache.
    pub fn put_object(
        &mut self,
        object_id: &str,
        version: u64,
        type_tag: Option<String>,
        bcs_bytes: Vec<u8>,
    ) -> Result<()> {
        if self.read_only {
            return Err(anyhow!("Cache is read-only"));
        }

        use base64::Engine;

        let normalized = normalize_address(object_id);

        // Check if we already have this version
        if let Some(existing) = self.object_index.get(&normalized) {
            if existing.version >= version {
                return Ok(()); // Already have same or newer version
            }
        }

        // Create or load cache file
        let cache_file = self
            .cache_dir
            .join(format!("obj_{}.json", &normalized[2..10]));

        let mut data = if cache_file.exists() {
            let content = fs::read_to_string(&cache_file)?;
            serde_json::from_str::<CachedTransactionData>(&content).unwrap_or_else(|_| {
                CachedTransactionData {
                    transaction: serde_json::Value::Null,
                    packages: HashMap::new(),
                    objects: HashMap::new(),
                    object_types: HashMap::new(),
                    object_versions: HashMap::new(),
                    package_versions: HashMap::new(),
                    cached_at: 0,
                }
            })
        } else {
            CachedTransactionData {
                transaction: serde_json::Value::Null,
                packages: HashMap::new(),
                objects: HashMap::new(),
                object_types: HashMap::new(),
                object_versions: HashMap::new(),
                package_versions: HashMap::new(),
                cached_at: 0,
            }
        };

        // Add the object
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bcs_bytes);
        data.objects.insert(normalized.clone(), encoded);
        data.object_versions.insert(normalized.clone(), version);
        if let Some(ref type_str) = type_tag {
            data.object_types
                .insert(normalized.clone(), type_str.clone());
        }
        data.cached_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Save to disk
        let content = serde_json::to_string_pretty(&data)?;
        fs::write(&cache_file, content)?;

        // Update index
        self.object_index.insert(
            normalized.clone(),
            CachedObjectEntry::new(normalized, version, type_tag, cache_file),
        );

        Ok(())
    }

    // ========== Statistics ==========

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        let disk_size = self.calculate_disk_size();

        CacheStats {
            package_count: self.package_index.len(),
            object_count: self.object_index.len(),
            transaction_count: self.count_transactions(),
            disk_size_bytes: disk_size,
        }
    }

    fn calculate_disk_size(&self) -> u64 {
        if !self.cache_dir.exists() {
            return 0;
        }

        fs::read_dir(&self.cache_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    fn count_transactions(&self) -> usize {
        if !self.cache_dir.exists() {
            return 0;
        }

        fs::read_dir(&self.cache_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .map(|ext| ext == "json")
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Get the number of indexed packages.
    pub fn package_count(&self) -> usize {
        self.package_index.len()
    }

    /// Get the number of indexed objects.
    pub fn object_count(&self) -> usize {
        self.object_index.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.package_index.is_empty() && self.object_index.is_empty()
    }

    /// List all indexed package IDs.
    pub fn list_packages(&self) -> Vec<&str> {
        self.package_index.keys().map(|s| s.as_str()).collect()
    }

    /// List all indexed object IDs.
    pub fn list_objects(&self) -> Vec<&str> {
        self.object_index.keys().map(|s| s.as_str()).collect()
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Create a writable copy (for testing).
    #[cfg(test)]
    fn clone_for_write(&self) -> Self {
        Self {
            cache_dir: self.cache_dir.clone(),
            package_index: self.package_index.clone(),
            object_index: self.object_index.clone(),
            read_only: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_empty_cache() {
        let cache = CacheManager::empty();
        assert!(cache.is_empty());
        assert_eq!(cache.package_count(), 0);
        assert_eq!(cache.object_count(), 0);
    }

    #[test]
    fn test_put_and_get_package() {
        let temp_dir = TempDir::new().unwrap();
        let mut cache = CacheManager::new(temp_dir.path()).unwrap();

        let modules = vec![
            ("test_module".to_string(), vec![0u8, 1, 2, 3]),
            ("another_module".to_string(), vec![4, 5, 6, 7]),
        ];

        cache
            .put_package("0x123", 1, modules.clone())
            .expect("put_package should succeed");

        assert!(cache.has_package("0x123"));

        let pkg = cache
            .get_package("0x123")
            .expect("get_package should succeed")
            .expect("package should exist");

        assert_eq!(pkg.version, 1);
        assert_eq!(pkg.modules.len(), 2);
        assert_eq!(pkg.modules[0].0, "test_module");
        assert_eq!(pkg.modules[0].1, vec![0u8, 1, 2, 3]);
    }

    #[test]
    fn test_put_and_get_object() {
        let temp_dir = TempDir::new().unwrap();
        let mut cache = CacheManager::new(temp_dir.path()).unwrap();

        let bcs_bytes = vec![10u8, 20, 30, 40];
        let type_tag = Some("0x2::coin::Coin<0x2::sui::SUI>".to_string());

        cache
            .put_object("0xabc", 5, type_tag.clone(), bcs_bytes.clone())
            .expect("put_object should succeed");

        assert!(cache.has_object("0xabc"));

        let obj = cache
            .get_object("0xabc")
            .expect("get_object should succeed")
            .expect("object should exist");

        assert_eq!(obj.version, 5);
        assert_eq!(obj.type_tag, type_tag);
        assert_eq!(obj.bcs_bytes, bcs_bytes);
    }

    #[test]
    fn test_address_normalization() {
        let temp_dir = TempDir::new().unwrap();
        let mut cache = CacheManager::new(temp_dir.path()).unwrap();

        let modules = vec![("m".to_string(), vec![1u8])];

        // Put with short address
        cache.put_package("0x2", 1, modules).unwrap();

        // Should find with various formats
        assert!(cache.has_package("0x2"));
        assert!(cache.has_package("2"));
        assert!(
            cache.has_package("0x0000000000000000000000000000000000000000000000000000000000000002")
        );
    }

    #[test]
    fn test_version_tracking() {
        let temp_dir = TempDir::new().unwrap();
        let mut cache = CacheManager::new(temp_dir.path()).unwrap();

        // Put version 1
        cache
            .put_package("0x100", 1, vec![("v1".to_string(), vec![1u8])])
            .unwrap();

        // Put version 2
        cache
            .put_package("0x100", 2, vec![("v2".to_string(), vec![2u8])])
            .unwrap();

        let pkg = cache.get_package("0x100").unwrap().unwrap();
        assert_eq!(pkg.version, 2);
        assert_eq!(pkg.modules[0].0, "v2");

        // Try to put version 1 again (should be ignored)
        cache
            .put_package("0x100", 1, vec![("old".to_string(), vec![0u8])])
            .unwrap();

        let pkg = cache.get_package("0x100").unwrap().unwrap();
        assert_eq!(pkg.version, 2); // Still version 2
    }

    #[test]
    fn test_read_only_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache = CacheManager::read_only(temp_dir.path()).unwrap();

        let _result = cache.clone_for_write().put_package("0x1", 1, vec![]);
        // Can't actually clone_for_write, so let's test differently

        // Create read-only directly
        let mut ro_cache = CacheManager::new(temp_dir.path()).unwrap();
        ro_cache.read_only = true;

        let result = ro_cache.put_package("0x1", 1, vec![]);
        assert!(result.is_err());
    }
}
