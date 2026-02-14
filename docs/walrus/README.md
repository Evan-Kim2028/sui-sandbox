# Walrus Replay (Sui Sandbox)

Walrus is the **primary data source** for transaction replay. It provides free, unauthenticated access to all Sui checkpoint data via decentralized storage. No API keys, no configuration.

## Quick Start

```bash
# Replay a real transaction (zero setup)
sui-sandbox replay At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2 \
  --source walrus --checkpoint 239615926 --compare
```

### Scan Latest Checkpoints

```bash
# Scan the latest 5 checkpoints — auto-discovers tip, prints summary
sui-sandbox replay '*' --source walrus --latest 5 --compare

# Scan a larger latest window
sui-sandbox replay '*' --source walrus --latest 10 --compare
```

### Batch Replay

```bash
# Replay ALL transactions in a checkpoint range
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926

# Specific checkpoints
sui-sandbox replay '*' --source walrus --checkpoint 239615920,239615923,239615926

# Multiple specific digests
sui-sandbox replay "digest1,digest2" --source walrus --checkpoint 239615926
```

### Export and Offline Replay

```bash
# Export state to JSON (for offline replay, CI/CD, or custom pipelines)
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json

# Replay from JSON (no network needed)
sui-sandbox replay <DIGEST> --state-json state.json
```

### Cache Warmup (Optional)

Pre-ingest package index entries from a checkpoint range:

```bash
sui-sandbox fetch checkpoints 238627315 238627325
```

## What Walrus Provides

Each checkpoint contains:
- Transaction data (PTB commands, inputs, sender, gas)
- Input objects at their exact historical versions (BCS-encoded)
- Output objects (post-execution state)
- Transaction effects (status, created/mutated/deleted)
- Package bytecode with linkage tables

The replay pipeline resolves transitive package dependencies via GraphQL fallback when packages are not in the fetched checkpoint(s).

## Documentation Map

- `INTEGRATION.md` — architecture, data sources, and how replay + cache work together
- `BENCHMARKS.md` — benchmark methodology and results
