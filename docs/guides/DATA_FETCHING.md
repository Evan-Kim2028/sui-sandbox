# Data Fetching Guide

Use this guide to choose the right data source and fetching path for replay, analysis, and monitoring.

## Quick Decision Table

| Need | Recommended path | Auth | Why |
|------|------------------|------|-----|
| Replay historical transactions quickly | `sui-sandbox replay --source walrus` | None | Zero setup, checkpoint-native data |
| Deterministic offline replay | `--export-state` + `--state-json` | None | No network dependency |
| Programmatic replay hydration (Rust) | `HistoricalStateProvider` | Provider gRPC access | Version-aware replay state builder |
| Fetch latest object/package state | `sui-sandbox fetch` or `GraphQLClient` | None | Simple point queries |
| Real-time transaction feed | `tools stream-transactions` / `GrpcClient` | gRPC endpoint | Push stream, low latency |
| No gRPC access but periodic snapshots are enough | `tools poll-transactions` | None | Simple pull model over GraphQL |

## CLI Workflows

### Replay-Oriented Fetching

```bash
# Inspect hydration readiness before execution
sui-sandbox analyze replay <DIGEST> --json

# Replay from Walrus (recommended)
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CHECKPOINT> --compare

# Export replay state for offline reproduction
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CHECKPOINT> --export-state state.json

# Replay from exported state (no network)
sui-sandbox replay <DIGEST> --state-json state.json
```

See [TRANSACTION_REPLAY.md](TRANSACTION_REPLAY.md) for replay flow details.

### Point Queries (Package/Object/Checkpoint)

```bash
# Fetch package bytecode/modules into local session
sui-sandbox fetch package 0x2

# Fetch package with transitive dependencies
sui-sandbox fetch package 0x2 --with-deps

# Fetch object state
sui-sandbox fetch object 0x6

# Walrus checkpoint metadata
sui-sandbox fetch latest-checkpoint
sui-sandbox --json fetch checkpoint <CHECKPOINT>
```

### Monitoring and Collection

```bash
# GraphQL polling collector
sui-sandbox tools poll-transactions --duration 600 --interval-ms 1500 --output txs.jsonl

# gRPC streaming collector
sui-sandbox tools stream-transactions --endpoint <GRPC_ENDPOINT> --duration 60 --output stream.jsonl
```

Use `--ptb-only` on both commands to focus on user PTB transactions.

## Programmatic APIs (Rust)

### Historical Replay State

Use `sui_state_fetcher::HistoricalStateProvider` when you need replay state with historical object versions.

```rust
use sui_state_fetcher::HistoricalStateProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = HistoricalStateProvider::mainnet().await?;
    let state = provider.fetch_replay_state("<DIGEST>").await?;
    println!(
        "commands={} objects={} packages={}",
        state.transaction.commands.len(),
        state.objects.len(),
        state.packages.len()
    );
    Ok(())
}
```

### GraphQL Point Queries

Use `sui_transport::graphql::GraphQLClient` for current object/package/transaction queries.

```rust
use sui_transport::graphql::GraphQLClient;

fn main() -> anyhow::Result<()> {
    let client = GraphQLClient::mainnet();
    let pkg = client.fetch_package("0x2")?;
    let obj = client.fetch_object("0x6")?;
    println!("modules={} object_version={}", pkg.modules.len(), obj.version);
    Ok(())
}
```

### gRPC Streaming

Use `sui_transport::grpc::GrpcClient` for real-time checkpoint streams.

```rust
use sui_transport::grpc::GrpcClient;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = GrpcClient::mainnet().await?;
    let mut stream = client.subscribe_checkpoints().await?;
    while let Some(next) = stream.next().await {
        let checkpoint = next?;
        println!(
            "checkpoint={} txs={}",
            checkpoint.sequence_number,
            checkpoint.transactions.len()
        );
    }
    Ok(())
}
```

## Data Source Tradeoffs

| Dimension | Walrus replay | GraphQL | gRPC streaming |
|-----------|---------------|---------|----------------|
| Setup | Lowest | Low | Medium (endpoint/provider) |
| Historical replay | Strong | Good | Mixed (depends on endpoint/history window) |
| Real-time feed | No | Polling only | Yes |
| Effects detail for object changes | Replay output focused | Strong for query responses | Often limited in stream payload |
| Best use | Replay and offline export | Query-driven analysis | Live monitoring/indexing |

## Recommended Patterns

1. Replay/debug workflow:
   - Start with Walrus replay.
   - Export JSON state for deterministic reruns.
   - Use `analyze replay --json` when hydration fails.
2. Monitoring workflow:
   - Use gRPC streaming for low-latency ingest.
   - Backfill and enrich with GraphQL/Walrus as needed.
3. Offline/CI workflow:
   - Store exported replay JSON in fixtures.
   - Run `--state-json` in tests to avoid network drift.

## Common Failure Modes

- Missing replay inputs/packages:
  - Run `sui-sandbox analyze replay <DIGEST> --json`.
  - Check `missing_inputs`, `missing_packages`, and `suggestions`.
- Replay differs from chain:
  - Re-run with `--compare --json`.
  - Ensure checkpoint/digest pairing is correct.
- Stream disconnects:
  - Add reconnect logic in custom consumers.
  - For one-off capture jobs, use finite `--duration` and restart.

## Related Docs

- [TRANSACTION_REPLAY.md](TRANSACTION_REPLAY.md)
- [REPLAY_TRIAGE.md](REPLAY_TRIAGE.md)
- [../reference/CLI_REFERENCE.md](../reference/CLI_REFERENCE.md)
- [../architecture/PREFETCHING.md](../architecture/PREFETCHING.md)
