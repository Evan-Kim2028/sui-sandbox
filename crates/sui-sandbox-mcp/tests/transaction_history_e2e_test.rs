//! E2E tests demonstrating transaction history capture with various PTB edge cases.

use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use sui_sandbox_mcp::state::ToolDispatcher;

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

struct TestContext {
    dispatcher: ToolDispatcher,
    #[allow(dead_code)]
    world_name: String,
}

impl TestContext {
    async fn new(test_name: &str) -> Self {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("history_test_{}_{}_", test_name, counter))
            .tempdir()
            .unwrap();
        std::env::set_var("SUI_SANDBOX_HOME", temp_dir.path());
        let _temp_path = temp_dir.keep();

        let dispatcher = ToolDispatcher::new().unwrap();
        let world_name = format!("{}_{}", test_name, counter);

        // Create world
        let result = dispatcher
            .dispatch(
                "world_create",
                json!({
                    "name": &world_name,
                    "description": format!("History test: {}", test_name)
                }),
            )
            .await;
        assert!(result.success, "Failed to create world: {:?}", result.error);

        // Open world
        let result = dispatcher
            .dispatch("world_open", json!({"name_or_id": &world_name}))
            .await;
        assert!(result.success, "Failed to open world: {:?}", result.error);

        Self {
            dispatcher,
            world_name,
        }
    }

    async fn dispatch(
        &self,
        tool: &str,
        input: serde_json::Value,
    ) -> sui_sandbox_mcp::ToolResponse {
        self.dispatcher.dispatch(tool, input).await
    }
}

// =============================================================================
// Edge Case 1: Empty PTB (no commands)
// =============================================================================

#[tokio::test]
async fn test_edge_case_empty_ptb() {
    let ctx = TestContext::new("empty_ptb").await;

    // Empty PTB should fail gracefully (commands are required)
    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [],
                "commands": [],
                "options": {
                    "description": "Edge case: Empty PTB"
                }
            }),
        )
        .await;

    // Should fail because commands are required
    assert!(!result.success || result.result.get("error").is_some());
    println!(
        "Empty PTB: success={}, error={:?}",
        result.success, result.error
    );
}

// =============================================================================
// Edge Case 2: Pure values with various types
// =============================================================================

#[tokio::test]
async fn test_edge_case_pure_value_types() {
    let ctx = TestContext::new("pure_values").await;

    // Test various pure value formats with SplitCoins (a simple command)
    let test_cases = vec![
        ("u64_zero", json!({"pure": {"u64": 0}})),
        ("u64_large", json!({"pure": {"u64": 1000000000}})),
        ("u64_string", json!({"pure": {"u64": "12345"}})),
    ];

    for (name, input) in test_cases {
        println!("\nTesting pure value: {}", name);

        let result = ctx
            .dispatch(
                "execute_ptb",
                json!({
                    "inputs": [input],
                    "commands": [
                        {
                            "split_coins": {
                                "coin": {"gas_coin": true},
                                "amounts": [{"input": 0}]
                            }
                        }
                    ],
                    "options": {
                        "description": format!("Edge case: Pure value {}", name)
                    }
                }),
            )
            .await;

        println!(
            "  success={}, gas_used={:?}",
            result.success,
            result.result.get("gas_used")
        );
    }

    // Check history captured all transactions
    let history = ctx.dispatch("history_list", json!({"limit": 10})).await;
    println!("\nTransaction history: {:?}", history.result);
}

// =============================================================================
// Edge Case 3: SplitCoins and MergeCoins
// =============================================================================

#[tokio::test]
async fn test_edge_case_coin_operations() {
    let ctx = TestContext::new("coin_ops").await;

    // Test SplitCoins from gas coin
    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"pure": {"u64": 1000}},
                    {"pure": {"u64": 500}},
                    {"pure": {"u64": 300}}
                ],
                "commands": [
                    {
                        "split_coins": {
                            "coin": {"gas_coin": true},
                            "amounts": [
                                {"input": 0},
                                {"input": 1},
                                {"input": 2}
                            ]
                        }
                    }
                ],
                "options": {
                    "description": "Edge case: SplitCoins multiple amounts"
                }
            }),
        )
        .await;

    println!(
        "SplitCoins: success={}, result={:?}",
        result.success, result.result
    );

    // Test Split then Merge in same PTB
    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"pure": {"u64": 100}},
                    {"pure": {"u64": 200}}
                ],
                "commands": [
                    {
                        "split_coins": {
                            "coin": {"gas_coin": true},
                            "amounts": [{"input": 0}, {"input": 1}]
                        }
                    },
                    {
                        "merge_coins": {
                            "destination": {"nested_result": [0, 0]},
                            "sources": [{"nested_result": [0, 1]}]
                        }
                    }
                ],
                "options": {
                    "description": "Edge case: Split then Merge in one PTB"
                }
            }),
        )
        .await;

    println!("Split+Merge: success={}", result.success);
    if !result.success {
        println!("  error: {:?}", result.error);
    }
}

