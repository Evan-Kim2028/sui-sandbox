# Walrus Replay Benchmarks

This document summarizes the checkpoint replay benchmarks that validate
Walrus + gRPC fallback as a viable data source for local PTB replay.

## Summary (representative run)

```
Checkpoints:      238627315 → 238627324 (10 total)
Transactions:     69
PTBs:             35 (50.7%)
Objects:          227 (100% deserialized from Walrus)
Packages:         48 (100% fetched via gRPC archive)
Ready for exec:   35/35 PTBs
```

## Throughput

```
Total Time:       ~7.6s (warm cache)
Throughput:       ~9 tx/sec
```

## What was validated

- Walrus checkpoints contain **complete PTB structure**
- Input object BCS is available for deserialization
- gRPC archive fills **package bytecode** gaps
- Cache significantly reduces repeated package/object fetches

## How to run

```bash
# Basic replay
cargo run --release --example walrus_checkpoint_replay

# Cache + replay
cargo run --release --example walrus_cache_build -- \
  --cache-dir ./walrus-cache \
  --blobs 10

cargo run --release --example walrus_checkpoint_replay -- \
  --start 238627315 \
  --end 238627325 \
  --cache-dir ./walrus-cache
```

## Notes

These benchmarks are intended to validate completeness and replay readiness,
not to replace production execution benchmarks.
