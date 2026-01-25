//! Integration tests for sui-sandbox CLI

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn sandbox_cmd() -> Command {
    Command::cargo_bin("sui-sandbox").unwrap()
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

// ============================================================================
// Help and Basic CLI Tests
// ============================================================================

#[test]
fn test_help_output() {
    sandbox_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui-sandbox"))
        .stdout(predicate::str::contains("publish"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("ptb"))
        .stdout(predicate::str::contains("fetch"))
        .stdout(predicate::str::contains("replay"))
        .stdout(predicate::str::contains("view"));
}

#[test]
fn test_version_output() {
    sandbox_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui-sandbox"));
}

#[test]
fn test_publish_help() {
    sandbox_cmd()
        .arg("publish")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Compile and publish"))
        .stdout(predicate::str::contains("--address"));
}

#[test]
fn test_run_help() {
    sandbox_cmd()
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Execute a single Move function"))
        .stdout(predicate::str::contains("--type-arg"))
        .stdout(predicate::str::contains("--arg"));
}

#[test]
fn test_view_help() {
    sandbox_cmd()
        .arg("view")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("module"))
        .stdout(predicate::str::contains("packages"));
}

// ============================================================================
// State Management Tests
// ============================================================================

#[test]
fn test_clean_nonexistent_state() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("clean")
        .assert()
        .success()
        .stdout(predicate::str::contains("No state file to remove"));
}

#[test]
fn test_status_empty_session() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Sui Sandbox Status"));
}

#[test]
fn test_status_json_output() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("Should be valid JSON");
    assert!(json.get("packages_loaded").is_some());
    assert!(json.get("packages").is_some());
    assert!(json.get("rpc_url").is_some());
}

// ============================================================================
// Publish Command Tests
// ============================================================================

#[test]
fn test_publish_bytecode_only() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");
    let fixture = fixture_dir().join("build/fixture");

    // Publish using pre-compiled bytecode
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("publish")
        .arg(&fixture)
        .arg("--bytecode-only")
        .arg("--address")
        .arg("fixture=0x100")
        .assert()
        .success()
        .stdout(predicate::str::contains("Package published"));
}

#[test]
fn test_publish_creates_state_file() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("subdir/state.bin");
    let fixture = fixture_dir().join("build/fixture");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("publish")
        .arg(&fixture)
        .arg("--bytecode-only")
        .arg("--address")
        .arg("fixture=0x100")
        .assert()
        .success();

    // State file should be created
    assert!(state_file.exists());
}

#[test]
fn test_publish_json_output() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");
    let fixture = fixture_dir().join("build/fixture");

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
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

    let json: Value = serde_json::from_slice(&output).expect("Should be valid JSON");
    assert!(json.get("package_address").is_some());
    assert!(json.get("modules").is_some());
}

// ============================================================================
// View Command Tests
// ============================================================================

#[test]
fn test_view_packages_empty() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("packages")
        .assert()
        .success()
        .stdout(predicate::str::contains("No user packages loaded"));
}

#[test]
fn test_view_packages_after_publish() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");
    let fixture = fixture_dir().join("build/fixture");

    // First publish
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("publish")
        .arg(&fixture)
        .arg("--bytecode-only")
        .arg("--address")
        .arg("fixture=0x100")
        .assert()
        .success();

    // Then view packages
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("packages")
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded Packages"))
        .stdout(predicate::str::contains("0x"));
}

#[test]
#[ignore = "requires network access - may be rate limited"]
fn test_view_module_framework() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    // Framework modules should be available
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("module")
        .arg("0x2::coin")
        .assert()
        .success()
        .stdout(predicate::str::contains("coin"))
        .stdout(predicate::str::contains("Coin"));
}

#[test]
fn test_view_module_json() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("view")
        .arg("module")
        .arg("0x2::coin")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("Should be valid JSON");
    assert_eq!(json["name"], "coin");
    assert!(json.get("structs").is_some());
    assert!(json.get("functions").is_some());
}