// =============================================================================
// Edge Case 4: TransferObjects
// =============================================================================

#[tokio::test]
async fn test_edge_case_transfer_objects() {
    let ctx = TestContext::new("transfer").await;

    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"pure": {"u64": 100}},
                    {"pure": {"address": "0x1"}}
                ],
                "commands": [
                    {
                        "split_coins": {
                            "coin": {"gas_coin": true},
                            "amounts": [{"input": 0}]
                        }
                    },
                    {
                        "transfer_objects": {
                            "objects": [{"nested_result": [0, 0]}],
                            "address": {"input": 1}
                        }
                    }
                ],
                "options": {
                    "description": "Edge case: Split and Transfer"
                }
            }),
        )
        .await;

    println!("TransferObjects: success={}", result.success);
    if result.success {
        println!("  effects: {:?}", result.result.get("effects"));
    }
}

// =============================================================================
// Edge Case 5: MakeMoveVec
// =============================================================================

#[tokio::test]
async fn test_edge_case_make_move_vec() {
    let ctx = TestContext::new("make_vec").await;

    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"pure": {"u64": 1}},
                    {"pure": {"u64": 2}},
                    {"pure": {"u64": 3}}
                ],
                "commands": [
                    {
                        "make_move_vec": {
                            "type": "u64",
                            "elements": [
                                {"input": 0},
                                {"input": 1},
                                {"input": 2}
                            ]
                        }
                    }
                ],
                "options": {
                    "description": "Edge case: MakeMoveVec<u64>"
                }
            }),
        )
        .await;

    println!("MakeMoveVec: success={}", result.success);
    if !result.success {
        println!("  error: {:?}", result.error);
    }
}

// =============================================================================
// Edge Case 6: Failed transaction recording
// =============================================================================

#[tokio::test]
async fn test_edge_case_failed_transaction() {
    let ctx = TestContext::new("failed_tx").await;

    // Try to call a non-existent function - should fail
    let result = ctx
        .dispatch(
            "call_function",
            json!({
                "package": "0x1",
                "module": "nonexistent",
                "function": "does_not_exist",
                "args": [],
                "options": {
                    "description": "Edge case: Calling non-existent function"
                }
            }),
        )
        .await;

    println!(
        "Failed call: success={}, error={:?}",
        result.success, result.error
    );

    // Check that the failed transaction was recorded in history
    let history = ctx.dispatch("history_list", json!({"limit": 5})).await;
    println!("History after failed tx: {:?}", history.result);

    // Search for failed transactions
    let search = ctx
        .dispatch(
            "history_search",
            json!({
                "success": false,
                "limit": 10
            }),
        )
        .await;
    println!("Failed transactions: {:?}", search.result);
}

// =============================================================================
// Edge Case 7: Transaction history operations
// =============================================================================

#[tokio::test]
async fn test_edge_case_history_operations() {
    let ctx = TestContext::new("history_ops").await;

    // Execute a few transactions
    for i in 0..3 {
        let _ = ctx
            .dispatch(
                "execute_ptb",
                json!({
                    "inputs": [{"pure": {"u64": (i + 1) * 100}}],
                    "commands": [
                        {
                            "split_coins": {
                                "coin": {"gas_coin": true},
                                "amounts": [{"input": 0}]
                            }
                        }
                    ],
                    "options": {
                        "description": format!("Batch transaction {}", i),
                        "tags": ["test", "batch"]
                    }
                }),
            )
            .await;
    }

    // Get summary
    let summary = ctx.dispatch("history_summary", json!({})).await;
    println!("History summary: {:?}", summary.result);

    // List with pagination
    let page1 = ctx
        .dispatch("history_list", json!({"limit": 2, "offset": 0}))
        .await;
    println!("Page 1: {:?}", page1.result);

    // Get transaction by sequence
    let detail = ctx.dispatch("history_get", json!({"sequence": 1})).await;
    println!("Transaction #1: {:?}", detail.result);

    // Search by description
    let search = ctx
        .dispatch(
            "history_search",
            json!({
                "description_contains": "Batch",
                "limit": 10
            }),
        )
        .await;
    println!("Search 'Batch': {:?}", search.result);
}

// =============================================================================
// Edge Case 8: Gas budget variations
// =============================================================================

