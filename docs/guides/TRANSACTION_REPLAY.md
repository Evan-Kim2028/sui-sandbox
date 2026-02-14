# Transaction Replay Guide

Replay historical Sui transactions locally for debugging, verification, and offline reproduction.

## Replay Pipeline (High Level)

1. Fetch transaction/checkpoint data from Walrus, gRPC, or JSON.
2. Build historical object/package state at input versions.
3. Execute transaction commands in the local Move VM.
4. Optionally compare local effects with on-chain effects.

## Quick Start (Walrus, No Auth)

```bash
# Replay one known transaction and compare effects
sui-sandbox replay At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2 \
  --source walrus --checkpoint 239615926 --compare
```

Walrus is the default recommended path for most replay work.

## Common Replay Commands

### Scan Latest Checkpoints

```bash
sui-sandbox replay '*' --source walrus --latest 5 --compare
```

### Replay a Checkpoint Range

```bash
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926
```

### Replay Specific Checkpoints

```bash
sui-sandbox replay '*' --source walrus --checkpoint 239615920,239615923,239615926
```

### Replay Multiple Digests in One Checkpoint

```bash
sui-sandbox replay "digest1,digest2,digest3" --source walrus --checkpoint 239615926
```

### Export and Reproduce Offline

```bash
# Export from any source
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json

# Replay with no network access
sui-sandbox replay <DIGEST> --state-json state.json
```

## Data Source Choice

| Source | Auth | Best for |
|--------|------|----------|
| Walrus | None | Most replay scenarios, zero setup |
| JSON (`--state-json`) | None | Offline reproduction, CI fixtures, custom pipelines |
| gRPC | API key/provider config | Provider-based workflows, live integrations |

gRPC replay uses configured environment values (for example `SUI_GRPC_ENDPOINT`, `SUI_GRPC_API_KEY`) when set.

## Replay Diagnostics

### Analyze Hydration Inputs Before Executing

```bash
sui-sandbox analyze replay <DIGEST> --json
```

Check:

- `missing_inputs`
- `missing_packages`
- `hydration`
- `suggestions`

### Compare Local vs On-Chain Effects

```bash
sui-sandbox replay <DIGEST> --compare --json
```

For a step-by-step failure loop, use [REPLAY_TRIAGE.md](REPLAY_TRIAGE.md).

## Advanced Notes

- Historical object versions matter. Replaying with current object versions can produce false mismatches.
- Dynamic fields may require deeper prefetch settings (`--prefetch-depth`, `--prefetch-limit`) for complex DeFi transactions.
- Package upgrades rely on correct linkage/address alias resolution in replay hydration.

Architecture details:

- [../ARCHITECTURE.md](../ARCHITECTURE.md)
- [../architecture/PREFETCHING.md](../architecture/PREFETCHING.md)

## Programmatic Usage

If you need Rust-level integration, use:

- `sui_state_fetcher::HistoricalStateProvider` for state assembly
- replay orchestration in `crates/sui-sandbox-core/src/tx_replay.rs`

See examples:

- [../../examples/README.md](../../examples/README.md)
- [../../examples/advanced/fork_state.rs](../../examples/advanced/fork_state.rs)
- [../../examples/walrus_ptb_universe.rs](../../examples/walrus_ptb_universe.rs)

## Related Docs

- [REPLAY_TRIAGE.md](REPLAY_TRIAGE.md)
- [DATA_FETCHING.md](DATA_FETCHING.md)
- [../reference/CLI_REFERENCE.md](../reference/CLI_REFERENCE.md)
- [../reference/LIMITATIONS.md](../reference/LIMITATIONS.md)
