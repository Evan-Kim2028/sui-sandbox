#![allow(deprecated)]
//! Fast test suite for quick CLI validation.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn sandbox_cmd() -> Command {
    Command::cargo_bin("sui-sandbox").expect("binary not found")
}

#[test]
fn fast_help_output() {
    sandbox_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui-sandbox"));
}

#[test]
fn fast_version_output() {
    sandbox_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("sui-sandbox"));
}

#[test]
fn fast_status_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let state_file = temp_dir.path().join("fast-state.json");

    sandbox_cmd()
        .arg("--state-file")
        .arg(&state_file)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Sui Sandbox Status"));
}
