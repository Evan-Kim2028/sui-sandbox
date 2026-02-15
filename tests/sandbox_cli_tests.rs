#![allow(deprecated)]
//! Integration tests for sui-sandbox CLI

use assert_cmd::Command;
use base64::Engine;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn sandbox_cmd() -> Command {
    Command::cargo_bin("sui-sandbox").expect("binary not found")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn write_minimal_replay_state_json(temp_dir: &TempDir) -> PathBuf {
    let path = temp_dir.path().join("replay_state.json");
    let state = serde_json::json!({
        "transaction": {
            "digest": "dummy_digest",
            "sender": "0x1",
            "gas_budget": 1_000_000u64,
            "gas_price": 1_000u64,
            "commands": [],
            "inputs": [],
            "effects": serde_json::Value::Null,
            "timestamp_ms": serde_json::Value::Null,
            "checkpoint": serde_json::Value::Null
        },
        "objects": {},
        "packages": {},
        "protocol_version": 64u64,
        "epoch": 0u64,
        "reference_gas_price": serde_json::Value::Null,
        "checkpoint": serde_json::Value::Null
    });
    fs::write(&path, serde_json::to_string_pretty(&state).unwrap()).expect("write replay state");
    path
}

fn write_multi_replay_state_json(temp_dir: &TempDir) -> PathBuf {
    let path = temp_dir.path().join("replay_state_multi.json");
    let state_a = serde_json::json!({
        "transaction": {
            "digest": "digest_a",
            "sender": "0x1",
            "gas_budget": 1_000_000u64,
            "gas_price": 1_000u64,
            "commands": [],
            "inputs": [],
            "effects": serde_json::Value::Null,
            "timestamp_ms": serde_json::Value::Null,
            "checkpoint": serde_json::Value::Null
        },
        "objects": {},
        "packages": {},
        "protocol_version": 64u64,
        "epoch": 0u64,
        "reference_gas_price": serde_json::Value::Null,
        "checkpoint": serde_json::Value::Null
    });
    let state_b = serde_json::json!({
        "transaction": {
            "digest": "digest_b",
            "sender": "0x2",
            "gas_budget": 2_000_000u64,
            "gas_price": 2_000u64,
            "commands": [],
            "inputs": [],
            "effects": serde_json::Value::Null,
            "timestamp_ms": serde_json::Value::Null,
            "checkpoint": serde_json::Value::Null
        },
        "objects": {},
        "packages": {},
        "protocol_version": 64u64,
        "epoch": 0u64,
        "reference_gas_price": serde_json::Value::Null,
        "checkpoint": serde_json::Value::Null
    });
    let states = serde_json::json!([state_a, state_b]);
    fs::write(&path, serde_json::to_string_pretty(&states).unwrap())
        .expect("write multi replay state");
    path
}

fn write_python_style_flow_context_json(temp_dir: &TempDir) -> PathBuf {
    let path = temp_dir.path().join("flow_context.python.json");
    let module_path = fixture_dir().join("build/fixture/bytecode_modules/test_module.mv");
    let module_bytes = fs::read(&module_path).expect("read fixture module bytecode");
    let encoded = base64::engine::general_purpose::STANDARD.encode(module_bytes);
    let payload = serde_json::json!({
        "version": 1u64,
        "package_id": "0x100",
        "resolve_deps": false,
        "generated_at_ms": 0u64,
        "packages": {
            "0x100": [encoded]
        },
        "count": 1u64
    });
    fs::write(&path, serde_json::to_string_pretty(&payload).unwrap())
        .expect("write python-style flow context");
    path
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
        .stdout(predicate::str::contains("import"))
        .stdout(predicate::str::contains("replay"))
        .stdout(predicate::str::contains("view"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("context"))
        .stdout(predicate::str::contains("adapter"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("script"))
        .stdout(predicate::str::contains("pipeline"))
        .stdout(predicate::str::contains("snapshot"))
        .stdout(predicate::str::contains("reset"));
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

#[test]
fn test_tools_help_excludes_internal_harness_commands() {
    sandbox_cmd()
        .arg("tools")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("poll-transactions"))
        .stdout(predicate::str::contains("stream-transactions"))
        .stdout(predicate::str::contains("tx-sim"))
        .stdout(predicate::str::contains("ptb-replay-harness").not())
        .stdout(predicate::str::contains("walrus-warmup").not());
}

#[test]
fn test_tools_call_view_function_help_includes_historical_flags() {
    sandbox_cmd()
        .arg("tools")
        .arg("call-view-function")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--checkpoint"))
        .stdout(predicate::str::contains("--grpc-endpoint"))
        .stdout(predicate::str::contains("--historical-packages-file"));
}

#[test]
fn test_doctor_help() {
    sandbox_cmd()
        .arg("doctor")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Validate local environment"))
        .stdout(predicate::str::contains("--timeout-secs"));
}

#[test]
fn test_flow_help_lists_subcommands() {
    sandbox_cmd()
        .arg("context")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("prepare"))
        .stdout(predicate::str::contains("replay"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("discover"));
}

#[test]
fn test_flow_alias_help_lists_subcommands() {
    sandbox_cmd()
        .arg("flow")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("prepare"))
        .stdout(predicate::str::contains("replay"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("discover"));
}

#[test]
fn test_protocol_help_lists_subcommands() {
    sandbox_cmd()
        .arg("adapter")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("prepare"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("discover"));
}

#[test]
fn test_protocol_alias_help_lists_subcommands() {
    sandbox_cmd()
        .arg("protocol")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("prepare"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("discover"));
}

#[test]
fn test_protocol_prepare_requires_package_override_when_no_default() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("protocol")
        .arg("prepare")
        .arg("--protocol")
        .arg("cetus")
        .assert()
        .failure()
        .stderr(predicate::str::contains("provide --package-id"));
}

#[test]
fn test_protocol_discover_requires_package_override_when_no_default() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("protocol")
        .arg("discover")
        .arg("--protocol")
        .arg("suilend")
        .arg("--checkpoint")
        .arg("1")
        .assert()
        .failure()
        .stderr(predicate::str::contains("provide --package-id"));
}

#[test]
fn test_flow_replay_accepts_python_context_wrapper() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let replay_state = write_minimal_replay_state_json(&temp_dir);
    let context = write_python_style_flow_context_json(&temp_dir);

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("flow")
        .arg("replay")
        .arg("dummy_digest")
        .arg("--context")
        .arg(&context)
        .arg("--state-json")
        .arg(&replay_state)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("flow replay should emit JSON");
    assert_eq!(json["local_success"], true);
    assert_eq!(json["execution_path"]["effective_source"], "state_json");
}

