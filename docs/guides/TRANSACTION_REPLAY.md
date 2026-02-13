# Transaction Replay Guide

Replay Sui mainnet transactions locally to verify behavior and debug issues.

## Overview

Transaction replay:

1. Fetches a checkpoint from Walrus (or transaction from gRPC/JSON)
2. Extracts all objects at their historical versions (before the transaction modified them)
3. Resolves packages with transitive dependencies
4. Executes the transaction in the local Move VM
5. Compares local results with on-chain effects

## Quick Start (Walrus — Zero Setup)

The fastest way to replay a transaction. No API keys, no configuration:

```bash
# Replay a known-good Cetus swap
sui-sandbox replay At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2 \
  --source walrus --checkpoint 239615926 --compare

# Or use the example script
./examples/replay.sh
```

Walrus provides free, unauthenticated access to all Sui checkpoint data via decentralized storage. The checkpoint contains the transaction, all input/output objects at their exact versions, and sibling package data.

### Scan Latest Checkpoints

Auto-discover the tip checkpoint and replay the most recent transactions:

```bash
# Scan the latest 5 checkpoints (prints success/fail summary)
sui-sandbox replay '*' --source walrus --latest 5 --compare
```

### Batch Replay

Replay all transactions in a checkpoint range:

```bash
# Replay all transactions in checkpoints 239615920 through 239615926
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926

# Replay specific checkpoints
sui-sandbox replay '*' --source walrus --checkpoint 239615920,239615923,239615926

# Multi-digest replay
sui-sandbox replay "digest1,digest2,digest3" --source walrus --checkpoint 239615926
```

### Export and Offline Replay

Export replay state from any source, then replay offline:

```bash
# Export state to JSON
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json

# Replay from JSON (completely offline, no network needed)
sui-sandbox replay <DIGEST> --state-json state.json
```

The JSON format is a generic state container — you can produce it from any data source (custom indexers, Snowflake, etc.) and replay without network access.

### gRPC Replay

If you have a gRPC endpoint configured:

```bash
sui-sandbox replay <DIGEST> --source grpc --compare
```

Uses `SUI_GRPC_ENDPOINT` in `.env` or environment when set.
If the gRPC endpoint is not configured, the replay service falls back to `--rpc-url`.

## Data Sources

| Source | Auth | What You Need | Best For |
|--------|------|--------------|----------|
| **Walrus** | None | Digest + checkpoint number | Most replay scenarios |
| **JSON** | None | Exported state file | Offline replay, CI/CD, custom data |
| **gRPC** | API key | `.env` configuration | Latest state, streaming |

## Using HistoricalStateProvider (Rust API)

For programmatic replay in Rust, use `sui_state_fetcher::HistoricalStateProvider`:

```rust
use sui_state_fetcher::HistoricalStateProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = HistoricalStateProvider::mainnet().await?;
    let state = provider.fetch_replay_state("8JTTa...").await?;

    // state.transaction - PTB commands and inputs
    // state.objects - objects at their exact historical versions
    // state.packages - packages with linkage resolved

    Ok(())
}
```

The `HistoricalStateProvider`:

- Automatically fetches objects at their **input versions** from `unchanged_loaded_runtime_objects`
- Uses a **versioned cache** keyed by `(object_id, version)`
- Resolves package **linkage tables** for upgraded packages
- Provides an **on-demand fetcher** callback for dynamic field children discovered at runtime

## Replay Diagnostics

If replay fails due to missing data:

```bash
sui-sandbox analyze replay <DIGEST>
```

This reports missing inputs/packages and suggests next steps.

For a compact diagnose/fix loop, see [Replay Triage Workflow](./REPLAY_TRIAGE.md).

## Rust Examples

For learning the replay internals, see the Rust examples (require gRPC):

```bash
cargo run --example cetus_swap           # Cetus AMM swap
cargo run --example deepbook_orders      # BigVector handling
cargo run --example deepbook_replay      # Flash loan replay
```

## How It Works

### Step 1: Fetch Transaction

```rust
use sui_transport::grpc::GrpcClient;

// Connect to mainnet (reads SUI_GRPC_ENDPOINT env var or uses default)
let grpc = rt.block_on(GrpcClient::mainnet())?;
// Or with custom endpoint and API key:
// let grpc = rt.block_on(GrpcClient::with_api_key(&endpoint, Some(api_key)))?;

let tx = rt.block_on(grpc.get_transaction(&digest))?;
```

