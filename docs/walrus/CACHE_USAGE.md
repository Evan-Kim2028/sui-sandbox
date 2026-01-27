# Walrus Historical Cache Usage Guide

## Overview

The Walrus historical cache provides a filesystem-backed L2 cache for objects and packages used during checkpoint replay. This significantly reduces gRPC calls by caching historical object versions and packages locally.

## Building the Cache

Build a cache from Walrus checkpoint blobs:

```bash
cargo run --release --example walrus_cache_build -- \
  --cache-dir ./walrus-cache \
  --blobs 10
```

### Options

- `--cache-dir <path>`: Directory where the cache will be stored (required)
- `--blobs N`: Number of blobs to ingest (default: 10)
- `--start-checkpoint X --end-checkpoint Y`: Override blob selection with a specific checkpoint range
- `--blob-id <id>`: Process a specific blob (for debugging)
- `--max-blob-chunk-bytes <bytes>`: Max bytes per merged blob download (default: 128 MiB)
- `--workers N`: Number of parallel blob workers (default: 4)

### Example: Build cache for 10 most recent blobs

```bash
cargo run --release --example walrus_cache_build -- \
  --cache-dir ./walrus-cache \
  --blobs 10
```

### Example: Build cache for a specific checkpoint range

```bash
cargo run --release --example walrus_cache_build -- \
  --cache-dir ./walrus-cache \
  --start-checkpoint 238627315 \
  --end-checkpoint 238627325
```

## Using the Cache During Replay

Enable the cache during replay by passing `--cache-dir`:

```bash
cargo run --release --example walrus_checkpoint_replay -- \
  --start 238627315 \
  --end 238627325 \
  --cache-dir ./walrus-cache
```

The replay engine will:
1. Check Walrus JSON for objects (always first)
2. Check in-memory cache
3. **Check disk cache** (L2)
4. Fall back to gRPC if not found

## Cache Structure

The cache uses a sharded filesystem layout:

```
cache_root/
├── objects/
│   └── <aa>/
│       └── <bb>/
│           └── <objectid_hex>/
│               ├── <version>.bcs          # Raw BCS bytes
│               └── <version>.meta.json    # Metadata (type_tag, owner, checkpoint)
├── packages/
│   └── <aa>/
│       └── <packageid_hex>.json           # Package modules + metadata
└── progress/
    ├── state.json                          # Progress snapshot
    └── events.jsonl                        # Append-only event log
```

## Cache Behavior

### Objects

- **Key**: `(object_id, version)`
- Objects are extracted from checkpoint `input_objects` and `output_objects` arrays
- Only Move objects with BCS contents are cached
- Duplicate `(id, version)` entries are skipped (idempotent)

### Packages

- **Key**: `package_id` (storage address)
- Packages are cached when fetched from gRPC during replay (miss-fill)
- Latest version wins (versioned packages may be added later)
- Includes linkage/alias metadata for dependency resolution

### Progress Tracking

- Tracks ingested blobs and checkpoints for resumable builds
- Can interrupt and resume cache building without data loss
- Progress state is saved atomically after each blob completes

## Metrics

When replaying with `--cache-dir`, metrics are printed at the end:

```
Cache Metrics Report
==================================================
Object Lookups:
  Walrus JSON:     1234
  Memory Cache:    567
  Disk Cache:       890
  gRPC (miss):      123
  Cache Hit Rate:  92.2%
  Disk Hit Rate:   56.4%

Package Lookups:
  Disk Cache:       45
  gRPC (miss):       12
  Hit Rate:        78.9%

Dynamic Fields:
  Disk Cache:       67
  gRPC (miss):       23
```

## Performance Tips

1. **Build cache for the checkpoint range you'll replay**: The cache is most effective when it covers the same checkpoints you're replaying.

2. **Use batched blob downloads**: The cache builder uses `WalrusClient::get_checkpoints_batched` to minimize network requests.

3. **Resume interrupted builds**: If cache building is interrupted, rerun the same command to resume from the last checkpoint.

4. **Cache size**: A cache for ~10 blobs (~140k checkpoints) typically uses several GB of disk space, depending on object density.

## Troubleshooting

### Cache not being used

- Verify `--cache-dir` points to a valid cache directory
- Check that the cache was built for the checkpoint range you're replaying
- Look for "Disk cache enabled" message in replay output

### Cache build fails

- Check network connectivity to Walrus endpoints
- Verify sufficient disk space
- Check `progress/state.json` to see which blobs completed

### Low cache hit rate

- Ensure cache covers the checkpoint range being replayed
- Check that objects exist in the cache: `ls cache_root/objects/<aa>/<bb>/<objectid>/`
- Verify object versions match what's needed (check `progress/events.jsonl`)

## Limitations

- Cache is read-only during replay (objects are only written during cache build or when ingesting from Walrus)
- Package cache is populated on-demand during replay (gRPC miss-fill)
- No automatic cache invalidation (delete cache directory to rebuild)
- Cache does not include dynamic field children unless they appear as output objects in checkpoints

## Future Improvements

- Add checkpoint->blob offset map to avoid per-checkpoint metadata calls
- Add "version window" mode to ingest objects from N checkpoints before target range
- Add optional compression (zstd) for `.bcs` files
- Add cache statistics CLI command
