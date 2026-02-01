#![allow(deprecated)]
//! Parity harness between CLI workflow and MCP tool workflow.

use assert_cmd::Command;
use base64::Engine;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn sandbox_cmd() -> Command {
    Command::cargo_bin("sui-sandbox").expect("binary not found")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn load_fixture_modules() -> Vec<Value> {
    let bytecode_dir = fixture_dir().join("build/fixture/bytecode_modules");
    let mut modules = Vec::new();
    for entry in fs::read_dir(bytecode_dir).expect("bytecode_modules exists") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().map(|e| e == "mv").unwrap_or(false) {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let bytes = fs::read(&path).expect("read module");
            let bytes_b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            modules.push(json!({ "name": name, "bytes_b64": bytes_b64 }));
        }
    }
    modules
}

fn run_tool(sandbox_home: &Path, state_file: &Path, tool: &str, input: &Value) -> Value {
    let output = sandbox_cmd()
        .env("SUI_SANDBOX_HOME", sandbox_home)
        .arg("--state-file")
        .arg(state_file)
        .arg("--json")
        .arg("tool")
        .arg(tool)
        .arg("--input")
        .arg(serde_json::to_string(input).expect("serialize input"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).expect("valid JSON output")
}

fn read_mcp_logs(log_dir: &Path) -> Vec<Value> {
    if !log_dir.exists() {
        return Vec::new();
    }
    let mut entries = Vec::new();
    let mut files: Vec<PathBuf> = fs::read_dir(log_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    files.sort();
    for file in files {
        let content = fs::read_to_string(&file).unwrap_or_default();
        for line in content.lines() {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                entries.push(value);
            }
        }
    }
    entries
}

#[test]
fn test_replay_parity_dynamic_field_filtering() {
    if env::var("SUI_RUN_REPLAY_PARITY_TESTS").is_err() {
        eprintln!("Skipping replay parity test (set SUI_RUN_REPLAY_PARITY_TESTS=1 to enable)");
        return;
    }

    let digest = "S4CNtSVn2A7HW1QNCsTNJJRRp4MSrHeEKvTWydCgNfq";
    let temp_dir = TempDir::new().unwrap();
    let sandbox_home = temp_dir.path().join("home");
    fs::create_dir_all(&sandbox_home).unwrap();

    let cli_state = sandbox_home.join("cli-state.json");
    let mcp_state = sandbox_home.join("mcp-state.json");

    let cli_output = sandbox_cmd()
        .env("SUI_SANDBOX_HOME", &sandbox_home)
        .arg("--state-file")
        .arg(&cli_state)
        .arg("--json")
        .arg("replay")
        .arg(digest)
        .arg("--compare")
        .arg("--fetch-strategy")
        .arg("full")
        .arg("--prefetch-depth")
        .arg("3")
        .arg("--prefetch-limit")
        .arg("200")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let cli_json: Value = serde_json::from_slice(&cli_output).unwrap();
    let cli_comparison = cli_json.get("comparison").expect("comparison");
    assert_eq!(
        cli_comparison.get("status_match").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        cli_comparison
            .get("created_match")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        cli_comparison
            .get("mutated_match")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        cli_comparison
            .get("deleted_match")
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    let mcp_input = json!({
        "digest": digest,
        "options": {
            "compare_effects": true,
            "fetch_strategy": "full",
            "prefetch_depth": 3,
            "prefetch_limit": 200,
            "auto_system_objects": true
        }
    });
    let mcp_output = run_tool(&sandbox_home, &mcp_state, "replay_transaction", &mcp_input);
    assert_eq!(
        mcp_output.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    let effects_match = mcp_output
        .get("result")
        .and_then(|v| v.get("effects_match"))
        .and_then(|v| v.as_bool());
    assert_eq!(effects_match, Some(true));
}

#[test]
fn test_cli_mcp_parity_fixture_workflow() {
    let temp_dir = TempDir::new().unwrap();
    let sandbox_home = temp_dir.path().join("home");
    fs::create_dir_all(&sandbox_home).unwrap();

    let cli_state = sandbox_home.join("cli-state.json");
    let mcp_state = sandbox_home.join("mcp-state.json");

    let fixture = fixture_dir().join("build/fixture");

    let publish_output = sandbox_cmd()
        .env("SUI_SANDBOX_HOME", &sandbox_home)
        .arg("--state-file")
        .arg(&cli_state)
        .arg("--json")
        .arg("publish")
        .arg(&fixture)
        .arg("--bytecode-only")
        .arg("--address")
        .arg("fixture=0x100")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let publish_json: Value = serde_json::from_slice(&publish_output).unwrap();
    let package_address = publish_json
        .get("package_address")
        .and_then(|v| v.as_str())
        .expect("package_address")
        .to_string();

    let run_output = sandbox_cmd()
        .env("SUI_SANDBOX_HOME", &sandbox_home)
        .arg("--state-file")
        .arg(&cli_state)
        .arg("--json")
        .arg("run")
        .arg(format!("{}::test_module::simple_func", package_address))
        .arg("--arg")
        .arg("42")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let run_json: Value = serde_json::from_slice(&run_output).unwrap();
    assert_eq!(
        run_json.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    let cli_gas = run_json
        .get("gas_used")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let modules = load_fixture_modules();
    let load_input = json!({
        "package_id": package_address,
        "modules": modules,
        "_meta": {
            "reason": "parity_test_load",
            "tags": ["parity"]
        }
    });
    let load_output = run_tool(&sandbox_home, &mcp_state, "load_package_bytes", &load_input);
    assert_eq!(
        load_output.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );

    let call_input = json!({
        "package": package_address,
        "module": "test_module",
        "function": "simple_func",
        "args": [42],
        "_meta": {
            "reason": "parity_test_call",
            "tags": ["parity"]
        }
    });
    let call_output = run_tool(&sandbox_home, &mcp_state, "call_function", &call_input);
    assert_eq!(
        call_output.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    let mcp_gas = call_output
        .get("result")
        .and_then(|r| r.get("gas_used"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    assert_eq!(cli_gas, mcp_gas);

    let log_dir = sandbox_home.join("logs").join("mcp");
    let logs = read_mcp_logs(&log_dir);
    let call_log = logs.iter().find(|entry| {
        entry
            .get("tool")
            .and_then(|v| v.as_str())
            .map(|t| t == "call_function")
            .unwrap_or(false)
    });
    let call_log = call_log.expect("call_function log");
    assert_eq!(
        call_log
            .get("llm_reason")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "parity_test_call"
    );
    let tags = call_log
        .get("tags")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        tags.iter().any(|v| v.as_str() == Some("parity")),
        "expected parity tag"
    );
}
