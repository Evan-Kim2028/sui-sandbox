//! End-to-End Tests for DeFi Suite - Complex Multi-Module PTB Demonstration
//!
//! This test suite demonstrates the power of Programmable Transaction Blocks (PTB)
//! by coordinating multiple DeFi protocols in atomic transactions.
//!
//! ## DeFi Suite Modules
//! - flash_loan: Hot-potato flash loans (must repay in same PTB)
//! - collateral_vault: Collateral management with health factors
//! - swap_pool: AMM constant-product swaps (x * y = k)
//! - fee_distributor: Fee collection and staking rewards

use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use sui_sandbox_mcp::state::ToolDispatcher;

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

const FLASH_LOAN_MODULE: &str = include_str!("../examples/defi_suite/sources/flash_loan.move");
const COLLATERAL_VAULT_MODULE: &str =
    include_str!("../examples/defi_suite/sources/collateral_vault.move");
const LENDING_POOL_MODULE: &str = include_str!("../examples/defi_suite/sources/lending_pool.move");
const SWAP_POOL_MODULE: &str = include_str!("../examples/defi_suite/sources/swap_pool.move");
const FEE_DISTRIBUTOR_MODULE: &str =
    include_str!("../examples/defi_suite/sources/fee_distributor.move");
const LIQUIDATION_ENGINE_MODULE: &str =
    include_str!("../examples/defi_suite/sources/liquidation_engine.move");
const MOVE_TOML: &str = include_str!("../examples/defi_suite/Move.toml");

struct TestContext {
    dispatcher: ToolDispatcher,
    #[allow(dead_code)]
    world_path: String,
    package_id: String,
}

impl TestContext {
    async fn new(test_name: &str) -> Self {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("defi_test_{}_{}_", test_name, counter))
            .tempdir()
            .unwrap();
        std::env::set_var("SUI_SANDBOX_HOME", temp_dir.path());
        let _temp_path = temp_dir.into_path();

        let dispatcher = ToolDispatcher::new().unwrap();
        let world_name = format!("{}_{}", test_name, counter);

        let result = dispatcher
            .dispatch(
                "world_create",
                json!({"name": &world_name, "description": format!("DeFi Suite: {}", test_name)}),
            )
            .await;
        assert!(result.success, "Failed to create world: {:?}", result.error);

        let world_path = result.result["world"]["path"].as_str().unwrap().to_string();

        let result = dispatcher
            .dispatch("world_open", json!({"name_or_id": &world_name}))
            .await;
        assert!(result.success, "Failed to open world: {:?}", result.error);

        fs::write(
            format!("{}/sources/flash_loan.move", world_path),
            FLASH_LOAN_MODULE,
        )
        .unwrap();
        fs::write(
            format!("{}/sources/collateral_vault.move", world_path),
            COLLATERAL_VAULT_MODULE,
        )
        .unwrap();
        fs::write(
            format!("{}/sources/lending_pool.move", world_path),
            LENDING_POOL_MODULE,
        )
        .unwrap();
        fs::write(
            format!("{}/sources/swap_pool.move", world_path),
            SWAP_POOL_MODULE,
        )
        .unwrap();
        fs::write(
            format!("{}/sources/fee_distributor.move", world_path),
            FEE_DISTRIBUTOR_MODULE,
        )
        .unwrap();
        fs::write(
            format!("{}/sources/liquidation_engine.move", world_path),
            LIQUIDATION_ENGINE_MODULE,
        )
        .unwrap();
        let _ = fs::remove_file(format!("{}/sources/{}.move", world_path, world_name));
        fs::write(format!("{}/Move.toml", world_path), MOVE_TOML).unwrap();

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
                json!({"type": "sui_coin", "amount": amount}),
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
                json!({"inputs": inputs, "commands": commands}),
            )
            .await;
        assert!(result.success, "Dispatch failed: {:?}", result.error);
        let ptb_success = result.result["success"].as_bool().unwrap_or(true);
        assert!(
            ptb_success,
            "PTB failed: {}",
            result.result["error"].as_str().unwrap_or("unknown")
        );
        result.result
    }

    fn find_created_object(
        &self,
        result: &serde_json::Value,
        type_contains: &str,
    ) -> (String, String) {
        let changes = result["effects"]["object_changes"]
            .as_array()
            .expect("No object_changes");
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
        let all_types: Vec<_> = changes
            .iter()
            .filter(|c| c["kind"].as_str() == Some("created"))
            .map(|c| c["type"].as_str().unwrap_or("unknown"))
            .collect();
        panic!(
            "Object of type {} not found. Created: {:?}",
            type_contains, all_types
        );
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
// FLASH LOAN TESTS
// =============================================================================

#[tokio::test]
async fn test_flash_loan_create_pool() {
    let ctx = TestContext::new("flash_create").await;
    let initial_sui = ctx.create_sui(100_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([{"object_id": initial_sui}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "flash_loan", "function": "create_pool_entry", "args": [{"input": 0}]}]),
    ).await;

    let (pool_id, _) = ctx.find_created_object(&result, "FlashPool");
    let (admin_id, _) = ctx.find_created_object(&result, "FlashPoolAdmin");

    assert!(!pool_id.is_empty());
    assert!(!admin_id.is_empty());

    println!("✓ Flash loan pool created");
    println!("  FlashPool: {}", pool_id);
    println!("  Admin: {}", admin_id);
}