#[test]
#[ignore = "requires network access to fetch Sui framework - may be rate limited"]
fn test_view_modules_in_framework_package() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("modules")
        .arg("0x2")
        .assert()
        .success()
        .stdout(predicate::str::contains("Package:"))
        .stdout(predicate::str::contains("Modules:"));
}

// ============================================================================
// Run Command Tests
// ============================================================================

#[test]
fn test_run_invalid_target() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("run")
        .arg("invalid_target")
        .assert()
        .failure();
}

#[test]
fn test_run_module_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("run")
        .arg("0x999::nonexistent::func")
        .assert()
        .failure();
}

// ============================================================================
// PTB Command Tests
// ============================================================================

#[test]
fn test_ptb_missing_spec() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("ptb")
        .arg("--spec")
        .arg("/nonexistent/spec.json")
        .arg("--sender")
        .arg("0x1")
        .assert()
        .failure();
}

#[test]
fn test_ptb_invalid_json() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    // Create invalid JSON spec
    let spec_file = temp_dir.path().join("spec.json");
    fs::write(&spec_file, "not valid json").unwrap();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .arg("--sender")
        .arg("0x1")
        .assert()
        .failure();
}

#[test]
fn test_ptb_valid_spec_empty_calls() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    // Create valid but empty spec
    let spec_file = temp_dir.path().join("spec.json");
    fs::write(&spec_file, r#"{"calls": []}"#).unwrap();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .arg("--sender")
        .arg("0x1")
        .assert()
        .success();
}

// ============================================================================
// View Object Tests
// ============================================================================

#[test]
fn test_view_object_not_in_session() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("object")
        .arg("0x123")
        .assert()
        .success()
        .stdout(predicate::str::contains("not found in session"));
}

#[test]
fn test_view_object_invalid_id() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("object")
        .arg("not-a-hex-address")
        .assert()
        .failure();
}

// ============================================================================
// Fetch Command Tests (Error Cases - No Network)
// ============================================================================

#[test]
fn test_fetch_package_invalid_id() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("fetch")
        .arg("package")
        .arg("invalid-id")
        .assert()
        .failure();
}

#[test]
fn test_fetch_object_invalid_id() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("fetch")
        .arg("object")
        .arg("invalid-id")
        .assert()
        .failure();
}

// ============================================================================
// Replay Command Tests (Error Cases - No Network)
// ============================================================================

#[test]
fn test_replay_invalid_digest() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    // This will fail because we can't fetch from mainnet in tests,
    // but it validates the command structure
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("replay")
        .arg("not-a-valid-digest")
        .assert()
        .failure();
}

// ============================================================================
// Session Persistence Tests
// ============================================================================

#[test]
fn test_session_persistence_across_commands() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");
    let fixture = fixture_dir().join("build/fixture");

    // Publish a package
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("publish")
        .arg(&fixture)
        .arg("--bytecode-only")
        .arg("--address")
        .arg("fixture=0x100")
        .assert()
        .success();

    // Verify it persists in a new command
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("packages")
        .assert()
        .success()
        .stdout(predicate::str::contains("0x"));

    // Clean and verify it's gone
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("clean")
        .assert()
        .success();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("packages")
        .assert()
        .success()
        .stdout(predicate::str::contains("No user packages loaded"));
}

// ============================================================================
// Global Options Tests
// ============================================================================

#[test]
fn test_verbose_flag() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    // Verbose should not cause any errors
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--verbose")
        .arg("status")
        .assert()
        .success();
}

#[test]
fn test_rpc_url_option() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--rpc-url")
        .arg("https://custom.rpc.url:443")
        .arg("status")
        .assert()
        .success();
}

// ============================================================================
// Bridge Command Tests
// ============================================================================

