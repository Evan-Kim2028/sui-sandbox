# Case Study 01: Cetus LEIA/SUI Swap Replay

**Transaction:** `7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp`
**Pool:** `0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772`
**Checkpoint:** 234219761
**Status:** ✓ SUCCESS

## Executive Summary

This case study documents the successful replay of a historical Cetus DEX swap transaction (LEIA → SUI) using our local Move VM simulation sandbox. The replay demonstrates that our infrastructure can:

1. Fetch and load upgraded Move packages at their original addresses
2. Retrieve historical object state from Sui's gRPC archive
3. Handle complex dynamic field lookups (skip_list nodes)
4. Execute Programmable Transaction Blocks (PTBs) locally with full fidelity

---

## What's Working

### Core Simulation Infrastructure

| Component | Status | Description |
|-----------|--------|-------------|
| **VMHarness** | ✓ | Orchestrates Move VM execution with custom native functions |
| **PTBExecutor** | ✓ | Executes PTB commands (MoveCall, SplitCoins, MergeCoins, TransferObjects) |
| **LocalModuleResolver** | ✓ | Loads packages from bytecode with address remapping |
| **InMemoryStorage** | ✓ | Provides object state to the Move VM |
| **Native Functions** | ✓ | 40+ native functions implemented (real, mocked, or passthrough) |
| **Dynamic Field Runtime** | ✓ | Full support for `borrow_child_object`, `hash_type_and_key` |
| **On-Demand Fetcher** | ✓ | Lazy-loads missing dynamic field children from gRPC archive |

### Supported Operations

- **Move Function Calls**: Full support for entry and non-entry functions
- **Coin Operations**: SplitCoins, MergeCoins with proper balance tracking
- **Object Transfers**: TransferObjects with ownership tracking
- **Dynamic Fields**: Tables, Bags, skip_lists via `sui::dynamic_field` natives
- **Type Checking**: Runtime type verification for generic parameters
- **BCS Serialization**: Proper encoding/decoding for all Move types

---

## How the Simulation Handles Things

### 1. Package Loading and Linkage

```
┌─────────────────────────────────────────────────────────────────┐
│  Package Resolution Pipeline                                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   1. Identify packages from transaction Move calls               │
│   2. Fetch bytecode via JSON-RPC `getNormalizedMoveModulesByPackage` │
│   3. For upgraded packages:                                      │
│      - Fetch UPGRADED bytecode (e.g., 0x75b2e9ec...)            │
│      - Load at ORIGINAL address (e.g., 0x1eabed72...)           │
│   4. LocalModuleResolver handles address remapping               │
│   5. Module linkage resolves via CompiledModuleResolver          │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Key Insight**: Sui packages are immutable, but can be upgraded. The blockchain tracks upgrades via a linkage table. For replay, we must load the **upgraded bytecode** at the **original address** so existing references resolve correctly.

### 2. Historical Object Fetching

```
┌─────────────────────────────────────────────────────────────────┐
│  Object State Timeline                                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Pool v751561008 ──► Pool v751677305 ──► Pool v755027007       │
│   (creation)         (tx time)            (current)              │
│                                                                  │
│   ✗ Using current state = WRONG tick indices, liquidity         │
│   ✓ Using tx-time state = CORRECT for faithful replay           │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

We use Sui's **gRPC Archive** (`archive.mainnet.sui.io:443`) to fetch objects at historical versions:

```rust
// Fetch Pool at transaction-time version
let pool = fetcher.fetch_object_at_version_full(pool_id, 751677305)?;
historical_objects.insert(pool_id, base64_encode(&pool.bcs_bytes));
```

**Important**: The gRPC `bcs` field returns historical state, while `contents` always returns current state regardless of version requested.

### 3. Dynamic Field Resolution

Cetus pools use a **skip_list** data structure for tick management. Each node is stored as a dynamic field:

```
┌──────────────────────────────────────────────────────────────────┐
│  Skip List Structure (tick indices)                              │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│   tick_index:  -443636 → 37680 → 69120 → 443636                 │
│                  ↓         ↓       ↓        ↓                    │
│   key (u64):     0    → 481316 → 512756 → 887272                │
│                  ↓         ↓       ↓        ↓                    │
│   child_id:   0x364f... → 0x01af... → 0xfd19... → 0x4037...     │
│                                                                  │
│   Formula: key = tick_index + TICK_BOUND (443636)               │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

**Dynamic Field ID Derivation** (the `hash_type_and_key` native function):

```rust
child_id = Blake2b256(
    0xf0 ||                      // HashingIntentScope::ChildObjectId
    parent_address ||            // 32 bytes
    key_length as u64 LE ||      // 8 bytes little-endian
    key_bytes ||                 // BCS-serialized key
    type_tag_bytes               // BCS-serialized TypeTag
)
```

**Bug Fixed**: Our original implementation used SHA256 without the scope byte or length prefix. This caused all dynamic field lookups to fail.

### 4. PTB Execution Flow

```
┌─────────────────────────────────────────────────────────────────┐
│  PTB Execution Pipeline                                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   1. Parse transaction inputs → InputValue (Pure/Object)         │
│   2. Convert commands → Command (MoveCall/SplitCoins/...)        │
│   3. For each command:                                           │
│      a. Resolve arguments (inputs, prior results)                │
│      b. Execute via VMHarness                                    │
│      c. Store results for subsequent commands                    │
│   4. Collect effects (created, mutated, deleted objects)         │
│   5. Compare with on-chain effects (optional)                    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Issues Encountered and Solutions

### Issue 1: Dynamic Field ID Mismatch

**Symptom**: `borrow_child_object` couldn't find skip_list nodes

**Root Cause**: `hash_type_and_key` was using SHA256 instead of Blake2b256, and missing the scope byte (0xf0) and length prefix.

**Solution**:

```rust
// Correct implementation
fn hash_type_and_key(parent: AccountAddress, key_type: TypeTag, key_bytes: &[u8]) -> AccountAddress {
    let mut hasher = blake2::Blake2b256::new();
    hasher.update(&[0xf0]);  // HashingIntentScope::ChildObjectId
    hasher.update(parent.as_ref());
    hasher.update(&(key_bytes.len() as u64).to_le_bytes());
    hasher.update(key_bytes);
    hasher.update(&bcs::to_bytes(&key_type).unwrap());
    AccountAddress::new(hasher.finalize().into())
}
```

### Issue 2: Historical State vs Current State

**Symptom**: Swap logic computed wrong tick indices, leading to incorrect skip_list traversal

**Root Cause**: Using cached (current) Pool state instead of tx-time historical state. The Pool's `current_tick_index` and liquidity had changed since the transaction.

**Solution**: Fetch Pool at its exact version at transaction time via gRPC archive:

```rust
let pool = fetcher.fetch_object_at_version_full(pool_id, tx_time_version)?;
```

### Issue 3: Dynamic Field Children Not Found

**Symptom**: Skip_list nodes existed but couldn't be fetched at tx-time version

**Root Cause**: Dynamic field children only exist at their **creation version**, not at every subsequent version. The nodes were created when the Pool was created, not when they were last accessed.

**Solution**: Fetch children at the Pool's **creation version** (751561008), not the transaction version:

```rust
let creation_version = 751561008;  // Pool's initialSharedVersion
let node = fetcher.fetch_object_at_version_full(&child_id, creation_version)?;
```

### Issue 4: Package Upgrade Linkage

**Symptom**: Module not found errors for CLMM functions

**Root Cause**: Loading upgraded package at its new address, but transaction references the original address.

**Solution**: Use `add_package_modules_at` to load upgraded bytecode at the original address:

```rust
// Load upgraded CLMM (0x75b2e9ec...) at original address (0x1eabed72...)
resolver.add_package_modules_at(upgraded_modules, Some(original_address))?;
```

---

## Complete Execution Steps

### Prerequisites

