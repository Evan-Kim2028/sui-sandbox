//! Storage logging for LLM sandbox artifacts.
//!
//! This module captures and persists data useful for analyzing LLM behavior:
//!
//! - **Compiled Packages**: Move source code written by LLMs, compilation results,
//!   and bytecode output. Useful for studying Move coding style and common errors.
//!
//! - **Tool Call Traces**: Sequence of tool calls made during a session, including
//!   parameters, results, and timing. Useful for understanding LLM reasoning patterns.
//!
//! - **Object Synthesis**: Objects created to satisfy transaction dependencies.
//!   Useful for understanding what state LLMs think they need.
//!
//! - **Object Lifecycle**: Full tracking of object creation, mutation, transfer,
//!   wrapping, and deletion events. Useful for debugging state issues.
//!
//! - **Execution Traces**: Detailed per-command execution traces with gas profiles,
//!   shared object access patterns, and dynamic field operations.
//!
//! - **Gas Profiles**: Per-transaction and per-command gas breakdown for
//!   understanding execution costs and optimizing transactions.
//!
//! ## Storage Structure
//!
//! ```text
//! ~/.sui-llm-logs/
//! ├── packages/
//! │   └── {timestamp}_{package_name}/
//! │       ├── source.move           # Original source code
//! │       ├── compilation.json      # Compilation metadata
//! │       └── bytecode/             # Compiled modules (if successful)
//! │           └── {module_name}.mv
//! ├── sessions/
//! │   └── {session_id}.jsonl        # Tool call traces (append-only)
//! ├── objects/
//! │   ├── {session_id}_objects.jsonl   # Synthesized objects
//! │   └── {session_id}_lifecycle.jsonl # Object lifecycle events
//! └── executions/
//!     ├── {session_id}_exec.jsonl      # Basic execution logs
//!     ├── {session_id}_traces.jsonl    # Detailed execution traces
//!     └── {session_id}_gas.jsonl       # Gas profiles
//! ```

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Default log directory
pub const DEFAULT_LOG_DIR: &str = ".sui-llm-logs";

/// A compiled package record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageLog {
    /// Unique ID for this compilation attempt
    pub id: String,
    /// Timestamp of compilation
    pub timestamp: DateTime<Utc>,
    /// Package name
    pub package_name: String,
    /// Module name
    pub module_name: String,
    /// Original Move source code
    pub source: String,
    /// Whether compilation succeeded
    pub success: bool,
    /// Compilation diagnostics/errors
    pub diagnostics: String,
    /// Bytecode sizes per module (if successful)
    pub bytecode_sizes: Vec<(String, usize)>,
    /// Session ID if part of a session
    pub session_id: Option<String>,
    /// Model that generated this code (if known)
    pub model: Option<String>,
    /// Prompt that led to this code (if known)
    pub prompt: Option<String>,
}

/// A tool call record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallLog {
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Tool name (e.g., "ListModules", "CompileSource")
    pub tool: String,
    /// Tool parameters as JSON
    pub params: serde_json::Value,
    /// Whether the call succeeded
    pub success: bool,
    /// Result data (truncated if large)
    pub result: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// An object synthesis record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectSynthesisLog {
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Type path of the synthesized object
    pub type_path: String,
    /// Object ID assigned
    pub object_id: String,
    /// Field values provided
    pub fields: serde_json::Value,
    /// Whether object is shared
    pub is_shared: bool,
    /// BCS bytes length
    pub bcs_len: usize,
}

/// An execution attempt record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLog {
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Attempt number within session
    pub attempt: u32,
    /// Brief description of what was executed
    pub description: String,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Objects used in this execution
    pub objects_used: Vec<String>,
    /// Gas used (if successful)
    pub gas_used: Option<u64>,
}

// ============================================================================
// Enhanced Logging Types
// ============================================================================

/// Object lifecycle event types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObjectLifecycleEvent {
    /// Object was created
    Created,
    /// Object was mutated
    Mutated,
    /// Object was deleted
    Deleted,
    /// Object was wrapped (stored inside another object)
    Wrapped,
    /// Object was unwrapped (extracted from another object)
    Unwrapped,
    /// Object was transferred to a new owner
    Transferred,
    /// Object was frozen (made immutable)
    Frozen,
    /// Object was shared (made shared)
    Shared,
}

