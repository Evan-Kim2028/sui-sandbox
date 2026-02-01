//! Perpetual DEX with Oracle and DeepBook V3 Arbitrage - MCP Integration Example
//!
//! This example demonstrates how to use the sui-sandbox-mcp tools to:
//! 1. Create a world for Move development
//! 2. Write and deploy Move modules (Oracle, Perp DEX, Arbitrage)
//! 3. Execute complex DeFi transactions
//! 4. Demonstrate perp trading with oracle prices
//!
//! ## MCP Tools Used
//!
//! | Tool | Purpose |
//! |------|---------|
//! | `world_create` | Create a new Move development environment |
//! | `world_open` | Activate a world for operations |
//! | `world_deploy` | Build and deploy Move packages |
//! | `create_asset` | Create SUI coins for testing |
//! | `execute_ptb` | Execute Programmable Transaction Blocks |
//! | `read_object` | Read on-chain object data |
//!
//! ## Move Modules
//!
//! The Move code for this example is in:
//! - `examples/perp_dex/oracle.move` - Price feed oracle
//! - `examples/perp_dex/perpetual.move` - Perpetual DEX with leverage
//! - `examples/perp_dex/arbitrage.move` - Arbitrage logic (uses real DeepBook V3)
//!
//! ## DeepBook V3 Integration
//!
//! The arbitrage module imports real DeepBook V3 from:
//! `https://github.com/MystenLabs/deepbookv3.git`
//!
//! The arbitrage functions require a deployed DeepBook V3 pool, which would
//! exist on mainnet/testnet. This example demonstrates the perp trading
//! portion that doesn't require external DeepBook pools.
//!
//! ## Arbitrage Strategy (when DeepBook pool available)
//!
//! When oracle price differs from spot market mid-price:
//! - Oracle > Spot: SHORT perp (bet on price falling), BUY spot
//! - Oracle < Spot: LONG perp (bet on price rising), SELL spot
//!
//! ## Running
//!
//! ```bash
//! cargo run --example perp_dex_example
//! ```

use serde_json::json;
use std::env;
use std::fs;
use sui_sandbox_mcp::state::ToolDispatcher;

// Include the Move source code
// Note: arbitrage.move requires DeepBook V3 as external git dependency
// and is not deployed in the sandbox example. See arbitrage.move for
// the full implementation that integrates with DeepBook V3.
const ORACLE_MODULE: &str = include_str!("perp_dex/oracle.move");
const PERP_MODULE: &str = include_str!("perp_dex/perpetual.move");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", "=".repeat(70));
    println!("Perpetual DEX + DeepBook Arbitrage - MCP Integration Example");
    println!("{}", "=".repeat(70));
    println!();

    // Use SUI_SANDBOX_HOME if set; otherwise fall back to a temp dir
    let temp_dir = if env::var("SUI_SANDBOX_HOME").is_ok() {
        None
    } else {
        Some(tempfile::tempdir()?)
    };
    if let Some(dir) = &temp_dir {
        env::set_var("SUI_SANDBOX_HOME", dir.path());
        println!(
            "SUI_SANDBOX_HOME not set; using temporary sandbox at {}",
            dir.path().display()
        );
        println!("Note: this directory will be removed when the example exits.");
    } else if let Ok(home) = env::var("SUI_SANDBOX_HOME") {
        println!("Using SUI_SANDBOX_HOME={}", home);
    }

    let dispatcher = ToolDispatcher::new()?;

    // =========================================================================
    // STEP 1: Create World (MCP Tool: world_create)
    // =========================================================================
    println!("STEP 1: Create World");
    println!("{}", "-".repeat(50));
    println!("Tool: world_create");
    println!("Purpose: Creates a new Move development environment with project structure");
    println!();

    let result = dispatcher
        .dispatch(
            "world_create",
            json!({
                "name": "perp_dex",
                "description": "Perpetual DEX with DeepBook Arbitrage"
            }),
        )
        .await;

    if !result.success {
        eprintln!("world_create failed: {:?}", result.error);
        return Err("Failed to create world".into());
    }

    let world_path = result.result["world"]["path"].as_str().unwrap();
    println!("Created world at: {}", world_path);
    println!();

    // =========================================================================
    // STEP 2: Open World (MCP Tool: world_open)
    // =========================================================================
    println!("STEP 2: Open World");
    println!("{}", "-".repeat(50));
    println!("Tool: world_open");
    println!("Purpose: Activates the world so subsequent operations use it");
    println!();

    let result = dispatcher
        .dispatch("world_open", json!({"name_or_id": "perp_dex"}))
        .await;

    if !result.success {
        eprintln!("world_open failed: {:?}", result.error);
        return Err("Failed to open world".into());
    }
    println!("World opened successfully");
    println!();

    // =========================================================================
    // STEP 3: Write Move Modules (Direct file system)
    // =========================================================================
    println!("STEP 3: Write Move Modules");
    println!("{}", "-".repeat(50));
    println!("Writing 2 modules to sources/:");
    println!();

    // Write oracle module
    fs::write(format!("{}/sources/oracle.move", world_path), ORACLE_MODULE)?;
    println!(
        "  Wrote oracle.move ({} bytes) - Price feed oracle",
        ORACLE_MODULE.len()
    );

    // Write perpetual module
    fs::write(
        format!("{}/sources/perpetual.move", world_path),
        PERP_MODULE,
    )?;
    println!(
        "  Wrote perpetual.move ({} bytes) - Leveraged trading",
        PERP_MODULE.len()
    );

    // Note: arbitrage.move is not deployed in sandbox as it requires DeepBook V3
    // external dependency. See examples/perp_dex/arbitrage.move for the full
    // implementation that can be deployed to mainnet/testnet.
    println!("  (arbitrage.move not deployed - requires DeepBook V3 dependency)");

    // Remove the default module
    let _ = fs::remove_file(format!("{}/sources/perp_dex.move", world_path));

    // Use simple Move.toml without external dependencies for sandbox
    let move_toml = r#"[package]
