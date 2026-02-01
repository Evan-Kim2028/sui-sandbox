//! Transaction History Storage for sui-sandbox.
//!
//! Captures and persists every transaction executed in a world for:
//! - Debugging and replay
//! - Audit trails
//! - Analytics and learning
//!
//! ## Storage Structure
//!
//! Each world has its own transaction history:
//! ```text
//! ~/.sui-sandbox/worlds/{world-name}/
//! └── history/
//!     ├── transactions.jsonl    # Append-only transaction log
//!     └── index.json            # Summary index for fast queries
//! ```

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

/// A recorded transaction entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRecord {
    /// Unique transaction ID (UUID)
    pub tx_id: String,
    /// Sequential transaction number within this world
    pub sequence: u64,
    /// Timestamp when executed
    pub timestamp: DateTime<Utc>,
    /// Sender address
    pub sender: String,
    /// Whether the transaction succeeded
    pub success: bool,
    /// Gas used
    pub gas_used: u64,
    /// Brief description of what was executed
    pub description: String,
    /// The PTB inputs (JSON)
    pub inputs: serde_json::Value,
    /// The PTB commands (JSON)
    pub commands: serde_json::Value,
    /// Objects created (IDs)
    pub objects_created: Vec<String>,
    /// Objects mutated (IDs)
    pub objects_mutated: Vec<String>,
    /// Objects deleted (IDs)
    pub objects_deleted: Vec<String>,
    /// Events emitted
    pub events: Vec<TransactionEvent>,
    /// Error message if failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Detailed error context if failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_context: Option<String>,
    /// Index of failed command (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_command_index: Option<usize>,
    /// Return values from the transaction
    #[serde(default)]
    pub return_values: Vec<serde_json::Value>,
    /// Package ID if this was a publish transaction
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_package: Option<String>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

/// An event emitted during transaction execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEvent {
    /// Event type (e.g., "0x2::coin::MintEvent")
    pub event_type: String,
    /// Sequence number within the transaction
    pub sequence: u64,
    /// Event data
    pub data: serde_json::Value,
}

/// Index entry for fast lookups
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub tx_id: String,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub description: String,
    /// File offset for fast seeking (bytes from start)
    pub file_offset: u64,
}

/// Summary index for the transaction history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryIndex {
    /// Version for compatibility
    pub version: u32,
    /// Total transaction count
    pub total_transactions: u64,
    /// Successful transaction count
    pub successful_transactions: u64,
    /// Failed transaction count
    pub failed_transactions: u64,
    /// Next sequence number
    pub next_sequence: u64,
    /// First transaction timestamp
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_transaction: Option<DateTime<Utc>>,
    /// Last transaction timestamp
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transaction: Option<DateTime<Utc>>,
    /// Recent transactions for quick access (last 100)
    pub recent: Vec<IndexEntry>,
    /// Packages published in this world
    pub packages: Vec<String>,
    /// Total gas used across all transactions
    pub total_gas_used: u64,
}

impl Default for HistoryIndex {
    fn default() -> Self {
        Self {
            version: 1,
            total_transactions: 0,
            successful_transactions: 0,
            failed_transactions: 0,
            next_sequence: 1,
            first_transaction: None,
            last_transaction: None,
            recent: Vec::new(),
            packages: Vec::new(),
            total_gas_used: 0,
        }
    }
}

/// Configuration for transaction history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// Whether history capture is enabled
    pub enabled: bool,
    /// Maximum number of transactions to keep (0 = unlimited)
    pub max_transactions: u64,
    /// Whether to capture full input/command details
    pub capture_full_details: bool,
    /// Whether to capture return values
    pub capture_return_values: bool,
    /// Tags to automatically add to all transactions
    pub auto_tags: Vec<String>,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_transactions: 0, // Unlimited
            capture_full_details: true,
            capture_return_values: true,
            auto_tags: Vec::new(),
        }
    }
}

/// Transaction history manager for a single world
pub struct TransactionHistory {
    /// Path to history directory
    history_dir: PathBuf,
    /// Path to transactions log file
    transactions_path: PathBuf,
    /// Path to index file
    index_path: PathBuf,
    /// In-memory index
    index: HistoryIndex,
    /// Configuration
    config: HistoryConfig,
    /// Current file offset for new entries
    current_offset: u64,
}