/// A single object lifecycle event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectLifecycleLog {
    /// Timestamp of the event
    pub timestamp: DateTime<Utc>,
    /// Object ID
    pub object_id: String,
    /// Object type (e.g., "0x2::coin::Coin<0x2::sui::SUI>")
    pub object_type: String,
    /// Event type
    pub event: ObjectLifecycleEvent,
    /// Version before this event (if applicable)
    pub version_before: Option<u64>,
    /// Version after this event (if applicable)
    pub version_after: Option<u64>,
    /// Previous owner (for transfers)
    pub previous_owner: Option<String>,
    /// New owner (for transfers and creation)
    pub new_owner: Option<String>,
    /// Transaction/command that caused this event
    pub caused_by: Option<String>,
    /// BCS bytes size after mutation (if applicable)
    pub bytes_size: Option<usize>,
}

/// Detailed execution trace for a PTB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTraceLog {
    /// Timestamp when execution started
    pub timestamp: DateTime<Utc>,
    /// Unique transaction ID
    pub transaction_id: String,
    /// Sender address
    pub sender: String,
    /// Total execution duration in milliseconds
    pub duration_ms: u64,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Per-command execution details
    pub commands: Vec<CommandTraceLog>,
    /// Objects created during execution
    pub objects_created: Vec<String>,
    /// Objects mutated during execution
    pub objects_mutated: Vec<String>,
    /// Objects deleted during execution
    pub objects_deleted: Vec<String>,
    /// Events emitted during execution
    pub events_emitted: Vec<EventLog>,
    /// Gas profile for this execution
    pub gas_profile: GasProfileLog,
    /// Shared objects accessed (with lock info)
    pub shared_objects: Vec<SharedObjectAccessLog>,
    /// Dynamic fields accessed
    pub dynamic_fields_accessed: Vec<DynamicFieldAccessLog>,
    /// Epoch at time of execution
    pub epoch: u64,
    /// Lamport clock value
    pub lamport_clock: u64,
}

/// Per-command trace within a PTB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandTraceLog {
    /// Command index within the PTB
    pub index: usize,
    /// Command type (e.g., "MoveCall", "SplitCoins", "TransferObjects")
    pub command_type: String,
    /// Command details as JSON
    pub details: serde_json::Value,
    /// Duration in microseconds
    pub duration_us: u64,
    /// Gas used by this command
    pub gas_used: u64,
    /// Whether this command succeeded
    pub success: bool,
    /// Error if failed
    pub error: Option<String>,
    /// Return values (BCS hex encoded)
    pub return_values: Vec<String>,
    /// Objects read by this command
    pub objects_read: Vec<String>,
    /// Objects written by this command
    pub objects_written: Vec<String>,
}

/// Event emitted during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    /// Event type (e.g., "0x2::coin::CoinCreated<0x2::sui::SUI>")
    pub event_type: String,
    /// Sequence number within the transaction
    pub sequence: u64,
    /// Event data as JSON (if parseable) or hex
    pub data: serde_json::Value,
    /// BCS bytes size
    pub bytes_size: usize,
}

/// Gas profile for execution analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasProfileLog {
    /// Total gas used
    pub total_gas_used: u64,
    /// Gas budget (if set)
    pub gas_budget: Option<u64>,
    /// Computation gas
    pub computation_gas: u64,
    /// Storage gas (for new objects)
    pub storage_gas: u64,
    /// Storage rebate (for deleted objects)
    pub storage_rebate: u64,
    /// Per-command gas breakdown
    pub per_command_gas: Vec<(usize, u64)>,
    /// Gas price used
    pub gas_price: u64,
    /// Whether execution was close to budget (>80%)
    pub near_budget: bool,
}

impl Default for GasProfileLog {
    fn default() -> Self {
        Self {
            total_gas_used: 0,
            gas_budget: None,
            computation_gas: 0,
            storage_gas: 0,
            storage_rebate: 0,
            per_command_gas: Vec::new(),
            gas_price: 1000,
            near_budget: false,
        }
    }
}

/// Shared object access record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedObjectAccessLog {
    /// Object ID
    pub object_id: String,
    /// Object type
    pub object_type: String,
    /// Whether access was mutable
    pub is_mutable: bool,
    /// Version at access time
    pub version: u64,
    /// Whether there was lock contention
    pub had_contention: bool,
    /// Time spent waiting for lock (microseconds)
    pub lock_wait_us: Option<u64>,
}

