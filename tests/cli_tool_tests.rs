use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn sandbox_cmd() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("sui-sandbox"))
}

fn run_tool(
    home: &std::path::Path,
    state_file: &std::path::Path,
    name: &str,
    input: &Value,
) -> Value {
    let output = sandbox_cmd()
        .env("HOME", home)
        .arg("--state-file")
        .arg(state_file)
        .arg("tool")
        .arg(name)
        .arg("--input")
        .arg(input.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&output).expect("valid json")
}

fn run_tool_with_args(
    home: &std::path::Path,
    state_file: &std::path::Path,
    name: &str,
    input: &Value,
    extra_args: &[&str],
) -> Value {
    let mut cmd = sandbox_cmd();
    cmd.env("HOME", home)
        .arg("--state-file")
        .arg(state_file)
        .arg("tool");
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd
        .arg(name)
        .arg("--input")
        .arg(input.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&output).expect("valid json")
}

#[test]
fn cli_tool_project_flow() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("home dir");
    let state_file = temp.path().join("mcp-state.json");

    let create = run_tool(
        &home,
        &state_file,
        "create_move_project",
        &serde_json::json!({
            "name": "cli_demo",
            "persist": false
        }),
    );
    assert!(create["success"].as_bool().unwrap_or(false));
    let project_id = create["result"]["project_id"]
        .as_str()
        .expect("project id")
        .to_string();

    let read = run_tool(
        &home,
        &state_file,
        "read_move_file",
        &serde_json::json!({
            "project_id": project_id.clone(),
            "file": "sources/cli_demo.move"
        }),
    );
    assert!(read["success"].as_bool().unwrap_or(false));
    let content = read["result"]["content"].as_str().unwrap_or("");
    assert!(content.contains("module cli_demo"));

    let updated_source = "module cli_demo { public fun ping() { } }";
    let edit = run_tool(
        &home,
        &state_file,
        "edit_move_file",
        &serde_json::json!({
            "project_id": project_id.clone(),
            "file": "sources/cli_demo.move",
            "content": updated_source
        }),
    );
    assert!(edit["success"].as_bool().unwrap_or(false));

    let reread = run_tool(
        &home,
        &state_file,
        "read_move_file",
        &serde_json::json!({
            "project_id": project_id.clone(),
            "file": "sources/cli_demo.move"
        }),
    );
    assert!(reread["success"].as_bool().unwrap_or(false));
    let content = reread["result"]["content"].as_str().unwrap_or("");
    assert!(content.contains("fun ping"));

    let list = run_tool(&home, &state_file, "list_projects", &serde_json::json!({}));
    assert!(list["success"].as_bool().unwrap_or(false));
    let projects = list["result"]["projects"]
        .as_array()
        .expect("projects array");
    assert!(!projects.is_empty());
    assert!(projects.iter().any(|p| p["id"] == project_id));
}

#[test]
fn cli_tool_persists_provider_config() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("home dir");
    let state_file = temp.path().join("mcp-state.json");

    let _ = run_tool_with_args(
        &home,
        &state_file,
        "list_projects",
        &serde_json::json!({}),
        &[
            "--network",
            "testnet",
            "--graphql-url",
            "https://example.test/graphql",
            "--rpc-url",
            "https://grpc.example.test:443",
        ],
    );

    let provider_path = state_file
        .parent()
        .expect("state file parent")
        .join(format!(
            "{}.provider.json",
            state_file
                .file_name()
                .expect("state file name")
                .to_string_lossy()
        ));
    let data = fs::read_to_string(&provider_path).expect("provider config");
    let json: Value = serde_json::from_str(&data).expect("provider json");
    assert_eq!(json["network"], "testnet");
    assert_eq!(json["graphql_endpoint"], "https://example.test/graphql");
    assert_eq!(json["grpc_endpoint"], "https://grpc.example.test:443");
}

#[test]
fn cli_tool_status_includes_state_file() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir_all(&home).expect("home dir");
    let state_file = temp.path().join("mcp-state.json");

    let status = run_tool(&home, &state_file, "status", &serde_json::json!({}));
    assert!(status["success"].as_bool().unwrap_or(false));

    let state_path = status["state_file"].as_str().unwrap_or("");
    assert_eq!(state_path, state_file.to_string_lossy());
}