impl TransactionHistory {
    /// Create or open transaction history for a world
    pub fn open(world_path: &Path) -> Result<Self> {
        let history_dir = world_path.join("history");
        fs::create_dir_all(&history_dir)?;

        let transactions_path = history_dir.join("transactions.jsonl");
        let index_path = history_dir.join("index.json");
        let config_path = history_dir.join("config.json");

        // Load or create config
        let config = if config_path.exists() {
            let data = fs::read_to_string(&config_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            let config = HistoryConfig::default();
            let json = serde_json::to_string_pretty(&config)?;
            fs::write(&config_path, json)?;
            config
        };

        // Load or create index
        let index = if index_path.exists() {
            let data = fs::read_to_string(&index_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HistoryIndex::default()
        };

        // Get current file offset
        let current_offset = if transactions_path.exists() {
            fs::metadata(&transactions_path)?.len()
        } else {
            0
        };

        Ok(Self {
            history_dir,
            transactions_path,
            index_path,
            index,
            config,
            current_offset,
        })
    }

    /// Check if history capture is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Enable or disable history capture
    pub fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        self.config.enabled = enabled;
        self.save_config()
    }

    /// Record a transaction
    pub fn record(&mut self, record: TransactionRecord) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let mut record = record;

        // Assign sequence number
        record.sequence = self.index.next_sequence;
        self.index.next_sequence += 1;

        // Add auto tags
        for tag in &self.config.auto_tags {
            if !record.tags.contains(tag) {
                record.tags.push(tag.clone());
            }
        }

        // Update index
        self.index.total_transactions += 1;
        if record.success {
            self.index.successful_transactions += 1;
        } else {
            self.index.failed_transactions += 1;
        }
        self.index.total_gas_used += record.gas_used;

        if self.index.first_transaction.is_none() {
            self.index.first_transaction = Some(record.timestamp);
        }
        self.index.last_transaction = Some(record.timestamp);

        // Track published packages
        if let Some(ref pkg_id) = record.published_package {
            if !self.index.packages.contains(pkg_id) {
                self.index.packages.push(pkg_id.clone());
            }
        }

        // Add to recent (keep last 100)
        let entry = IndexEntry {
            tx_id: record.tx_id.clone(),
            sequence: record.sequence,
            timestamp: record.timestamp,
            success: record.success,
            description: record.description.clone(),
            file_offset: self.current_offset,
        };
        self.index.recent.push(entry);
        if self.index.recent.len() > 100 {
            self.index.recent.remove(0);
        }

        // Serialize and write record
        let json = serde_json::to_string(&record)?;
        let line = format!("{}\n", json);
        let line_bytes = line.as_bytes();

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.transactions_path)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(line_bytes)?;
        writer.flush()?;

        self.current_offset += line_bytes.len() as u64;

        // Save index
        self.save_index()?;