#[tokio::test]
async fn test_flash_loan_add_liquidity() {
    let ctx = TestContext::new("flash_liq").await;
    let initial_sui = ctx.create_sui(100_000_000_000).await;
    let additional_sui = ctx.create_sui(50_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([{"object_id": initial_sui}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "flash_loan", "function": "create_pool_entry", "args": [{"input": 0}]}]),
    ).await;

    let (_, pool_ref) = ctx.find_created_object(&result, "FlashPool");

    let result = ctx.execute_ptb(
        json!([{"object_ref": pool_ref}, {"object_id": additional_sui}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "flash_loan", "function": "add_liquidity_entry", "args": [{"input": 0}, {"input": 1}]}]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("LiquidityAdded")));

    println!("✓ Flash loan liquidity added");
}

// =============================================================================
// COLLATERAL VAULT TESTS
// =============================================================================

#[tokio::test]
async fn test_collateral_vault_deposit() {
    let ctx = TestContext::new("vault_deposit").await;
    let deposit_sui = ctx.create_sui(10_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "create_vault_entry", "args": []}]),
    ).await;

    let (_, vault_ref) = ctx.find_created_object(&result, "CollateralVault");

    let result = ctx.execute_ptb(
        json!([{"object_ref": vault_ref}, {"object_id": deposit_sui}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "deposit_entry", "args": [{"input": 0}, {"input": 1}]}]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("CollateralDeposited")));

    println!("✓ Collateral deposited to vault");
}

// =============================================================================
// SWAP POOL TESTS
// =============================================================================

#[tokio::test]
async fn test_swap_pool_create_and_swap() {
    let ctx = TestContext::new("swap_basic").await;
    let sui_reserve = ctx.create_sui(100_000_000_000).await;
    let token_reserve = ctx.create_sui(100_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([{"object_id": sui_reserve}, {"object_id": token_reserve}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "create_pool_entry", "args": [{"input": 0}, {"input": 1}]}]),
    ).await;

    let (pool_id, pool_ref) = ctx.find_created_object(&result, "SwapPool");
    let (lp_id, _) = ctx.find_created_object(&result, "LPToken");
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("PoolCreated")));

    println!("✓ Swap pool created");
    println!("  SwapPool: {}", pool_id);
    println!("  LPToken: {}", lp_id);

    // Swap
    let swap_amount = ctx.create_sui(1_000_000_000).await;
    let result = ctx.execute_ptb(
        json!([{"object_ref": pool_ref}, {"object_id": swap_amount}, {"kind": "pure", "value": 0u64, "type": "u64"}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "swap_sui_for_token_entry", "args": [{"input": 0}, {"input": 1}, {"input": 2}]}]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("Swapped")));

    println!("✓ Swap executed (SUI → TOKEN)");
}

// =============================================================================
// FEE DISTRIBUTOR TESTS
// =============================================================================

#[tokio::test]
async fn test_fee_distributor_collect_fees() {
    let ctx = TestContext::new("fee_dist").await;
    let fees_sui = ctx.create_sui(1_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "fee_distributor", "function": "create_distributor_entry", "args": []}]),
    ).await;

    let (dist_id, dist_ref) = ctx.find_created_object(&result, "FeeDistributor");
    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("DistributorCreated")));

    println!("✓ Fee distributor created: {}", dist_id);

    let result = ctx.execute_ptb(
        json!([{"object_ref": dist_ref}, {"object_id": fees_sui}, {"kind": "pure", "value": "swap_pool", "type": "vector<u8>"}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "fee_distributor", "function": "collect_fees_entry", "args": [{"input": 0}, {"input": 1}, {"input": 2}]}]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("FeesCollected")));

    println!("✓ Fees collected from swap_pool");
}

// =============================================================================
// MULTI-MODULE PTB TESTS - THE MAIN EVENT
// =============================================================================

