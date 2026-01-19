# DeepBook Case Study

This case study demonstrates **PTB replay and what-if simulation** using DeepBook CLOB (Central Limit Order Book) transactions.

## Overview

| Capability | Demonstration |
|------------|---------------|
| **Faithful Replay** | Successful transactions replay as SUCCESS |
| **Error Reproduction** | Failed transactions replay with correct error codes |
| **Data Fetching** | gRPC for exact historical state |
| **Package Resolution** | Linkage tables for upgraded packages |
| **What-If Simulation** | Modify inputs/objects to test alternative scenarios |
| **PTB Command Modification** | Rewrite transaction commands arbitrarily |

**Test File**: `tests/execute_deepbook_swap.rs`

---

## Test Results Summary

| Test | Transaction | Description | Result |
|------|-------------|-------------|--------|
| `test_deepbook_two_phase_replay` | `FbrMKMyzWm...` (index 0) | Cancel Order | ✓ SUCCESS |
| `test_deepbook_two_phase_replay` | `DwrqFzBSVH...` (index 2) | Flash Loan Swap | ✓ SUCCESS |
| `test_sandbox_success_vs_failure_contrast` | `DwrqFzBSVH...` | Successful swap | ✓ SUCCESS |
| `test_sandbox_success_vs_failure_contrast` | `D9sMA7x9b8...` | Failed arbitrage | ✗ FAILURE (correct) |
| `test_what_if_execute_modified_loan` | `D9sMA7x9b8...` | Modified loan amounts | ✗ All failed (expected) |
| `test_ptb_command_modification` | `D9sMA7x9b8...` | Simplified PTB | ✓ SUCCESS |

---

# Part 1: PTB Replay

## Data Fetching via gRPC

The core of accurate replay is fetching the **exact historical state** that existed when the transaction executed.

### Key Requirements

| Requirement | Purpose |
|-------------|---------|
| `unchanged_loaded_runtime_objects` | Exact versions for read-only objects |
| Historical object versions | Fetch objects at specific versions |
| On-demand child fetching | Lazy fetch of dynamic fields during execution |

```rust
// Fetch exact versions for read-only objects
let tx = grpc_fetcher.fetch_transaction(&digest).await?;

// Unchanged objects: use exact version from effects
for (object_id, version) in &tx.effects.unchanged_loaded_runtime_objects {
    let obj = fetcher.fetch_object_at_version(object_id, *version).await?;
    storage.insert(obj);
}

// Modified objects: use version-1 for input state
for (object_id, output_version) in &tx.effects.changed_objects {
    let input_version = output_version - 1;
    let obj = fetcher.fetch_object_at_version(&object_id, input_version).await?;
    storage.insert(obj);
}
```text

### On-Demand Child Object Fetching

DeepBook pools use dynamic fields (`big_vector` for order entries). These children are fetched lazily during execution via the gRPC interface:

```rust
// Binary search finds the correct version at the transaction's checkpoint
fn borrow_child_object(parent: &ObjectID, child_id: &ObjectID) -> Object {
    if !storage.contains(child_id) {
        let child = fetcher.binary_search_object_at_checkpoint(child_id, checkpoint)?;
        storage.insert(child);
    }
    storage.get(child_id)
}
```text

---

## Package Linkage Resolution

DeepBook transactions call into upgraded packages (Bluefin, FlowX). We use **linkage tables** to find the correct bytecode.

### The Problem: Version Checks

```move
// Bluefin's config module has a version check
public fun verify_version(config: &GlobalConfig) {
    assert!(config.package_version == CURRENT_VERSION, EInvalidVersion);
}
```text

If we load the original Bluefin package (v1), but GlobalConfig has `package_version = 17`, the check fails.

### The Solution: Linkage Tables

Dependent packages (like the `jk` aggregator) store a linkage table mapping original → storage addresses:

| Package | Original Address | Storage Address | Version |
|---------|------------------|-----------------|---------|
| Bluefin | `0x3492c874c1...` | `0xd075338d10...` | v17 |
| FlowX | `0x25929e7f29...` | `0xde2c47eb0d...` | v7 |
| Integer Mate | `0x03637b7b60...` | `0xab5e63352d...` | v3 |

