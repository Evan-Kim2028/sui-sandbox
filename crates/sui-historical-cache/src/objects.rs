//! Filesystem-backed object version store.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::paths::{atomic_write, atomic_write_json, object_bcs_path, object_meta_path};

/// Metadata for a cached object (stored separately from BCS bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    /// Type tag string (e.g., "0x2::coin::Coin<0x2::sui::SUI>")
    pub type_tag: String,
    /// Owner kind (best-effort, for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_kind: Option<String>,
    /// Source checkpoint where this (id, version) was observed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_checkpoint: Option<u64>,
}

/// A cached object (BCS bytes + metadata).
#[derive(Debug, Clone)]
pub struct CachedObject {
    pub bcs_bytes: Vec<u8>,
    pub meta: ObjectMeta,
}

/// Trait for object version stores.
pub trait ObjectVersionStore: Send + Sync {
    /// Get an object at a specific version.
    fn get(&self, id: AccountAddress, version: u64) -> Result<Option<CachedObject>>;

    /// Store an object at a specific version.
    fn put(&self, id: AccountAddress, version: u64, bcs: &[u8], meta: &ObjectMeta) -> Result<()>;

    /// Check if an object at a specific version exists (without loading bytes).
    fn has(&self, id: AccountAddress, version: u64) -> bool {
        self.get(id, version).map(|opt| opt.is_some()).unwrap_or(false)
    }
}

/// Filesystem-backed object version store with sharded directory layout.
pub struct FsObjectStore {
    cache_root: Arc<Path>,
}

impl FsObjectStore {
    /// Create a new filesystem object store.
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

impl ObjectVersionStore for FsObjectStore {
    fn get(&self, id: AccountAddress, version: u64) -> Result<Option<CachedObject>> {
        let bcs_path = object_bcs_path(&self.cache_root, &id, version);
        let meta_path = object_meta_path(&self.cache_root, &id, version);

        // Check if both files exist
        if !bcs_path.exists() || !meta_path.exists() {
            return Ok(None);
        }

        // Load BCS bytes
        let bcs_bytes = std::fs::read(&bcs_path)
            .map_err(|e| anyhow!("Failed to read BCS file {}: {}", bcs_path.display(), e))?;

        // Load metadata
        let meta_json = std::fs::read_to_string(&meta_path)
            .map_err(|e| anyhow!("Failed to read metadata file {}: {}", meta_path.display(), e))?;
        let meta: ObjectMeta = serde_json::from_str(&meta_json)
            .map_err(|e| anyhow!("Failed to parse metadata JSON: {}", e))?;

        Ok(Some(CachedObject { bcs_bytes, meta }))
    }

    fn put(&self, id: AccountAddress, version: u64, bcs: &[u8], meta: &ObjectMeta) -> Result<()> {
        let bcs_path = object_bcs_path(&self.cache_root, &id, version);
        let meta_path = object_meta_path(&self.cache_root, &id, version);

        // Skip if already exists (idempotent)
        if bcs_path.exists() && meta_path.exists() {
            return Ok(());
        }

        // Write BCS bytes atomically
        atomic_write(&bcs_path, bcs)?;

        // Write metadata atomically
        atomic_write_json(&meta_path, meta)?;

        Ok(())
    }

    fn has(&self, id: AccountAddress, version: u64) -> bool {
        let bcs_path = object_bcs_path(&self.cache_root, &id, version);
        let meta_path = object_meta_path(&self.cache_root, &id, version);
        bcs_path.exists() && meta_path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_put_and_get() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = FsObjectStore::new(temp_dir.path())?;

        let id = AccountAddress::from_hex_literal("0x123")?;
        let version = 100;
        let bcs_bytes = vec![1, 2, 3, 4, 5];
        let meta = ObjectMeta {
            type_tag: "0x2::coin::Coin<0x2::sui::SUI>".to_string(),
            owner_kind: Some("address".to_string()),
            source_checkpoint: Some(12345),
        };

        // Put
        store.put(id, version, &bcs_bytes, &meta)?;

        // Get
        let cached = store.get(id, version)?.expect("object should exist");
        assert_eq!(cached.bcs_bytes, bcs_bytes);
        assert_eq!(cached.meta.type_tag, meta.type_tag);
        assert_eq!(cached.meta.source_checkpoint, meta.source_checkpoint);

        // Has
        assert!(store.has(id, version));
        assert!(!store.has(id, version + 1));

        Ok(())
    }

    #[test]
    fn test_idempotent_put() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let store = FsObjectStore::new(temp_dir.path())?;

        let id = AccountAddress::from_hex_literal("0x456")?;
        let version = 200;
        let bcs_bytes = vec![10, 20, 30];
        let meta = ObjectMeta {
            type_tag: "0x2::clock::Clock".to_string(),
            owner_kind: None,
            source_checkpoint: None,
        };

        // Put twice
        store.put(id, version, &bcs_bytes, &meta)?;
        store.put(id, version, &bcs_bytes, &meta)?; // Should not error

        // Should still be retrievable
        let cached = store.get(id, version)?.expect("object should exist");
        assert_eq!(cached.bcs_bytes, bcs_bytes);

        Ok(())
    }
}