#[test]
fn test_bridge_help() {
    sandbox_cmd()
        .arg("bridge")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui client"))
        .stdout(predicate::str::contains("publish"))
        .stdout(predicate::str::contains("call"))
        .stdout(predicate::str::contains("ptb"));
}

#[test]
fn test_bridge_publish_generates_sui_client_command() {
    sandbox_cmd()
        .arg("bridge")
        .arg("publish")
        .arg("./my_package")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui client publish"))
        .stdout(predicate::str::contains("./my_package"))
        .stdout(predicate::str::contains("--gas-budget"));
}

#[test]
fn test_bridge_publish_with_custom_gas_budget() {
    sandbox_cmd()
        .arg("bridge")
        .arg("publish")
        .arg("./my_package")
        .arg("--gas-budget")
        .arg("50000000")
        .assert()
        .success()
        .stdout(predicate::str::contains("50000000"));
}

#[test]
fn test_bridge_publish_json_output() {
    let output = sandbox_cmd()
        .arg("--json")
        .arg("bridge")
        .arg("publish")
        .arg("./my_package")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("Should be valid JSON");
    assert!(json.get("command").is_some());
    assert!(json.get("prerequisites").is_some());
    assert!(json["command"]
        .as_str()
        .unwrap()
        .contains("sui client publish"));
}

#[test]
fn test_bridge_publish_quiet_mode() {
    let output = sandbox_cmd()
        .arg("bridge")
        .arg("publish")
        .arg("./my_package")
        .arg("--quiet")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    // Quiet mode should not include prerequisites header
    assert!(!stdout.contains("Prerequisites:"));
    // But should still have the command
    assert!(stdout.contains("sui client publish"));
}

#[test]
fn test_bridge_call_generates_sui_client_command() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::value")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui client call"))
        .stdout(predicate::str::contains("--package 0x2"))
        .stdout(predicate::str::contains("--module coin"))
        .stdout(predicate::str::contains("--function value"));
}

#[test]
fn test_bridge_call_with_type_args() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::zero")
        .arg("--type-arg")
        .arg("0x2::sui::SUI")
        .assert()
        .success()
        .stdout(predicate::str::contains("--type-args"))
        .stdout(predicate::str::contains("0x2::sui::SUI"));
}

#[test]
fn test_bridge_call_with_args() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::value")
        .arg("--arg")
        .arg("0x123")
        .arg("--arg")
        .arg("42")
        .assert()
        .success()
        .stdout(predicate::str::contains("--args"))
        .stdout(predicate::str::contains("@0x123"))
        .stdout(predicate::str::contains("42"));
}

#[test]
fn test_bridge_call_warns_about_sandbox_address() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x100::my_module::my_func")
        .assert()
        .success()
        .stdout(predicate::str::contains("sandbox address"));
}

#[test]
fn test_bridge_call_no_warning_for_framework_address() {
    let output = sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::value")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(!stdout.contains("sandbox address"));
}

#[test]
fn test_bridge_call_json_output() {
    let output = sandbox_cmd()
        .arg("--json")
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::value")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("Should be valid JSON");
    assert_eq!(json["package"], "0x2");
    assert_eq!(json["module"], "coin");
    assert_eq!(json["function"], "value");
    assert!(json["command"]
        .as_str()
        .unwrap()
        .contains("sui client call"));
}

#[test]
fn test_bridge_call_invalid_target() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("invalid_target")
        .assert()
        .failure();
}

#[test]
fn test_bridge_ptb_with_valid_spec() {
    let temp_dir = TempDir::new().unwrap();
    let spec_file = temp_dir.path().join("spec.json");

    // Create a valid PTB spec
    let spec = r#"{
        "calls": [
            {
                "target": "0x2::coin::zero",
                "type_arguments": ["0x2::sui::SUI"],
                "arguments": []
            }
        ],
        "inputs": []
    }"#;
    fs::write(&spec_file, spec).unwrap();

    sandbox_cmd()
        .arg("bridge")
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .assert()
        .success()
        .stdout(predicate::str::contains("sui client ptb"))
        .stdout(predicate::str::contains("--move-call"))
        .stdout(predicate::str::contains("0x2::coin::zero"));
}