```rust
// 1. Fetch aggregator package to get its linkage table
let jk_pkg = client.get_object(jk_pkg_id).await?;

// 2. Build linkage map: original_id → (storage_id, version)
for (original_id, storage_id, version) in jk_pkg.linkage {
    package_linkage.insert(original_id, (storage_id, version));
}

// 3. Load upgraded bytecode at original address
let (storage_id, _) = package_linkage.get(&bluefin_original_id)?;
let upgraded_modules = fetcher.fetch_package_modules(&storage_id).await?;
resolver.add_package_modules_at(upgraded_modules, Some(bluefin_original_id))?;
```text

---

## Sandbox Validation: Success vs Failure

The most important validation is that the sandbox **correctly distinguishes** between transactions that should succeed and transactions that should fail.

### The Contrast Test

`test_sandbox_success_vs_failure_contrast` runs two flash loan transactions:

| Transaction | Description | On-Chain | Local Replay |
|-------------|-------------|----------|--------------|
| `DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs` | Profitable flash loan swap | ✓ SUCCESS | ✓ SUCCESS |
| `D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva` | Unprofitable arbitrage | ✗ FAILURE | ✗ FAILURE |

### Results

```text
╔══════════════════════════════════════════════════════════════════════╗
║                        VALIDATION SUMMARY                            ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ DwrqFzBSVHRA | Expected: SUCCESS | Actual: SUCCESS
║ ✓ D9sMA7x9b8xD | Expected: FAILURE | Actual: FAILURE
║   └─ error code 2
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ SANDBOX VALIDATION PASSED                                         ║
║   The local sandbox correctly distinguishes success from failure.   ║
╚══════════════════════════════════════════════════════════════════════╝
```text

This proves the sandbox faithfully reproduces on-chain behavior.

---

## What DeepBook PTBs Do

### Cancel Order (2 commands)

```text
1. MoveCall - generate_proof_as_owner() proves account ownership
2. MoveCall - cancel_order() removes order from book
```text

### Place Limit Order (2 commands)

```text
1. MoveCall - generate_proof_as_owner() proves account ownership
2. MoveCall - place_limit_order() adds order to book at price
```text

### Flash Loan Swap (7 commands)

```text
1. MoveCall  - deepbook::pool::borrow_flashloan_base()
2. MoveCall  - jk::swap() on Bluefin via aggregator
3. MoveCall  - deepbook::pool::swap_exact_base_for_quote()
4. MoveCall  - deepbook::pool::return_flashloan_base()
5. MoveCall  - coin::zero() create empty coin
6. MoveCall  - coin::destroy_zero() destroy empty coin
7. TransferObjects - transfer profits to user
```text

---

# Part 2: What-If Simulation

PTB replay faithfully recreates historical transactions. But what if you want to explore:

- What if the pool had more liquidity?
- What if the price was different?
- What if the flash loan amount was smaller?
- What if the arbitrage had been profitable?

This section shows how to modify objects and transaction inputs to simulate alternative scenarios.

---

## Case Study: Failed Flash Loan Arbitrage

### The Transaction

| Field | Value |
|-------|-------|
| Digest | `D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva` |
| Checkpoint | 235248874 |
| Commands | 16 |
| Status | FAILURE (on-chain) |
| Protocols | DeepBook, Bluefin, FlowX, stSUI |

### What the PTB Does

This is a complex **multi-DEX flash loan arbitrage**:

```text
PTB Commands:
1.  MoveCall - deepbook::pool::borrow_flashloan_base() borrow SUI
2.  MoveCall - jk::deepbookswapv3_0() swap SUI → USDC on DeepBook
3.  MoveCall - jk::bluefinswap_1() swap USDC → stSUI on Bluefin
4.  MoveCall - jk::flowxswapv3_1() swap stSUI → SUI on FlowX
5.  MoveCall - deepbook::pool::swap_exact_base_for_quote() DeepBook swap
6.  MoveCall - deepbook::pool::return_flashloan_base() repay loan
... (cleanup and transfer commands)
```text

### Why It Failed

The original transaction failed on-chain with error code 2 in `deepbook_v3::swap_a2b_`:

```text
VMError {
  major_status: ABORTED,
  sub_status: Some(2),
  message: "deepbook_v3::swap_a2b_ at offset 43"
}
```text

This error indicates **insufficient output** - the swap couldn't produce enough tokens to continue the arbitrage profitably.

---

## Simulation Levels

There are three levels of transaction modification:

| Level | What You Modify | Example |
|-------|-----------------|---------|
| **Input Values** | Pure inputs (amounts, flags) | Change flash loan amount from 587 SUI to 10 SUI |
| **Object State** | BCS bytes of objects | Increase pool liquidity by 10x |
| **PTB Commands** | Transaction structure | Remove swap commands, keep only borrow+return |