#[tokio::test]
async fn test_edge_case_gas_budget() {
    let ctx = TestContext::new("gas_budget").await;

    // Normal gas budget
    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [{"pure": {"u64": 100}}],
                "commands": [
                    {
                        "split_coins": {
                            "coin": {"gas_coin": true},
                            "amounts": [{"input": 0}]
                        }
                    }
                ],
                "options": {
                    "gas_budget": 10000000,
                    "description": "Normal gas budget"
                }
            }),
        )
        .await;

    println!(
        "Normal gas: success={}, gas_used={:?}",
        result.success,
        result.result.get("gas_used")
    );

    // Very high gas budget
    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [{"pure": {"u64": 100}}],
                "commands": [
                    {
                        "split_coins": {
                            "coin": {"gas_coin": true},
                            "amounts": [{"input": 0}]
                        }
                    }
                ],
                "options": {
                    "gas_budget": 50000000000_u64,
                    "description": "High gas budget"
                }
            }),
        )
        .await;

    println!(
        "High gas: success={}, gas_used={:?}",
        result.success,
        result.result.get("gas_used")
    );
}

// =============================================================================
// Edge Case 9: Complex chaining
// =============================================================================

#[tokio::test]
async fn test_edge_case_complex_chain() {
    let ctx = TestContext::new("complex_chain").await;

    // Complex PTB: split, transfer some, merge rest
    let result = ctx
        .dispatch(
            "execute_ptb",
            json!({
                "inputs": [
                    {"pure": {"u64": 1000}},
                    {"pure": {"u64": 500}},
                    {"pure": {"u64": 200}},
                    {"pure": {"address": "0x1"}},
                    {"pure": {"address": "0x2"}}
                ],
                "commands": [
                    // Split into 3 coins
                    {
                        "split_coins": {
                            "coin": {"gas_coin": true},
                            "amounts": [{"input": 0}, {"input": 1}, {"input": 2}]
                        }
                    },
                    // Transfer first coin to address1
                    {
                        "transfer_objects": {
                            "objects": [{"nested_result": [0, 0]}],
                            "address": {"input": 3}
                        }
                    },
                    // Transfer second coin to address2
                    {
                        "transfer_objects": {
                            "objects": [{"nested_result": [0, 1]}],
                            "address": {"input": 4}
                        }
                    }
                    // Third coin (nested_result: [0, 2]) stays with sender
                ],
                "options": {
                    "description": "Complex chain: split and multi-transfer"
                }
            }),
        )
        .await;

    println!("Complex chain: success={}", result.success);
    if result.success {
        if let Some(effects) = result.result.get("effects") {
            println!("  created: {:?}", effects.get("created"));
            println!("  object_changes: {:?}", effects.get("object_changes"));
        }
    } else {
        println!("  error: {:?}", result.error);
    }

    // Check history captured the transaction with all details
    let history = ctx.dispatch("history_get", json!({"sequence": 1})).await;
    if history.success {
        if let Some(tx) = history.result.get("transaction") {
            println!("Recorded transaction:");
            println!("  description: {:?}", tx.get("description"));
            println!("  objects_created: {:?}", tx.get("objects_created"));
            println!("  gas_used: {:?}", tx.get("gas_used"));
        }
    }
}

// =============================================================================
// Edge Case 10: History configuration
// =============================================================================

#[tokio::test]
async fn test_edge_case_history_config() {
    let ctx = TestContext::new("history_config").await;

    // Check initial config
    let config = ctx.dispatch("history_configure", json!({})).await;
    println!("Initial config: {:?}", config.result);

    // Execute a transaction
    let _ = ctx.dispatch("execute_ptb", json!({
        "inputs": [{"pure": {"u64": 100}}],
        "commands": [{"split_coins": {"coin": {"gas_coin": true}, "amounts": [{"input": 0}]}}],
        "options": {"description": "Before disable"}
    })).await;

    // Disable history
    let config = ctx
        .dispatch("history_configure", json!({"enabled": false}))
        .await;
    println!("After disable: {:?}", config.result);

    // This transaction should NOT be recorded
    let _ = ctx.dispatch("execute_ptb", json!({
        "inputs": [{"pure": {"u64": 200}}],
        "commands": [{"split_coins": {"coin": {"gas_coin": true}, "amounts": [{"input": 0}]}}],
        "options": {"description": "While disabled"}
    })).await;

    // Re-enable history
    let config = ctx
        .dispatch("history_configure", json!({"enabled": true}))
        .await;
    println!("After re-enable: {:?}", config.result);

    // This should be recorded
    let _ = ctx.dispatch("execute_ptb", json!({
        "inputs": [{"pure": {"u64": 300}}],
        "commands": [{"split_coins": {"coin": {"gas_coin": true}, "amounts": [{"input": 0}]}}],
        "options": {"description": "After re-enable"}
    })).await;

    // Check - should have 2 transactions (first and third)
    let history = ctx.dispatch("history_list", json!({"limit": 10})).await;
    println!("Final history: {:?}", history.result);
}
