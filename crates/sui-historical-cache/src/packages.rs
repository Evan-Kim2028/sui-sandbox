//! Filesystem-backed package store (gRPC miss-fill cache).

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::paths::{atomic_write_json, package_path};

/// A cached package (modules + metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPackage {
    /// Package version (if known from gRPC)
    pub version: u64,
    /// Modules: (name, base64-encoded bytecode)
    pub modules: Vec<(String, String)>,
    /// Original package ID (for upgraded packages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_id: Option<String>,
    /// Package linkage (for dependency resolution)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linkage: Option<Vec<LinkageEntry>>,
}

/// A linkage entry (original -> upgraded mapping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkageEntry {
    pub original_id: String,
    pub upgraded_id: String,
    pub upgraded_version: u64,
}

/// Trait for package stores.
pub trait PackageStore: Send + Sync {
    /// Get a package by storage address.
    fn get(&self, id: AccountAddress) -> Result<Option<CachedPackage>>;

    /// Store a package.
    fn put(&self, id: AccountAddress, pkg: &CachedPackage) -> Result<()>;

    /// Check if a package exists (without loading).
    fn has(&self, id: AccountAddress) -> bool {
        self.get(id).map(|opt| opt.is_some()).unwrap_or(false)
    }
}

/// Filesystem-backed package store with sharded directory layout.
pub struct FsPackageStore {
    cache_root: Arc<Path>,
}

impl FsPackageStore {
    /// Create a new filesystem package store.
    pub fn new<P: AsRef<Path>>(cache_root: P) -> Result<Self> {
        let cache_root = cache_root.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache_root)
            .map_err(|e| anyhow!("Failed to create cache root {}: {}", cache_root.display(), e))?;
        Ok(Self {
            cache_root: Arc::from(cache_root),
        })
    }

    /// Get the cache root path.
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }
}

impl PackageStore for FsPackageStore {
    fn get(&self, id: AccountAddress) -> Result<Option<CachedPackage>> {
        let pkg_path = package_path(&self.cache_root, &id);

        if !pkg_path.exists() {
            return Ok(None);
        }

        let json = std::fs::read_to_string(&pkg_path)
            .map_err(|e| anyhow!("Failed to read package file {}: {}", pkg_path.display(), e))?;
        let pkg: CachedPackage = serde_json::from_str(&json)
            .map_err(|e| anyhow!("Failed to parse package JSON: {}", e))?;

        Ok(Some(pkg))
    }

    fn put(&self, id: AccountAddress, pkg: &CachedPackage) -> Result<()> {
        let pkg_path = package_path(&self.cache_root, &id);

        // Skip if already exists and version is same or newer (idempotent with version check)
        if pkg_path.exists() {
            if let Ok(Some(existing)) = self.get(id) {
                if existing.version >= pkg.version {
                    return Ok(());
                }
            }
        }

        // Write atomically
        atomic_write_json(&pkg_path, pkg)?;

        Ok(())
    }
}

impl CachedPackage {
    /// Decode modules from base64 to bytes.
    pub fn decode_modules(&self) -> Result<Vec<(String, Vec<u8>)>> {
        use base64::Engine;
        self.modules
            .iter()
            .map(|(name, b64)| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| anyhow!("Failed to decode module {}: {}", name, e))?;
                Ok((name.clone(), bytes))
            })
            .collect()
    }

    /// Create from module bytes (encode to base64 for storage).
    pub fn from_modules(version: u64, modules: Vec<(String, Vec<u8>)>) -> Self {
        use base64::Engine;
        let encoded_modules: Vec<(String, String)> = modules
            .into_iter()
            .map(|(name, bytes)| {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                (name, b64)
            })
            .collect();
        Self {
            version,
            modules: encoded_modules,
            original_id: None,
            linkage: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_put_and_get() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = FsPackageStore::new(temp_dir.path())?;

        let id = AccountAddress::from_hex_literal("0xabc")?;
        let modules = vec![
            ("test_module".to_string(), vec![1, 2, 3, 4]),
            ("another_module".to_string(), vec![5, 6, 7, 8]),
        ];
        let pkg = CachedPackage::from_modules(1, modules);

        // Put
        store.put(id, &pkg)?;

        // Get
        let cached = store.get(id)?.expect("package should exist");
        assert_eq!(cached.version, 1);
        let decoded = cached.decode_modules()?;
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, "test_module");
        assert_eq!(decoded[0].1, vec![1, 2, 3, 4]);

        // Has
        assert!(store.has(id));

        Ok(())
    }

    #[test]
    fn test_version_check() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = FsPackageStore::new(temp_dir.path())?;

        let id = AccountAddress::from_hex_literal("0xdef")?;
        let pkg_v1 = CachedPackage::from_modules(1, vec![("m".to_string(), vec![1])]);
        let pkg_v2 = CachedPackage::from_modules(2, vec![("m".to_string(), vec![2])]);

        // Put v1
        store.put(id, &pkg_v1)?;

        // Try to put v1 again (should skip)
        store.put(id, &pkg_v1)?;
        let cached = store.get(id)?.expect("package should exist");
        assert_eq!(cached.version, 1);

        // Put v2 (should update)
        store.put(id, &pkg_v2)?;
        let cached = store.get(id)?.expect("package should exist");
        assert_eq!(cached.version, 2);

        // Try to put v1 again (should skip)
        store.put(id, &pkg_v1)?;
        let cached = store.get(id)?.expect("package should exist");
        assert_eq!(cached.version, 2); // Still v2

        Ok(())
    }
}
