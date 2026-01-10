use assert_cmd::Command;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_local_bytecode_extraction() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sui_move_interface_extractor"));
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

    // Check package_id (canonicalized)
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
fn test_smi_tx_sim_build_only_static_analysis() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_smi_tx_sim"));
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = manifest_dir.join("tests/fixture/build/fixture");

    // Create a temp PTB spec
    let temp_dir = TempDir::new().unwrap();
    let spec_path = temp_dir.path().join("spec.json");

    let spec = serde_json::json!({
        "calls": [
            {
                "target": "0x1::stress_tests::probe_coin",
                "type_args": ["0x1::stress_tests::MyCoin"],
                "args": []
            }
        ]
    });
    std::fs::write(&spec_path, serde_json::to_string(&spec).unwrap()).unwrap();

    let assert = cmd
        .arg("--mode")
        .arg("build-only")
        .arg("--sender")
        .arg("0x123")
        .arg("--ptb-spec")
        .arg(&spec_path)
        .arg("--bytecode-package-dir")
        .arg(&fixture_dir)
        .assert()
        .success();

    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(json["modeUsed"], "build_only");

    let static_created = json["staticCreatedObjectTypes"].as_array().unwrap();
    let expected = "0x0000000000000000000000000000000000000000000000000000000000000002::coin::Coin<0x0000000000000000000000000000000000000000000000000000000000000001::stress_tests::MyCoin>";

    assert!(static_created.iter().any(|v| v.as_str() == Some(expected)));
}
