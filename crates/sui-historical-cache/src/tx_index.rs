//! Filesystem-backed tx digest -> checkpoint index.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::paths::{ensure_parent_dirs, tx_digest_index_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxDigestIndexEntry {
    pub digest: String,
    pub checkpoint: u64,
}

pub struct FsTxDigestIndex {
    cache_root: Arc<Path>,
}

impl FsTxDigestIndex {
    pub fn new<P: AsRef<Path>>(cache_root: P) -> Result<Self> {
        let cache_root = cache_root.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache_root).map_err(|e| {
            anyhow!(
                "Failed to create cache root {}: {}",
                cache_root.display(),
                e
            )
        })?;
        Ok(Self {
            cache_root: Arc::from(cache_root),
        })
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn put(&self, digest: &str, checkpoint: u64) -> Result<()> {
        let path = tx_digest_index_path(&self.cache_root, digest);
        if path.exists() {
            return Ok(());
        }
        ensure_parent_dirs(&path)?;
        let entry = TxDigestIndexEntry {
            digest: digest.to_string(),
            checkpoint,
        };
        let json = serde_json::to_vec(&entry)
            .map_err(|e| anyhow!("Failed to serialize tx index entry: {}", e))?;
        std::fs::write(&path, json)
            .map_err(|e| anyhow!("Failed to write tx index file {}: {}", path.display(), e))?;
        Ok(())
    }

    pub fn get_checkpoint(&self, digest: &str) -> Result<Option<u64>> {
        let path = tx_digest_index_path(&self.cache_root, digest);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| anyhow!("Failed to read tx index file {}: {}", path.display(), e))?;
        let entry: TxDigestIndexEntry = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow!("Failed to parse tx index entry: {}", e))?;
        Ok(Some(entry.checkpoint))
    }
}
