use assert_cmd::Command;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
#[ignore = "Deprecated: Old CLI structure no longer supported"]
fn test_local_bytecode_extraction() {
    #[allow(deprecated)]
    let mut cmd = Command::cargo_bin("sui-sandbox").unwrap();
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