1. **Sui Framework**: Built-in, loaded automatically
2. **gRPC Archive Access**: `archive.mainnet.sui.io:443`
3. **Transaction Cache**: Pre-fetched transaction data (optional)

### Step-by-Step Replay

```rust
// Step 1: Initialize resolver with Sui framework
let mut resolver = LocalModuleResolver::with_sui_framework()?;

// Step 2: Load upgraded CLMM at original address
let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";
let original_clmm = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
let modules = fetcher.fetch_package_modules(upgraded_clmm)?;
resolver.add_package_modules_at(modules, Some(parse_address(original_clmm)))?;

// Step 3: Load other packages
for pkg_id in ["router", "math_libs", "skip_list", ...] {
    let modules = fetcher.fetch_package_modules(pkg_id)?;
    resolver.add_package_modules(modules)?;
}

// Step 4: Create VMHarness
let mut harness = VMHarness::new(&resolver, false)?;

// Step 5: Fetch historical Pool state
let pool_id = "0x8b7a1b6e...";
let pool_version = 751677305;  // From transaction effects
let historical_pool = fetcher.fetch_object_at_version_full(pool_id, pool_version)?;
historical_objects.insert(pool_id, base64_encode(&historical_pool.bcs_bytes));

// Step 6: Pre-load dynamic field children
let skip_list_uid = "0x6dd50d25...";  // Pool's skip_list UID
let creation_version = 751561008;     // Pool's initialSharedVersion
for key in [0, 481316, 512756, 887272] {
    let child_id = derive_dynamic_field_id_u64(skip_list_uid, key)?;
    let child = fetcher.fetch_object_at_version_full(&child_id, creation_version)?;
    preload_fields.push((skip_list_uid, child_id, child.type_tag, child.bcs_bytes));
}
harness.preload_dynamic_fields(preload_fields);

// Step 7: Set up on-demand fetcher for any missing children
harness.set_child_fetcher(Box::new(move |child_id| {
    fetcher.fetch_object_at_version_full(&child_id, creation_version).ok()
}));

// Step 8: Execute replay
let result = transaction.replay_with_objects(&mut harness, &historical_objects)?;
assert!(result.local_success);
```

---

## Required Data

### Packages (6 total)

| Package | Address | Modules | Notes |
|---------|---------|---------|-------|
| Sui Framework | 0x1, 0x2 | ~50 | Built-in |
| Cetus CLMM | 0x1eabed72... | 13 | Upgraded from 0x75b2e9ec... |
| Cetus Router | 0xeffc8ae6... | 5 | Entry point |
| Integer Math | 0x714a63a0... | 8 | Full math operations |
| Skip List | 0xbe21a06... | 4 | Data structure |
| DEX Aggregator | Various | ~10 | Additional routing |

### Input Objects (6 total)

| Object | Type | Version | Source |
|--------|------|---------|--------|
| Pool | `pool::Pool<LEIA, SUI>` | 751677305 | gRPC archive |
| GlobalConfig | `config::GlobalConfig` | 751677714 | gRPC archive |
| Partner | `partner::Partner` | 751677715 | gRPC archive |
| Clock | `clock::Clock` | 697295377 | Constructed |
| User LEIA Coin | `coin::Coin<LEIA>` | 751668780 | gRPC archive |
| User SUI Coin | `coin::Coin<SUI>` | 751660496 | gRPC archive |

### Dynamic Field Children (4 skip_list nodes)

| Key | Object ID | Version |
|-----|-----------|---------|
| 0 | 0x364f5bc3... | 751561008 |
| 481316 | 0x01aff7f7... | 751561008 |
| 512756 | 0xfd19db35... | 751561008 |
| 887272 | 0x4037bc12... | 751561008 |

---

## Verification

### Test Command

```bash
cargo test test_replay_cetus_with_grpc_archive_data -- --nocapture
```

### Expected Output