        Ok(())
    }

    /// Get transaction history summary
    pub fn summary(&self) -> HistorySummary {
        HistorySummary {
            total_transactions: self.index.total_transactions,
            successful_transactions: self.index.successful_transactions,
            failed_transactions: self.index.failed_transactions,
            first_transaction: self.index.first_transaction,
            last_transaction: self.index.last_transaction,
            packages_published: self.index.packages.len() as u64,
            total_gas_used: self.index.total_gas_used,
            enabled: self.config.enabled,
        }
    }

    /// Get recent transactions
    pub fn recent(&self, limit: usize) -> Vec<IndexEntry> {
        let start = if self.index.recent.len() > limit {
            self.index.recent.len() - limit
        } else {
            0
        };
        self.index.recent[start..].to_vec()
    }

    /// Get a specific transaction by ID
    pub fn get_transaction(&self, tx_id: &str) -> Result<Option<TransactionRecord>> {
        // Check if in recent index
        if let Some(entry) = self.index.recent.iter().find(|e| e.tx_id == tx_id) {
            return self.read_at_offset(entry.file_offset);
        }

        // Fall back to scanning the file
        self.find_transaction(|r| r.tx_id == tx_id)
    }

    /// Get a transaction by sequence number
    pub fn get_by_sequence(&self, sequence: u64) -> Result<Option<TransactionRecord>> {
        // Check if in recent index
        if let Some(entry) = self.index.recent.iter().find(|e| e.sequence == sequence) {
            return self.read_at_offset(entry.file_offset);
        }

        // Fall back to scanning
        self.find_transaction(|r| r.sequence == sequence)
    }

    /// List transactions with pagination
    pub fn list(&self, offset: u64, limit: usize) -> Result<Vec<TransactionRecord>> {
        let mut results = Vec::new();
        let file = match File::open(&self.transactions_path) {
            Ok(f) => f,
            Err(_) => return Ok(results),
        };

        let reader = BufReader::new(file);
        for (i, line) in reader.lines().enumerate() {
            if (i as u64) < offset {
                continue;
            }
            if results.len() >= limit {
                break;
            }
            let line = line?;
            if let Ok(record) = serde_json::from_str::<TransactionRecord>(&line) {
                results.push(record);
            }
        }

        Ok(results)
    }

    /// Search transactions by criteria
    pub fn search(&self, criteria: &SearchCriteria) -> Result<Vec<TransactionRecord>> {
        let mut results = Vec::new();
        let file = match File::open(&self.transactions_path) {
            Ok(f) => f,
            Err(_) => return Ok(results),
        };

        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if let Ok(record) = serde_json::from_str::<TransactionRecord>(&line) {
                if criteria.matches(&record) {
                    results.push(record);
                    if let Some(limit) = criteria.limit {
                        if results.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Clear all transaction history
    pub fn clear(&mut self) -> Result<()> {
        if self.transactions_path.exists() {
            fs::remove_file(&self.transactions_path)?;
        }
        self.index = HistoryIndex::default();
        self.current_offset = 0;
        self.save_index()
    }

    /// Export history to a file
    pub fn export(&self, output_path: &Path) -> Result<u64> {
        if !self.transactions_path.exists() {
            return Ok(0);
        }
        fs::copy(&self.transactions_path, output_path)?;
        Ok(self.index.total_transactions)
    }

    // =========================================================================
    // Private helpers
    // =========================================================================

    fn save_index(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.index)?;
        fs::write(&self.index_path, json)?;
        Ok(())
    }

    fn save_config(&self) -> Result<()> {
        let config_path = self.history_dir.join("config.json");
        let json = serde_json::to_string_pretty(&self.config)?;
        fs::write(&config_path, json)?;
        Ok(())
    }

    fn read_at_offset(&self, offset: u64) -> Result<Option<TransactionRecord>> {
        use std::io::{Seek, SeekFrom};

        let mut file = File::open(&self.transactions_path)?;
        file.seek(SeekFrom::Start(offset))?;

        let reader = BufReader::new(file);
        if let Some(Ok(line)) = reader.lines().next() {
            let record: TransactionRecord = serde_json::from_str(&line)?;
            return Ok(Some(record));
        }
        Ok(None)
    }

    fn find_transaction<F>(&self, predicate: F) -> Result<Option<TransactionRecord>>
    where
        F: Fn(&TransactionRecord) -> bool,
    {
        let file = match File::open(&self.transactions_path) {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };

        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if let Ok(record) = serde_json::from_str::<TransactionRecord>(&line) {
                if predicate(&record) {
                    return Ok(Some(record));
                }
            }
        }
        Ok(None)
    }
}

/// Search criteria for querying transactions
#[derive(Debug, Clone, Default)]
pub struct SearchCriteria {
    /// Filter by success status
    pub success: Option<bool>,
    /// Filter by sender
    pub sender: Option<String>,
    /// Filter by tag
    pub tag: Option<String>,
    /// Filter by description containing text
    pub description_contains: Option<String>,
    /// Filter by date range start
    pub from_date: Option<DateTime<Utc>>,
    /// Filter by date range end
    pub to_date: Option<DateTime<Utc>>,
    /// Filter by object involvement (created, mutated, or deleted)
    pub involves_object: Option<String>,
    /// Maximum results to return
    pub limit: Option<usize>,
}

impl SearchCriteria {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }

    pub fn sender(mut self, sender: impl Into<String>) -> Self {
        self.sender = Some(sender.into());
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    fn matches(&self, record: &TransactionRecord) -> bool {
        if let Some(success) = self.success {
            if record.success != success {
                return false;
            }
        }
        if let Some(ref sender) = self.sender {
            if &record.sender != sender {
                return false;
            }
        }
        if let Some(ref tag) = self.tag {
            if !record.tags.contains(tag) {
                return false;
            }
        }
        if let Some(ref text) = self.description_contains {
            if !record
                .description
                .to_lowercase()
                .contains(&text.to_lowercase())
            {
                return false;
            }
        }
        if let Some(from) = self.from_date {
            if record.timestamp < from {
                return false;
            }
        }
        if let Some(to) = self.to_date {
            if record.timestamp > to {
                return false;
            }
        }
        if let Some(ref obj_id) = self.involves_object {
            let involved = record.objects_created.contains(obj_id)
                || record.objects_mutated.contains(obj_id)
                || record.objects_deleted.contains(obj_id);
            if !involved {
                return false;
            }
        }
        true
    }
}

/// Summary of transaction history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySummary {
    pub total_transactions: u64,
    pub successful_transactions: u64,
    pub failed_transactions: u64,
    pub first_transaction: Option<DateTime<Utc>>,
    pub last_transaction: Option<DateTime<Utc>>,
    pub packages_published: u64,
    pub total_gas_used: u64,
    pub enabled: bool,
}

/// Builder for creating transaction records from execution results
pub struct TransactionRecordBuilder {
    record: TransactionRecord,
}

