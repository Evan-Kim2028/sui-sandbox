# Case Study 02: Cetus JACKSON/SUI Swap Replay

**Transaction:** `6YPypxnkG5LW3C3cgeJoezPh8HCyykvWt25N51qzRiAu`
**Pool:** `0xdcd97bb5d843844a6debf28b774488f20d46bc645ac0afbb6f1ebb8d38a9e19b`
**Date:** 2026-01-16
**Status:** ✓ SUCCESS

## Executive Summary

This case study documents the successful replay of a second Cetus DEX swap transaction (JACKSON → SUI). This transaction exercised additional code paths not seen in Case Study 01, revealing two new issues:

1. **Package Version Compatibility**: Loading both upgraded and original packages causes version check failures
2. **Clock Timestamp Mismatch**: Time-dependent validations fail when Clock has wrong timestamp

Both issues were resolved, demonstrating the robustness of our replay infrastructure across different transaction patterns.

---

## What's Working

### All Infrastructure from Case Study 01

Everything documented in Case Study 01 continues to work:

| Component | Status | Description |
|-----------|--------|-------------|
| **VMHarness** | ✓ | Move VM execution orchestration |
| **PTBExecutor** | ✓ | PTB command execution |
| **LocalModuleResolver** | ✓ | Package loading with address remapping |
| **Dynamic Field Runtime** | ✓ | `borrow_child_object`, `hash_type_and_key` |
| **On-Demand Fetcher** | ✓ | Lazy-loads children from gRPC archive |
| **Historical Fetching** | ✓ | Objects at tx-time versions |

### New Capabilities Verified

| Capability | Status | Description |
|------------|--------|-------------|
| **Clock Object Handling** | ✓ | Proper timestamp for time-dependent validations |
| **Package Exclusion** | ✓ | Skip original packages when upgraded version loaded |
| **Rewarder Module** | ✓ | Time-based reward distribution logic |
| **Different Entry Points** | ✓ | `pool_script_v2::swap_a2b` (vs router in CS01) |

---

## How the Simulation Handles Things

### 1. Clock Object Construction

The Clock object (0x6) is a system object that provides the current timestamp. For replay, it must contain the **exact timestamp** from the original transaction:

```
┌─────────────────────────────────────────────────────────────────┐
│  Clock Object BCS Layout                                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Offset  Size   Field                                          │
│   ──────  ────   ─────                                          │
│   0       32     id: UID (object ID = 0x6)                      │
│   32      8      timestamp_ms: u64 (little-endian)              │
│                                                                  │
│   Total: 40 bytes                                                │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

```rust
// Construct Clock with transaction's timestamp
let mut clock_bytes = Vec::with_capacity(40);
let clock_id = parse_address("0x0...06");
clock_bytes.extend_from_slice(clock_id.as_ref());      // 32 bytes UID
clock_bytes.extend_from_slice(&tx_timestamp_ms.to_le_bytes());  // 8 bytes timestamp
historical_objects.insert(clock_id_str, base64_encode(&clock_bytes));
```

### 2. Package Version Management

When an upgraded package is loaded at the original address, we must **not** also load the original package, as they have conflicting `CURRENT_VERSION` constants:

```
┌─────────────────────────────────────────────────────────────────┐
│  Package Loading Strategy                                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Original CLMM (0x1eabed72...):                                │
│     - const CURRENT_VERSION: u64 = 8;  ← OLD                    │
│                                                                  │
│   Upgraded CLMM (0x75b2e9ec...):                                │
│     - const CURRENT_VERSION: u64 = 12;  ← CURRENT               │
│                                                                  │
│   GlobalConfig.package_version = 12                              │
│                                                                  │
│   ✗ Loading both → config::checked_package_version aborts       │
│   ✓ Loading only upgraded at original address → Works           │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

