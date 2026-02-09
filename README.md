# sui-sandbox

[![Version](https://img.shields.io/badge/version-0.14.0-green.svg)](Cargo.toml)
[![Sui](https://img.shields.io/badge/sui-mainnet--v1.63.4-blue.svg)](https://github.com/MystenLabs/sui)

Local Move VM execution for Sui. Replay mainnet transactions offline with real cryptography.

## What This Does

This tool runs the **real Sui Move VM** locally, enabling you to:

- **Replay mainnet transactions** - Execute historical transactions and verify results match on-chain effects
- **Test PTBs offline** - Dry-run transactions before spending gas
- **Develop against real state** - Deploy your own contracts into a sandbox with forked mainnet protocol state
- **Explore contracts** - Introspect module interfaces and function signatures

## Quick Start

**Zero setup required.** Walrus provides free, unauthenticated access to Sui checkpoint data.

```bash
# Build
cargo build --release --bin sui-sandbox

# Replay a real mainnet transaction (no API key, no configuration)
sui-sandbox replay At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2 \
  --source walrus --checkpoint 239615926 --compare

# Or use the example script
./examples/replay.sh
```

That's it. The replay command fetches the transaction, all objects, and all packages from Walrus decentralized storage, executes it in the local Move VM, and compares the results against on-chain effects.

### More Ways to Replay

```bash
# Scan the latest 5 checkpoints (auto-discovers tip, prints summary)
sui-sandbox replay '*' --source walrus --latest 5 --compare

# Replay a checkpoint range (all transactions)
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926

# Export state for offline replay later
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json

# Replay from exported JSON (completely offline, no network)
sui-sandbox replay <DIGEST> --state-json state.json

# Use gRPC if you have an endpoint configured
sui-sandbox replay <DIGEST> --source grpc --compare
```

## CLI

The `sui-sandbox` CLI is the primary developer interface. Replay is the flagship feature — everything else builds on it.

```bash
# Transaction replay (primary workflow)
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare
sui-sandbox replay '*' --source walrus --latest 5 --compare        # Scan latest
sui-sandbox replay '*' --source walrus --checkpoint 100..110       # Batch replay
sui-sandbox replay <DIGEST> --state-json state.json                # Offline replay
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state out.json

# Development workflow
sui-sandbox fetch package 0x2                 # Import Sui framework
sui-sandbox publish ./my_package              # Deploy your code
sui-sandbox run 0x100::module::func --arg 42  # Call a function
sui-sandbox analyze package --package-id 0x2  # Package introspection
sui-sandbox analyze replay <DIGEST>           # Replay-state introspection

# Session management
sui-sandbox init --example quickstart         # Scaffold workflow template
sui-sandbox run-flow flow.quickstart.yaml     # Run deterministic YAML workflow
sui-sandbox snapshot save baseline            # Save session snapshot
sui-sandbox status --json                     # Inspect state + counts
sui-sandbox bridge publish ./my_package       # Generate real deploy command
```

See [CLI Reference](docs/reference/CLI_REFERENCE.md) for all commands.

## Data Sources

| Source | Auth Required | Setup | Best For |
|--------|--------------|-------|----------|
| **Walrus** (default) | None | Zero | Replaying any transaction — just need digest + checkpoint |
| **JSON** | None | Zero | Offline replay, custom data pipelines, CI/CD |
| **gRPC** | API key | `.env` file | Real-time monitoring, streaming, latest state queries |

Walrus is the recommended starting point. It provides free, unauthenticated access to all Sui checkpoint data via decentralized storage.

## Troubleshooting

- Replay fails while hydrating state:
  - Try `--source walrus --checkpoint <CP>` (no auth needed).
  - Try `--source grpc` if you have a gRPC endpoint configured.
  - If deterministic behavior is required, use `--vm-only`.
  - If missing historical data is acceptable, enable `--allow-fallback`.
- Command failed and output is unclear:
  - Re-run with `--verbose`.
  - Add `--debug-json` to emit structured failure diagnostics.
- Session looks inconsistent:
  - Use `sui-sandbox snapshot load <name>` to restore a known-good snapshot.
  - Use `sui-sandbox reset` to clear in-memory session state.
  - Use `sui-sandbox clean` to remove persisted state file.

## Start Here: Examples

**The best way to understand the library is through the examples.** Start with replay — no setup needed:

| Level | Example | API Key | What You'll Learn |
|-------|---------|---------|-------------------|
| 0 | `replay.sh` | **No** | Transaction replay via Walrus (zero setup) |
| 0.5 | `scan_checkpoints.sh` | **No** | Scan & replay latest N checkpoints with summary |
| 1 | `cli_workflow.sh` | No | CLI basics, no compilation needed |
| 2 | `ptb_basics` | No | Basic PTB operations (split, transfer) |
| 3+ | `fork_state`, `cetus_swap`, etc. | Yes | Advanced: mainnet forking, DeFi replay |

```bash
# Start here: replay a real transaction (no API key needed)
./examples/replay.sh

# CLI exploration (no setup required)
./examples/cli_workflow.sh

# Your first Rust example
cargo run --example ptb_basics
```

See **[examples/README.md](examples/README.md)** for the full learning path.

## How Replay Works

```
1. Fetch checkpoint from Walrus (or transaction from gRPC/JSON)
2. Extract objects at their HISTORICAL versions (before modification)
3. Resolve packages with transitive dependencies
4. Execute in local Move VM
5. Compare local effects with on-chain effects
```

**Key insight**: Objects must be fetched at their *input* versions, not current versions. Walrus checkpoints contain objects at their exact versions. For gRPC, the `unchanged_loaded_runtime_objects` field provides this.

## What's Real vs Simulated

| Component | Implementation |
|-----------|----------------|
| Move VM execution | **Real** (move-vm-runtime) |
| Type checking | **Real** |
| BCS serialization | **Real** |
| Cryptography (ed25519, secp256k1, groth16) | **Real** (fastcrypto) |
| Dynamic fields | **Real** |
| Object storage | Simulated (in-memory) |
| Clock/timestamps | Configurable |
| Randomness | Deterministic |
| Gas metering | **Accurate** (Sui-compatible) |

**Rule of thumb**: If a transaction succeeds here, it will succeed on mainnet (assuming state hasn't changed).

## Why This Works

This sandbox uses **the same Move VM** that powers Sui validators, not a reimplementation:

```
┌─────────────────────────────────────────────────────────┐
│  Your Code                                              │
├─────────────────────────────────────────────────────────┤
│  PTBExecutor (simulation layer)                         │
│  - In-memory object store                               │
│  - Tracks mutations, events, effects                    │
├─────────────────────────────────────────────────────────┤
│  move-vm-runtime                                        │
│  - Real bytecode interpreter from Sui                   │
│  - Pinned to mainnet-v1.63.4                            │
├─────────────────────────────────────────────────────────┤
│  fastcrypto                                             │
│  - Real ed25519, secp256k1, BLS12-381, Groth16          │
│  - Same crypto library used by Sui validators           │
└─────────────────────────────────────────────────────────┘
```

Move bytecode is deterministic—given the same bytecode, inputs, and object state, execution produces identical results whether run locally or on mainnet. The sandbox replaces only the *storage layer* (objects live in memory instead of the blockchain) while keeping bytecode execution and cryptography real.

**Verify it yourself**: Run `./examples/replay.sh` to replay a real mainnet Cetus swap locally and compare effects against on-chain results. No API key needed.

## Documentation

| What you want | Where to look |
|---------------|---------------|
| **Docs index** | [docs/README.md](docs/README.md) |
| **Get started** | [examples/README.md](examples/README.md) |
| **Golden flow (CLI)** | [Golden Flow Guide](docs/guides/GOLDEN_FLOW.md) |
| **Replay transactions** | [Transaction Replay Guide](docs/guides/TRANSACTION_REPLAY.md) |
| **Understand the system** | [Architecture](docs/ARCHITECTURE.md) |
| **Debug failures** | [Limitations](docs/reference/LIMITATIONS.md) |
| **CLI commands** | [CLI Reference](docs/reference/CLI_REFERENCE.md) |
| **Testing** | [Contributing](docs/CONTRIBUTING.md) |

## Testing

```bash
# Run unit + integration tests (core)
cargo test

# Fast CLI smoke tests
cargo test -p sui-sandbox --test fast_suite

# CLI integration tests only
cargo test -p sui-sandbox --test sandbox_cli_tests

# Heavier integration tests (offline)
cargo test -p sui-sandbox-integration-tests

# Network tests (opt-in)
cargo test -p sui-sandbox-integration-tests --features network-tests -- --ignored --nocapture
```

Tip: set `SUI_SANDBOX_HOME` to isolate cache/logs/projects during tests.

## Project Structure

```
sui-sandbox/
├── examples/               # ← START HERE
│   ├── replay.sh           # Flagship: replay via Walrus (zero setup)
│   ├── scan_checkpoints.sh # Scan latest N checkpoints with summary
│   ├── cli_workflow.sh     # CLI demo (no setup)
│   ├── ptb_basics.rs       # Basic PTB operations
│   ├── fork_state.rs       # Mainnet forking (requires gRPC)
│   ├── cetus_swap.rs       # DeFi replay (requires gRPC)
│   └── deepbook_orders.rs  # BigVector replay (requires gRPC)
├── src/                    # Main library and CLI
├── crates/
│   ├── sui-sandbox-core/   # Core VM, PTB execution, gas metering
│   ├── sui-transport/      # Network layer (gRPC, GraphQL, Walrus)
│   ├── sui-prefetch/       # Strategic data loading, MM2 analysis
│   ├── sui-resolver/       # Address resolution & normalization
│   └── sui-state-fetcher/  # State provider abstraction
└── docs/                   # Guides and reference
```

## Limitations

The simulation layer differs from mainnet in these ways:

- **Randomness is deterministic** - Reproducible locally, different from mainnet VRF
- **Dynamic fields computed at runtime** - Some DeFi protocols traverse data structures unpredictably

Most limitations only matter for edge cases. For typical DeFi transactions, local execution matches mainnet exactly. See [Limitations](docs/reference/LIMITATIONS.md) for the complete list.

## License

Apache 2.0