#[test]
fn test_flow_discover_rejects_zero_limit() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("flow")
        .arg("discover")
        .arg("--checkpoint")
        .arg("1")
        .arg("--limit")
        .arg("0")
        .assert()
        .failure()
        .stderr(predicate::str::contains("limit must be greater than zero"));
}

#[test]
fn test_flow_discover_rejects_invalid_package_id() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("flow")
        .arg("discover")
        .arg("--checkpoint")
        .arg("1")
        .arg("--package-id")
        .arg("not-a-package")
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid package id"));
}

#[test]
fn test_flow_run_requires_target_selection() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("flow")
        .arg("run")
        .arg("--package-id")
        .arg("0x2")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Provide --digest, --state-json, or --discover-latest",
        ));
}

#[test]
fn test_flow_replay_discover_latest_requires_package_context() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("flow")
        .arg("replay")
        .arg("--discover-latest")
        .arg("5")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--discover-latest requires package context",
        ));
}

#[test]
fn test_protocol_run_requires_target_selection() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("protocol")
        .arg("run")
        .arg("--protocol")
        .arg("deepbook")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Provide --digest, --state-json, or --discover-latest",
        ));
}

// ============================================================================
// State Management Tests
// ============================================================================

#[test]
fn test_clean_nonexistent_state() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    assert!(json.get("objects_loaded").is_some());
    assert!(json.get("modules_loaded").is_some());
    assert!(json.get("dynamic_fields_loaded").is_some());
    assert!(json.get("rpc_url").is_some());
    assert!(json.get("state_file").is_some());
}