```rust
// Skip original CLMM when loading packages
let original_clmm_str = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
for (pkg_id, modules) in &package_modules_raw {
    if pkg_id == upgraded_clmm || pkg_id == original_clmm_str {
        continue;  // Already loaded upgraded at original address
    }
    resolver.add_package_modules(modules.clone())?;
}
```

### 3. Time-Dependent Validation (Rewarder)

The Cetus rewarder module performs time-based validations during swaps:

```move
// Simplified rewarder::settle logic
public(friend) fun settle(
    rewarder_manager: &mut RewarderManager,
    liquidity: u128,
    current_time: u64  // From Clock
) {
    let last_updated_time = rewarder_manager.last_updated_time;

    // CRITICAL: This check fails if clock timestamp is wrong
    if (last_updated_time > current_time) {
        abort 3  // Invalid time: pool was updated in the "future"
    }

    // ... reward distribution calculations ...
}
```

**Why it matters**: If the Clock's `timestamp_ms` is older than the Pool's `last_updated_time`, the check fails because it appears the pool was updated in the "future" relative to the clock.

---

## Issues Encountered and Solutions

### Issue 1: Package Version Check Abort (Code 10)

**Symptom**:

```
VMError { major_status: ABORTED, sub_status: Some(10),
          message: "0x1eabed72...::config::checked_package_version" }
```

**Stack Trace**:

1. `pool_script_v2::swap_a2b`
2. `pool::flash_swap`
3. `config::checked_package_version` ← ABORT

**Root Cause Analysis**:

The test was loading **both** the upgraded CLMM package (0x75b2e9ec...) AND the original CLMM package (0x1eabed72...) into the resolver. When the code ran:

```move
public fun checked_package_version(config: &GlobalConfig) {
    assert!(CURRENT_VERSION == config.package_version, 10);
}
```

The original CLMM had `CURRENT_VERSION = 8`, but `GlobalConfig.package_version = 12`. The assertion failed with abort code 10.

**Solution**:

When iterating through packages to load, skip any package that matches either the upgraded CLMM address or the original CLMM address:

```rust
// Load other packages (but skip CLMM packages - we already loaded the upgraded one)
let original_clmm_str = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
for (pkg_id, modules) in &package_modules_raw {
    if pkg_id == upgraded_clmm || pkg_id == original_clmm_str {
        continue; // Already loaded upgraded CLMM at original address
    }
    resolver.add_package_modules(modules.clone())?;
}
```

### Issue 2: Rewarder Settle Abort (Code 3)

**Symptom**:

```
VMError { major_status: ABORTED, sub_status: Some(3),
          message: "0x1eabed72...::rewarder::settle at offset 16" }
```

**Stack Trace**:

1. `pool_script_v2::swap_a2b`
2. `pool::flash_swap`
3. `pool::swap_in_pool`
4. `rewarder::settle` ← ABORT

**Root Cause Analysis**:

The rewarder module checks that `last_updated_time <= current_clock_time`. The values were:

| Variable | Value | Human Readable |
|----------|-------|----------------|
| `last_updated_time` | 1768570877 seconds | Jan 16, 2026 |
| `current_clock_time` | 1704067200 seconds | Jan 1, 2024 |

The clock was **2 years in the past** relative to the pool's state!

**Why it happened**:

1. The test was skipping the Clock object during historical fetch ("system object")
2. The `to_ptb_commands_with_objects_and_aliases` function used a 32-byte fallback for missing objects
3. But Clock needs 40 bytes: 32 UID + 8 timestamp
4. With only 32 bytes, there was no timestamp field → effectively timestamp = 0
5. The check `1768570877 <= 0` failed

**Solution**:

Manually construct the Clock object with the transaction's timestamp:

