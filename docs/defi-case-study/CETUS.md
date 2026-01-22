# Cetus DEX Swap Replay Case Study

This case study demonstrates **successful PTB replay** of Cetus AMM swap transactions.

## Overview

| Capability | Demonstration |
|------------|---------------|
| **Faithful Replay** | Successful transactions replay as SUCCESS |
| **Package Upgrading** | Load upgraded bytecode at original addresses |
| **Historical State** | Fetch objects at exact tx-time versions |
| **Dynamic Fields** | On-demand skip_list node fetching |
| **Clock Construction** | Transaction timestamp for time assertions |

---

## PTB Replay Results

| Transaction | Operation | Status |
|-------------|-----------|--------|
| `7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp` | LEIA → SUI swap | ✓ SUCCESS |
| `6YPypxnkG5LW3C3cgeJoezPh8HCyykvWt25N51qzRiAu` | JACKSON → SUI swap | ✓ SUCCESS |

**Test File**: `tests/execute_cetus_swap.rs`

---

## What the PTB Does

A Cetus swap PTB executes the following sequence:

```text
PTB Commands:
1. SplitCoins     - Split exact input amount from user's coin
2. MoveCall       - flash_swap() borrows output from pool
3. MoveCall       - repay_add_liquidity() pays input to pool
4. MoveCall       - collect_reward() claims any LP rewards
5. TransferObjects - Send output coin to user
```text

The Move VM executes each command sequentially, with results from earlier commands feeding into later ones. The swap reads pool tick data via dynamic fields (skip_list nodes) and updates pool state atomically.

---

## How PTB Replay Works

### Step 1: Fetch Transaction Data

The PTB executor needs three things from the original transaction:

| Data | Source | Purpose |
|------|--------|---------|
| PTB commands | `transaction.data.transaction.data.programmable` | What to execute |
| Input objects | `effects.input_objects` + gRPC archive | Initial state |
| Package bytecode | `transaction.data.transaction.data.input_objects[*].package_id` | Move code |

```rust
let tx = fetcher.fetch_transaction(&digest).await?;
let commands = &tx.data.transaction.data.programmable.commands;
let inputs = &tx.data.transaction.data.programmable.inputs;
```text

### Step 2: Load Packages at Correct Addresses

Sui packages can be upgraded. The blockchain calls the **original address** but we must load the **upgraded bytecode** that was active at transaction time:

```rust
// Cetus CLMM was upgraded: 0x1eabed72... → 0x75b2e9ec...
// Load upgraded bytecode at original address
resolver.add_package_modules_at(upgraded_modules, Some(original_address))?;
```text

**Why this works**: The PTB references `0x1eabed72::pool::swap`. The resolver finds modules at that address. The bytecode matches what was actually executed on-chain because we loaded the upgraded version.

### Step 3: Fetch Historical Object State

Objects must be at their **exact versions** from transaction time. Current state causes wrong results:

```rust
// Pool at tx time: v751677305 (correct liquidity, tick indices)
// Pool now:        v755027007 (different state)
let pool = fetcher.fetch_object_at_version(&pool_id, 751677305).await?;
```text

**Why this works**: The swap computation depends on pool state (current_tick_index, liquidity, fee_rate). Using historical state ensures our execution reads the same values as the original.

### Step 4: Execute with On-Demand Dynamic Field Fetching

Cetus pools store tick data in a skip_list. During execution, the VM calls `borrow_child_object` to access tick nodes. These don't exist in our initial object set—we fetch them on demand:

```rust
// VM requests child object during execution
fn borrow_child_object(parent: &ObjectID, child_id: &ObjectID) -> Object {
    if !storage.contains(child_id) {
        // Fetch from gRPC archive at creation version
        let child = fetcher.fetch_object_at_version(child_id, creation_version)?;
        storage.insert(child);
    }
    storage.get(child_id)
}
```text

**Why this works**: Dynamic field children exist at their creation version. The Pool's `initialSharedVersion` tells us when children were created. The VM gets the exact objects it needs, when it needs them.

### Step 5: Construct Clock with Correct Timestamp

Cetus validates `last_updated_time <= clock.timestamp_ms`. The Clock object (0x6) must contain the original transaction's timestamp:

