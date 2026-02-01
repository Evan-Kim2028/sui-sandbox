use serde_json::{json, Value};
use std::env;
use sui_sandbox_mcp::state::ToolDispatcher;

fn find_object_id(changes: &[Value], type_hint: &str) -> Option<String> {
    for change in changes {
        let typ = change.get("type").and_then(Value::as_str).unwrap_or("");
        if !typ.contains(type_hint) {
            continue;
        }
        if let Some(id) = change.get("object_id").and_then(Value::as_str) {
            return Some(id.to_string());
        }
    }
    None
}

fn object_changes(result: &Value) -> Vec<Value> {
    let effects = result
        .get("effects")
        .or_else(|| result.get("result").and_then(|r| r.get("effects")));

    effects
        .and_then(|e| e.get("object_changes"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn first_created_id(result: &Value) -> Option<String> {
    let effects = result
        .get("effects")
        .or_else(|| result.get("result").and_then(|r| r.get("effects")));

    effects
        .and_then(|e| e.get("created"))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

#[tokio::test]
async fn test_convertible_debt_flow() {
    if env::var("SUI_RUN_CONVERTIBLE_TESTS").is_err() {
        eprintln!("Skipping convertible debt test (set SUI_RUN_CONVERTIBLE_TESTS=1 to enable)");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

    let dispatcher = ToolDispatcher::new().unwrap();

    let package_path = format!(
        "{}/../../examples/convertible_debt",
        env!("CARGO_MANIFEST_DIR")
    );
    let publish = dispatcher
        .dispatch("publish", json!({"path": package_path}))
        .await;
    assert!(publish.success, "publish failed: {:?}", publish.error);
    let package_id = publish.result["package_id"].as_str().unwrap().to_string();

    let init = dispatcher
        .dispatch(
            "ptb",
            json!({
                "inputs": [],
                "calls": [
                    {"target": format!("{}::oracle::create_shared", package_id), "args": [{"u64": 2000000000}]}
                ],
                "sender": "0x300"
            }),
        )
        .await;
    assert!(init.success, "init ptb failed: {:?}", init.error);
    let init_changes = object_changes(&init.result);

    let oracle_id = find_object_id(&init_changes, "::oracle::Oracle")
        .or_else(|| first_created_id(&init.result))
        .expect("oracle id");

    let _ = dispatcher
        .dispatch(
            "configure",
            json!({"action": "set_sender", "params": {"address": "0x200"}}),
        )
        .await;

    let usd_coin = dispatcher
        .dispatch(
            "create_asset",
            json!({
                "type": "custom_coin",
                "amount": 1000000000,
                "type_tag": format!("{}::tokens::USD", package_id)
            }),
        )
        .await;
    assert!(usd_coin.success, "usd coin failed: {:?}", usd_coin.error);
    let usd_coin_id = usd_coin.result["object_id"].as_str().unwrap().to_string();

    let _ = dispatcher
        .dispatch(
            "configure",
            json!({"action": "set_sender", "params": {"address": "0x100"}}),
        )
        .await;

    let eth_coin = dispatcher
        .dispatch(
            "create_asset",
            json!({
                "type": "custom_coin",
                "amount": 1000000000,
                "type_tag": format!("{}::tokens::ETH", package_id)
            }),
        )
        .await;
    assert!(eth_coin.success, "eth coin failed: {:?}", eth_coin.error);
    let eth_coin_id = eth_coin.result["object_id"].as_str().unwrap().to_string();

    let offer = dispatcher
        .dispatch(
            "ptb",
            json!({
                "inputs": [
                    {"imm_or_owned_object": eth_coin_id},
                    {"shared_object": {"id": oracle_id, "mutable": false}}
                ],
                "calls": [
                    {
                        "target": format!("{}::convertible_debt::create_offer", package_id),
                        "args": [
                            {"input": 0},
                            {"u64": 1000000000},
                            {"u64": 500},
                            {"u64": 0},
                            {"input": 1}
                        ]
                    }
                ],
                "sender": "0x100"
            }),
        )
        .await;
    assert!(offer.success, "offer ptb failed: {:?}", offer.error);
    let offer_changes = object_changes(&offer.result);
    let offer_id = find_object_id(&offer_changes, "::convertible_debt::Offer")
        .or_else(|| first_created_id(&offer.result))
        .expect("offer id");

    let take = dispatcher
        .dispatch(
            "ptb",
            json!({
                "inputs": [
                    {"shared_object": {"id": offer_id, "mutable": true}},
                    {"imm_or_owned_object": usd_coin_id}
                ],
                "calls": [
                    {
                        "target": format!("{}::convertible_debt::take_offer", package_id),
                        "args": [
                            {"input": 0},
                            {"input": 1}
                        ]
                    }
                ],
                "sender": "0x200"
            }),
        )
        .await;
    assert!(take.success, "take ptb failed: {:?}", take.error);
    let take_changes = object_changes(&take.result);
    let note_id = find_object_id(&take_changes, "::convertible_debt::Note")
        .or_else(|| first_created_id(&take.result))
        .expect("note id");

    let convert = dispatcher
        .dispatch(
            "ptb",
            json!({
                "inputs": [
                    {"shared_object": {"id": note_id, "mutable": true}}
                ],
                "calls": [
                    {"target": format!("{}::convertible_debt::convert", package_id), "args": [{"input": 0}]}
                ],
                "sender": "0x200"
            }),
        )
        .await;
    assert!(convert.success, "convert ptb failed: {:?}", convert.error);
}
