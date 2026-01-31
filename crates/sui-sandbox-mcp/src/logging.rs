use anyhow::Result;
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::paths::default_paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    pub enabled: bool,
    pub path: PathBuf,
    pub level: String,
    pub rotation_mb: u64,
}

impl Default for LogConfig {
    fn default() -> Self {
        let base = default_paths().logs_dir();
        Self {
            enabled: true,
            path: base,
            level: "info".to_string(),
            rotation_mb: 50,
        }
    }
}

#[derive(Debug)]
pub struct McpLogger {
    config: Mutex<LogConfig>,
    file: Mutex<Option<File>>,
    file_path: Mutex<Option<PathBuf>>,
}

impl McpLogger {
    pub fn new(config: LogConfig) -> Self {
        Self {
            config: Mutex::new(config),
            file: Mutex::new(None),
            file_path: Mutex::new(None),
        }
    }

    pub fn config(&self) -> LogConfig {
        self.config.lock().clone()
    }

    pub fn update_config(&self, new_config: LogConfig) {
        *self.config.lock() = new_config;
        *self.file.lock() = None;
        *self.file_path.lock() = None;
    }

    pub fn log_tool_call(&self, record: &LogRecord) -> Result<()> {
        let config = self.config.lock().clone();
        if !config.enabled {
            return Ok(());
        }

        fs::create_dir_all(&config.path)?;
        self.rotate_if_needed(&config)?;

        let mut file_guard = self.file.lock();
        if file_guard.is_none() {
            let file_path = self.current_log_path(&config)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)?;
            *file_guard = Some(file);
            *self.file_path.lock() = Some(file_path);
        }

        if let Some(file) = file_guard.as_mut() {
            let line = serde_json::to_string(record)?;
            writeln!(file, "{}", line)?;
        }
        Ok(())
    }

    fn rotate_if_needed(&self, config: &LogConfig) -> Result<()> {
        let current = self.file_path.lock().clone();
        if let Some(path) = current {
            if let Ok(metadata) = fs::metadata(&path) {
                let size_mb = metadata.len() / (1024 * 1024);
                if size_mb >= config.rotation_mb {
                    *self.file.lock() = None;
                    *self.file_path.lock() = None;
                }
            }
        }
        Ok(())
    }

    fn current_log_path(&self, config: &LogConfig) -> Result<PathBuf> {
        let ts = Utc::now().format("%Y%m%d-%H%M%S");
        let filename = format!("mcp-{}.jsonl", ts);
        let mut path = config.path.clone();
        path.push(filename);
        Ok(path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    pub ts: String,
    pub request_id: String,
    pub tool: String,
    pub input: Value,
    pub output: Value,
    pub duration_ms: u128,
    pub success: bool,
    pub error: Option<String>,
    pub cache_hit: Option<bool>,
    pub llm_reason: Option<String>,
    pub tags: Option<Vec<String>>,
}

pub fn redact_sensitive(value: &Value) -> Value {
    fn redact_value(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    let key_l = k.to_lowercase();
                    if key_l.contains("key")
                        || key_l.contains("token")
                        || key_l.contains("secret")
                        || key_l.contains("password")
                    {
                        new_map.insert(k.clone(), Value::String("***redacted***".to_string()));
                    } else {
                        new_map.insert(k.clone(), redact_value(v));
                    }
                }
                Value::Object(new_map)
            }
            Value::Array(arr) => Value::Array(arr.iter().map(redact_value).collect()),
            _ => value.clone(),
        }
    }

    redact_value(value)
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}
