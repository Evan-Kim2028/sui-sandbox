# sui-sandbox

[![Version](https://img.shields.io/badge/version-0.19.0-green.svg)](Cargo.toml)
[![Sui](https://img.shields.io/badge/sui-mainnet--v1.64.2-blue.svg)](https://github.com/MystenLabs/sui)

Local Sui execution and replay harness. Run the real Move VM locally, hydrate historical state, and compare local effects against on-chain results.

## TL;DR

- Replay historical transactions from Walrus/gRPC/JSON with deterministic local state.
- Execute PTBs locally with Sui-compatible gas/effects behavior.
- Analyze packages/interfaces without running a full node.
- Use CLI or Python bindings (`pip install sui-sandbox`).

## What This Is (and Is Not)

This project is a local execution harness around Sui VM/runtime components.

Included:

- PTB execution semantics and VM harness integration
- Historical replay hydration and effects comparison
- Local object/package stores for deterministic simulation

Not included:

- Fullnode/validator authority services
- Consensus, checkpoint production, mempool, or P2P networking
- Long-running RPC service behavior

For a short talk-track and positioning, see [docs/START_HERE.md](docs/START_HERE.md).
For internals and control flow, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Quick Start (No API Key)

Walrus replay is zero-setup: no credentials required.

```bash
# Build CLI
cargo build --release --bin sui-sandbox

# Replay one mainnet transaction via Walrus and compare effects
sui-sandbox replay At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2 \
  --source walrus --checkpoint 239615926 --compare

# Or run the example scripts
./examples/scan_checkpoints.sh
./examples/replay.sh
```

## Common CLI Workflows

| Goal | Command |
|------|---------|
| Replay one tx (Walrus) | `sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare` |
| Scan latest checkpoints | `sui-sandbox replay '*' --source walrus --latest 5 --compare` |
| Export offline replay state | `sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json` |
| Replay fully offline | `sui-sandbox replay <DIGEST> --state-json state.json` |
| Import package | `sui-sandbox fetch package 0x2` |
| Publish + run local package | `sui-sandbox publish ./my_package` then `sui-sandbox run 0x100::module::func --arg 42` |
| Inspect replay inputs/hydration | `sui-sandbox analyze replay <DIGEST>` |

Full command/flag reference: [docs/reference/CLI_REFERENCE.md](docs/reference/CLI_REFERENCE.md)

## Data Sources

| Source | Auth | Best for |
|--------|------|----------|
| Walrus (default) | None | Historical replay with zero setup |
| JSON (`--state-json`) | None | Offline replay, CI fixtures, custom pipelines |
| gRPC | API key | Provider-based fetch/replay workflows |

## Python Bindings

Install:

```bash
pip install sui-sandbox
```

Minimal usage:

```python
import sui_sandbox

interface = sui_sandbox.extract_interface(package_id="0x2")
replay = sui_sandbox.replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    compare=True,
)
print(replay["local_success"])
```

More:

- Python API reference: [crates/sui-python/README.md](crates/sui-python/README.md)
- Local wheel/build workflow: [docs/guides/PYTHON_BINDINGS.md](docs/guides/PYTHON_BINDINGS.md)

## Docs Map

| I want to... | Read this |
|--------------|-----------|
| Understand what this project is | [docs/START_HERE.md](docs/START_HERE.md) |
| Learn by running examples | [examples/README.md](examples/README.md) |
| Replay transactions end-to-end | [docs/guides/TRANSACTION_REPLAY.md](docs/guides/TRANSACTION_REPLAY.md) |
| Debug replay failures | [docs/guides/REPLAY_TRIAGE.md](docs/guides/REPLAY_TRIAGE.md) |
| Find every CLI flag | [docs/reference/CLI_REFERENCE.md](docs/reference/CLI_REFERENCE.md) |
| Review caveats/parity limits | [docs/reference/LIMITATIONS.md](docs/reference/LIMITATIONS.md) |
| Build/test Python bindings locally | [docs/guides/PYTHON_BINDINGS.md](docs/guides/PYTHON_BINDINGS.md) |
| Understand internals | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |

Full docs index: [docs/README.md](docs/README.md)

## Testing

```bash
# Full workspace tests
cargo test

# CLI-focused suites
cargo test -p sui-sandbox --test fast_suite
cargo test -p sui-sandbox --test sandbox_cli_tests

# Integration tests
cargo test -p sui-sandbox-integration-tests

# Optional network tests
cargo test -p sui-sandbox-integration-tests --features network-tests -- --ignored --nocapture
```

Tip: set `SUI_SANDBOX_HOME` to isolate cache/logs/projects during tests.

## Repository Layout

```text
sui-sandbox/
├── examples/                 # starter scripts and sample workflows
├── src/                      # CLI entrypoints
├── crates/
│   ├── sui-sandbox-core/     # VM + PTB execution kernel
│   ├── sui-transport/        # Walrus/gRPC/GraphQL clients
│   ├── sui-state-fetcher/    # replay input/data provider layer
│   ├── sui-package-extractor/  # Move bytecode/interface extraction
│   └── sui-python/           # PyO3 bindings
└── docs/                     # guides, references, architecture
```

## Limitations

Simulation parity is high but not perfect. Key caveats include deterministic randomness, runtime-mode differences, and some edge cases around dynamic/shared-object behavior.

Details: [docs/reference/LIMITATIONS.md](docs/reference/LIMITATIONS.md)

## License

Apache 2.0
