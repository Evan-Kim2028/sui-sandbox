//! Progress tracking for resumable cache ingestion.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use crate::paths::{atomic_write_json, progress_events_path, progress_state_path};

/// Progress state (snapshot for resumption).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressState {
    /// Blob IDs that have been fully processed
    pub ingested_blobs: HashSet<String>,
    /// Last checkpoint ingested per blob (for partial runs)
    pub last_checkpoint_per_blob: HashMap<String, u64>,
    /// Total checkpoints processed
    pub checkpoints_processed: u64,
    /// Total objects written
    pub objects_written: u64,
    /// Total duplicates skipped
    pub duplicates_skipped: u64,
}

impl Default for ProgressState {
    fn default() -> Self {
        Self {
            ingested_blobs: HashSet::new(),
            last_checkpoint_per_blob: HashMap::new(),
            checkpoints_processed: 0,
            objects_written: 0,
            duplicates_skipped: 0,
        }
    }
}

/// Progress event (append-only log entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub timestamp: u64,
    pub event_type: String,
    pub blob_id: Option<String>,
    pub checkpoint: Option<u64>,
    pub objects_written: Option<u64>,
    pub message: Option<String>,
}

/// Progress tracker for resumable cache ingestion.
pub struct ProgressTracker {
    cache_root: Arc<Path>,
    state: parking_lot::RwLock<ProgressState>,
    events_file: Arc<Path>,
    events_lock: parking_lot::Mutex<()>,
}

impl ProgressTracker {
    /// Create a new progress tracker.
    pub fn new<P: AsRef<Path>>(cache_root: P) -> Result<Self> {
        let cache_root = cache_root.as_ref().to_path_buf();
        let progress_dir = cache_root.join("progress");
        std::fs::create_dir_all(&progress_dir)
            .map_err(|e| anyhow!("Failed to create progress directory: {}", e))?;

        let state_path = progress_state_path(&cache_root);
        let events_path = progress_events_path(&cache_root);

        // Load existing state if it exists
        let state = if state_path.exists() {
            let json = std::fs::read_to_string(&state_path)
                .map_err(|e| anyhow!("Failed to read progress state: {}", e))?;
            serde_json::from_str(&json)
                .map_err(|e| anyhow!("Failed to parse progress state: {}", e))?
        } else {
            ProgressState::default()
        };

        Ok(Self {
            cache_root: Arc::from(cache_root),
            state: parking_lot::RwLock::new(state),
            events_file: Arc::from(events_path),
            events_lock: parking_lot::Mutex::new(()),
        })
    }

    /// Check if a blob has been fully ingested.
    pub fn is_blob_ingested(&self, blob_id: &str) -> bool {
        self.state.read().ingested_blobs.contains(blob_id)
    }

    /// Get the last checkpoint ingested for a blob.
    pub fn last_checkpoint(&self, blob_id: &str) -> Option<u64> {
        self.state
            .read()
            .last_checkpoint_per_blob
            .get(blob_id)
            .copied()
    }

    /// Record that a checkpoint has been processed.
    pub fn record_checkpoint(&self, blob_id: &str, checkpoint: u64) -> Result<()> {
        let mut state = self.state.write();
        state.checkpoints_processed += 1;
        state
            .last_checkpoint_per_blob
            .insert(blob_id.to_string(), checkpoint);

        self.log_event(ProgressEvent {
            timestamp: current_timestamp(),
            event_type: "checkpoint".to_string(),
            blob_id: Some(blob_id.to_string()),
            checkpoint: Some(checkpoint),
            objects_written: None,
            message: None,
        })?;

        Ok(())
    }

    /// Record that objects have been written.
    pub fn record_objects_written(&self, count: u64, duplicates: u64) -> Result<()> {
        let mut state = self.state.write();
        state.objects_written += count;
        state.duplicates_skipped += duplicates;
        Ok(())
    }

    /// Mark a blob as fully ingested.
    pub fn mark_blob_complete(&self, blob_id: &str) -> Result<()> {
        let mut state = self.state.write();
        state.ingested_blobs.insert(blob_id.to_string());

        self.log_event(ProgressEvent {
            timestamp: current_timestamp(),
            event_type: "blob_complete".to_string(),
            blob_id: Some(blob_id.to_string()),
            checkpoint: None,
            objects_written: None,
            message: None,
        })?;

        self.save_state()?;
        Ok(())
    }

    /// Save the current state to disk.
    pub fn save_state(&self) -> Result<()> {
        let state = self.state.read();
        let state_path = progress_state_path(&self.cache_root);
        atomic_write_json(&state_path, &*state)?;
        Ok(())
    }

    /// Log an event to the append-only events file.
    fn log_event(&self, event: ProgressEvent) -> Result<()> {
        let json = serde_json::to_string(&event)
            .map_err(|e| anyhow!("Failed to serialize event: {}", e))?;
        // Serialize writes across threads to keep jsonl lines intact.
        let _guard = self.events_lock.lock();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&*self.events_file)
            .map_err(|e| anyhow!("Failed to open events file: {}", e))?;
        writeln!(file, "{}", json).map_err(|e| anyhow!("Failed to write event: {}", e))?;
        Ok(())
    }

    /// Get current statistics.
    pub fn stats(&self) -> ProgressState {
        self.state.read().clone()
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_progress_tracking() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let tracker = ProgressTracker::new(temp_dir.path())?;

        assert!(!tracker.is_blob_ingested("blob1"));
        assert_eq!(tracker.last_checkpoint("blob1"), None);

        // Record checkpoints
        tracker.record_checkpoint("blob1", 100)?;
        tracker.record_checkpoint("blob1", 101)?;
        assert_eq!(tracker.last_checkpoint("blob1"), Some(101));

        // Record objects
        tracker.record_objects_written(50, 10)?;
        let stats = tracker.stats();
        assert_eq!(stats.objects_written, 50);
        assert_eq!(stats.duplicates_skipped, 10);

        // Mark complete
        tracker.mark_blob_complete("blob1")?;
        assert!(tracker.is_blob_ingested("blob1"));

        // Reload and verify persistence
        let tracker2 = ProgressTracker::new(temp_dir.path())?;
        assert!(tracker2.is_blob_ingested("blob1"));
        assert_eq!(tracker2.last_checkpoint("blob1"), Some(101));
        let stats2 = tracker2.stats();
        assert_eq!(stats2.objects_written, 50);

        Ok(())
    }
}
