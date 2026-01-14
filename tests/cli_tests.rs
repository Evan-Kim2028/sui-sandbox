use assert_cmd::Command;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_local_bytecode_extraction() {
    #[allow(deprecated)]
    let mut cmd = Command::cargo_bin("sui_move_interface_extractor").unwrap();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = manifest_dir.join("tests/fixture/build/fixture");

    // Create a temp dir for output
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("out.json");

    cmd.arg("--bytecode-package-dir")
        .arg(&fixture_dir)
        .arg("--package-id")
        .arg("0x1")
        .arg("--emit-bytecode-json")
        .arg(&output_path)
        .assert()
        .success();

    // Verify output exists
    assert!(output_path.exists());

    // Verify content
    let content = std::fs::read_to_string(&output_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    // Check package_id (canonicalized) - fixture uses 0xDEADBEEF but test passes explicit 0x1
    // The output should match what we requested, not the module's address
    assert_eq!(json["package_id"], "0x1");

    // Check module name
    let modules = json["modules"].as_object().unwrap();
    assert!(modules.contains_key("test_module"));

    // Check function
    let functions = modules["test_module"]["functions"].as_object().unwrap();
    assert!(functions.contains_key("simple_func"));

    let simple_func = &functions["simple_func"];
    assert_eq!(simple_func["visibility"], "public");
    assert_eq!(simple_func["is_entry"], false);
}

#[test]
fn test_benchmark_local_cli_invocation() {
    #[allow(deprecated)]
    let mut cmd = Command::cargo_bin("sui_move_interface_extractor").unwrap();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = manifest_dir.join("tests/fixture/build/fixture");

    // Create a temp dir for output
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("benchmark_output.jsonl");

    cmd.arg("benchmark-local")
        .arg("--target-corpus")
        .arg(&fixture_dir)
        .arg("--output")
        .arg(&output_path)
        .arg("--tier-a-only")
        .assert()
        .success();

    // Verify output exists
    assert!(output_path.exists());

    // Verify output is valid JSONL
    let content = std::fs::read_to_string(&output_path).unwrap();
    let line_count = content.lines().count();

    // Should have at least one line (one function attempt)
    assert!(line_count >= 1, "expected at least one line in output");

    // Parse first line and verify it's valid JSON
    let first_line = content.lines().next().unwrap();
    let json: serde_json::Value = serde_json::from_str(first_line).unwrap();

    // Verify required fields exist
    assert!(json.get("target_package").is_some());
    assert!(json.get("target_module").is_some());
    assert!(json.get("target_function").is_some());
    assert!(json.get("status").is_some());

    // Verify status is one of expected values
    let status = json["status"].as_str().unwrap();
    assert!(
        status == "tier_a_hit" || status == "tier_b_hit" || status == "miss",
        "unexpected status: {status}"
    );
}

#[test]
fn test_benchmark_local_cli_with_restricted_state() {
    #[allow(deprecated)]
    let mut cmd = Command::cargo_bin("sui_move_interface_extractor").unwrap();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = manifest_dir.join("tests/fixture/build/fixture");

    // Create a temp dir for output
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("benchmark_restricted.jsonl");

    cmd.arg("benchmark-local")
        .arg("--target-corpus")
        .arg(&fixture_dir)
        .arg("--output")
        .arg(&output_path)
        .arg("--restricted-state")
        .assert()
        .success();

    // Verify output exists
    assert!(output_path.exists());

    // Verify output is valid JSONL
    let content = std::fs::read_to_string(&output_path).unwrap();
    let first_line = content.lines().next().unwrap();
    let json: serde_json::Value = serde_json::from_str(first_line).unwrap();

    // Verify output structure is correct
    assert!(json.get("target_package").is_some());
    assert!(json.get("status").is_some());
}