```rust
// Clock struct: { id: UID (32 bytes), timestamp_ms: u64 (8 bytes) } = 40 bytes
let clock_id_str = "0x0000000000000000000000000000000000000000000000000000000000000006";
{
    use base64::Engine;
    let mut clock_bytes = Vec::with_capacity(40);
    let clock_id = parse_address(clock_id_str);
    clock_bytes.extend_from_slice(clock_id.as_ref());  // 32 bytes UID
    clock_bytes.extend_from_slice(&tx_timestamp_ms.to_le_bytes());  // 8 bytes timestamp
    let clock_base64 = base64::engine::general_purpose::STANDARD.encode(&clock_bytes);
    historical_objects.insert(clock_id_str.to_string(), clock_base64);
    println!("   ✓ Clock @ timestamp {} ms: {} bytes", tx_timestamp_ms, clock_bytes.len());
}
```

After fix:

- `last_updated_time`: 1768570877 seconds
- `current_clock_time`: 1768570886 seconds (from tx timestamp)
- Check `1768570877 <= 1768570886` = TRUE ✓

---

## Complete Execution Steps

### Prerequisites

1. **Sui Framework**: Built-in, loaded automatically
2. **gRPC Archive Access**: `archive.mainnet.sui.io:443`
3. **Transaction Metadata**: Including `timestamp_ms` field

### Step-by-Step Replay

```rust
// Step 1: Fetch the transaction
let tx = fetcher.fetch_transaction_sync(TX_DIGEST)?;
let tx_timestamp_ms = tx.timestamp_ms.unwrap_or(1768570886558);

// Step 2: Initialize resolver with Sui framework
let mut resolver = LocalModuleResolver::with_sui_framework()?;

// Step 3: Load upgraded CLMM at original address
let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
let original_clmm = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
let modules = fetcher.fetch_package_modules(upgraded_clmm)?;
resolver.add_package_modules_at(modules, Some(parse_address(original_clmm)))?;

// Step 4: Load other packages (SKIP original CLMM!)
for pkg in packages_to_fetch {
    if pkg == upgraded_clmm || pkg == original_clmm {
        continue;  // Don't load - would cause version conflict
    }
    let modules = fetcher.fetch_package_modules(pkg)?;
    resolver.add_package_modules(modules)?;
}

// Step 5: Create VMHarness with clock configuration
let config = SimulationConfig::default().with_clock_base(tx_timestamp_ms);
let mut harness = VMHarness::with_config(&resolver, false, config)?;

// Step 6: Fetch historical shared objects from effects
let shared_versions = tx.effects.shared_object_versions;
let mut historical_objects = HashMap::new();

// Step 6a: Construct Clock with correct timestamp (CRITICAL!)
let clock_id = "0x0...06";
let mut clock_bytes = Vec::with_capacity(40);
clock_bytes.extend_from_slice(parse_address(clock_id).as_ref());
clock_bytes.extend_from_slice(&tx_timestamp_ms.to_le_bytes());
historical_objects.insert(clock_id, base64_encode(&clock_bytes));

// Step 6b: Fetch other shared objects at historical versions
for (object_id, version) in shared_versions {
    if object_id == clock_id { continue; }  // Already handled
    let obj = fetcher.fetch_object_at_version_full(&object_id, version)?;
    historical_objects.insert(object_id, base64_encode(&obj.bcs_bytes));
}

// Step 7: Set up on-demand child fetcher
let pool_creation_version = 703722732;  // From Pool's initialSharedVersion
harness.set_child_fetcher(Box::new(move |child_id| {
    fetcher.fetch_object_at_version_full(&child_id, pool_creation_version).ok()
}));

// Step 8: Execute replay
let result = transaction.replay_with_objects_and_aliases(
    &mut harness,
    &historical_objects,
    &address_aliases
)?;
assert!(result.local_success);
```

---

## Required Data

### Packages (6 total)

| Package | Address | Modules | Notes |
|---------|---------|---------|-------|
| Sui Framework | 0x1, 0x2 | ~50 | Built-in |
| pool_script_v2 | 0xb2db7142... | 14 | Entry point |
| Cetus CLMM | 0x1eabed72... | 13 | Upgraded from 0x75b2e9ec... |
| Skip List | 0xbe21a06... | 4 | Data structure |
| Integer Math | 0x714a63a0... | 8 | Full math operations |
| JACKSON Token | 0x5ffe80c9... | 1 | Token module |