// ============================================================================
// Publish Command Tests
// ============================================================================

#[test]
fn test_publish_bytecode_only() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
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
    let state_file = temp_dir.path().join("subdir/state.json");
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
    let state_file = temp_dir.path().join("state.json");
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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");
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
#[cfg(feature = "network-tests")]
#[ignore = "requires network access - may be rate limited"]
fn test_view_module_framework() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

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
#[cfg(feature = "network-tests")]
#[ignore = "requires network access to fetch Sui framework - module not bundled in state"]
fn test_view_module_json() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

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
#[cfg(feature = "network-tests")]
#[ignore = "requires network access to fetch Sui framework - may be rate limited"]
fn test_view_modules_in_framework_package() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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
    let state_file = temp_dir.path().join("state.json");

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

#[test]
fn test_replay_and_analyze_replay_help_share_hydration_flags() {
    let replay_help = sandbox_cmd()
        .arg("replay")
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let analyze_help = sandbox_cmd()
        .arg("analyze")
        .arg("replay")
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let replay_help = String::from_utf8_lossy(&replay_help);
    let analyze_help = String::from_utf8_lossy(&analyze_help);
    let shared_flags = [
        "--source <SOURCE>",
        "--cache-dir <CACHE_DIR>",
        "--allow-fallback <ALLOW_FALLBACK>",
        "--auto-system-objects <AUTO_SYSTEM_OBJECTS>",
        "--prefetch-depth <PREFETCH_DEPTH>",
        "--prefetch-limit <PREFETCH_LIMIT>",
        "--no-prefetch",
    ];

    for flag in shared_flags {
        assert!(
            replay_help.contains(flag),
            "replay --help missing shared hydration flag: {flag}"
        );
        assert!(
            analyze_help.contains(flag),
            "analyze replay --help missing shared hydration flag: {flag}"
        );
    }
}

#[test]
fn test_replay_help_includes_execution_path_flags() {
    sandbox_cmd()
        .arg("replay")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--source"))
        .stdout(predicate::str::contains("--allow-fallback"))
        .stdout(predicate::str::contains("--vm-only"))
        .stdout(predicate::str::contains("--synthesize-missing"))
        .stdout(predicate::str::contains("--self-heal-dynamic-fields"));
}

#[test]
fn test_analyze_replay_help_includes_mm2_flag() {
    sandbox_cmd()
        .arg("analyze")
        .arg("replay")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--mm2"));
}

#[test]
fn test_replay_help_hides_igloo_flags_without_feature() {
    sandbox_cmd()
        .arg("replay")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--igloo-hybrid-loader").not())
        .stdout(predicate::str::contains("--igloo-config").not())
        .stdout(predicate::str::contains("--igloo-command").not());
}

#[test]
fn test_replay_mutate_help_includes_target_and_mode_flags() {
    sandbox_cmd()
        .arg("replay")
        .arg("mutate")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--fixture"))
        .stdout(predicate::str::contains("--targets-file"))
        .stdout(predicate::str::contains("--no-op"))
        .stdout(predicate::str::contains("--demo"))
        .stdout(predicate::str::contains("--strategy"))
        .stdout(predicate::str::contains("--mutator"))
        .stdout(predicate::str::contains("--oracle"))
        .stdout(predicate::str::contains("--invariant"))
        .stdout(predicate::str::contains("--minimize"))
        .stdout(predicate::str::contains("--replay-source"))
        .stdout(predicate::str::contains("--jobs"))
        .stdout(predicate::str::contains("--retries"))
        .stdout(predicate::str::contains("--keep-going"))
        .stdout(predicate::str::contains("--differential-source"))
        .stdout(predicate::str::contains("--corpus-in"))
        .stdout(predicate::str::contains("--corpus-out"));
}