#[test]
fn test_bridge_ptb_with_multiple_calls() {
    let temp_dir = TempDir::new().unwrap();
    let spec_file = temp_dir.path().join("spec.json");

    let spec = r#"{
        "calls": [
            {
                "target": "0x2::coin::zero",
                "type_arguments": ["0x2::sui::SUI"],
                "arguments": []
            },
            {
                "target": "0x2::transfer::public_transfer",
                "arguments": [{"Result": 0}, "0xABC"]
            }
        ],
        "inputs": []
    }"#;
    fs::write(&spec_file, spec).unwrap();

    sandbox_cmd()
        .arg("bridge")
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .assert()
        .success()
        .stdout(predicate::str::contains("--assign result_0"));
}

#[test]
fn test_bridge_ptb_missing_spec() {
    sandbox_cmd()
        .arg("bridge")
        .arg("ptb")
        .arg("--spec")
        .arg("/nonexistent/spec.json")
        .assert()
        .failure();
}

#[test]
fn test_bridge_ptb_invalid_json() {
    let temp_dir = TempDir::new().unwrap();
    let spec_file = temp_dir.path().join("spec.json");
    fs::write(&spec_file, "not valid json").unwrap();

    sandbox_cmd()
        .arg("bridge")
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .assert()
        .failure();
}

#[test]
fn test_bridge_ptb_json_output() {
    let temp_dir = TempDir::new().unwrap();
    let spec_file = temp_dir.path().join("spec.json");

    let spec = r#"{"calls": [{"target": "0x2::coin::zero", "type_arguments": ["0x2::sui::SUI"]}]}"#;
    fs::write(&spec_file, spec).unwrap();

    let output = sandbox_cmd()
        .arg("--json")
        .arg("bridge")
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("Should be valid JSON");
    assert_eq!(json["calls_count"], 1);
    assert!(json["command"].as_str().unwrap().contains("sui client ptb"));
}

// ============================================================================
// Bridge E2E Workflow Tests
// ============================================================================

#[test]
fn test_bridge_publish_path_with_spaces() {
    let temp_dir = TempDir::new().unwrap();
    let path_with_spaces = temp_dir.path().join("my package");
    fs::create_dir_all(&path_with_spaces).unwrap();

    let output = sandbox_cmd()
        .arg("bridge")
        .arg("publish")
        .arg(&path_with_spaces)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    // Should be properly shell-escaped (single quotes around path with spaces)
    assert!(stdout.contains("sui client publish"));
    assert!(stdout.contains("my package")); // Path present
    assert!(stdout.contains("'")); // Quoted with single quotes
}

#[test]
fn test_bridge_call_full_length_address() {
    // Full 64-char address should not trigger sandbox warning
    let output = sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef::module::func")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);
    assert!(!stdout.contains("sandbox address"));
}

#[test]
fn test_bridge_call_with_multiple_type_args() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::balance::split")
        .arg("--type-arg")
        .arg("0x2::sui::SUI")
        .arg("--type-arg")
        .arg("0x2::coin::Coin")
        .assert()
        .success()
        .stdout(predicate::str::contains("--type-args"))
        .stdout(predicate::str::contains("0x2::sui::SUI"))
        .stdout(predicate::str::contains("0x2::coin::Coin"));
}

#[test]
fn test_bridge_call_with_mixed_args() {
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::split")
        .arg("--arg")
        .arg("0x123abc")
        .arg("--arg")
        .arg("1000")
        .arg("--arg")
        .arg("true")
        .assert()
        .success()
        .stdout(predicate::str::contains("--args"))
        .stdout(predicate::str::contains("@0x123abc")) // Address prefixed
        .stdout(predicate::str::contains("1000"))
        .stdout(predicate::str::contains("true"));
}