### Input Objects

| Object | Type | Version | Source |
|--------|------|---------|--------|
| Pool | `pool::Pool<JACKSON, SUI>` | 755027007 | gRPC archive |
| GlobalConfig | `config::GlobalConfig` | 755027142 | gRPC archive |
| Clock | `clock::Clock` | - | Constructed (40 bytes) |
| User JACKSON Coin | Owned | - | gRPC archive |
| User SUI Coin | Owned | - | gRPC archive |

### Critical Configuration

| Parameter | Value | Notes |
|-----------|-------|-------|
| Transaction Timestamp | 1768570886558 ms | Must match Clock |
| Pool's last_updated_time | 1768570877 seconds | From Pool BCS |
| CLMM CURRENT_VERSION | 12 | Must match GlobalConfig |
| Pool Creation Version | 703722732 | For child object fetching |

---

## Verification

### Test Command

```bash
cargo test test_replay_second_cetus_swap -- --nocapture
```

### Expected Output

```
=== Replay Second Cetus Swap Transaction ===

Step 1: Fetching transaction 6YPypxnkG5LW3C3cgeJoezPh8HCyykvWt25N51qzRiAu...
   ✓ Transaction fetched

Step 2: Fetching input objects...
   ✓ Fetched 5 input objects

Step 3: Fetching packages...
   ✓ 0xb2db7142...: 14 modules
   ✓ 0x1eabed72...: 13 modules
   ✓ 0x75b2e9ec...: 13 modules
   ...

Step 4: Initializing resolver...
   Loaded 13 CLMM modules at original address
   Loaded 14 modules from 0xb2db7142...
   ...

Step 5: Creating VM harness...
   Using clock timestamp: 1768570886558 ms (1768570886 seconds)

Step 6: Fetching shared objects at historical versions from effects...
   ✓ Clock @ timestamp 1768570886558 ms: 40 bytes
   ✓ 0xdcd97bb5... @ v755027007: 551 bytes
   ✓ 0xdaa46... @ v755027142: 411 bytes

Step 7: Setting up on-demand child fetcher...
   ✓ Child fetcher configured

Step 8: Replaying transaction...

=== RESULT ===
Success: true

✓ SECOND CETUS SWAP REPLAYED SUCCESSFULLY!
```

---

## Comparison with Case Study 01

| Aspect | LEIA/SUI (CS01) | JACKSON/SUI (CS02) |
|--------|-----------------|-------------------|
| Pool Address | 0x8b7a1b6e... | 0xdcd97bb5... |
| Entry Point | Router (0xeffc8ae6...) | pool_script_v2 |
| Package Loading | Standard | Must exclude original CLMM |
| Clock Handling | Implicit (worked by chance) | Explicit construction required |
| Time Validation | Passed | Required correct timestamp |
| Issues Found | 4 (dynamic fields, state, etc.) | 2 (package version, clock) |
| Result | SUCCESS | SUCCESS |

---

## Key Learnings

### 1. Clock Object is Critical for Time-Dependent Logic

Many DeFi protocols have time-based invariants:

- Reward distribution calculations (`rewarder::settle`)
- Oracle price staleness checks
- Position age requirements
- Slippage protection timestamps

**Always construct the Clock with the transaction's exact timestamp.**

### 2. Package Loading Order Matters

When loading upgraded packages:

1. Load upgraded bytecode at original address first
2. Explicitly skip loading the original package
3. Load remaining packages normally

### 3. System Objects Need Special Handling

System objects like Clock (0x6) are:

- Not fetched from regular object queries
- Have fixed, simple BCS layouts
- Must be manually constructed for replay

### 4. Different Entry Points = Different Code Paths

