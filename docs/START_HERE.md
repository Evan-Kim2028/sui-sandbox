# Start Here

If someone asks what `sui-sandbox` is, use this:

`sui-sandbox` runs the real Sui Move VM locally so you can replay historical transactions, execute PTBs, and analyze packages with deterministic local state.

## Short Talk-Track

Copy/paste version:

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

## Pick Your Depth

- 5-15 min (core examples): [../examples/README.md](../examples/README.md)
- Core replay workflow: [guides/TRANSACTION_REPLAY.md](guides/TRANSACTION_REPLAY.md)
- Full command surface: [reference/CLI_REFERENCE.md](reference/CLI_REFERENCE.md)
- Internals: [ARCHITECTURE.md](ARCHITECTURE.md)
