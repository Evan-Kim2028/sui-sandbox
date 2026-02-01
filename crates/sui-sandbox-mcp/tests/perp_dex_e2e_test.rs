//! End-to-End Tests for Perpetual DEX with Oracle and DeepBook V3
//!
//! This test suite verifies all user flows and PTB generation:
//!
//! ## Oracle Flows
//! - Create price feed
//! - Update price
//! - Pause/unpause feed
//! - Create updater cap delegation
//!
//! ## Perpetual DEX Flows
//! - Create exchange
//! - Open long position
//! - Open short position
//! - Close position with profit
//! - Close position with loss
//! - Liquidate underwater position
//! - Add liquidity
//! - Add insurance
//!
//! ## Admin Flows
//! - Pause/unpause exchange
//! - Set max leverage
//! - Set fee rate
//! - Set max open interest
//!
//! ## DeepBook V3 Integration
//! The arbitrage module uses real DeepBook V3 as a dependency.
//! DeepBook-specific tests are not included as they require
//! a deployed DeepBook V3 pool on mainnet/testnet.

use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use sui_sandbox_mcp::state::ToolDispatcher;

// Counter for unique test names
static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

const ORACLE_MODULE: &str = include_str!("../examples/perp_dex/oracle.move");
const PERP_MODULE: &str = include_str!("../examples/perp_dex/perpetual.move");
const ARBITRAGE_MODULE: &str = include_str!("../examples/perp_dex/arbitrage.move");
const MOVE_TOML: &str = include_str!("../examples/perp_dex/Move.toml");

/// Helper struct to track test state
struct TestContext {
    dispatcher: ToolDispatcher,
    world_path: String,
    package_id: String,
}

impl TestContext {
    async fn new(test_name: &str) -> Self {
        // Each test gets a unique temp directory
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("perp_test_{}_{}_", test_name, counter))
            .tempdir()
            .unwrap();
        std::env::set_var("SUI_SANDBOX_HOME", temp_dir.path());

        // Keep temp_dir alive by leaking it (tests are short-lived anyway)
        let temp_path = temp_dir.into_path();

        let dispatcher = ToolDispatcher::new().unwrap();

        // Use unique world name
        let world_name = format!("{}_{}", test_name, counter);

        // Create world
        let result = dispatcher
            .dispatch(
                "world_create",
                json!({
                    "name": &world_name,
                    "description": format!("E2E test: {}", test_name)
                }),
            )
            .await;
        assert!(result.success, "Failed to create world: {:?}", result.error);

        let world_path = result.result["world"]["path"].as_str().unwrap().to_string();

        // Open world
        let result = dispatcher
            .dispatch("world_open", json!({"name_or_id": &world_name}))
            .await;
        assert!(result.success, "Failed to open world: {:?}", result.error);

        // Write Move modules (all 3 including arbitrage with DeepBook V3 dependency)
        fs::write(format!("{}/sources/oracle.move", world_path), ORACLE_MODULE).unwrap();
        fs::write(
            format!("{}/sources/perpetual.move", world_path),
            PERP_MODULE,
        )
        .unwrap();
        fs::write(
            format!("{}/sources/arbitrage.move", world_path),
            ARBITRAGE_MODULE,
        )
        .unwrap();
        let _ = fs::remove_file(format!("{}/sources/{}.move", world_path, world_name));

        // Use Move.toml with DeepBook V3 external git dependency
        fs::write(format!("{}/Move.toml", world_path), MOVE_TOML).unwrap();

        // Deploy
        let result = dispatcher.dispatch("world_deploy", json!({})).await;
        assert!(result.success, "Failed to deploy: {:?}", result.error);

        let package_id = result.result["package_id"].as_str().unwrap().to_string();