#[test]
fn test_replay_mutate_noop_fixture_json_output() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/replay_mutation_fixture_v1.json");

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("mutate")
        .arg("--fixture")
        .arg(&fixture)
        .arg("--no-op")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("valid replay mutate JSON");
    assert_eq!(json["status"].as_str(), Some("no_op"));
    assert!(
        json["targets"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "replay mutate no-op should resolve at least one fixture target"
    );
    assert_eq!(json["strategy"]["name"].as_str(), Some("default"));
}

#[test]
fn test_replay_mutate_noop_strategy_override() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/replay_mutation_fixture_v1.json");
    let strategy = temp_dir.path().join("strategy.yaml");
    fs::write(
        &strategy,
        r#"
name: test-strategy
mutators:
  - baseline_vs_heal
oracles:
  - fail_to_heal
invariants:
  - commands_executed_gt_zero
scoring: balanced
minimization:
  enabled: false
  mode: none
"#,
    )
    .unwrap();

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("mutate")
        .arg("--fixture")
        .arg(&fixture)
        .arg("--strategy")
        .arg(&strategy)
        .arg("--no-op")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("valid replay mutate JSON");
    assert_eq!(json["strategy"]["name"].as_str(), Some("test-strategy"));
    assert_eq!(
        json["mutation_plan"]["mutators"][0].as_str(),
        Some("baseline_vs_heal")
    );
    assert_eq!(json["oracle_plan"]["scoring"].as_str(), Some("balanced"));
    assert_eq!(json["oracle_plan"]["minimization"].as_bool(), Some(false));
}

#[test]
fn test_replay_mutate_latest_requires_walrus_source() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("replay")
        .arg("mutate")
        .arg("--latest")
        .arg("2")
        .arg("--replay-source")
        .arg("grpc")
        .arg("--no-op")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--latest discovery currently requires --replay-source walrus",
        ));
}

#[test]
fn test_replay_mutate_differential_source_must_differ() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/replay_mutation_fixture_v1.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("replay")
        .arg("mutate")
        .arg("--fixture")
        .arg(&fixture)
        .arg("--replay-source")
        .arg("walrus")
        .arg("--differential-source")
        .arg("walrus")
        .arg("--no-op")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--differential-source must differ from --replay-source",
        ));
}

#[test]
fn test_replay_json_output_execution_path_contract_from_state_json() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let replay_state_path = write_minimal_replay_state_json(&temp_dir);

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("anydigest")
        .arg("--state-json")
        .arg(&replay_state_path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("valid replay JSON");
    let execution_path = json
        .get("execution_path")
        .and_then(Value::as_object)
        .expect("execution_path object");

    for key in [
        "requested_source",
        "effective_source",
        "dependency_fetch_mode",
    ] {
        assert!(
            execution_path.get(key).and_then(Value::as_str).is_some(),
            "execution_path.{key} should be a string"
        );
    }
    for key in [
        "vm_only",
        "allow_fallback",
        "auto_system_objects",
        "fallback_used",
        "dynamic_field_prefetch",
    ] {
        assert!(
            execution_path.get(key).and_then(Value::as_bool).is_some(),
            "execution_path.{key} should be a bool"
        );
    }
    for key in [
        "prefetch_depth",
        "prefetch_limit",
        "dependency_packages_fetched",
        "synthetic_inputs",
    ] {
        assert!(
            execution_path.get(key).and_then(Value::as_u64).is_some(),
            "execution_path.{key} should be a u64"
        );
    }
}

#[test]
fn test_replay_analyze_only_emits_analysis_payload() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let replay_state_path = write_minimal_replay_state_json(&temp_dir);

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("anydigest")
        .arg("--state-json")
        .arg(&replay_state_path)
        .arg("--analyze-only")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("valid replay analyze JSON");
    assert_eq!(
        json.get("local_success").and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        json.get("effects").is_none(),
        "analyze-only should not include effects"
    );
    let analysis = json
        .get("analysis")
        .and_then(Value::as_object)
        .expect("analysis payload expected");
    assert!(analysis.get("commands").and_then(Value::as_u64).is_some());
    assert!(analysis.get("packages").and_then(Value::as_u64).is_some());
}

