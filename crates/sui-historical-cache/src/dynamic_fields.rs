//! Filesystem-backed dynamic field cache.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;

use crate::paths::{dynamic_field_cache_path, ensure_parent_dirs};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFieldEntry {
    pub checkpoint: u64,
    pub parent_id: String,
    pub child_id: String,
    pub version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_tx: Option<String>,
}

pub struct FsDynamicFieldCache {
    cache_root: Arc<Path>,
}

impl FsDynamicFieldCache {
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

    pub fn put_entry(&self, entry: DynamicFieldEntry) -> Result<()> {
        let parent = AccountAddress::from_hex_literal(&entry.parent_id)
            .map_err(|e| anyhow!("Invalid parent id {}: {}", entry.parent_id, e))?;
        let path = dynamic_field_cache_path(&self.cache_root, &parent);
        ensure_parent_dirs(&path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| anyhow!("Failed to open dynamic field cache {}: {}", path.display(), e))?;

        let line = serde_json::to_string(&entry)
            .map_err(|e| anyhow!("Failed to serialize dynamic field entry: {}", e))?;
        writeln!(file, "{}", line)
            .map_err(|e| anyhow!("Failed to write dynamic field entry: {}", e))?;
        Ok(())
    }

    pub fn get_children(
        &self,
        parent: AccountAddress,
        checkpoint: u64,
    ) -> Result<Vec<DynamicFieldEntry>> {
        let path = dynamic_field_cache_path(&self.cache_root, &parent);
        if !path.exists() {
            return Ok(vec![]);
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| anyhow!("Failed to open dynamic field cache {}: {}", path.display(), e))?;
        let reader = BufReader::new(file);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: DynamicFieldEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.checkpoint == checkpoint {
                out.push(entry);
            }
        }
        Ok(out)
    }

    /// Get children for the latest checkpoint <= requested checkpoint.
    pub fn get_children_at_or_before(
        &self,
        parent: AccountAddress,
        checkpoint: u64,
    ) -> Result<Vec<DynamicFieldEntry>> {
        let path = dynamic_field_cache_path(&self.cache_root, &parent);
        if !path.exists() {
            return Ok(vec![]);
        }
        let file = std::fs::File::open(&path)
            .map_err(|e| anyhow!("Failed to open dynamic field cache {}: {}", path.display(), e))?;
        let reader = BufReader::new(file);
        let mut latest: std::collections::HashMap<String, DynamicFieldEntry> =
            std::collections::HashMap::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let entry: DynamicFieldEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.checkpoint <= checkpoint {
                let key = entry.child_id.clone();
                let replace = match latest.get(&key) {
                    Some(existing) => entry.checkpoint >= existing.checkpoint,
                    None => true,
                };
                if replace {
                    latest.insert(key, entry);
                }
            }
        }
        Ok(latest.into_values().collect())
    }
}