/// Dynamic field access record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFieldAccessLog {
    /// Parent object ID
    pub parent_id: String,
    /// Field key (type + value)
    pub field_key: String,
    /// Child object ID
    pub child_id: String,
    /// Access type
    pub access_type: DynamicFieldAccessType,
    /// Field type
    pub field_type: String,
}

/// Types of dynamic field access
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DynamicFieldAccessType {
    /// Field was read
    Read,
    /// Field was written/mutated
    Write,
    /// Field was added
    Add,
    /// Field was removed
    Remove,
    /// Existence was checked
    Exists,
}

/// Storage logger for LLM artifacts
pub struct StorageLogger {
    /// Base directory for logs
    log_dir: PathBuf,
    /// Current session ID
    session_id: String,
    /// Session log file writer
    session_writer: Option<BufWriter<File>>,
}

impl StorageLogger {
    /// Create a new storage logger with default location (~/.sui-llm-logs)
    pub fn new() -> Result<Self> {
        let log_dir = if let Ok(path) = std::env::var("SUI_LLM_LOG_DIR") {
            PathBuf::from(path)
        } else {
            dirs::home_dir()
                .ok_or_else(|| anyhow!("Could not determine home directory"))?
                .join(DEFAULT_LOG_DIR)
        };

        Self::with_log_dir(log_dir)
    }

    /// Create with a specific log directory
    pub fn with_log_dir(log_dir: PathBuf) -> Result<Self> {
        // Create directory structure
        fs::create_dir_all(&log_dir)?;
        fs::create_dir_all(log_dir.join("packages"))?;
        fs::create_dir_all(log_dir.join("sessions"))?;
        fs::create_dir_all(log_dir.join("objects"))?;
        fs::create_dir_all(log_dir.join("executions"))?;

        let session_id = Uuid::new_v4().to_string();

        Ok(Self {
            log_dir,
            session_id,
            session_writer: None,
        })
    }

    /// Get the current session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Start a new session with a specific ID (useful for continuing sessions)
    pub fn set_session_id(&mut self, session_id: String) {
        self.session_id = session_id;
        self.session_writer = None; // Force reopen
    }

    /// Log a compiled package
    pub fn log_package(&self, log: &PackageLog) -> Result<PathBuf> {
        let timestamp = log.timestamp.format("%Y%m%d_%H%M%S");
        let dir_name = format!("{}_{}", timestamp, sanitize_filename(&log.package_name));
        let pkg_dir = self.log_dir.join("packages").join(&dir_name);

        fs::create_dir_all(&pkg_dir)?;

        // Write source code
        let source_path = pkg_dir.join(format!("{}.move", log.module_name));
        fs::write(&source_path, &log.source)?;

        // Write compilation metadata
        let meta_path = pkg_dir.join("compilation.json");
        let meta = serde_json::to_string_pretty(log)?;
        fs::write(&meta_path, meta)?;

        Ok(pkg_dir)
    }

    /// Log a compiled package with bytecode
    pub fn log_package_with_bytecode(
        &self,
        log: &PackageLog,
        bytecode: &[(String, Vec<u8>)],
    ) -> Result<PathBuf> {
        let pkg_dir = self.log_package(log)?;

        if !bytecode.is_empty() {
            let bc_dir = pkg_dir.join("bytecode");
            fs::create_dir_all(&bc_dir)?;

            for (name, bytes) in bytecode {
                let bc_path = bc_dir.join(format!("{}.mv", name));
                fs::write(&bc_path, bytes)?;
            }
        }

        Ok(pkg_dir)
    }

    /// Log a tool call (appends to session log)
    pub fn log_tool_call(&mut self, log: &ToolCallLog) -> Result<()> {
        self.ensure_session_writer()?;

        if let Some(writer) = &mut self.session_writer {
            let line = serde_json::to_string(log)?;
            writeln!(writer, "{}", line)?;
            writer.flush()?;
        }

        Ok(())
    }

    /// Log an object synthesis
    pub fn log_object_synthesis(&mut self, log: &ObjectSynthesisLog) -> Result<()> {
        let objects_file = self
            .log_dir
            .join("objects")
            .join(format!("{}_objects.jsonl", self.session_id));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&objects_file)?;