- CS01 used the Router entry point
- CS02 used pool_script_v2 directly
- This revealed the rewarder time check (not hit in CS01)

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     Case Study 02 Execution Flow                             │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   ┌──────────────────────────────────────────────────────────────────┐      │
│   │  Transaction Metadata                                             │      │
│   │                                                                   │      │
│   │  digest: 6YPypxnkG5LW3C3cgeJoezPh8HCyykvWt25N51qzRiAu           │      │
│   │  timestamp_ms: 1768570886558  ◄─── CRITICAL for Clock            │      │
│   │  commands: [MoveCall(pool_script_v2::swap_a2b, ...)]             │      │
│   └──────────────────────────────────────────────────────────────────┘      │
│                              │                                               │
│                              ▼                                               │
│   ┌──────────────────────────────────────────────────────────────────┐      │
│   │  Package Loading                                                  │      │
│   │                                                                   │      │
│   │  1. Load Sui Framework (built-in)                                │      │
│   │  2. Fetch upgraded CLMM (0x75b2e9ec...)                          │      │
│   │  3. Load at original address (0x1eabed72...)                     │      │
│   │  4. Load pool_script_v2, skip_list, integer_mate, JACKSON        │      │
│   │  5. SKIP original CLMM (version conflict!)                       │      │
│   └──────────────────────────────────────────────────────────────────┘      │
│                              │                                               │
│                              ▼                                               │
│   ┌──────────────────────────────────────────────────────────────────┐      │
│   │  Object Preparation                                               │      │
│   │                                                                   │      │
│   │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │      │
│   │  │   Clock (0x6)   │  │   Pool          │  │  GlobalConfig   │  │      │
│   │  │                 │  │                 │  │                 │  │      │
│   │  │  CONSTRUCTED    │  │  gRPC archive   │  │  gRPC archive   │  │      │
│   │  │  timestamp_ms = │  │  @ v755027007   │  │  @ v755027142   │  │      │
│   │  │  1768570886558  │  │                 │  │                 │  │      │
│   │  └─────────────────┘  └─────────────────┘  └─────────────────┘  │      │
│   └──────────────────────────────────────────────────────────────────┘      │
│                              │                                               │
│                              ▼                                               │
│   ┌──────────────────────────────────────────────────────────────────┐      │
│   │  Execution: pool_script_v2::swap_a2b                             │      │
│   │                                                                   │      │
│   │  swap_a2b()                                                       │      │
│   │    └── pool::flash_swap()                                        │      │
│   │          ├── config::checked_package_version()  ✓ CURRENT_VERSION=12    │
│   │          └── pool::swap_in_pool()                                │      │
│   │                └── rewarder::settle()                            │      │
│   │                      └── assert!(last_updated <= clock_time)  ✓  │      │
│   │                            1768570877 <= 1768570886 = TRUE       │      │
│   └──────────────────────────────────────────────────────────────────┘      │
│                              │                                               │
│                              ▼                                               │
│                        ┌──────────┐                                          │
│                        │ SUCCESS! │                                          │
│                        └──────────┘                                          │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Implications for General Transaction Replay

This case study demonstrates that successful transaction replay requires:

1. **Correct Package Versions**: Load upgraded packages at original addresses, exclude duplicates
2. **Historical Object State**: Fetch all objects at their tx-time versions
3. **System Object Construction**: Manually build Clock, Random, etc. with correct values
4. **Time Consistency**: Transaction timestamp must match Clock object timestamp
5. **Dynamic Field Support**: On-demand fetching for child objects at creation versions

These requirements apply to **any** transaction replay, not just Cetus swaps.

---

## References

- **Test File**: `tests/execute_cetus_swap.rs::test_replay_second_cetus_swap`
- **Cetus Rewarder**: `0x1eabed72...::rewarder::settle`
- **Cetus Config**: `0x1eabed72...::config::checked_package_version`
- **Case Study 01**: `docs/defi-case-study/01_CETUS_SWAP_LEIA_SUI.md`