```rust
let clock_bytes = [
    clock_id.as_ref(),           // 32 bytes UID
    tx.timestamp_ms.to_le_bytes() // 8 bytes timestamp
].concat();
storage.insert_object(clock_id, clock_bytes);
```text

**Why this works**: Time-dependent assertions pass because our Clock matches the original execution environment.

---

## Why PTB Replay Succeeds

The replay produces identical results because we reconstruct the exact execution environment:

| Requirement | How We Achieve It |
|-------------|-------------------|
| Correct bytecode | Load upgraded packages at original addresses |
| Correct state | Fetch objects at historical versions via gRPC |
| Correct dynamic fields | On-demand fetch children at creation versions |
| Correct time | Build Clock with transaction timestamp |
| Correct execution | PTBExecutor dispatches commands to Move VM |

The PTBExecutor handles the command sequence (SplitCoins → MoveCall → TransferObjects), tracks intermediate results, and the VMHarness provides the Move VM with native function support. Each piece is necessary for faithful replay.

---

## PTB Replay Flow

```text
┌─────────────────────────────────────────────────────────────────────────┐
│                         PTB Replay Execution                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   Original TX on Sui Network                                            │
│   ─────────────────────────                                             │
│   Digest: 7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp                  │
│   Commands: SplitCoins → flash_swap → repay → collect_reward → Transfer │
│                                                                          │
│                              │                                           │
│                              ▼                                           │
│   ┌─────────────────────────────────────────────────────────────────┐   │
│   │  1. Fetch: Transaction + Packages + Historical Objects          │   │
│   │     - PTB commands from transaction data                        │   │
│   │     - CLMM/Router bytecode (upgraded at original address)       │   │
│   │     - Pool, Config, Coin objects at tx-time versions            │   │
│   └─────────────────────────────────────────────────────────────────┘   │
│                              │                                           │
│                              ▼                                           │
│   ┌─────────────────────────────────────────────────────────────────┐   │
│   │  2. Load: Modules + Objects into VM                              │   │
│   │     - LocalModuleResolver provides package bytecode              │   │
│   │     - InMemoryStorage provides object state                      │   │
│   │     - Clock constructed with transaction timestamp               │   │
│   └─────────────────────────────────────────────────────────────────┘   │
│                              │                                           │
│                              ▼                                           │
│   ┌─────────────────────────────────────────────────────────────────┐   │
│   │  3. Execute: PTBExecutor runs each command                       │   │
│   │     - SplitCoins: Creates coin with exact input amount           │   │
│   │     - MoveCall: VM executes flash_swap, accesses skip_list       │   │
│   │       → On-demand fetch of tick nodes as dynamic fields          │   │
│   │     - MoveCall: repay_add_liquidity completes swap               │   │
│   │     - TransferObjects: Output coin sent to user                  │   │
│   └─────────────────────────────────────────────────────────────────┘   │
│                              │                                           │
│                              ▼                                           │
│   ┌─────────────────────────────────────────────────────────────────┐   │
│   │  4. Result: Execution succeeds with matching effects             │   │
│   │     ✓ Same coin outputs                                          │   │
│   │     ✓ Same pool state changes                                    │   │
│   │     ✓ Same gas consumption                                       │   │
│   └─────────────────────────────────────────────────────────────────┘   │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```text

---

## Running the Tests

```bash
# Run from scratch (no cache needed, no API keys required)
cargo test --test execute_cetus_swap test_replay_cetus_with_historical_state -- --nocapture

# Clear cache first if you want a completely fresh run
rm -rf .tx-cache && cargo test --test execute_cetus_swap test_replay_cetus_with_historical_state -- --nocapture
```

**Expected Output**: `Local success: true` with `HISTORICAL TRANSACTION REPLAYED SUCCESSFULLY!`

---

## Key Insights for PTB Replay

1. **PTB commands are the execution plan** - Parse and execute them in order
2. **Package addresses matter** - Load upgraded bytecode at addresses the PTB references
3. **Historical state is required** - Current state breaks replay; fetch at tx-time versions
4. **Dynamic fields are lazy** - Fetch child objects on-demand during execution
5. **System objects need construction** - Clock must have the original timestamp

These principles apply to replaying any Sui PTB, not just Cetus swaps.