        let line = serde_json::to_string(log)?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    /// Log an execution attempt
    pub fn log_execution(&mut self, log: &ExecutionLog) -> Result<()> {
        let exec_file = self
            .log_dir
            .join("executions")
            .join(format!("{}_exec.jsonl", self.session_id));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exec_file)?;

        let line = serde_json::to_string(log)?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    /// Log a detailed execution trace (PTB-level)
    pub fn log_execution_trace(&mut self, log: &ExecutionTraceLog) -> Result<()> {
        let trace_file = self
            .log_dir
            .join("executions")
            .join(format!("{}_traces.jsonl", self.session_id));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&trace_file)?;

        let line = serde_json::to_string(log)?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    /// Log an object lifecycle event
    pub fn log_object_lifecycle(&mut self, log: &ObjectLifecycleLog) -> Result<()> {
        let lifecycle_file = self
            .log_dir
            .join("objects")
            .join(format!("{}_lifecycle.jsonl", self.session_id));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&lifecycle_file)?;

        let line = serde_json::to_string(log)?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    /// Log a gas profile
    pub fn log_gas_profile(&mut self, profile: &GasProfileLog, transaction_id: &str) -> Result<()> {
        let gas_file = self
            .log_dir
            .join("executions")
            .join(format!("{}_gas.jsonl", self.session_id));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gas_file)?;

        let entry = serde_json::json!({
            "timestamp": Utc::now(),
            "transaction_id": transaction_id,
            "profile": profile,
        });
        let line = serde_json::to_string(&entry)?;
        writeln!(file, "{}", line)?;

        Ok(())
    }

    /// Get path to packages directory
    pub fn packages_dir(&self) -> PathBuf {
        self.log_dir.join("packages")
    }

    /// List all logged packages
    pub fn list_packages(&self) -> Result<Vec<PathBuf>> {
        let pkg_dir = self.packages_dir();
        let mut packages = Vec::new();

        if pkg_dir.exists() {
            for entry in fs::read_dir(&pkg_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    packages.push(entry.path());
                }
            }
        }

        // Sort by name (which includes timestamp)
        packages.sort();
        Ok(packages)
    }

    /// Load a package log from a directory
    pub fn load_package_log(&self, pkg_dir: &Path) -> Result<PackageLog> {
        let meta_path = pkg_dir.join("compilation.json");
        let content = fs::read_to_string(&meta_path)?;
        let log: PackageLog = serde_json::from_str(&content)?;
        Ok(log)
    }

    /// Get the log directory
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Ensure session writer is open
    fn ensure_session_writer(&mut self) -> Result<()> {
        if self.session_writer.is_none() {
            let session_file = self
                .log_dir
                .join("sessions")
                .join(format!("{}.jsonl", self.session_id));

            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&session_file)?;

            self.session_writer = Some(BufWriter::new(file));
        }
        Ok(())
    }
}

impl Default for StorageLogger {
    fn default() -> Self {
        Self::new().expect("Failed to create storage logger")
    }
}

/// Sanitize a string for use as a filename
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ============================================================================
// Analysis Helpers
// ============================================================================

/// Summary statistics for logged packages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageStats {
    /// Total packages logged
    pub total: usize,
    /// Successful compilations
    pub successful: usize,
    /// Failed compilations
    pub failed: usize,
    /// Most common error patterns
    pub common_errors: Vec<(String, usize)>,
    /// Average source code length
    pub avg_source_len: usize,
    /// Models that generated packages
    pub models: Vec<(String, usize)>,
}

/// Analyze all logged packages
pub fn analyze_packages(logger: &StorageLogger) -> Result<PackageStats> {
    let packages = logger.list_packages()?;

    let mut total = 0;
    let mut successful = 0;
    let mut failed = 0;
    let mut total_source_len = 0;
    let mut error_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut model_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for pkg_dir in &packages {
        if let Ok(log) = logger.load_package_log(pkg_dir) {
            total += 1;
            total_source_len += log.source.len();

            if log.success {
                successful += 1;
            } else {
                failed += 1;
                // Extract first line of error as pattern
                let error_pattern = log
                    .diagnostics
                    .lines()
                    .next()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                *error_counts.entry(error_pattern).or_insert(0) += 1;
            }

            if let Some(model) = log.model {
                *model_counts.entry(model).or_insert(0) += 1;
            }
        }
    }

    // Sort errors by count
    let mut common_errors: Vec<_> = error_counts.into_iter().collect();
    common_errors.sort_by(|a, b| b.1.cmp(&a.1));
    common_errors.truncate(10); // Top 10

    let mut models: Vec<_> = model_counts.into_iter().collect();
    models.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(PackageStats {
        total,
        successful,
        failed,
        common_errors,
        avg_source_len: if total > 0 {
            total_source_len / total
        } else {
            0
        },
        models,
    })
}