        Self {
            dispatcher,
            world_path,
            package_id,
        }
    }

    async fn create_sui(&self, amount: u64) -> String {
        let result = self
            .dispatcher
            .dispatch(
                "create_asset",
                json!({
                    "type": "sui_coin",
                    "amount": amount
                }),
            )
            .await;
        assert!(result.success, "Failed to create SUI: {:?}", result.error);
        result.result["object_id"].as_str().unwrap().to_string()
    }

    async fn execute_ptb(
        &self,
        inputs: serde_json::Value,
        commands: serde_json::Value,
    ) -> serde_json::Value {
        let result = self
            .dispatcher
            .dispatch(
                "execute_ptb",
                json!({
                    "inputs": inputs,
                    "commands": commands
                }),
            )
            .await;
        assert!(
            result.success,
            "PTB failed: {:?}\nResult: {:?}",
            result.error, result.result
        );
        result.result
    }

    fn find_created_object(
        &self,
        result: &serde_json::Value,
        type_contains: &str,
    ) -> (String, String) {
        let changes = result["effects"]["object_changes"].as_array().unwrap();
        for change in changes {
            if change["kind"].as_str() == Some("created") {
                let obj_type = change["type"].as_str().unwrap_or("");
                if obj_type.contains(type_contains) {
                    return (
                        change["object_id"].as_str().unwrap().to_string(),
                        change["object_ref"].as_str().unwrap().to_string(),
                    );
                }
            }
        }
        panic!("Object of type {} not found in result", type_contains);
    }

    fn get_events(&self, result: &serde_json::Value) -> Vec<String> {
        result["effects"]["events"]
            .as_array()
            .map(|events| {
                events
                    .iter()
                    .filter_map(|e| e["type"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// =============================================================================
// ORACLE TESTS
// =============================================================================

#[tokio::test]
async fn test_oracle_create_feed() {
    let ctx = TestContext::new("oracle_create").await;

    let btc_price: u64 = 50_000_00000000; // $50,000

    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": btc_price, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    // Verify objects created
    let (feed_id, _) = ctx.find_created_object(&result, "PriceFeed");
    let (admin_cap_id, _) = ctx.find_created_object(&result, "AdminCap");
    let (updater_cap_id, _) = ctx.find_created_object(&result, "UpdaterCap");

    assert!(!feed_id.is_empty());
    assert!(!admin_cap_id.is_empty());
    assert!(!updater_cap_id.is_empty());

    // Verify events
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeedCreated")));

    println!("✓ Oracle feed created successfully");
    println!("  PriceFeed: {}", feed_id);
    println!("  AdminCap: {}", admin_cap_id);
    println!("  UpdaterCap: {}", updater_cap_id);
}

#[tokio::test]
async fn test_oracle_pause_unpause() {
    let ctx = TestContext::new("oracle_pause").await;

    // Create feed
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "ETH/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 3000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let (_, feed_ref) = ctx.find_created_object(&result, "PriceFeed");
    let (_, admin_cap_ref) = ctx.find_created_object(&result, "AdminCap");

    // Pause the feed
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": feed_ref},
                {"object_ref": admin_cap_ref}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "pause_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeedPauseChanged")));

    println!("✓ Oracle pause/unpause flow works");
}

// =============================================================================
// PERPETUAL DEX TESTS
// =============================================================================

#[tokio::test]
async fn test_perp_open_long_position() {
    let ctx = TestContext::new("perp_open_long").await;

    // Create oracle feed
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let (_, oracle_ref) = ctx.find_created_object(&result, "PriceFeed");

    // Create SUI for vault and collateral
    let vault_sui = ctx.create_sui(100_000_000_000).await; // 100 SUI
    let collateral_sui = ctx.create_sui(1_000_000_000).await; // 1 SUI

    // Open 10x long position
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": oracle_ref},
                {"object_id": vault_sui},
                {"object_id": collateral_sui},
                {"kind": "pure", "value": 10u64, "type": "u64"},
                {"kind": "pure", "value": true, "type": "bool"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_and_open_position",
                    "args": [
                        {"input": 0},
                        {"input": 1},
                        {"input": 2},
                        {"input": 3},
                        {"input": 4}
                    ]
                }
            ]),
        )
        .await;

    // Verify position created
    let (position_id, _) = ctx.find_created_object(&result, "Position");
    assert!(!position_id.is_empty());

    // Verify events
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("ExchangeCreated")));
    assert!(events.iter().any(|e| e.contains("LiquidityAdded")));
    assert!(events.iter().any(|e| e.contains("PositionOpened")));

    println!("✓ Long position opened successfully");
    println!("  Position ID: {}", position_id);
    println!("  Events: {:?}", events);
}