---

## Level 1: Input Value Modification

### Flash Loan Amount Testing

The simplest modification is changing pure input values like loan amounts:

```text
=== What-If Simulation: Modified Flash Loan Amount ===

Transaction: D9sMA7x9b8xD6vNJgmhc (flashloan_arb_failed)

Step 2: Analyze pure inputs (potential modification targets)...
  Input 1: 587000000000 (587.00 SUI) <- POTENTIAL LOAN AMOUNT
  Input 10: 146750000000 (146.75 SUI) <- POTENTIAL LOAN AMOUNT

Step 3: Modify flash loan amount for what-if scenarios...

--- Scenario: 1 SUI - small ---
  Modifying loan: 587000000000 -> 1000000000 MIST

--- Scenario: 10 SUI - medium ---
  Modifying loan: 587000000000 -> 10000000000 MIST
```text

### Execution Results

Running `test_what_if_execute_modified_loan`:

```text
=== SUMMARY ===

| Loan Amount (SUI) | Result | Details |
|-------------------|--------|---------|
|                 1 | ✗ FAILED | error code 2 |
|                 5 | ✗ FAILED | error code 2 |
|                10 | ✗ FAILED | error code 2 |
|                50 | ✗ FAILED | error code 2 |
|               587 | ✗ FAILED | error code 2 |

No loan amounts succeeded - arbitrage was not profitable at any size
```text

**Interpretation**: All loan amounts failed, proving the arbitrage route was fundamentally unprofitable at that historical moment - not a sizing issue.

---

## Level 2: Object State Modification

Object state modification allows changing the BCS bytes of objects before execution. The sandbox uses the modified bytes during replay, enabling "what-if" analysis of alternative states.

### Test Output

Running `test_what_if_object_state_modification`:

```text
╔══════════════════════════════════════════════════════════════════════╗
║     OBJECT STATE MODIFICATION: SUCCESS → FAILURE via BCS Bytes       ║
╚══════════════════════════════════════════════════════════════════════╝

Transaction: DwrqFzBSVHRA (flashloan_swap_success)
On-chain status: SUCCESS

======================================================================
SCENARIO 1: Original Objects (unmodified)
======================================================================
  Result: ✓ SUCCESS - success

======================================================================
SCENARIO 2: Modified BalanceManager (owner zeroed)
======================================================================
  BalanceManager: 0x7e63babd8f7e98bf2f21f4a60528f69fba27a5...
  Original owner (bytes 32-63): 0x9a0ee11c9bdc858b9745...
  Modified owner (bytes 32-63): 0x000...000 (zeroed)
  Result: ✗ FAILED - error code 0

╔══════════════════════════════════════════════════════════════════════╗
║                    OBJECT STATE MODIFICATION SUMMARY                 ║
╠══════════════════════════════════════════════════════════════════════╣
║ Scenario              | BalanceManager Owner  | Result               ║
╠══════════════════════════════════════════════════════════════════════╣
║ Original              | Valid (sender addr)   | ✓ SUCCESS            ║
║ Modified              | Invalid (0x000...000) | ✗ FAILED: error code 0 ║
╚══════════════════════════════════════════════════════════════════════╝

✓ OBJECT STATE MODIFICATION VERIFIED!
  Original transaction: SUCCESS
  With zeroed owner: FAILED
  → The sandbox correctly uses modified object state during execution!
```text

### What This Proves

| Aspect | Verification |
|--------|--------------|
| **BCS Bytes Used** | Modified bytes are passed to execution, not re-fetched |
| **State Affects Outcome** | Zeroing owner causes authentication failure |
| **SUCCESS → FAILURE** | Same transaction, corrupted state → different result |
| **Sandbox Fidelity** | Object state modifications are faithfully applied |

### BCS Layout Reference

```text
Coin<T>:
  0-31:  UID (object ID)
  32-39: Balance { value: u64 }

BalanceManager:
  0-31:  UID
  32-63: owner (address)
  64+:   Bag (balances container)
```text

### Modifiable Objects

| Object Type | Purpose | Modifiable Fields |
|-------------|---------|-------------------|
| Pool state | Liquidity, tick, price | `sqrt_price`, `liquidity`, `tick_index` |
| BalanceManager | User balances | `owner`, `balances` |
| GlobalConfig | Protocol settings | `package_version`, `fee_rate` |
| Coin objects | Token amounts | `value` (balance) |

Combined with Level 3 (PTB command modification), you can fully rewrite both the transaction logic and the state it operates on.

---

