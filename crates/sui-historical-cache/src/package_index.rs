//! Filesystem-backed index for mapping package versions to checkpoints.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;

use crate::paths::{ensure_parent_dirs, package_index_path};

/// Index entry mapping a package version to a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageIndexEntry {
    pub version: u64,
    pub checkpoint: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_digest: Option<String>,
}

/// Filesystem-backed package version index (append-only JSONL).
pub struct FsPackageIndex {
    cache_root: Arc<Path>,
}

impl FsPackageIndex {
    /// Create a new filesystem index rooted at cache_root.
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

    /// Get the cache root path.
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Append an index entry for (package_id, version).
    pub fn put(
        &self,
        id: AccountAddress,
        version: u64,
        checkpoint: u64,
        tx_digest: Option<String>,
    ) -> Result<()> {
        let path = package_index_path(&self.cache_root, &id);
        ensure_parent_dirs(&path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| anyhow!("Failed to open index file {}: {}", path.display(), e))?;

        let entry = PackageIndexEntry {
            version,
            checkpoint,
            tx_digest,
        };
        let line = serde_json::to_string(&entry)
            .map_err(|e| anyhow!("Failed to serialize index entry: {}", e))?;
        writeln!(file, "{}", line)
            .map_err(|e| anyhow!("Failed to write index entry: {}", e))?;
        Ok(())
    }

    /// Find the checkpoint for a package version.
    pub fn get_checkpoint(&self, id: AccountAddress, version: u64) -> Result<Option<u64>> {
        let path = package_index_path(&self.cache_root, &id);
        if !path.exists() {
            return Ok(None);
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| anyhow!("Failed to open index file {}: {}", path.display(), e))?;
        let reader = BufReader::new(file);
        let mut found = None;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: PackageIndexEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.version == version {
                found = Some(entry.checkpoint);
            }
        }
        Ok(found)
    }

    /// Find the most recent entry for a package version.
    pub fn get_entry(&self, id: AccountAddress, version: u64) -> Result<Option<PackageIndexEntry>> {
        let path = package_index_path(&self.cache_root, &id);
        if !path.exists() {
            return Ok(None);
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| anyhow!("Failed to open index file {}: {}", path.display(), e))?;
        let reader = BufReader::new(file);
        let mut found: Option<PackageIndexEntry> = None;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: PackageIndexEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.version == version {
                found = Some(entry);
            }
        }
        Ok(found)
    }

    /// Get the latest entry for a package (last line in the JSONL file).
    pub fn get_latest(&self, id: AccountAddress) -> Result<Option<PackageIndexEntry>> {
        let path = package_index_path(&self.cache_root, &id);
        if !path.exists() {
            return Ok(None);
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| anyhow!("Failed to open index file {}: {}", path.display(), e))?;
        let reader = BufReader::new(file);
        let mut last: Option<PackageIndexEntry> = None;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: PackageIndexEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            last = Some(entry);
        }
        Ok(last)
    }

    /// Get the latest entry for a package at or before the given checkpoint.
    ///
    /// This scans all entries and returns the entry with the highest checkpoint
    /// less than or equal to `checkpoint`. If multiple entries share the same
    /// checkpoint, the highest version is returned.
    pub fn get_at_or_before_checkpoint(
        &self,
        id: AccountAddress,
        checkpoint: u64,
    ) -> Result<Option<PackageIndexEntry>> {
        let path = package_index_path(&self.cache_root, &id);
        if !path.exists() {
            return Ok(None);
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| anyhow!("Failed to open index file {}: {}", path.display(), e))?;
        let reader = BufReader::new(file);
        let mut best: Option<PackageIndexEntry> = None;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: PackageIndexEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.checkpoint > checkpoint {
                continue;
            }
            match &best {
                Some(current) => {
                    if entry.checkpoint > current.checkpoint
                        || (entry.checkpoint == current.checkpoint && entry.version > current.version)
                    {
                        best = Some(entry);
                    }
                }
                None => best = Some(entry),
            }
        }
        Ok(best)
    }
}
