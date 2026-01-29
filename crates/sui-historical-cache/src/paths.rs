//! Path utilities for sharded filesystem layout.

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use std::path::{Path, PathBuf};

/// Normalize an AccountAddress to a 64-character lowercase hex string (no 0x prefix).
pub fn normalize_object_id(id: &AccountAddress) -> String {
    let hex = hex::encode(id.as_ref());
    // Ensure it's exactly 64 chars (pad with zeros if needed, though AccountAddress should always be 32 bytes)
    format!("{:0>64}", hex)
}

/// Get the shard path components (aa/bb) from an object ID.
pub fn object_shard_path(id: &AccountAddress) -> (String, String) {
    let normalized = normalize_object_id(id);
    let aa = normalized[0..2].to_string();
    let bb = normalized[2..4].to_string();
    (aa, bb)
}

/// Get the full filesystem path for an object's BCS file.
pub fn object_bcs_path(cache_root: &Path, id: &AccountAddress, version: u64) -> PathBuf {
    let (aa, bb) = object_shard_path(id);
    let normalized_id = normalize_object_id(id);
    cache_root
        .join("objects")
        .join(&aa)
        .join(&bb)
        .join(&normalized_id)
        .join(format!("{}.bcs", version))
}

/// Get the full filesystem path for an object's metadata file.
pub fn object_meta_path(cache_root: &Path, id: &AccountAddress, version: u64) -> PathBuf {
    let (aa, bb) = object_shard_path(id);
    let normalized_id = normalize_object_id(id);
    cache_root
        .join("objects")
        .join(&aa)
        .join(&bb)
        .join(&normalized_id)
        .join(format!("{}.meta.json", version))
}

/// Get the shard path component (aa) from a package ID.
pub fn package_shard_path(id: &AccountAddress) -> String {
    let normalized = normalize_object_id(id);
    normalized[0..2].to_string()
}

/// Get the full filesystem path for a package file.
pub fn package_path(cache_root: &Path, id: &AccountAddress) -> PathBuf {
    let aa = package_shard_path(id);
    let normalized_id = normalize_object_id(id);
    cache_root
        .join("packages")
        .join(&aa)
        .join(format!("{}.json", normalized_id))
}

/// Get the progress state file path.
pub fn progress_state_path(cache_root: &Path) -> PathBuf {
    cache_root.join("progress").join("state.json")
}

/// Get the progress events file path.
pub fn progress_events_path(cache_root: &Path) -> PathBuf {
    cache_root.join("progress").join("events.jsonl")
}

/// Ensure all parent directories exist for a path.
pub fn ensure_parent_dirs(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Failed to create directory {}: {}", parent.display(), e))?;
    }
    Ok(())
}

/// Write a file atomically (write to .tmp, then rename).
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    ensure_parent_dirs(path)?;
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("tmp")
    ));
    std::fs::write(&tmp_path, contents)
        .map_err(|e| anyhow!("Failed to write temp file {}: {}", tmp_path.display(), e))?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        anyhow!(
            "Failed to rename {} to {}: {}",
            tmp_path.display(),
            path.display(),
            e
        )
    })?;
    Ok(())
}

/// Write a JSON file atomically (compact format, no pretty printing).
pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_vec(value).map_err(|e| anyhow!("Failed to serialize JSON: {}", e))?;
    atomic_write(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_object_id() {
        let id = AccountAddress::from_hex_literal("0x2").unwrap();
        let normalized = normalize_object_id(&id);
        assert_eq!(normalized.len(), 64);
        assert!(!normalized.starts_with("0x"));
    }

    #[test]
    fn test_object_shard_path() {
        let id = AccountAddress::from_hex_literal("0x1234567890abcdef").unwrap();
        let (aa, bb) = object_shard_path(&id);
        assert_eq!(aa.len(), 2);
        assert_eq!(bb.len(), 2);
    }
}
