# Walrus Integration

This document explains how Walrus is used as a historical checkpoint source for
sui-sandbox replay, and how the cache + gRPC fallback fill remaining gaps.

## Architecture

```
Walrus (checkpoint JSON + BCS)
  └─> WalrusClient (sui-transport)
       └─> ReplayEngine / PTB parsing (examples)
            ├─> Object + effect data from checkpoint
            ├─> Optional disk cache (historical objects/packages)
            └─> gRPC archive fallback for package bytecode
```

## Data availability

**From Walrus (checkpoint):**
- Transaction commands (PTB structure)
- Input object IDs + versions
- Input object state (BCS)
- Output object state
- Effects + gas + status

**From gRPC/archive (fallback):**
- Package bytecode (immutable, cacheable)
- Historical objects that predate the checkpoint (when needed)

## Replay flow (high level)

1. Fetch checkpoint data from Walrus.
2. Parse PTB inputs/commands and deserialize object BCS from checkpoint.
3. Resolve required packages:
   - disk cache → gRPC archive → in-memory cache
4. Execute or analyze PTBs locally.

## Cache usage

The disk cache is a simple filesystem layout used by the replay examples:

```
cache_root/
├── objects/<shard>/<object_id>/<version>.bcs
├── packages/<shard>/<package_id>.json
└── progress/ (resumable build metadata)
```

Build the cache:

```bash
# Warm the local Walrus store (checkpoint data + object index)
cargo run --bin sui-sandbox --features walrus -- tools walrus-warmup --count 50

# Replay using Walrus as the primary source
cargo run --bin sui-sandbox --features walrus -- replay <DIGEST> --source walrus
```

## Trade-offs

- Walrus is free and unauthenticated, but it only stores checkpoint data.
- Package bytecode still requires gRPC/archive fetch (highly cacheable).
- Sequential replay benefits the most from cache locality.

For benchmark details and numbers, see `BENCHMARKS.md`.