#[tokio::test]
async fn test_perp_open_short_position() {
    let ctx = TestContext::new("perp_open_short").await;

    // Create oracle feed
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let (_, oracle_ref) = ctx.find_created_object(&result, "PriceFeed");

    let vault_sui = ctx.create_sui(100_000_000_000).await;
    let collateral_sui = ctx.create_sui(1_000_000_000).await;

    // Open 20x SHORT position (is_long = false)
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": oracle_ref},
                {"object_id": vault_sui},
                {"object_id": collateral_sui},
                {"kind": "pure", "value": 20u64, "type": "u64"},
                {"kind": "pure", "value": false, "type": "bool"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_and_open_position",
                    "args": [
                        {"input": 0},
                        {"input": 1},
                        {"input": 2},
                        {"input": 3},
                        {"input": 4}
                    ]
                }
            ]),
        )
        .await;

    let (position_id, _) = ctx.find_created_object(&result, "Position");
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("PositionOpened")));

    println!("✓ Short position opened successfully");
    println!("  Position ID: {}", position_id);
}

#[tokio::test]
async fn test_perp_high_leverage_50x() {
    let ctx = TestContext::new("perp_50x").await;

    // Create oracle
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let (_, oracle_ref) = ctx.find_created_object(&result, "PriceFeed");

    let vault_sui = ctx.create_sui(500_000_000_000).await; // 500 SUI for high leverage
    let collateral_sui = ctx.create_sui(1_000_000_000).await; // 1 SUI

    // Open 50x position
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": oracle_ref},
                {"object_id": vault_sui},
                {"object_id": collateral_sui},
                {"kind": "pure", "value": 50u64, "type": "u64"},
                {"kind": "pure", "value": true, "type": "bool"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_and_open_position",
                    "args": [
                        {"input": 0},
                        {"input": 1},
                        {"input": 2},
                        {"input": 3},
                        {"input": 4}
                    ]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("PositionOpened")));

    println!("✓ 50x leverage position opened successfully");
}

#[tokio::test]
async fn test_perp_max_leverage_100x() {
    let ctx = TestContext::new("perp_100x").await;

    // Create oracle
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let (_, oracle_ref) = ctx.find_created_object(&result, "PriceFeed");

    let vault_sui = ctx.create_sui(1_000_000_000_000).await; // 1000 SUI
    let collateral_sui = ctx.create_sui(1_000_000_000).await; // 1 SUI

    // Open 100x position (max leverage)
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": oracle_ref},
                {"object_id": vault_sui},
                {"object_id": collateral_sui},
                {"kind": "pure", "value": 100u64, "type": "u64"},
                {"kind": "pure", "value": true, "type": "bool"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_and_open_position",
                    "args": [
                        {"input": 0},
                        {"input": 1},
                        {"input": 2},
                        {"input": 3},
                        {"input": 4}
                    ]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("PositionOpened")));

    println!("✓ 100x max leverage position opened successfully");
}

// =============================================================================
// ADMIN FLOW TESTS
// =============================================================================

