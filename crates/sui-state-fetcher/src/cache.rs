//! Versioned cache for historical state.
//!
//! This cache is keyed by `(object_id, version)` to support historical replay.
//! Unlike a simple object cache, this allows storing multiple versions of the
//! same object, which is essential for replaying transactions at different points
//! in history.
//!
//! # Example
//!
//! ```ignore
//! use sui_state_fetcher::cache::VersionedCache;
//!
//! let mut cache = VersionedCache::new();
//!
//! // Store object at version 5
//! cache.put_object(obj_v5);
//!
//! // Store same object at version 10
//! cache.put_object(obj_v10);
//!
//! // Retrieve specific versions
//! let v5 = cache.get_object(&id, 5);  // Gets version 5
//! let v10 = cache.get_object(&id, 10); // Gets version 10
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use parking_lot::RwLock;

use crate::types::{ObjectID, PackageData, VersionedObject};

/// In-memory cache keyed by (object_id, version).
///
/// Thread-safe via internal RwLock. Can optionally persist to disk.
#[derive(Debug)]
pub struct VersionedCache {
    /// Objects: (object_id, version) -> VersionedObject
    objects: RwLock<HashMap<(ObjectID, u64), VersionedObject>>,

    /// Packages: (package_id, version) -> PackageData
    /// Note: packages are immutable, but upgrades create new versions at new addresses.
    packages: RwLock<HashMap<(AccountAddress, u64), PackageData>>,

    /// Optional persistence directory.
    storage_dir: Option<PathBuf>,
}

impl Default for VersionedCache {
    fn default() -> Self {
        Self::new()
    }
}