#[test]
fn test_replay_auto_system_objects_explicit_bool_true_false() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let replay_state_path = write_minimal_replay_state_json(&temp_dir);

    let false_output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("anydigest")
        .arg("--state-json")
        .arg(&replay_state_path)
        .arg("--auto-system-objects")
        .arg("false")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let false_json: Value = serde_json::from_slice(&false_output).expect("valid replay JSON");
    assert_eq!(
        false_json["execution_path"]["auto_system_objects"].as_bool(),
        Some(false)
    );

    let true_output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("anydigest")
        .arg("--state-json")
        .arg(&replay_state_path)
        .arg("--auto-system-objects")
        .arg("true")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let true_json: Value = serde_json::from_slice(&true_output).expect("valid replay JSON");
    assert_eq!(
        true_json["execution_path"]["auto_system_objects"].as_bool(),
        Some(true)
    );
}

#[test]
fn test_replay_state_json_multi_state_select_by_digest() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let replay_state_path = write_multi_replay_state_json(&temp_dir);

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("digest_b")
        .arg("--state-json")
        .arg(&replay_state_path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("valid replay JSON");
    assert_eq!(json["digest"].as_str(), Some("digest_b"));
}

#[test]
fn test_import_state_file_and_replay_from_local_cache() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let replay_state_path = write_minimal_replay_state_json(&temp_dir);
    let cache_dir = temp_dir.path().join("replay_cache");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("import")
        .arg("--state")
        .arg(&replay_state_path)
        .arg("--output")
        .arg(&cache_dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Imported 1 replay state"));

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--json")
        .arg("replay")
        .arg("dummy_digest")
        .arg("--source")
        .arg("local")
        .arg("--cache-dir")
        .arg(&cache_dir)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).expect("valid replay JSON");
    assert_eq!(
        json["execution_path"]["effective_source"].as_str(),
        Some("local_cache")
    );
}

// ============================================================================
// Session Persistence Tests
// ============================================================================

#[test]
fn test_session_persistence_across_commands() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
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

#[test]
fn test_reset_clears_session_packages() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
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

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("reset")
        .assert()
        .success()
        .stdout(predicate::str::contains("Session reset"));

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
fn test_snapshot_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
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

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("snapshot")
        .arg("save")
        .arg("fixture-state")
        .assert()
        .success()
        .stdout(predicate::str::contains("Saved snapshot"));

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("snapshot")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("fixture-state"));

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("reset")
        .assert()
        .success();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("snapshot")
        .arg("load")
        .arg("fixture-state")
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded snapshot"));

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("view")
        .arg("packages")
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded Packages"));

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("snapshot")
        .arg("delete")
        .arg("fixture-state")
        .assert()
        .success()
        .stdout(predicate::str::contains("Deleted snapshot"));
}

#[test]
fn test_init_creates_flow_template() {
    let temp_dir = TempDir::new().unwrap();
    sandbox_cmd()
        .arg("init")
        .arg("--example")
        .arg("quickstart")
        .arg("--output-dir")
        .arg(temp_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized script template"));

    assert!(temp_dir.path().join("flow.quickstart.yaml").exists());
    assert!(temp_dir.path().join("FLOW_README.md").exists());
}

#[test]
fn test_run_flow_dry_run() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let flow_file = temp_dir.path().join("flow.yaml");

    fs::write(
        &flow_file,
        r#"version: 1
steps:
  - command: ["status"]
  - command: ["view", "packages"]
"#,
    )
    .unwrap();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("run-flow")
        .arg(&flow_file)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("Flow complete"));
}

#[test]
fn test_run_flow_end_to_end_publish_and_view() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let flow_file = temp_dir.path().join("flow.yaml");

    let fixture = fixture_dir().join("build/fixture");
    let yaml = format!(
        "version: 1\nsteps:\n  - command: [\"publish\", \"{}\", \"--bytecode-only\", \"--address\", \"fixture=0x100\"]\n  - command: [\"view\", \"packages\"]\n",
        fixture.display()
    );
    fs::write(&flow_file, yaml).unwrap();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("run-flow")
        .arg(&flow_file)
        .assert()
        .success()
        .stdout(predicate::str::contains("Flow complete"));
}

