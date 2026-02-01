//! Integration test for session continuity (Phase 3) and templates (Phase 5)
//!
//! These tests must run serially since they modify the SUI_SANDBOX_HOME env var.

use base64::Engine;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use sui_sandbox_mcp::state::ToolDispatcher;

// Global lock to ensure tests run serially
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn fixture_modules() -> Vec<Value> {
    let bytecode_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixture/build/fixture/bytecode_modules");
    assert!(
        bytecode_dir.exists(),
        "fixture bytecode dir missing: {:?}",
        bytecode_dir
    );

    let mut modules = Vec::new();
    for entry in fs::read_dir(bytecode_dir).expect("read bytecode modules") {
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

#[tokio::test]
async fn test_world_templates() {
    let _lock = TEST_LOCK.lock().unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    let dispatcher = ToolDispatcher::new().unwrap();

    // Test defi template
    let result = dispatcher
        .dispatch(
            "world_create",
            json!({
                "name": "my_defi",
                "template": "defi"
            }),
        )
        .await;

    assert!(result.success, "world_create failed: {:?}", result.error);
    println!("world_create result: {:?}", result.result);

    // Check files were created
    let world_path = temp_dir.path().join("worlds");
    let entries: Vec<_> = fs::read_dir(&world_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir()) // Only directories, not registry.json
        .collect();
    assert!(!entries.is_empty(), "World directory should exist");

    let world_dir = &entries[0].path();
    println!("World dir: {:?}", world_dir);
    let sources_dir = world_dir.join("sources");
    println!(
        "Sources dir: {:?}, exists: {}",
        sources_dir,
        sources_dir.exists()
    );

    // List files in sources directory
    match fs::read_dir(&sources_dir) {
        Ok(source_entries) => {
            println!("Files in sources dir:");
            for entry in source_entries {
                if let Ok(e) = entry {
                    println!("  {:?}", e.path());
                }
            }
        }
        Err(e) => {
            println!("Error reading sources dir: {:?}", e);
        }
    }

    // DeFi template should have coin_a.move, coin_b.move, and pool.move
    assert!(
        sources_dir.join("coin_a.move").exists(),
        "coin_a.move should exist"
    );
    assert!(
        sources_dir.join("coin_b.move").exists(),
        "coin_b.move should exist"
    );
    assert!(
        sources_dir.join("pool.move").exists(),
        "pool.move should exist"
    );

    // Verify pool.move has swap functions
    let pool_content = fs::read_to_string(sources_dir.join("pool.move")).unwrap();
    assert!(
        pool_content.contains("swap_a_for_b"),
        "pool.move should have swap function"
    );
    assert!(
        pool_content.contains("add_liquidity"),
        "pool.move should have add_liquidity function"
    );

    println!("SUCCESS: Templates work correctly!");
}

#[tokio::test]
async fn test_session_continuity() {
    let _lock = TEST_LOCK.lock().unwrap();

    // Use a temp directory for this test
    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    // Step 1: Create dispatcher and a world
    let world_id: String;
    {
        let dispatcher = ToolDispatcher::new().unwrap();

        // Create a world
        let result = dispatcher
            .dispatch(
                "world_create",
                json!({
                    "name": "test_world",
                    "description": "Testing session"
                }),
            )
            .await;

        assert!(result.success, "world_create failed: {:?}", result.error);
        println!("Created world: {:?}", result.result);

        // Extract the world ID
        world_id = result.result["world"]["id"].as_str().unwrap().to_string();

        // Open the world (should set it as active)
        let result = dispatcher
            .dispatch("world_open", json!({"name_or_id": "test_world"}))
            .await;

        assert!(result.success, "world_open failed: {:?}", result.error);
        println!("Opened world: {:?}", result.result);

        // Check session was written
        let session_path = temp_dir.path().join("session.json");
        assert!(session_path.exists(), "session.json should exist");

        let session_content = fs::read_to_string(&session_path).unwrap();
        println!("Session after open: {}", session_content);
        assert!(
            session_content.contains(&world_id),
            "session should contain the world ID"
        );

        // Shutdown dispatcher (simulates server restart)
        dispatcher.shutdown().unwrap();
    }

    // Step 2: Create new dispatcher - should auto-resume
    {
        let dispatcher = ToolDispatcher::new().unwrap();

        // Check world_status - should have an active world
        let result = dispatcher.dispatch("world_status", json!({})).await;

        assert!(result.success, "world_status failed: {:?}", result.error);
        println!("Status after restart: {:?}", result.result);

        // The active world should be restored
        let status: &Value = &result.result;
        assert!(
            status.get("world").is_some(),
            "Should have active world after restart"
        );

        let world = status.get("world").unwrap();
        assert_eq!(
            world.get("name").and_then(|v: &Value| v.as_str()),
            Some("test_world"),
            "Active world should be test_world"
        );

        println!("SUCCESS: Session continuity works!");
    }
}

#[tokio::test]
async fn test_world_close_clears_session() {
    let _lock = TEST_LOCK.lock().unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    let dispatcher = ToolDispatcher::new().unwrap();

    // Create and open a world
    let result = dispatcher
        .dispatch(
            "world_create",
            json!({"name": "close_test", "description": "Test closing"}),
        )
        .await;
    assert!(result.success, "world_create failed: {:?}", result.error);

    let result = dispatcher
        .dispatch("world_open", json!({"name_or_id": "close_test"}))
        .await;
    assert!(result.success, "world_open failed: {:?}", result.error);

    // Close the world
    let result = dispatcher.dispatch("world_close", json!({})).await;
    assert!(result.success, "world_close failed: {:?}", result.error);

    // Session should have no active world now
    let session_path = temp_dir.path().join("session.json");
    let session_content = fs::read_to_string(&session_path).unwrap();
    println!("Session after close: {}", session_content);

    // The session should not contain an active_world (or it should be null)
    let session: Value = serde_json::from_str(&session_content).unwrap();
    assert!(
        session.get("active_world").is_none() || session["active_world"].is_null(),
        "active_world should be null or missing after close"
    );

    // Shutdown current dispatcher
    dispatcher.shutdown().unwrap();

    // Create new dispatcher - should NOT have active world
    let dispatcher2 = ToolDispatcher::new().unwrap();
    let result = dispatcher2.dispatch("world_status", json!({})).await;

    // world_status without active world should still succeed but with no world
    println!("Status after restart: {:?}", result.result);

    let status: &Value = &result.result;
    let has_no_active_world = status.get("world").is_none()
        || status
            .get("world")
            .map(|v: &Value| v.is_null())
            .unwrap_or(false);

    assert!(
        has_no_active_world,
        "Should NOT have active world after close and restart, got: {:?}",
        status
    );

    println!("SUCCESS: Close clears session!");
}

#[tokio::test]
async fn test_world_file_tools_and_state_file() {
    let _lock = TEST_LOCK.lock().unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    let dispatcher = ToolDispatcher::new().unwrap();

    let result = dispatcher
        .dispatch("world_create", json!({"name": "file_world"}))
        .await;
    assert!(result.success, "world_create failed: {:?}", result.error);

    let result = dispatcher
        .dispatch("world_open", json!({"name_or_id": "file_world"}))
        .await;
    assert!(result.success, "world_open failed: {:?}", result.error);

    let write = dispatcher
        .dispatch(
            "world_write_file",
            json!({
                "file": "sources/test.move",
                "content": "module file_world::test { public fun ping() {} }"
            }),
        )
        .await;
    assert!(write.success, "world_write_file failed: {:?}", write.error);
    let state_file = write.state_file.as_ref().expect("state_file should be set");
    assert!(
        state_file.ends_with("mcp-state.json"),
        "state_file should end with mcp-state.json, got: {}",
        state_file
    );

    let read = dispatcher
        .dispatch("world_read_file", json!({"file": "sources/test.move"}))
        .await;
    assert!(read.success, "world_read_file failed: {:?}", read.error);
    let content = read.result["content"].as_str().unwrap_or("");
    assert!(
        content.contains("fun ping"),
        "read content should include updated source"
    );

    let bad = dispatcher
        .dispatch(
            "world_write_file",
            json!({"file": "../oops.txt", "content": "nope"}),
        )
        .await;
    assert!(
        !bad.success,
        "path traversal should be blocked, got: {:?}",
        bad.result
    );
}

#[tokio::test]
async fn test_status_and_clean_alias_tools() {
    let _lock = TEST_LOCK.lock().unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    let dispatcher = ToolDispatcher::new().unwrap();

    let status = dispatcher.dispatch("status", json!({})).await;
    assert!(status.success, "status failed: {:?}", status.error);
    assert!(
        status
            .state_file
            .as_ref()
            .map(|p| p.ends_with("mcp-state.json"))
            .unwrap_or(false),
        "status should include state_file"
    );

    let clean = dispatcher.dispatch("clean", json!({})).await;
    assert!(clean.success, "clean failed: {:?}", clean.error);
}

#[tokio::test]
async fn test_run_ptb_view_alias_tools() {
    let _lock = TEST_LOCK.lock().unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    let dispatcher = ToolDispatcher::new().unwrap();

    let modules = fixture_modules();
    let package_id = "0x555";
    let load = dispatcher
        .dispatch(
            "load_package_bytes",
            json!({
                "package_id": package_id,
                "modules": modules
            }),
        )
        .await;
    assert!(load.success, "load_package_bytes failed: {:?}", load.error);

    let run = dispatcher
        .dispatch(
            "run",
            json!({
                "target": format!("{package_id}::test_module::simple_func"),
                "args": ["42"]
            }),
        )
        .await;
    assert!(run.success, "run failed: {:?}", run.error);

    let ptb = dispatcher
        .dispatch(
            "ptb",
            json!({
                "inputs": [],
                "calls": [{
                    "target": format!("{package_id}::test_module::simple_func"),
                    "args": [{"u64": 42}]
                }]
            }),
        )
        .await;
    assert!(ptb.success, "ptb failed: {:?}", ptb.error);

    let view = dispatcher
        .dispatch(
            "view",
            json!({
                "kind": "module",
                "module": format!("{package_id}::test_module")
            }),
        )
        .await;
    assert!(view.success, "view failed: {:?}", view.error);
}