## Level 3: PTB Command Modification

Beyond modifying inputs and objects, we can **arbitrarily modify the PTB commands themselves**.

### The Approach

The failed arbitrage has 16 commands. By removing swap commands and directly returning borrowed coins:

```text
Original PTB (16 commands):
  0: MoveCall - borrow_flashloan_base
  1: MoveCall - deepbookswapv3_0 (swap SUI → USDC)
  2: MoveCall - bluefinswap_1 (swap USDC → stSUI)
  3: MoveCall - flowxswapv3_1 (swap stSUI → SUI)
  ...
  N: MoveCall - return_flashloan_base

Modified PTB (2 commands):
  0: MoveCall - borrow_flashloan_base
  1: MoveCall - return_flashloan_base (rewired to use borrow's output)
```text

### Test Results

```text
╔══════════════════════════════════════════════════════════════════════╗
║          PTB COMMAND MODIFICATION: Make Failed TX Succeed            ║
╚══════════════════════════════════════════════════════════════════════╝

| Modification | Result | Details |
|--------------|--------|---------|
| Original (unmodified) | ✗ FAILURE | error code 2 |
| Simplified (borrow+return) | ✓ SUCCESS |  |
| Simplified + 1 SUI loan | ✓ SUCCESS |  |

✓ SUCCESS: We modified the failed PTB to execute successfully!
```text

### Argument Rewiring

When modifying commands, rewire argument references:

```rust
let modified_return = PtbCommand::MoveCall {
    package: "0x2c8d603bc51326b8c13cef9dd07031a408a48dddb541963357661df5d3204809",
    module: "pool",
    function: "return_flashloan_base",
    arguments: vec![
        arguments[0].clone(),  // Pool reference (unchanged)
        PtbArgument::NestedResult { index: 0, result_index: 0 }, // Coin from borrow
        PtbArgument::NestedResult { index: 0, result_index: 1 }, // FlashLoan from borrow
    ],
};
```text

---

## Technical Notes

### BCS Layout Discovery

To modify objects, you need to know their BCS layout:

1. **Read the Move source** - struct definitions show field order
2. **Inspect known objects** - compare BCS with expected values
3. **Use protocol documentation** - some protocols document layouts

### State Consistency

When modifying state, ensure consistency:

- Liquidity changes may require tick state updates
- Balance changes must match coin objects
- Protocol invariants should be maintained

---

## Running the Tests

### Basic Replay

```bash
export SURFLUX_API_KEY=your_key_here

# Sandbox validation (success vs failure contrast)
cargo test --test execute_deepbook_swap test_sandbox_success_vs_failure_contrast -- --ignored --nocapture

# DeepBook replay tests
cargo test --test execute_deepbook_swap test_deepbook_two_phase_replay -- --ignored --nocapture

# Test individual transactions by index:
#   0 = cancel_order
#   1 = flashloan_arb_failed (for what-if simulation)
#   2 = flashloan_swap_success (for success/failure contrast)
DEEPBOOK_TX_INDEX=0 cargo test --test execute_deepbook_swap test_deepbook_two_phase_replay -- --ignored --nocapture
```text

### What-If Simulation

```bash
# Level 1: Input value modification - analyze pure inputs
cargo test --test execute_deepbook_swap test_what_if_simulation_modified_loan -- --ignored --nocapture

# Level 1: Execute with modified loan amounts
cargo test --test execute_deepbook_swap test_what_if_execute_modified_loan -- --ignored --nocapture

# Level 2: Object state modification - shows BCS bytes and layout
cargo test --test execute_deepbook_swap test_what_if_object_state_modification -- --ignored --nocapture

# Level 3: PTB command modification (make failed TX succeed)
cargo test --test execute_deepbook_swap test_ptb_command_modification -- --ignored --nocapture
```text

### Package Loading (No API Key)

```bash
cargo test --test execute_deepbook_swap test_deepbook_package_loading -- --nocapture
```text

---

## Key Insights

1. **gRPC provides exact versions** - `unchanged_loaded_runtime_objects` eliminates guesswork
2. **Linkage tables solve version checks** - Load upgraded bytecode at original addresses
3. **Success AND failure reproduce** - The sandbox is deterministic in both directions
4. **Three levels of modification** - Input values → Object state → PTB commands
5. **Trustworthy simulations** - Correct failure reproduction enables trustworthy what-if analysis

---

## Related

- [CETUS.md](CETUS.md) - Cetus AMM swap replay
- [README.md](README.md) - Overview of PTB replay requirements
