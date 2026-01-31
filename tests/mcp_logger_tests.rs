use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use sui_sandbox_mcp::logging::{redact_sensitive, LogConfig, LogRecord, McpLogger};

#[test]
fn redacts_sensitive_fields_recursively() {
    let input = serde_json::json!({
        "api_key": "secret",
        "nested": {
            "token": "t0k",
            "password": "pw",
            "safe": "ok"
        },
        "list": [{"secret": "shh", "ok": 1}]
    });

    let redacted = redact_sensitive(&input);
    assert_eq!(redacted["api_key"], "***redacted***");
    assert_eq!(redacted["nested"]["token"], "***redacted***");
    assert_eq!(redacted["nested"]["password"], "***redacted***");
    assert_eq!(redacted["nested"]["safe"], "ok");
    assert_eq!(redacted["list"][0]["secret"], "***redacted***");
}

#[test]
fn logger_writes_jsonl_records() {
    let temp = TempDir::new().expect("tempdir");
    let log_dir = temp.path().join("logs");
    let config = LogConfig {
        enabled: true,
        path: log_dir.clone(),
        level: "info".to_string(),
        rotation_mb: 50,
    };
    let logger = McpLogger::new(config);

    let record = LogRecord {
        ts: "2025-01-01T00:00:00Z".to_string(),
        request_id: "req-1".to_string(),
        tool: "create_move_project".to_string(),
        input: serde_json::json!({"name":"demo"}),
        output: serde_json::json!({"success":true}),
        duration_ms: 5,
        success: true,
        error: None,
        cache_hit: None,
        llm_reason: Some("test".to_string()),
        tags: Some(vec!["unit".to_string()]),
    };

    logger.log_tool_call(&record).expect("log");

    let entries: Vec<PathBuf> = fs::read_dir(&log_dir)
        .expect("read log dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert!(!entries.is_empty());

    let content = fs::read_to_string(&entries[0]).expect("read log");
    assert!(content.contains("\"tool\":\"create_move_project\""));
}
