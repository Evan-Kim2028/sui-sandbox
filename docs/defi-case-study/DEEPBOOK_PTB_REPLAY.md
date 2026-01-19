# DeFi PTB Case Study: DeepBook Transaction Replay

## Overview

This case study demonstrates successful local replay of DeepBook transactions using the Move VM sandbox. DeepBook is Sui's native central limit order book (CLOB) DEX, and its transactions are particularly challenging to replay because they:

1. Use extensive dynamic fields (tables, balance managers, order books)
2. Have "hot" objects that change frequently (order book pools)
3. Require precise historical state at the exact transaction checkpoint
4. Involve complex child object lifecycle (create, borrow, remove, re-add)

## Transactions Replayed

### Successfully Replayed

| Transaction | Type | Commands | Status |
|-------------|------|----------|--------|
| `7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY` | `cancel_order` | 2 | ✓ SUCCESS |
| `3AKpMt66kXcPutKxkQ4D3NuAu4MJ1YGEvTNkWoAzyVVE` | `place_limit_order` | 2 | ✓ SUCCESS |
| `6fZMHYnpJoShz6ZXuWW14dCTgwv9XpgZ4jbZh6HBHufU` | `place_limit_order` | 2 | ✓ SUCCESS |

### Example Output

```text
Testing transaction: 7aQBpHjvgNguGB4W (cancel_order)
  Checkpoint: 235248811
  Commands: 2
  Input versions: 10
  Unchanged runtime: 2

Step 1: Fetch transaction data... ✓
Step 2: Fetch packages... ✓ (2 packages)
Step 3: Set up resolver with aliasing... ✓
Step 4: Fetch input objects at INPUT versions... ✓
Step 5: Create harness with child fetcher... ✓

✓ REPLAY SUCCESS!
```

```text
Testing transaction: 6fZMHYnpJoShz6ZX (place_limit_order)
  Checkpoint: 235248811
  Commands: 2
  Input versions: 6
  Unchanged runtime: 1

Step 1: Fetch transaction data... ✓
Step 2: Fetch packages... ✓ (2 packages)
Step 3: Set up resolver with aliasing... ✓
Step 4: Fetch input objects at INPUT versions... ✓
Step 5: Create harness with child fetcher... ✓

✓ REPLAY SUCCESS!
```

### Failed Replay

| Transaction | Type | Commands | Failure Reason |
|-------------|------|----------|----------------|
| `DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs` | `flashloan_swap` | 7 | Third-party protocol version check |

The flashloan swap transaction failed because it calls external protocols (Bluefin) that have version verification in their contracts. The error occurred at `config::verify_version`, which is a protocol-level check that the sandbox cannot satisfy without mocking the protocol's version registry.

---

## How Local Transaction Replay Works

### Architecture

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                           PHASE 1: STATE COLLECTION                          │
├─────────────────────────────────────────────────────────────────────────────┤
│  Surflux gRPC API                                                            │
│  └─► fetch_transaction_sync(digest)                                          │
│      └─► FetchedTransaction {                                                │
│            commands, inputs, checkpoint, input_object_versions,              │
│            unchanged_runtime_objects                                         │
│          }                                                                   │
│                                                                              │
│  Package Fetching                                                            │
│  └─► fetch_transaction_packages() + type_arg packages + transitive deps      │
│      └─► LocalModuleResolver with address aliasing                           │
│                                                                              │
│  Object Fetching                                                             │
│  └─► fetch_object_at_version_full() for each input_object_versions          │
│      └─► Binary search fallback: get_object_at_checkpoint_binary()           │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                      PHASE 2: EXECUTION WITH ON-DEMAND FETCHING             │
├─────────────────────────────────────────────────────────────────────────────┤
│  VMHarness::with_config(resolver, SimulationConfig)                          │
│  └─► set_child_fetcher(callback)  // For dynamic field lazy loading          │
│                                                                              │
│  PTBExecutor::new(&mut harness)                                              │
│  └─► add_input() for each object                                             │
│  └─► execute_commands(&commands)                                             │
│      ├─► validate_ptb()           // Check causality, bounds                 │
│      ├─► FOR EACH Command:                                                   │
│      │   ├─► resolve_args()       // Convert Argument → BCS bytes            │
│      │   ├─► vm.execute_function_full()                                      │
│      │   │   └─► Move VM execution with native function calls                │
│      │   ├─► apply_mutable_ref_outputs()  // Propagate mutations             │
│      │   └─► store result for next commands                                  │
│      └─► compute_effects()        // Created, mutated, deleted objects       │
│                                                                              │
│  ReplayResult { local_success, comparison with on-chain effects }            │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | File | Purpose |
|-----------|------|---------|
| **TransactionFetcher** | `src/benchmark/tx_replay.rs` | Fetches transaction data from Surflux gRPC API |
| **SimulationConfig** | `src/benchmark/vm.rs` | Controls VM behavior (crypto mocking, timestamps, gas) |
| **VMHarness** | `src/benchmark/vm.rs` | Wraps Move VM, executes functions, handles TxContext |
| **PTBExecutor** | `src/benchmark/ptb.rs` | Orchestrates multi-command PTB execution |
| **ObjectRuntimeState** | `src/benchmark/object_runtime.rs` | Tracks dynamic field state during execution |
| **Dynamic Field Natives** | `src/benchmark/natives.rs` | Native functions for table/dynamic field operations |

---

## Transaction Verification

After local execution completes, we compare the results against on-chain effects to verify correctness.

### ReplayResult

The replay returns a `ReplayResult` struct containing:

```rust
pub struct ReplayResult {
    pub digest: TransactionDigest,      // Original transaction
    pub local_success: bool,            // Did local execution succeed?
    pub local_error: Option<String>,    // Error message if failed
    pub comparison: Option<EffectsComparison>,  // Comparison with on-chain
    pub commands_executed: usize,       // How many commands ran
    pub commands_failed: usize,         // How many commands failed
}
```

### EffectsComparison

The `EffectsComparison` struct compares local execution effects against on-chain transaction effects:

```rust
pub struct EffectsComparison {
    pub status_match: bool,           // Both success or both failure
    pub created_count_match: bool,    // Same number of created objects
    pub mutated_count_match: bool,    // Same number of mutated objects
    pub deleted_count_match: bool,    // Same number of deleted objects
    pub match_score: f64,             // Overall match (0.0 - 1.0)
    pub notes: Vec<String>,           // Details about any differences
}
```

**Comparison Criteria:**

| Criterion | How It's Compared |
|-----------|-------------------|
| **Status** | Both must succeed, or both must fail |
| **Created Objects** | Exact count match required |
| **Mutated Objects** | Allow ±1-2 difference (gas object mutations not tracked locally) |
| **Deleted Objects** | Exact count match required |

**Match Score Calculation:**

- Each criterion is worth 0.25 points
- Perfect replay = 1.0 (100%)
- The mutated count allows slack because on-chain execution always mutates the gas coin, which local simulation doesn't track

**Example Comparison Output:**

```text
EffectsComparison {
    status_match: true,
    created_count_match: true,
    mutated_count_match: true,   // on-chain: 3, local: 2 (diff=1, OK)
    deleted_count_match: true,
    match_score: 1.0,
    notes: []
}
```

If there are mismatches, `notes` will contain explanations:

```text
notes: [
    "Created count mismatch: on-chain=2, local=1",
    "Status mismatch: on-chain=Success, local=failure"
]
```

---

## Key Technical Challenges Solved

### 1. Hot Object Versioning

DeepBook pools change frequently. We need the exact version at transaction execution time.

**Solution:** Binary search at checkpoint level to find object state:

```rust
client.get_object_at_checkpoint_binary(&object_id, checkpoint, max_iterations).await
```

### 2. Dynamic Field Lazy Loading

Child objects (table entries, balance manager fields) aren't known until execution time.

**Solution:** On-demand fetching callback:

```rust
harness.set_child_fetcher(Box::new(move |child_id| {
    // 1. Check cache
    // 2. Check known versions
    // 3. Binary search at checkpoint
}));
```

### 3. Child Object Lifecycle

After `table::remove()` + `table::add()` with the same key, existence checks must see the new state, not stale archive data.

**Solution:** Track deleted children to prevent re-fetching:

```rust
pub deleted_children: HashSet<(AccountAddress, AccountAddress)>

// In has_child_object native:
if state.is_child_deleted(parent, child_id) {
    return false;  // Don't re-fetch from archive
}
```

### 4. Package Upgrade Aliasing

On-chain packages use upgraded addresses, but bytecode contains original self-addresses.

**Solution:** Build alias map from bytecode:

```rust
let aliases = build_address_aliases(&packages);
// 0xcaf6ba059d539a97... → 0x2c8d603bc51326b8c...
```

### 5. TxContext Injection

Entry functions require TxContext but it's not in the PTB argument list.

**Solution:** Auto-detect and retry:

```rust
if err_msg.contains("argument length mismatch") {
    let tx_context_bytes = synthesize_tx_context()?;
    resolved_args.push(tx_context_bytes);
    // Retry execution
}
```

---

## Error Resolution Journey

### Error 1: `balance_manager::withdraw_with_proof` abort (sub_status 3)

- **Root cause**: `has_child_object_with_ty` wasn't triggering on-demand fetching
- **Fix**: Added on-demand fetching to existence check natives

### Error 2: `remove_child_object` abort (E_FIELD_DOES_NOT_EXIST)

- **Root cause**: `remove_child_object` native wasn't checking shared state or doing on-demand fetch
- **Fix**: Rewrote to check shared state, on-demand fetch, then deserialize and return

### Error 3: `dynamic_field::add` abort (E_FIELD_ALREADY_EXISTS)

- **Root cause**: `has_child_object` was adding fetched children to shared state, causing stale lookups after remove+add
- **Fix**: Made existence checks NOT add to shared state, added `deleted_children` tracking

---

## Running the Tests

```bash
# Set up environment
export SURFLUX_API_KEY=<your-api-key>

# Run DeepBook replay test
cargo test --test execute_deepbook_swap test_deepbook_two_phase_replay -- --nocapture --ignored

# Run package loading test (no API key needed)
cargo test test_deepbook_package_loading -- --nocapture
```

---

## Future Work

### Additional Transactions to Test

| Digest | Type | Complexity | Notes |
|--------|------|------------|-------|
| `D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva` | `flashloan_arb` | 16 commands | Failed on-chain, good for failure replay testing |

### Planned Enhancements

1. **Cross-Protocol Transaction Support** - Handle version checks and config validations from third-party protocols (Bluefin, FlowX, etc.) by mocking their version registries

2. **Batch Replay Testing** - Run replay on a larger set of DeepBook transactions to measure success rate

3. **Effects Content Verification** - Currently we compare object counts; future work could verify actual object contents match by comparing BCS bytes

4. **Performance Benchmarking** - Measure local replay latency vs on-chain execution time

5. **Other DeFi Protocols** - Extend to Cetus, Turbos, and other Sui DeFi protocols