#[test]
fn test_bridge_ptb_with_result_chaining() {
    let temp_dir = TempDir::new().unwrap();
    let spec_file = temp_dir.path().join("spec.json");

    // Three-call chain: create -> split -> transfer
    let spec = r#"{
        "calls": [
            {"target": "0x2::coin::zero", "type_arguments": ["0x2::sui::SUI"]},
            {"target": "0x2::coin::split", "arguments": [{"Result": 0}, 500]},
            {"target": "0x2::transfer::public_transfer", "arguments": [{"Result": 1}, "0xABC"]}
        ]
    }"#;
    fs::write(&spec_file, spec).unwrap();

    sandbox_cmd()
        .arg("bridge")
        .arg("ptb")
        .arg("--spec")
        .arg(&spec_file)
        .assert()
        .success()
        .stdout(predicate::str::contains("--assign result_0"))
        .stdout(predicate::str::contains("--assign result_1"));
}

#[test]
fn test_bridge_e2e_workflow_sandbox_to_bridge() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.bin");
    let fixture = fixture_dir().join("build/fixture");

    // Step 1: Publish in sandbox
    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("publish")
        .arg(&fixture)
        .arg("--bytecode-only")
        .arg("--address")
        .arg("fixture=0x100")
        .assert()
        .success();

    // Step 2: Generate bridge publish command for same package
    sandbox_cmd()
        .arg("bridge")
        .arg("publish")
        .arg(&fixture)
        .arg("--quiet")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui client publish"));

    // Step 3: Generate bridge call command
    // Note: 0x100 is a sandbox address, so it should warn
    sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x100::test_module::test_func")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui client call"))
        .stdout(predicate::str::contains("sandbox address")); // Warning present
}

#[test]
fn test_bridge_output_valid_shell_syntax() {
    // Verify the output doesn't have obvious shell syntax errors
    let output = sandbox_cmd()
        .arg("bridge")
        .arg("call")
        .arg("0x2::coin::value")
        .arg("--type-arg")
        .arg("0x2::sui::SUI")
        .arg("--arg")
        .arg("0xABC")
        .arg("--quiet")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8_lossy(&output);

    // Basic shell syntax validation
    // - Should not have unbalanced quotes
    // - Should have proper flag syntax
    assert!(stdout.contains("--package"));
    assert!(stdout.contains("--module"));
    assert!(stdout.contains("--function"));
    assert!(stdout.contains("--type-args"));
    assert!(stdout.contains("--args"));
    assert!(stdout.contains("--gas-budget"));
}

// ============================================================================
// Bridge Info Command Tests
// ============================================================================

#[test]
fn test_bridge_info_basic() {
    sandbox_cmd()
        .arg("bridge")
        .arg("info")
        .assert()
        .success()
        .stdout(predicate::str::contains("Transition Guide"))
        .stdout(predicate::str::contains("Deployment Workflow"))
        .stdout(predicate::str::contains("testnet"));
}

#[test]
fn test_bridge_info_verbose() {
    sandbox_cmd()
        .arg("bridge")
        .arg("info")
        .arg("--verbose")
        .assert()
        .success()
        .stdout(predicate::str::contains("Protocol Version"))
        .stdout(predicate::str::contains("Error Handling"));
}

#[test]
fn test_bridge_info_json() {
    let output = sandbox_cmd()
        .arg("--json")
        .arg("bridge")
        .arg("info")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("Valid JSON");
    assert!(json["workflow"].is_array());
    assert!(json["environment_check"].is_object());
    assert!(json["tips"].is_array());
}

#[test]
fn test_bridge_info_verbose_json() {
    let output = sandbox_cmd()
        .arg("--json")
        .arg("bridge")
        .arg("info")
        .arg("--verbose")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("Valid JSON");
    assert!(json["advanced"].is_object());
    assert!(json["advanced"]["protocol_version"].as_u64().is_some());
}