### Step 2: Collect Required Objects

The key insight is that we need objects at their **input versions** (before the transaction modified them), not their current versions.

```rust
// Objects explicitly listed as inputs
let input_objects: Vec<ObjectID> = tx.inputs.iter()
    .filter_map(|input| input.object_id())
    .collect();

// Objects read but not modified (from transaction effects)
let runtime_objects: Vec<(ObjectID, u64)> = tx.unchanged_loaded_runtime_objects.clone();
```

### Step 3: Fetch Objects at Historical Versions

```rust
// Fetch objects at specific versions via gRPC
for (object_id, version) in &runtime_objects {
    let obj = rt.block_on(grpc.get_object_at_version(object_id, *version))?;
    objects.insert(*object_id, obj);
}
```

### Step 4: Fetch Packages with Dependencies

Packages must be fetched transitively, following linkage tables for upgrades:

```rust
use sui_transport::graphql::GraphQLClient;

let graphql = GraphQLClient::mainnet();

// Fetch package and all its dependencies
let packages = rt.block_on(graphql.fetch_package_with_dependencies(&package_id))?;
```

### Step 5: Build the VM Environment

```rust
use sui_sandbox_core::{VMHarness, ModuleBytecodeResolver};

// Create resolver with address aliasing for package upgrades
let mut resolver = ModuleBytecodeResolver::new();
for pkg in packages {
    resolver.add_package_with_aliases(&pkg)?;
}

// Create VM harness with transaction timestamp
let harness = VMHarness::new(resolver)
    .with_timestamp(tx.timestamp_ms);
```

### Step 6: Execute and Compare

```rust
// Convert transaction to PTB commands
let (commands, inputs) = tx.to_ptb_commands()?;

// Execute
let result = harness.execute_ptb(&commands, &inputs)?;

// Compare with expected effects
let matches = result.status == tx.expected_status;
```

## Handling Dynamic Fields

Complex DeFi transactions access dynamic fields (tables, bags, etc.) that aren't explicitly listed as inputs. The system handles this through:

1. **Ground truth prefetch**: Uses `unchanged_loaded_runtime_objects` from transaction effects
2. **Predictive prefetch**: Analyzes bytecode to predict additional accesses
3. **On-demand fetch**: Fallback during execution for any missed objects

See [Prefetching Architecture](../architecture/PREFETCHING.md) for details.

## Handling BigVector

Some protocols (like DeepBook) use **BigVector** internally - Sui's scalable vector
implementation that stores data in dynamic field "slices". These slices present a
unique challenge:

**The Problem**: BigVector slices that are only READ (not modified) during execution
may not appear in `unchanged_loaded_runtime_objects`. This causes standard replay to fail.

**The Solution**: Use prefetching with version validation:

```rust
use sui_prefetch::{prefetch_dynamic_fields, prefetch_dynamic_fields_at_checkpoint};
use common::create_enhanced_child_fetcher_with_cache;

// 1. Prefetch dynamic fields (checkpoint snapshot when available)
let prefetched = if let Some(cp) = state.checkpoint {
    prefetch_dynamic_fields_at_checkpoint(
        &graphql, &grpc, &rt, &historical_versions,
        3,   // depth: recurse into children
        200, // max fields per object
        cp
    )
} else {
    prefetch_dynamic_fields(
        &graphql, &grpc, &rt, &historical_versions,
        3,   // depth: recurse into children
        200  // max fields per object
    )
};

// 2. Create child fetcher with version validation
// Objects not in effects are validated: if version <= max_lamport_version, safe to use
let child_fetcher = create_enhanced_child_fetcher_with_cache(
    grpc,
    graphql,
    historical_versions.clone(),
    prefetched.clone(),
    Some(patcher),
    state.checkpoint,
    Some(discovery_cache),
);
harness.set_child_fetcher(child_fetcher);
```

The **max lamport version** is the maximum version among all objects in the transaction
effects. If an object's current version is <= this value, it hasn't been modified since
the transaction time and is safe to use for replay.