#[test]
fn test_workflow_validate_core_example_spec() {
    let spec = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/workflow_replay_analyze_demo.json");

    sandbox_cmd()
        .arg("workflow")
        .arg("validate")
        .arg("--spec")
        .arg(&spec)
        .assert()
        .success()
        .stdout(predicate::str::contains("Workflow spec valid"));
}

#[test]
fn test_workflow_auto_help_includes_discovery_flags() {
    sandbox_cmd()
        .arg("workflow")
        .arg("auto")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--discover-latest"))
        .stdout(predicate::str::contains("--walrus-network"))
        .stdout(predicate::str::contains("--walrus-caching-url"))
        .stdout(predicate::str::contains("--walrus-aggregator-url"));
}

#[test]
fn test_workflow_run_core_example_dry_run() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let spec = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/data/workflow_replay_analyze_demo.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("workflow")
        .arg("run")
        .arg("--spec")
        .arg(&spec)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("Workflow complete"));
}

#[test]
fn test_workflow_run_dry_run_report_includes_native_flags() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");
    let spec_path = temp_dir.path().join("workflow.flags.json");
    let report_path = temp_dir.path().join("workflow.report.json");

    let spec = serde_json::json!({
        "version": 1,
        "defaults": {
            "source": "hybrid",
            "vm_only": true,
            "synthesize_missing": true,
            "self_heal_dynamic_fields": true,
            "mm2": true
        },
        "steps": [
            {
                "id": "inspect",
                "kind": "analyze_replay",
                "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
                "checkpoint": 239615926
            },
            {
                "id": "replay",
                "kind": "replay",
                "digest": "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
                "checkpoint": "239615926"
            }
        ]
    });
    fs::write(&spec_path, serde_json::to_string_pretty(&spec).unwrap()).unwrap();

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("workflow")
        .arg("run")
        .arg("--spec")
        .arg(&spec_path)
        .arg("--dry-run")
        .arg("--report")
        .arg(&report_path)
        .assert()
        .success();

    let report_raw = fs::read_to_string(&report_path).expect("read workflow report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report json");
    let steps = report
        .get("steps")
        .and_then(Value::as_array)
        .expect("steps array");
    assert_eq!(steps.len(), 2);

    let analyze_cmd = steps[0]
        .get("command")
        .and_then(Value::as_array)
        .expect("analyze command");
    let replay_cmd = steps[1]
        .get("command")
        .and_then(Value::as_array)
        .expect("replay command");

    let analyze_tokens: Vec<&str> = analyze_cmd.iter().filter_map(Value::as_str).collect();
    let replay_tokens: Vec<&str> = replay_cmd.iter().filter_map(Value::as_str).collect();
    assert!(analyze_tokens.contains(&"--mm2"));
    assert!(replay_tokens.contains(&"--vm-only"));
    assert!(replay_tokens.contains(&"--synthesize-missing"));
    assert!(replay_tokens.contains(&"--self-heal-dynamic-fields"));
}

// ============================================================================
// Global Options Tests
// ============================================================================

#[test]
fn test_verbose_flag() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

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
fn test_debug_json_on_failure_emits_structured_diagnostic() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

    let output = sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("--debug-json")
        .arg("run")
        .arg("invalid_target")
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();

    let stderr = String::from_utf8_lossy(&output);
    assert!(stderr.contains("\"command\": \"run\""));
    assert!(stderr.contains("\"category\""));
    assert!(stderr.contains("\"hints\""));
}

#[test]
fn test_rpc_url_option() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("state.json");

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
                "arguments": [{"result": 0}, {"address": "0xABC"}]
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
            {"target": "0x2::coin::split", "arguments": [{"result": 0}, {"u64": 500}]},
            {"target": "0x2::transfer::public_transfer", "arguments": [{"result": 1}, {"address": "0xABC"}]}
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
    let state_file = temp_dir.path().join("state.json");
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
