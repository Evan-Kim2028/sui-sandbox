# Walrus Replay (Sui Sandbox)

This directory contains the **Walrus-specific** docs for the checkpoint replay tooling.
It is intentionally concise and organized for both users and developers.

## Quick start

Warm the Walrus local store and replay with Walrus as the primary source:

```bash
cargo run --bin sui-sandbox --features walrus -- tools walrus-warmup --count 50

cargo run --bin sui-sandbox --features walrus -- replay <DIGEST> --source walrus --compare
```

To ingest packages from a checkpoint range into the local index:

```bash
cargo run --bin sui-sandbox --features walrus -- fetch checkpoints 238627315 238627325
```

## Documentation map

- `INTEGRATION.md` — architecture, data sources, and how replay + cache work together
- `BENCHMARKS.md` — benchmark methodology and results

## What Walrus provides

Walrus supplies checkpoint data (transactions, effects, and object state) without auth.
The replay pipeline augments that data with package bytecode fetched via gRPC/archive.

If you’re looking for implementation details, start with `INTEGRATION.md`.
