# Start Here

`sui-sandbox` runs the real Sui Move VM locally so you can replay historical transactions, execute PTBs, and analyze packages with deterministic local state.

## Short Talk-Track

`sui-sandbox` is a local execution harness for Sui developers. It hydrates real historical transaction state (Walrus/gRPC/JSON), runs PTBs in a local Move VM session, and helps debug/compare effects without running a full node.

## How It Works (One Flow)

1. Hydrate transaction + objects + packages from a source (Walrus/gRPC/JSON).
2. Build local resolver/runtime context.
3. Execute PTB commands in the local Move VM.
4. Collect effects, events, gas, and diagnostics.
5. Optionally compare local results with on-chain effects.

## How It Differs from Other Tools

| Tooling | Best at | Execution location | Historical replay | Local deterministic session |
|---------|---------|--------------------|-------------------|-----------------------------|
| Generic Move sandbox tooling | Move package/unit test loops | Local | No | Yes (generic Move) |
| Fullnode RPC dry-run/dev-inspect | Quick preflight checks against live state | Remote fullnode | Limited | No |
| `sui-sandbox` | Replay + PTB + package analysis workflows | Local Move VM | Yes | Yes |

## Which Tool Should I Use?

- Use `sui-sandbox` when you need local reproducibility, replay diagnostics, or PTB iteration with controlled state.
- Use fullnode dry-run/dev-inspect when you only need a quick remote preflight check.
- Use generic Move sandbox tooling when working outside Sui-specific replay/runtime concerns.

## Fastest Start (No API Key)

```bash
# Build once
cargo build --release --bin sui-sandbox

# Core flow: stream replay over recent Walrus checkpoints
./examples/scan_checkpoints.sh

# Then drill into one transaction if needed
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare
```

## I Want To...

| Goal | Where to go |
|------|-------------|
| Replay a mainnet transaction | `./examples/replay.sh` or [Transaction Replay Guide](guides/TRANSACTION_REPLAY.md) |
| Analyze a package's modules and functions | `sui-sandbox fetch package <ID> --with-deps` then `sui-sandbox view modules <ID>` |
| Reverse-engineer an obfuscated contract | [Obfuscated Package Analysis](../examples/obfuscated_package_analysis/README.md) |
| Test my Move code locally before deploying | [Golden Flow Guide](guides/GOLDEN_FLOW.md) |
| Debug why a replay failed | [Replay Triage Guide](guides/REPLAY_TRIAGE.md) |
| Understand known differences from mainnet | [Limitations](reference/LIMITATIONS.md) |

## What to Read Next

- **Just want to run things** (5-15 min): [examples/README.md](../examples/README.md) — shell scripts, no setup required
- **Need to replay a specific transaction**: [Transaction Replay Guide](guides/TRANSACTION_REPLAY.md) — step-by-step with data source options
- **Building an integration or debugging internals**: [Architecture](ARCHITECTURE.md) — system components and control flow
- **Full command reference**: [CLI Reference](reference/CLI_REFERENCE.md) — every command, flag, and environment variable