/// Session statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    /// Total tool calls
    pub total_calls: usize,
    /// Successful calls
    pub successful_calls: usize,
    /// Failed calls
    pub failed_calls: usize,
    /// Tool call distribution
    pub tool_distribution: Vec<(String, usize)>,
    /// Average call duration in ms
    pub avg_duration_ms: f64,
}

/// Load and analyze a session log
pub fn analyze_session(logger: &StorageLogger, session_id: &str) -> Result<SessionStats> {
    let session_file = logger
        .log_dir()
        .join("sessions")
        .join(format!("{}.jsonl", session_id));

    if !session_file.exists() {
        return Err(anyhow!("Session not found: {}", session_id));
    }

    let content = fs::read_to_string(&session_file)?;
    let mut total_calls = 0;
    let mut successful_calls = 0;
    let mut failed_calls = 0;
    let mut total_duration = 0u64;
    let mut tool_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for line in content.lines() {
        if let Ok(log) = serde_json::from_str::<ToolCallLog>(line) {
            total_calls += 1;
            total_duration += log.duration_ms;

            if log.success {
                successful_calls += 1;
            } else {
                failed_calls += 1;
            }

            *tool_counts.entry(log.tool).or_insert(0) += 1;
        }
    }

    let mut tool_distribution: Vec<_> = tool_counts.into_iter().collect();
    tool_distribution.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(SessionStats {
        total_calls,
        successful_calls,
        failed_calls,
        tool_distribution,
        avg_duration_ms: if total_calls > 0 {
            total_duration as f64 / total_calls as f64
        } else {
            0.0
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_logger_creation() {
        let temp_dir = std::env::temp_dir().join(format!("test-log-{}", std::process::id()));
        let logger = StorageLogger::with_log_dir(temp_dir.clone()).unwrap();

        assert!(temp_dir.join("packages").exists());
        assert!(temp_dir.join("sessions").exists());
        assert!(temp_dir.join("objects").exists());
        assert!(temp_dir.join("executions").exists());

        println!("Session ID: {}", logger.session_id());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_log_package() {
        let temp_dir = std::env::temp_dir().join(format!("test-log-pkg-{}", std::process::id()));
        let logger = StorageLogger::with_log_dir(temp_dir.clone()).unwrap();

        let log = PackageLog {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            package_name: "test_package".to_string(),
            module_name: "counter".to_string(),
            source: "module test_package::counter {\n    // test\n}".to_string(),
            success: true,
            diagnostics: String::new(),
            bytecode_sizes: vec![("counter".to_string(), 256)],
            session_id: Some(logger.session_id().to_string()),
            model: Some("gpt-4".to_string()),
            prompt: Some("Write a counter module".to_string()),
        };

        let pkg_dir = logger.log_package(&log).unwrap();

        assert!(pkg_dir.join("counter.move").exists());
        assert!(pkg_dir.join("compilation.json").exists());

        // Verify we can load it back
        let loaded = logger.load_package_log(&pkg_dir).unwrap();
        assert_eq!(loaded.package_name, "test_package");
        assert!(loaded.success);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_log_tool_call() {
        let temp_dir = std::env::temp_dir().join(format!("test-log-tool-{}", std::process::id()));
        let mut logger = StorageLogger::with_log_dir(temp_dir.clone()).unwrap();

        let log = ToolCallLog {
            timestamp: Utc::now(),
            tool: "ListModules".to_string(),
            params: serde_json::json!({}),
            success: true,
            result: Some(serde_json::json!(["0x2::coin", "0x2::object"])),
            error: None,
            duration_ms: 5,
        };

        logger.log_tool_call(&log).unwrap();

        let session_file = temp_dir
            .join("sessions")
            .join(format!("{}.jsonl", logger.session_id()));
        assert!(session_file.exists());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("my_package"), "my_package");
        assert_eq!(sanitize_filename("my-package"), "my-package");
        assert_eq!(sanitize_filename("my package!@#"), "my_package___");
        assert_eq!(sanitize_filename("0x123::module"), "0x123__module");
    }
}