/// Test creating all DeFi infrastructure in a SINGLE PTB
#[tokio::test]
async fn test_create_full_defi_ecosystem_single_ptb() {
    let ctx = TestContext::new("full_ecosystem").await;

    println!("\n=== Creating Full DeFi Ecosystem in Single PTB ===\n");

    let flash_liquidity = ctx.create_sui(500_000_000_000).await;
    let swap_sui = ctx.create_sui(100_000_000_000).await;
    let swap_token = ctx.create_sui(100_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([{"object_id": flash_liquidity}, {"object_id": swap_sui}, {"object_id": swap_token}]),
        json!([
            {"kind": "move_call", "package": ctx.package_id, "module": "flash_loan", "function": "create_pool_entry", "args": [{"input": 0}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "create_vault_entry", "args": []},
            {"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "create_pool_entry", "args": [{"input": 1}, {"input": 2}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "fee_distributor", "function": "create_distributor_entry", "args": []}
        ]),
    ).await;

    let (flash_pool_id, _) = ctx.find_created_object(&result, "FlashPool");
    let (vault_id, _) = ctx.find_created_object(&result, "CollateralVault");
    let (swap_pool_id, _) = ctx.find_created_object(&result, "SwapPool");
    let (distributor_id, _) = ctx.find_created_object(&result, "FeeDistributor");

    println!("Created DeFi Ecosystem:");
    println!("  ✓ FlashPool: {}", flash_pool_id);
    println!("  ✓ CollateralVault: {}", vault_id);
    println!("  ✓ SwapPool: {}", swap_pool_id);
    println!("  ✓ FeeDistributor: {}", distributor_id);
    println!("\n✓ Full DeFi ecosystem created in SINGLE PTB!");
}

/// Test collateral deposit + swap in same PTB (DeFi composability)
#[tokio::test]
async fn test_collateral_and_swap_combo_ptb() {
    let ctx = TestContext::new("combo_ptb").await;

    println!("\n=== Testing Collateral + Swap Combo PTB ===\n");

    // Setup: Create vault and swap pool
    let setup_sui_1 = ctx.create_sui(100_000_000_000).await;
    let setup_sui_2 = ctx.create_sui(100_000_000_000).await;

    let result = ctx.execute_ptb(json!([]), json!([{"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "create_vault_entry", "args": []}])).await;
    let (_, vault_ref) = ctx.find_created_object(&result, "CollateralVault");

    let result = ctx.execute_ptb(
        json!([{"object_id": setup_sui_1}, {"object_id": setup_sui_2}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "create_pool_entry", "args": [{"input": 0}, {"input": 1}]}]),
    ).await;
    let (_, swap_pool_ref) = ctx.find_created_object(&result, "SwapPool");

    // Combo PTB: Deposit collateral AND swap
    let collateral_sui = ctx.create_sui(50_000_000_000).await;
    let swap_sui = ctx.create_sui(5_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([
            {"object_ref": vault_ref},
            {"object_id": collateral_sui},
            {"object_ref": swap_pool_ref},
            {"object_id": swap_sui},
            {"kind": "pure", "value": 0u64, "type": "u64"}
        ]),
        json!([
            {"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "deposit_entry", "args": [{"input": 0}, {"input": 1}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "swap_sui_for_token_entry", "args": [{"input": 2}, {"input": 3}, {"input": 4}]}
        ]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("CollateralDeposited")));
    assert!(events.iter().any(|e| e.contains("Swapped")));

    println!("✓ Collateral deposited");
    println!("✓ Swap executed");
    println!("✓ Both operations in SINGLE atomic PTB!");
}

/// Ultimate test: Create ecosystem + populate + operate
#[tokio::test]
async fn test_ultimate_defi_flow() {
    let ctx = TestContext::new("ultimate").await;

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("           ULTIMATE DEFI FLOW - PTB COMPOSABILITY DEMO");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Phase 1: Create entire DeFi ecosystem in single PTB
    println!("Phase 1: Creating DeFi Ecosystem (1 PTB)");
    println!("─────────────────────────────────────────");

    let flash_liq = ctx.create_sui(1_000_000_000_000).await;
    let swap_sui = ctx.create_sui(500_000_000_000).await;
    let swap_token = ctx.create_sui(500_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([{"object_id": flash_liq}, {"object_id": swap_sui}, {"object_id": swap_token}]),
        json!([
            {"kind": "move_call", "package": ctx.package_id, "module": "flash_loan", "function": "create_pool_entry", "args": [{"input": 0}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "create_vault_entry", "args": []},
            {"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "create_pool_entry", "args": [{"input": 1}, {"input": 2}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "fee_distributor", "function": "create_distributor_entry", "args": []}
        ]),
    ).await;

    let (flash_pool_id, flash_pool_ref) = ctx.find_created_object(&result, "FlashPool");
    let (vault_id, vault_ref) = ctx.find_created_object(&result, "CollateralVault");
    let (swap_pool_id, swap_pool_ref) = ctx.find_created_object(&result, "SwapPool");
    let (distributor_id, dist_ref) = ctx.find_created_object(&result, "FeeDistributor");
    let (_, dist_admin_ref) = ctx.find_created_object(&result, "DistributorAdmin");

    println!("  ✓ FlashPool:       {}", flash_pool_id);
    println!("  ✓ CollateralVault: {}", vault_id);
    println!("  ✓ SwapPool:        {}", swap_pool_id);
    println!("  ✓ FeeDistributor:  {}\n", distributor_id);

    // Phase 2: User deposits collateral (1 PTB)
    println!("Phase 2: User Deposits Collateral (1 PTB)");
    println!("─────────────────────────────────────────");

    let collateral = ctx.create_sui(100_000_000_000).await;

    let result = ctx.execute_ptb(
        json!([{"object_ref": vault_ref}, {"object_id": collateral}]),
        json!([{"kind": "move_call", "package": ctx.package_id, "module": "collateral_vault", "function": "deposit_entry", "args": [{"input": 0}, {"input": 1}]}]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("CollateralDeposited")));
    println!("  ✓ Deposited 100 SUI as collateral\n");

    // Phase 3: Execute swap + collect fees (1 PTB)
    println!("Phase 3: Execute Swap + Collect Fees (1 PTB)");
    println!("─────────────────────────────────────────────");

    let swap_amount = ctx.create_sui(10_000_000_000).await;
    let mock_fee = ctx.create_sui(30_000_000).await;

    let result = ctx.execute_ptb(
        json!([
            {"object_ref": swap_pool_ref},
            {"object_id": swap_amount},
            {"kind": "pure", "value": 0u64, "type": "u64"},
            {"object_ref": dist_ref},
            {"object_id": mock_fee},
            {"kind": "pure", "value": "swap_pool", "type": "vector<u8>"}
        ]),
        json!([
            {"kind": "move_call", "package": ctx.package_id, "module": "swap_pool", "function": "swap_sui_for_token_entry", "args": [{"input": 0}, {"input": 1}, {"input": 2}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "fee_distributor", "function": "collect_fees_entry", "args": [{"input": 3}, {"input": 4}, {"input": 5}]}
        ]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("Swapped")));
    assert!(events.iter().any(|e| e.contains("FeesCollected")));

    println!("  ✓ Swapped 10 SUI → TOKEN");
    println!("  ✓ Collected 0.03 SUI fee to distributor\n");

    // Phase 4: Advance epoch + add more liquidity (1 PTB)
    println!("Phase 4: Advance Epoch + Add Liquidity (1 PTB)");
    println!("───────────────────────────────────────────────");

    let extra_liq = ctx.create_sui(100_000_000_000).await;

    // Get fresh refs for mutated objects
    let result = ctx.execute_ptb(
        json!([
            {"object_ref": dist_ref},
            {"object_ref": dist_admin_ref},
            {"object_ref": flash_pool_ref},
            {"object_id": extra_liq}
        ]),
        json!([
            {"kind": "move_call", "package": ctx.package_id, "module": "fee_distributor", "function": "advance_epoch_entry", "args": [{"input": 0}, {"input": 1}]},
            {"kind": "move_call", "package": ctx.package_id, "module": "flash_loan", "function": "add_liquidity_entry", "args": [{"input": 2}, {"input": 3}]}
        ]),
    ).await;

    let events = ctx.get_events(&result);
    assert!(events.iter().any(|e| e.contains("EpochAdvanced")));
    assert!(events.iter().any(|e| e.contains("LiquidityAdded")));

    println!("  ✓ Advanced to next epoch");
    println!("  ✓ Added 100 SUI to flash loan pool\n");

    println!("═══════════════════════════════════════════════════════════════");
    println!("                    ULTIMATE FLOW COMPLETE!");
    println!("═══════════════════════════════════════════════════════════════\n");
    println!("Summary:");
    println!("  • Created 4 DeFi protocols in 1 PTB");
    println!("  • User deposited collateral in 1 PTB");
    println!("  • Executed swap + collected fees in 1 PTB");
    println!("  • Advanced epoch + added liquidity in 1 PTB\n");
    println!("Total PTBs used: 4 (vs 8+ individual transactions)");
    println!("Atomicity: All operations are atomic - all-or-nothing!\n");
}