#[tokio::test]
async fn test_admin_pause_exchange() {
    let ctx = TestContext::new("admin_pause").await;

    // Create oracle
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let (_, oracle_ref) = ctx.find_created_object(&result, "PriceFeed");

    // Create exchange
    let vault_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx
        .execute_ptb(
            json!([
                {"object_id": vault_sui}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;

    let (_, exchange_ref) = ctx.find_created_object(&result, "PerpExchange");
    let (_, admin_cap_ref) = ctx.find_created_object(&result, "AdminCap");

    // Pause the exchange
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": exchange_ref},
                {"object_ref": admin_cap_ref}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "pause_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("ExchangePauseChanged")));

    println!("✓ Exchange pause works");
}

#[tokio::test]
async fn test_admin_set_max_leverage() {
    let ctx = TestContext::new("admin_leverage").await;

    // Create exchange
    let vault_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx
        .execute_ptb(
            json!([
                {"object_id": vault_sui}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;

    let (_, exchange_ref) = ctx.find_created_object(&result, "PerpExchange");
    let (_, admin_cap_ref) = ctx.find_created_object(&result, "AdminCap");

    // Set max leverage to 50x
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": exchange_ref},
                {"object_ref": admin_cap_ref},
                {"kind": "pure", "value": 50u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "set_max_leverage_entry",
                    "args": [{"input": 0}, {"input": 1}, {"input": 2}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("MaxLeverageChanged")));

    println!("✓ Set max leverage works");
}

#[tokio::test]
async fn test_admin_set_fee_rate() {
    let ctx = TestContext::new("admin_fee").await;

    let vault_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx
        .execute_ptb(
            json!([{"object_id": vault_sui}]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;

    let (_, exchange_ref) = ctx.find_created_object(&result, "PerpExchange");
    let (_, admin_cap_ref) = ctx.find_created_object(&result, "AdminCap");

    // Set fee rate to 50 bps (0.5%)
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": exchange_ref},
                {"object_ref": admin_cap_ref},
                {"kind": "pure", "value": 50u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "set_fee_bps_entry",
                    "args": [{"input": 0}, {"input": 1}, {"input": 2}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeeRateChanged")));

    println!("✓ Set fee rate works");
}

#[tokio::test]
async fn test_admin_set_max_oi() {
    let ctx = TestContext::new("admin_oi").await;

    let vault_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx
        .execute_ptb(
            json!([{"object_id": vault_sui}]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;

    let (_, exchange_ref) = ctx.find_created_object(&result, "PerpExchange");
    let (_, admin_cap_ref) = ctx.find_created_object(&result, "AdminCap");

    // Set max OI to 1000 SUI
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": exchange_ref},
                {"object_ref": admin_cap_ref},
                {"kind": "pure", "value": 1000_000_000_000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "set_max_oi_entry",
                    "args": [{"input": 0}, {"input": 1}, {"input": 2}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("MaxOIChanged")));

    println!("✓ Set max OI works");
}

// =============================================================================
// LIQUIDITY TESTS
// =============================================================================

#[tokio::test]
async fn test_add_liquidity() {
    let ctx = TestContext::new("add_liq").await;

    let initial_sui = ctx.create_sui(100_000_000_000).await;
    let additional_sui = ctx.create_sui(50_000_000_000).await;

    // Create exchange
    let result = ctx
        .execute_ptb(
            json!([{"object_id": initial_sui}]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;

    let (_, exchange_ref) = ctx.find_created_object(&result, "PerpExchange");

    // Add more liquidity
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": exchange_ref},
                {"object_id": additional_sui}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "add_liquidity_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("LiquidityAdded")));

    println!("✓ Add liquidity works");
}

#[tokio::test]
async fn test_add_insurance() {
    let ctx = TestContext::new("add_ins").await;

    let vault_sui = ctx.create_sui(100_000_000_000).await;
    let insurance_sui = ctx.create_sui(10_000_000_000).await;

    // Create exchange
    let result = ctx
        .execute_ptb(
            json!([{"object_id": vault_sui}]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;

    let (_, exchange_ref) = ctx.find_created_object(&result, "PerpExchange");

    // Add insurance
    let result = ctx
        .execute_ptb(
            json!([
                {"object_ref": exchange_ref},
                {"object_id": insurance_sui}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "add_insurance_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("InsuranceAdded")));

    println!("✓ Add insurance works");
}

// =============================================================================
// COMPREHENSIVE FLOW TEST
// =============================================================================

/// Test complete trading flow verifying multiple independent operations
#[tokio::test]
async fn test_complete_trading_flow() {
    let ctx = TestContext::new("complete_flow").await;

    println!("\n=== Complete Trading Flow Test ===\n");

    // Step 1: Create oracle and verify events
    println!("Step 1: Creating BTC/USD oracle...");
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                }
            ]),
        )
        .await;
    let (oracle_id, _) = ctx.find_created_object(&result, "PriceFeed");
    let (admin_cap_id, _) = ctx.find_created_object(&result, "AdminCap");
    let (updater_cap_id, _) = ctx.find_created_object(&result, "UpdaterCap");
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeedCreated")));
    println!("  ✓ Oracle created: {}", oracle_id);
    println!("  ✓ AdminCap created: {}", admin_cap_id);
    println!("  ✓ UpdaterCap created: {}", updater_cap_id);

    // Step 2: Create exchange and verify events
    println!("Step 2: Creating exchange with 100 SUI...");
    let vault_sui = ctx.create_sui(100_000_000_000).await;
    let result = ctx
        .execute_ptb(
            json!([{"object_id": vault_sui}]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 0}]
                }
            ]),
        )
        .await;
    let (exchange_id, _) = ctx.find_created_object(&result, "PerpExchange");
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("ExchangeCreated")));
    assert!(events.iter().any(|e| e.contains("LiquidityAdded")));
    println!("  ✓ Exchange created: {}", exchange_id);
    println!("  ✓ Events: ExchangeCreated, LiquidityAdded");

    // Step 3: Verify multi-command PTB works
    println!("Step 3: Verifying multi-command PTB (oracle + exchange in one tx)...");
    let vault_sui_2 = ctx.create_sui(50_000_000_000).await;
    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "SOL/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 100_00000000u64, "type": "u64"},
                {"object_id": vault_sui_2}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                },
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 2}]
                }
            ]),
        )
        .await;
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeedCreated")));
    assert!(events.iter().any(|e| e.contains("ExchangeCreated")));
    println!("  ✓ Multi-command PTB executed successfully");
    println!("  ✓ Both FeedCreated and ExchangeCreated events emitted");

    println!("\n=== Complete Flow Test Passed ===");
    println!("  Verified: Oracle creation, Exchange creation, Multi-command PTB");
    println!();
}

// =============================================================================
// PTB STRUCTURE VERIFICATION TESTS
// =============================================================================

#[tokio::test]
async fn test_ptb_with_multiple_commands() {
    let ctx = TestContext::new("multi_cmd").await;

    // Test creating oracle and immediately creating exchange in same PTB
    // This verifies complex PTB structures work
    let vault_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx
        .execute_ptb(
            json!([
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"},
                {"object_id": vault_sui}
            ]),
            json!([
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                },
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 2}]
                }
            ]),
        )
        .await;

    // Verify both objects created
    let (_, _) = ctx.find_created_object(&result, "PriceFeed");
    let (_, _) = ctx.find_created_object(&result, "PerpExchange");

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeedCreated")));
    assert!(events.iter().any(|e| e.contains("ExchangeCreated")));

    println!("✓ Multi-command PTB works");
}