impl TransactionRecordBuilder {
    pub fn new() -> Self {
        Self {
            record: TransactionRecord {
                tx_id: uuid::Uuid::new_v4().to_string(),
                sequence: 0, // Will be assigned by history
                timestamp: Utc::now(),
                sender: "0x0".to_string(),
                success: false,
                gas_used: 0,
                description: String::new(),
                inputs: serde_json::Value::Null,
                commands: serde_json::Value::Null,
                objects_created: Vec::new(),
                objects_mutated: Vec::new(),
                objects_deleted: Vec::new(),
                events: Vec::new(),
                error: None,
                error_context: None,
                failed_command_index: None,
                return_values: Vec::new(),
                published_package: None,
                tags: Vec::new(),
            },
        }
    }

    pub fn sender(mut self, sender: impl Into<String>) -> Self {
        self.record.sender = sender.into();
        self
    }

    pub fn success(mut self, success: bool) -> Self {
        self.record.success = success;
        self
    }

    pub fn gas_used(mut self, gas: u64) -> Self {
        self.record.gas_used = gas;
        self
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.record.description = desc.into();
        self
    }

    pub fn inputs(mut self, inputs: serde_json::Value) -> Self {
        self.record.inputs = inputs;
        self
    }

    pub fn commands(mut self, commands: serde_json::Value) -> Self {
        self.record.commands = commands;
        self
    }

    pub fn objects_created(mut self, objects: Vec<String>) -> Self {
        self.record.objects_created = objects;
        self
    }

    pub fn objects_mutated(mut self, objects: Vec<String>) -> Self {
        self.record.objects_mutated = objects;
        self
    }

    pub fn objects_deleted(mut self, objects: Vec<String>) -> Self {
        self.record.objects_deleted = objects;
        self
    }

    pub fn events(mut self, events: Vec<TransactionEvent>) -> Self {
        self.record.events = events;
        self
    }

    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.record.error = Some(error.into());
        self
    }

    pub fn error_context(mut self, context: impl Into<String>) -> Self {
        self.record.error_context = Some(context.into());
        self
    }

    pub fn failed_command(mut self, index: usize) -> Self {
        self.record.failed_command_index = Some(index);
        self
    }

    pub fn return_values(mut self, values: Vec<serde_json::Value>) -> Self {
        self.record.return_values = values;
        self
    }

    pub fn published_package(mut self, package_id: impl Into<String>) -> Self {
        self.record.published_package = Some(package_id.into());
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.record.tags.push(tag.into());
        self
    }

    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.record.tags.extend(tags);
        self
    }

    pub fn build(self) -> TransactionRecord {
        self.record
    }
}

impl Default for TransactionRecordBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_transaction_history_basic() {
        let temp_dir = TempDir::new().unwrap();
        let mut history = TransactionHistory::open(temp_dir.path()).unwrap();

        assert!(history.is_enabled());
        assert_eq!(history.summary().total_transactions, 0);

        // Record a transaction
        let record = TransactionRecordBuilder::new()
            .sender("0x123")
            .success(true)
            .gas_used(1000)
            .description("Test transaction")
            .tag("test")
            .build();

        history.record(record).unwrap();

        let summary = history.summary();
        assert_eq!(summary.total_transactions, 1);
        assert_eq!(summary.successful_transactions, 1);
        assert_eq!(summary.total_gas_used, 1000);
    }

    #[test]
    fn test_transaction_search() {
        let temp_dir = TempDir::new().unwrap();
        let mut history = TransactionHistory::open(temp_dir.path()).unwrap();

        // Add some transactions
        for i in 0..5 {
            let record = TransactionRecordBuilder::new()
                .sender(format!("0x{}", i))
                .success(i % 2 == 0)
                .description(format!("Transaction {}", i))
                .build();
            history.record(record).unwrap();
        }

        // Search for successful transactions
        let criteria = SearchCriteria::new().success(true);
        let results = history.search(&criteria).unwrap();
        assert_eq!(results.len(), 3); // 0, 2, 4 are successful

        // Search by sender
        let criteria = SearchCriteria::new().sender("0x2");
        let results = history.search(&criteria).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_transaction_persistence() {
        let temp_dir = TempDir::new().unwrap();

        // Create and populate history
        {
            let mut history = TransactionHistory::open(temp_dir.path()).unwrap();
            let record = TransactionRecordBuilder::new()
                .sender("0xabc")
                .success(true)
                .description("Persistent test")
                .build();
            history.record(record).unwrap();
        }

        // Reopen and verify
        {
            let history = TransactionHistory::open(temp_dir.path()).unwrap();
            assert_eq!(history.summary().total_transactions, 1);

            let recent = history.recent(10);
            assert_eq!(recent.len(), 1);
            assert_eq!(recent[0].description, "Persistent test");
        }
    }
}
