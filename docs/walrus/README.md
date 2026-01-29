# Walrus Replay (Sui Sandbox)

This directory contains the **Walrus-specific** docs for the checkpoint replay tooling.
It is intentionally concise and organized for both users and developers.

## Quick start

Run the replay example against recent checkpoints:

```bash
cargo run --release --example walrus_checkpoint_replay
```

Build a local historical cache and then replay with it:

```bash
cargo run --release --example walrus_cache_build -- \
  --cache-dir ./walrus-cache \
  --blobs 10

cargo run --release --example walrus_checkpoint_replay -- \
  --start 238627315 \
  --end 238627325 \
  --cache-dir ./walrus-cache
```

## Documentation map

- `INTEGRATION.md` — architecture, data sources, and how replay + cache work together
- `BENCHMARKS.md` — benchmark methodology and results

## What Walrus provides

Walrus supplies checkpoint data (transactions, effects, and object state) without auth.
The replay pipeline augments that data with package bytecode fetched via gRPC/archive.

If you’re looking for implementation details, start with `INTEGRATION.md`.
