# Start Here

Use this page to understand what `sui-sandbox` is for and which doc to open next.

## One-Sentence Summary

`sui-sandbox` is a local Sui execution harness: hydrate historical state from Walrus/gRPC/JSON, execute in the real Move VM locally, and compare results with on-chain effects.

## When to Use It

Use `sui-sandbox` when you need:

- Deterministic local replay of historical transactions
- Local PTB execution/debugging with controllable state
- Package/interface analysis tied to replay workflows

Use fullnode dry-run/dev-inspect when you only need a quick remote preflight, not a local reproducible execution session.

## Tool Positioning

| Tooling | Execution location | Historical replay | Deterministic local session |
|---------|--------------------|-------------------|-----------------------------|
| Generic Move sandbox tooling | Local | No | Yes (generic Move) |
| Fullnode RPC dry-run/dev-inspect | Remote fullnode | Limited | No |
| `sui-sandbox` | Local Move VM | Yes | Yes |

## Fastest Path

Start here if you just want to run replay now:

1. Root quickstart:
   - [../README.md](../README.md)
2. Example-first path:
   - [../examples/README.md](../examples/README.md)
3. End-to-end replay guide:
   - [guides/TRANSACTION_REPLAY.md](guides/TRANSACTION_REPLAY.md)

## Pick Your Next Doc

| Goal | Doc |
|------|-----|
| Replay one or many transactions | [guides/TRANSACTION_REPLAY.md](guides/TRANSACTION_REPLAY.md) |
| Debug replay failures | [guides/REPLAY_TRIAGE.md](guides/REPLAY_TRIAGE.md) |
| Publish/run local Move packages | [guides/GOLDEN_FLOW.md](guides/GOLDEN_FLOW.md) |
| Understand architecture and control flow | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Find all CLI flags and commands | [reference/CLI_REFERENCE.md](reference/CLI_REFERENCE.md) |
| Understand known parity caveats | [reference/LIMITATIONS.md](reference/LIMITATIONS.md) |
| Build Python bindings locally | [guides/PYTHON_BINDINGS.md](guides/PYTHON_BINDINGS.md) |

Docs index: [README.md](README.md)