impl VersionedCache {
    /// Create a new in-memory cache (no persistence).
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            packages: RwLock::new(HashMap::new()),
            storage_dir: None,
        }
    }

    /// Create a cache with disk persistence.
    ///
    /// Existing cached data will be loaded from the directory.
    pub fn with_storage(storage_dir: impl AsRef<Path>) -> Result<Self> {
        let storage_dir = storage_dir.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        if !storage_dir.exists() {
            fs::create_dir_all(&storage_dir)?;
        }

        let mut cache = Self {
            objects: RwLock::new(HashMap::new()),
            packages: RwLock::new(HashMap::new()),
            storage_dir: Some(storage_dir.clone()),
        };

        // Load existing cached data
        cache.load_from_disk()?;

        Ok(cache)
    }

    // ==================== Object Operations ====================

    /// Get an object at a specific version.
    pub fn get_object(&self, id: &ObjectID, version: u64) -> Option<VersionedObject> {
        self.objects.read().get(&(*id, version)).cloned()
    }

    /// Get the latest cached version of an object.
    ///
    /// This is useful when you need any version of an object but don't
    /// care about the specific version (e.g., for packages that haven't upgraded).
    pub fn get_object_latest(&self, id: &ObjectID) -> Option<VersionedObject> {
        let objects = self.objects.read();
        objects
            .iter()
            .filter(|((obj_id, _), _)| obj_id == id)
            .max_by_key(|((_, version), _)| *version)
            .map(|(_, obj)| obj.clone())
    }

    /// Check if an object at a specific version is cached.
    pub fn has_object(&self, id: &ObjectID, version: u64) -> bool {
        self.objects.read().contains_key(&(*id, version))
    }

    /// Store an object in the cache.
    pub fn put_object(&self, obj: VersionedObject) {
        let key = obj.cache_key();
        self.objects.write().insert(key, obj);

        // Persist to disk if storage is enabled
        if self.storage_dir.is_some() {
            // Note: We batch writes for efficiency. See flush().
        }
    }

    /// Store multiple objects at once.
    pub fn put_objects(&self, objects: impl IntoIterator<Item = VersionedObject>) {
        let mut cache = self.objects.write();
        for obj in objects {
            let key = obj.cache_key();
            cache.insert(key, obj);
        }
    }

    /// Get all cached versions for an object.
    pub fn get_object_versions(&self, id: &ObjectID) -> Vec<u64> {
        self.objects
            .read()
            .keys()
            .filter_map(
                |(obj_id, version)| {
                    if obj_id == id {
                        Some(*version)
                    } else {
                        None
                    }
                },
            )
            .collect()
    }

    // ==================== Package Operations ====================

    /// Get a package at a specific version.
    pub fn get_package(&self, id: &AccountAddress, version: u64) -> Option<PackageData> {
        self.packages.read().get(&(*id, version)).cloned()
    }

    /// Get the latest cached version of a package.
    pub fn get_package_latest(&self, id: &AccountAddress) -> Option<PackageData> {
        let packages = self.packages.read();
        packages
            .iter()
            .filter(|((pkg_id, _), _)| pkg_id == id)
            .max_by_key(|((_, version), _)| *version)
            .map(|(_, pkg)| pkg.clone())
    }

    /// Check if a package at a specific version is cached.
    pub fn has_package(&self, id: &AccountAddress, version: u64) -> bool {
        self.packages.read().contains_key(&(*id, version))
    }

    /// Store a package in the cache.
    pub fn put_package(&self, pkg: PackageData) {
        let key = (pkg.address, pkg.version);
        self.packages.write().insert(key, pkg);
    }

    /// Store multiple packages at once.
    pub fn put_packages(&self, packages: impl IntoIterator<Item = PackageData>) {
        let mut cache = self.packages.write();
        for pkg in packages {
            let key = (pkg.address, pkg.version);
            cache.insert(key, pkg);
        }
    }

    // ==================== Cache Statistics ====================

    /// Get the number of cached objects.
    pub fn object_count(&self) -> usize {
        self.objects.read().len()
    }

    /// Get the number of cached packages.
    pub fn package_count(&self) -> usize {
        self.packages.read().len()
    }

    /// Get the number of unique object IDs (ignoring versions).
    pub fn unique_object_count(&self) -> usize {
        let objects = self.objects.read();
        let unique: std::collections::HashSet<_> = objects.keys().map(|(id, _)| id).collect();
        unique.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.read().is_empty() && self.packages.read().is_empty()
    }

    /// Clear all cached data.
    pub fn clear(&self) {
        self.objects.write().clear();
        self.packages.write().clear();
    }

    // ==================== Persistence ====================

    /// Flush cached data to disk (if storage is enabled).
    pub fn flush(&self) -> Result<()> {
        let Some(ref storage_dir) = self.storage_dir else {
            return Ok(());
        };

        // Write objects index
        let objects_file = storage_dir.join("objects.json");
        let objects = self.objects.read();
        let objects_data: Vec<_> = objects.values().collect();
        let json = serde_json::to_string_pretty(&objects_data)?;
        fs::write(&objects_file, json)?;

        // Write packages index
        let packages_file = storage_dir.join("packages.json");
        let packages = self.packages.read();
        let packages_data: Vec<_> = packages.values().collect();
        let json = serde_json::to_string_pretty(&packages_data)?;
        fs::write(&packages_file, json)?;

        Ok(())
    }

    /// Load cached data from disk.
    fn load_from_disk(&mut self) -> Result<()> {
        let Some(ref storage_dir) = self.storage_dir else {
            return Ok(());
        };

        // Load objects
        let objects_file = storage_dir.join("objects.json");
        if objects_file.exists() {
            let json = fs::read_to_string(&objects_file)?;
            let objects_data: Vec<VersionedObject> = serde_json::from_str(&json)?;
            let mut objects = self.objects.write();
            for obj in objects_data {
                let key = obj.cache_key();
                objects.insert(key, obj);
            }
        }

        // Load packages
        let packages_file = storage_dir.join("packages.json");
        if packages_file.exists() {
            let json = fs::read_to_string(&packages_file)?;
            let packages_data: Vec<PackageData> = serde_json::from_str(&json)?;
            let mut packages = self.packages.write();
            for pkg in packages_data {
                let key = (pkg.address, pkg.version);
                packages.insert(key, pkg);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_object(id_suffix: u8, version: u64) -> VersionedObject {
        let mut addr_bytes = [0u8; 32];
        addr_bytes[31] = id_suffix;
        VersionedObject {
            id: AccountAddress::new(addr_bytes),
            version,
            digest: None,
            type_tag: Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()),
            bcs_bytes: vec![1, 2, 3, version as u8],
            is_shared: false,
            is_immutable: false,
        }
    }

    #[test]
    fn test_versioned_cache_same_object_different_versions() {
        let cache = VersionedCache::new();

        let obj_v1 = test_object(1, 1);
        let obj_v2 = test_object(1, 2);
        let id = obj_v1.id;

        cache.put_object(obj_v1.clone());
        cache.put_object(obj_v2.clone());

        // Both versions should be retrievable
        let retrieved_v1 = cache.get_object(&id, 1).unwrap();
        let retrieved_v2 = cache.get_object(&id, 2).unwrap();

        assert_eq!(retrieved_v1.version, 1);
        assert_eq!(retrieved_v1.bcs_bytes, vec![1, 2, 3, 1]);

        assert_eq!(retrieved_v2.version, 2);
        assert_eq!(retrieved_v2.bcs_bytes, vec![1, 2, 3, 2]);

        // Latest should return v2
        let latest = cache.get_object_latest(&id).unwrap();
        assert_eq!(latest.version, 2);
    }

    #[test]
    fn test_cache_miss_returns_none() {
        let cache = VersionedCache::new();
        let id = AccountAddress::new([0u8; 32]);

        assert!(cache.get_object(&id, 1).is_none());
        assert!(cache.get_object_latest(&id).is_none());
    }

    #[test]
    fn test_has_object() {
        let cache = VersionedCache::new();
        let obj = test_object(1, 5);
        let id = obj.id;

        assert!(!cache.has_object(&id, 5));
        cache.put_object(obj);
        assert!(cache.has_object(&id, 5));
        assert!(!cache.has_object(&id, 6)); // Different version
    }

    #[test]
    fn test_get_object_versions() {
        let cache = VersionedCache::new();
        let id = test_object(1, 1).id;

        cache.put_object(test_object(1, 1));
        cache.put_object(test_object(1, 5));
        cache.put_object(test_object(1, 10));
        cache.put_object(test_object(2, 1)); // Different object

        let mut versions = cache.get_object_versions(&id);
        versions.sort();
        assert_eq!(versions, vec![1, 5, 10]);
    }

    #[test]
    fn test_put_objects_batch() {
        let cache = VersionedCache::new();

        let objects = vec![test_object(1, 1), test_object(1, 2), test_object(2, 1)];

        cache.put_objects(objects);

        assert_eq!(cache.object_count(), 3);
        assert_eq!(cache.unique_object_count(), 2);
    }

    #[test]
    fn test_cache_statistics() {
        let cache = VersionedCache::new();

        assert!(cache.is_empty());
        assert_eq!(cache.object_count(), 0);

        cache.put_object(test_object(1, 1));
        cache.put_object(test_object(1, 2));

        assert!(!cache.is_empty());
        assert_eq!(cache.object_count(), 2);
        assert_eq!(cache.unique_object_count(), 1);
    }

    #[test]
    fn test_clear() {
        let cache = VersionedCache::new();

        cache.put_object(test_object(1, 1));
        cache.put_object(test_object(2, 1));
        assert_eq!(cache.object_count(), 2);

        cache.clear();
        assert!(cache.is_empty());
    }
}