```
Step 1: Loading upgraded CLMM package...
   Loaded 13 modules
Step 2: Loading other cached packages...
   Loaded 42 modules from cache
Step 2.5: Fetching historical Pool state from gRPC archive...
   ✓ Historical Pool fetched at version 751677305 (555 bytes)
Step 3: Fetching historical dynamic field children via gRPC...
   ✓ Key 0: 168 bytes
   ✓ Key 481316: 168 bytes
   ✓ Key 512756: 168 bytes
   ✓ Key 887272: 168 bytes
   Prepared 4 fields for preloading
   Preloaded into VM
Step 4: Setting up on-demand child fetcher...
Step 5: Replaying transaction with historical Pool state...

=== RESULT ===
Success: true

✓ TRANSACTION REPLAYED SUCCESSFULLY WITH gRPC ARCHIVE DATA!
```

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Transaction Replay Architecture                       │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   ┌──────────────┐    ┌──────────────┐    ┌──────────────┐                  │
│   │ Transaction  │    │   Package    │    │   Object     │                  │
│   │   Details    │    │   Bytecode   │    │    State     │                  │
│   │              │    │              │    │              │                  │
│   │ - Commands   │    │ - CLMM       │    │ - Pool       │                  │
│   │ - Inputs     │    │ - Router     │    │ - Config     │                  │
│   │ - Types      │    │ - Math       │    │ - Coins      │                  │
│   └──────┬───────┘    └──────┬───────┘    └──────┬───────┘                  │
│          │                   │                   │                          │
│          └───────────────────┼───────────────────┘                          │
│                              │                                              │
│                              ▼                                              │
│                    ┌─────────────────────┐                                  │
│                    │  LocalModuleResolver │                                  │
│                    │                      │                                  │
│                    │  - Sui Framework    │                                  │
│                    │  - Loaded packages  │                                  │
│                    │  - Address aliases  │                                  │
│                    └──────────┬──────────┘                                  │
│                               │                                             │
│                               ▼                                             │
│                    ┌─────────────────────┐                                  │
│                    │     VMHarness       │                                  │
│                    │                     │                                  │
│                    │  - Move VM          │                                  │
│                    │  - Native functions │                                  │
│                    │  - Object storage   │                                  │
│                    └──────────┬──────────┘                                  │
│                               │                                             │
│                               ▼                                             │
│                    ┌─────────────────────┐      ┌─────────────────────┐    │
│                    │    PTBExecutor      │◄────►│  Dynamic Field      │    │
│                    │                     │      │  Runtime            │    │
│                    │  - Command dispatch │      │                     │    │
│                    │  - Result tracking  │      │  - hash_type_and_key│    │
│                    │  - Effects collect  │      │  - borrow_child     │    │
│                    └──────────┬──────────┘      │  - On-demand fetch  │    │
│                               │                 └──────────┬──────────┘    │
│                               │                            │               │
│                               ▼                            ▼               │
│                    ┌─────────────────────────────────────────────────┐     │
│                    │              gRPC Archive                        │     │
│                    │         archive.mainnet.sui.io:443               │     │
│                    │                                                  │     │
│                    │   GetObject(id, version) → Historical BCS       │     │
│                    └──────────────────────────────────────────────────┘     │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Key Learnings

1. **Package Upgrades**: Load upgraded bytecode at original addresses for correct linkage
2. **Historical State**: Always fetch objects at tx-time versions, not current state
3. **Dynamic Fields**: Children exist at creation version, not every subsequent version
4. **Blake2b256**: Use exact Sui formula with scope byte 0xf0 and length prefix
5. **gRPC vs JSON-RPC**: gRPC `bcs` field returns historical state; JSON-RPC does not

---

## References

- **Test File**: `tests/execute_cetus_swap.rs::test_replay_cetus_with_grpc_archive_data`
- **Sui Dynamic Fields**: `sui-types/src/dynamic_field.rs`
- **gRPC Archive Proto**: `sui-storage/proto/rpc.proto`
- **Cetus CLMM**: https://github.com/CetusProtocol/cetus-clmm-sui