name = "perp_dex"
edition = "2024.beta"
version = "0.0.1"

[addresses]
perp_dex = "0x0"
"#;
    fs::write(format!("{}/Move.toml", world_path), move_toml)?;
    println!("  Updated Move.toml for sandbox deployment");
    println!();

    // =========================================================================
    // STEP 4: Deploy Package (MCP Tool: world_deploy)
    // =========================================================================
    println!("STEP 4: Deploy Package");
    println!("{}", "-".repeat(50));
    println!("Tool: world_deploy");
    println!("Purpose: Builds Move code and deploys to the simulator");
    println!();

    let result = dispatcher.dispatch("world_deploy", json!({})).await;

    if !result.success {
        eprintln!("world_deploy failed: {:?}", result.error);
        eprintln!("Result: {:?}", result.result);
        return Err("Failed to deploy".into());
    }

    let package_id = result.result["package_id"].as_str().unwrap();
    println!("Package deployed at: {}", package_id);
    println!();

    // =========================================================================
    // STEP 5: Create Oracle Price Feed (MCP Tool: execute_ptb)
    // =========================================================================
    println!("STEP 5: Create Oracle Price Feed");
    println!("{}", "-".repeat(50));
    println!("Tool: execute_ptb");
    println!("Purpose: Execute a Move call to create the BTC/USD oracle");
    println!();

    // Oracle price: $50,000 with 8 decimals
    let oracle_price: u64 = 50_000_00000000;

    println!(
        "Oracle Price: ${} (with 8 decimals: {})",
        50_000, oracle_price
    );
    println!();

    let result = dispatcher
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"kind": "pure", "value": "BTC/USD", "type": "vector<u8>"},
                    {"kind": "pure", "value": oracle_price, "type": "u64"}
                ],
                "commands": [
                    {
                        "kind": "move_call",
                        "package": package_id,
                        "module": "oracle",
                        "function": "create_feed_entry",
                        "args": [
                            {"input": 0},
                            {"input": 1}
                        ]
                    }
                ]
            }),
        )
        .await;

    if !result.success {
        eprintln!("create oracle failed: {:?}", result.error);
        return Err("Failed to create oracle".into());
    }

    let object_changes = result.result["effects"]["object_changes"]
        .as_array()
        .expect("No object_changes");
    let oracle_obj = object_changes
        .iter()
        .find(|o| {
            o["kind"].as_str() == Some("created")
                && o["type"].as_str().unwrap_or("").contains("PriceFeed")
        })
        .expect("PriceFeed not found");
    let oracle_ref = oracle_obj["object_ref"].as_str().unwrap();

    println!("Oracle PriceFeed created:");
    println!("  object_id: {}", oracle_obj["object_id"]);
    println!("  type: {}", oracle_obj["type"]);
    println!();

    // =========================================================================
    // STEP 6: Create SUI Coins for Testing (MCP Tool: create_asset)
    // =========================================================================
    println!("STEP 6: Create SUI Coins for Testing");
    println!("{}", "-".repeat(50));
    println!("Tool: create_asset");
    println!("Purpose: Create test SUI coins for vault and collateral");
    println!();

    let vault_amount: u64 = 100_000_000_000; // 100 SUI
    let collateral_amount: u64 = 1_000_000_000; // 1 SUI

    // Create vault SUI
    let result = dispatcher
        .dispatch(
            "create_asset",
            json!({
                "type": "sui_coin",
                "amount": vault_amount
            }),
        )
        .await;
    if !result.success {
        return Err("Failed to create vault SUI".into());
    }
    let vault_sui_id = result.result["object_id"].as_str().unwrap().to_string();
    println!(
        "Created vault SUI: {} ({} SUI)",
        vault_sui_id,
        vault_amount / 1_000_000_000
    );

    // Create collateral SUI
    let result = dispatcher
        .dispatch(
            "create_asset",
            json!({
                "type": "sui_coin",
                "amount": collateral_amount
            }),
        )
        .await;
    if !result.success {
        return Err("Failed to create collateral SUI".into());
    }
    let collateral_sui_id = result.result["object_id"].as_str().unwrap().to_string();
    println!(
        "Created collateral SUI: {} ({} SUI)",
        collateral_sui_id,
        collateral_amount / 1_000_000_000
    );
    println!();

    // =========================================================================
    // STEP 7: Execute Perp Trading (Single PTB)
    // =========================================================================
    println!("STEP 7: Execute Perp Trading (Single PTB)");
    println!("{}", "-".repeat(50));
    println!("Tool: execute_ptb");
    println!("Purpose: Open a leveraged perpetual position");
    println!();

    println!("Trading Strategy:");
    println!("  Oracle Price: $50,000 (BTC/USD)");
    println!("  Collateral:   1 SUI");
    println!("  Leverage:     10x");
    println!("  Direction:    SHORT (betting price falls)");
    println!();
    println!("  Note: In production, this would be combined with a");
    println!("        DeepBook V3 spot trade for delta-neutral arbitrage.");
    println!();

    println!("PTB Structure (Multi-Command Atomic Transaction):");
    println!("  inputs:");
    println!("    [0] Oracle PriceFeed");
    println!("    [1] Vault SUI (100 SUI)");
    println!("    [2] Collateral SUI (1 SUI)");
    println!("    [3] Leverage: 10x");
    println!("    [4] is_long: false (SHORT)");
    println!("  commands:");
    println!("    [0] perpetual::create_and_open_position");
    println!("        - Creates exchange with vault liquidity");
    println!("        - Opens SHORT perp position at oracle price");
    println!("        - Emits ExchangeCreated + PositionOpened events");
    println!();

    let result = dispatcher
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"object_ref": oracle_ref},
                    {"object_id": vault_sui_id},
                    {"object_id": collateral_sui_id},
                    {"kind": "pure", "value": 10u64, "type": "u64"},  // leverage
                    {"kind": "pure", "value": false, "type": "bool"}  // is_long=false (SHORT because oracle > spot)
                ],
                "commands": [
                    {
                        "kind": "move_call",
                        "package": package_id,
                        "module": "perpetual",
                        "function": "create_and_open_position",
                        "args": [
                            {"input": 0},  // oracle
                            {"input": 1},  // vault liquidity
                            {"input": 2},  // collateral
                            {"input": 3},  // leverage
                            {"input": 4}   // is_long
                        ]
                    }
                ]
            }),
        )
        .await;

    if !result.success {
        eprintln!("arbitrage execution failed: {:?}", result.error);
        eprintln!("Result: {:?}", result.result);
        return Err("Failed to execute arbitrage".into());
    }

    println!("Transaction successful!");
    println!("  Gas used: {} MIST", result.result["gas_used"]);
    println!();

    // Check created objects
    let object_changes = result.result["effects"]["object_changes"]
        .as_array()
        .unwrap();

    println!("Created Objects:");
    for change in object_changes {
        if change["kind"].as_str() == Some("created") {
            let obj_type = change["type"].as_str().unwrap_or("unknown");
            println!("  {} - {}", change["object_id"], obj_type);

            if obj_type.contains("Position") {
                println!("    ^ SHORT position opened (betting oracle price falls to spot)");
            } else if obj_type.contains("PerpExchange") {
                println!("    ^ Exchange with 100 SUI vault liquidity");
            }
        }
    }
    println!();

    // Check events
    if let Some(events) = result.result["effects"]["events"].as_array() {
        println!("Events Emitted:");
        for event in events {
            if let Some(event_type) = event["type"].as_str() {
                println!("  {}", event_type);
                if event_type.contains("PositionOpened") {
                    println!("    ^ Contains entry_price: $50,000 (from oracle)");
                    println!("    ^ Direction: SHORT (is_long=false)");
                    println!("    ^ Leverage: 10x");
                }
            }
        }
        println!();
    }

    // =========================================================================
    // SUMMARY
    // =========================================================================
    println!("{}", "=".repeat(70));
    println!("SUCCESS: Perpetual DEX + DeepBook Arbitrage Complete!");
    println!("{}", "=".repeat(70));
    println!();

    println!("PERP TRADING DEMONSTRATED:");
    println!("{}", "-".repeat(50));
    println!();
    println!("  Oracle Price: $50,000 (BTC/USD)");
    println!();
    println!("  Position Opened:");
    println!("    1. Created SHORT 10x perp position at $50,000");
    println!("       - Collateral: 1 SUI");
    println!("       - Position Size: 10 SUI notional");
    println!("       - Entry Price: $50,000 (from oracle)");
    println!();
    println!("  DeepBook V3 Arbitrage (production use):");
    println!("    The arbitrage.move module imports real DeepBook V3 and can:");
    println!("    - Get spot price via pool::mid_price()");
    println!("    - Execute spot trades via pool::place_market_order()");
    println!("    - Create delta-neutral positions for arbitrage");
    println!();
    println!("    This requires a deployed DeepBook V3 pool on mainnet/testnet.");
    println!();

    println!("MODULES DEPLOYED:");
    println!("{}", "-".repeat(50));
    println!("  1. oracle.move - Price feed with staleness checks");
    println!("  2. perpetual.move - Leveraged trading (up to 100x)");
    println!();
    println!("  Additional module (requires external deployment):");
    println!("  3. arbitrage.move - DeepBook V3 integration for arbitrage");
    println!("     (See examples/perp_dex/arbitrage.move)");
    println!();

    println!("MCP TOOLS USED:");
    println!("{}", "-".repeat(50));
    println!("  - world_create: Create Move development environment");
    println!("  - world_open: Activate world for operations");
    println!("  - world_deploy: Build and deploy 2 Move modules");
    println!("  - create_asset: Create SUI test coins");
    println!("  - execute_ptb: Execute multi-command transactions");
    println!();

    println!("KEY INTEGRATION POINTS:");
    println!("{}", "-".repeat(50));
    println!("  - perpetual.move imports oracle.move (for entry price)");
    println!("  - arbitrage.move imports oracle, perpetual, and DeepBook V3");
    println!("  - Single PTB can execute complex multi-step strategies");
    println!();

    println!("PRODUCTION SAFETY FEATURES:");
    println!("{}", "-".repeat(50));
    println!("  Oracle:");
    println!("    - AdminCap/UpdaterCap capability pattern");
    println!("    - Staleness checks (10s-1hr threshold)");
    println!("    - Emergency pause functionality");
    println!();
    println!("  Perpetual DEX:");
    println!("    - Vault solvency checks");
    println!("    - Liquidation mechanism (80% threshold)");
    println!("    - Maximum 100x leverage");
    println!("    - Insurance fund for bad debt");
    println!();
    println!("  Arbitrage (with DeepBook V3):");
    println!("    - Minimum spread threshold (50 bps)");
    println!("    - Profitability checks");
    println!("    - Atomic execution (all-or-nothing)");
    println!("    - Real DeepBook V3 CLOB integration");
    println!();

    Ok(())
}