See `examples/deepbook_orders.rs` for a complete example of BigVector handling.

## Example Output

```
╔══════════════════════════════════════════════════════════════════════╗
║      DeepBook Flash Loan Replay - Pure gRPC (No Cache)               ║
╚══════════════════════════════════════════════════════════════════════╝

Step 1: Connecting to gRPC...
   ✓ Connected to gRPC

Step 2: Fetching transaction via gRPC...
   Digest: DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs
   Commands: 17
   Status: Success

...

╔══════════════════════════════════════════════════════════════════════╗
║                         VALIDATION SUMMARY                           ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ Flash Loan Swap           | local: SUCCESS | expected: SUCCESS     ║
║ ✓ Flash Loan Arb            | local: FAILURE | expected: FAILURE     ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ ALL TRANSACTIONS MATCH EXPECTED OUTCOMES                           ║
╚══════════════════════════════════════════════════════════════════════╝
```

## Common Issues

### Missing Objects

**Symptom**: `MissingObject { id: ... }` error

**Cause**: An object accessed during execution wasn't prefetched

**Solution**: The on-demand fetcher should handle this automatically. If it persists, check that:

- Your API key is valid
- The object exists at the historical version
- Network connectivity is working

### Version Mismatch

**Symptom**: Transaction fails with different abort code than expected

**Cause**: Objects fetched at wrong version

**Solution**: Ensure you're using `unchanged_loaded_runtime_objects` for version information, not fetching current versions.

### Transfer Ownership Errors

**Symptom**: `cannot transfer object: sender does not own it`

**Cause**: Input objects registered with wrong ownership type

Objects must be registered with their correct ownership:

- `GrpcInput::Object` → owned by sender (`Owner::AddressOwner`)
- `GrpcInput::SharedObject` → shared (`Owner::Shared`)
- `GrpcInput::Receiving` → owned by sender

**Solution**: Track ownership from input types when registering objects:

```rust
for input in &grpc_tx.inputs {
    match input {
        GrpcInput::Object { object_id, .. } => {
            ownership.insert(object_id, Owner::AddressOwner(sender));
        }
        GrpcInput::SharedObject { object_id, initial_version, .. } => {
            ownership.insert(object_id, Owner::Shared { initial_shared_version });
        }
        // ...
    }
}
```

### Package Upgrade Issues

**Symptom**: `LinkageNotFound`, module resolution errors, or `dynamic_field` abort (sub_status: 1)

**Cause**: Package was upgraded and address aliasing not configured correctly

Sui package upgrades create a key architectural complexity:

- **`original_id`**: The address used in bytecode (stable across upgrades, referenced in type tags)
- **`storage_id`**: Where the upgraded bytecode is actually stored on-chain

You need **both** directions of mapping:

- **Forward** (`storage_id → original_id`): For module resolution - load bytecode from storage, execute at original address
- **Reverse** (`original_id → storage_id`): For dynamic field hashing - child IDs are computed using the type's address

**Why reverse matters**: Dynamic field child IDs are `hash(parent, type_tag, key)`. If bytecode uses `original_id` in the type but children were created after an upgrade (using `storage_id`), the hash won't match and lookup fails.

**Solution**:

```rust
// Build aliases from linkage tables
let address_aliases = build_comprehensive_address_aliases(&cached, &linkage_upgrades);

// Pass to harness with version hints (picks highest-versioned storage address)
harness.set_address_aliases_with_versions(address_aliases, package_versions);
```

The replay pipeline handles these aliases automatically; see `examples/deepbook_replay.rs` for a
complete end-to-end example.

## Limitations

- **Historical data availability**: Very old transactions may have objects that are no longer available
- **Gas metering**: Local gas is approximate; use `sui_dryRunTransactionBlock` for exact gas
- **Randomness**: Local execution uses deterministic randomness, not VRF

## See Also

- **[Examples](../../examples/README.md)** - Self-contained replay examples to learn from
- [Prefetching Architecture](../architecture/PREFETCHING.md) - How data fetching works
- [Data Fetching Guide](DATA_FETCHING.md) - GraphQL and gRPC client details
- [DeFi Case Studies](../defi-case-study/README.md) - Protocol-specific replay examples
- [Limitations](../reference/LIMITATIONS.md) - Known differences from mainnet