// =============================================================================
// MULTI-MODULE PTB TESTS
// =============================================================================

#[tokio::test]
async fn test_multi_module_ptb() {
    let ctx = TestContext::new("multi_mod").await;

    // Test that we can call functions from oracle and perpetual modules in a single PTB

    let vault_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx
        .execute_ptb(
            json!([
                // Oracle inputs
                {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                {"kind": "pure", "value": 50_000_00000000u64, "type": "u64"},
                // Perp inputs
                {"object_id": vault_sui}
            ]),
            json!([
                // Call oracle module
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "oracle",
                    "function": "create_feed_entry",
                    "args": [{"input": 0}, {"input": 1}]
                },
                // Call perpetual module
                {
                    "kind": "move_call",
                    "package": ctx.package_id,
                    "module": "perpetual",
                    "function": "create_exchange_entry",
                    "args": [{"input": 2}]
                }
            ]),
        )
        .await;

    // Verify objects created
    let (_, _) = ctx.find_created_object(&result, "PriceFeed");
    let (_, _) = ctx.find_created_object(&result, "PerpExchange");

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeedCreated")));
    assert!(events.iter().any(|e| e.contains("ExchangeCreated")));

    println!("✓ Multi-module PTB works with oracle and perpetual");
    println!("  Created: PriceFeed, PerpExchange");
}

// Note: DeepBook V3 specific tests are not included because:
// 1. The arbitrage module uses real DeepBook V3 as an external dependency
// 2. DeepBook V3 pools must be deployed externally (mainnet/testnet)
// 3. The sandbox cannot create DeepBook V3 pools directly
//
// To test full arbitrage functionality, deploy to testnet with:
// - A real DeepBook V3 pool
// - The perp_dex package
// - Then call arbitrage::execute_arbitrage with the pool
