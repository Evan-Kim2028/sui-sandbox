# sui-sandbox

[![Version](https://img.shields.io/badge/version-0.18.0-green.svg)](Cargo.toml)
[![Sui](https://img.shields.io/badge/sui-mainnet--v1.64.2-blue.svg)](https://github.com/MystenLabs/sui)

Local Move VM execution for Sui. Replay mainnet transactions offline with real cryptography.

## What This Does

This tool runs the **real Sui Move VM** locally, enabling you to:

- **Replay mainnet transactions** - Execute historical transactions and verify results match on-chain effects
- **Test PTBs offline** - Dry-run transactions before spending gas
- **Develop against real state** - Deploy your own contracts into a sandbox with forked mainnet protocol state
- **Explore contracts** - Introspect module interfaces and function signatures

## Scope: What This Is (and Is Not)

This project is a **local execution harness**, not a full Sui node implementation.

What it includes:

- PTB execution kernel (`PTBExecutor`) and command semantics
- VM harnessing around `move-vm-runtime`
- Replay hydration from Walrus/gRPC/JSON and effects comparison
- Local in-memory object/runtime simulation and package resolution

What it does not include:

- Validator/fullnode authority services
- Consensus pipeline and checkpoint production
- Mempool/transaction manager and P2P networking
- Long-running node RPC service surface

## 20-Second Explanation

`sui-sandbox` is a local Sui execution workspace:

- It replays historical transactions from Walrus/gRPC/JSON.
- It executes PTBs locally with a deterministic session state.
- It analyzes package/bytecode surfaces for developer workflows.

Use it when you want fast local iteration with replay fidelity, not when you need to run a full node.

## How This Differs from Existing Tooling

| Tooling | Primary use | Where execution happens | Historical replay | Session model |
|---------|-------------|-------------------------|-------------------|---------------|
| Generic Move sandbox tooling | Move package/unit test workflows | Local | No | Local, generic Move |
| Fullnode RPC dry-run / dev-inspect | Preflight on live network state | Remote fullnode | Limited | Stateless request/response |
| `sui-sandbox` | Sui replay + PTB + package analysis workflows | Local Move VM | Yes (Walrus/gRPC/JSON) | Persistent local sandbox session |

See [docs/START_HERE.md](docs/START_HERE.md) for a short talk-track you can reuse.

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
sui-sandbox replay mutate --demo                                   # Guided replay-mutation demo
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --strategy examples/replay_mutate_strategies/default.yaml

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

For a systematic diagnostic workflow, see the [Replay Triage Guide](docs/guides/REPLAY_TRIAGE.md).

## Start Here: Examples

**The best way to understand the library is through the examples.** Start with Walrus checkpoint stream replay — no setup needed:

| Tier | Example | API Key | What You'll Learn |
|------|---------|---------|-------------------|
| Zero Setup | `scan_checkpoints.sh` | **No** | Core flow: stream replay over recent Walrus checkpoints |
| Zero Setup | `replay.sh` | **No** | Drill into single-transaction replay via Walrus |
| Zero Setup | `cli_workflow.sh` | No | CLI basics, no compilation needed |
| gRPC | `obfuscated_package_analysis` | Yes | Reverse-engineer obfuscated package via bytecode + replay |
| Rust | `ptb_basics` | No | Basic PTB operations (split, transfer) |
| Rust + gRPC | `fork_state`, `cetus_swap`, etc. | Yes | Advanced: mainnet forking, DeFi replay |

```bash
# Start here: stream replay from recent checkpoints (no API key needed)
./examples/scan_checkpoints.sh

# Then drill into a single transaction as needed
./examples/replay.sh

# CLI exploration (no setup required)
./examples/cli_workflow.sh
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

## PTB Execution Kernel (Control Flow)

The main command paths converge into the same execution kernel:

1. CLI entrypoint dispatches to `ptb`/`replay` command handlers.
2. Input data is parsed/hydrated into PTB commands + typed inputs.
3. `SimulationConfig` is built and `VMHarness` is initialized.
4. `validate_ptb` runs structural/causality checks.
5. `PTBExecutor::execute_commands` runs command handlers in order.
6. `VMHarness` executes Move calls inside a VM session with native extensions.
7. `TransactionEffects` are produced and optionally compared against on-chain effects.

Key files:

- `src/bin/sui_sandbox.rs` (CLI dispatch)
- `src/bin/sandbox_cli/ptb.rs` (direct PTB execution path)
- `src/bin/sandbox_cli/replay.rs` (replay hydration + execution path)
- `crates/sui-sandbox-core/src/ptb.rs` (kernel and command semantics)
- `crates/sui-sandbox-core/src/vm.rs` (VM session/runtime integration)
- `crates/sui-sandbox-core/src/tx_replay.rs` (replay orchestration and compare)

## What's Real vs Simulated

| Component | Implementation |
|-----------|----------------|
| Move VM execution | **Real** (move-vm-runtime) |
| Type checking | **Real** |
| BCS serialization | **Real** |
| Cryptography (ed25519, secp256k1, groth16) | **Real** (fastcrypto) |
| Dynamic fields | Runtime-mode dependent (see below) |
| Object storage | Simulated (in-memory) |
| Clock/timestamps | Configurable |
| Randomness | Deterministic |
| Gas metering | **Accurate** (Sui-compatible) |

Runtime modes:

- `use_sui_natives = false` (default): sandbox runtime path, tuned for local development and broad compatibility.
- `use_sui_natives = true` (opt-in via library API): Sui native object runtime path for maximum parity checks.

**Rule of thumb**: Success here is a strong signal for mainnet success when state/protocol inputs match, but it is not an absolute guarantee. See [Limitations](docs/reference/LIMITATIONS.md).

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
│  - Pinned to mainnet-v1.64.2                            │
├─────────────────────────────────────────────────────────┤
│  fastcrypto                                             │
│  - Real ed25519, secp256k1, BLS12-381, Groth16          │
│  - Same crypto library used by Sui validators           │
└─────────────────────────────────────────────────────────┘
```

Move bytecode is deterministic—given the same bytecode, inputs, and object state, execution produces identical results whether run locally or on mainnet. The sandbox replaces only the *storage layer* (objects live in memory instead of the blockchain) while keeping bytecode execution and cryptography real.

**Verify it yourself**: Run `./examples/replay.sh` to replay a real mainnet Cetus swap locally and compare effects against on-chain results. No API key needed.

## Python Bindings

Python package exposing package analysis, checkpoint replay, Move VM execution, and function fuzzing via PyO3. 7 of 9 functions are fully standalone — no CLI binary needed.

### Install

```bash
pip install sui-sandbox
```

Or build from source (requires Rust toolchain):

```bash
cd crates/sui-python && pip install maturin && maturin develop --release
```

### Usage

```python
import sui_sandbox

# --- Package introspection (no API key needed) ---
interface = sui_sandbox.extract_interface(package_id="0x1")

# --- Walrus checkpoint data (no API key needed) ---
cp = sui_sandbox.get_latest_checkpoint()
data = sui_sandbox.get_checkpoint(cp)

# --- Move view function execution ---
result = sui_sandbox.call_view_function(
    "0x2", "clock", "timestamp_ms",
    object_inputs=[{"Clock": "0x6"}],
)
print(result["return_values"])

# --- Move function fuzzing ---
report = sui_sandbox.fuzz_function("0x1", "u64", "max", iterations=50)
print(f"Verdict: {report['verdict']}, successes: {report['successes']}")

# --- Bytecode utilities ---
pkgs = sui_sandbox.fetch_package_bytecodes("0x2", resolve_deps=True)
bcs_bytes = sui_sandbox.json_to_bcs(type_str, object_json, pkgs["packages"])
```

See [crates/sui-python/README.md](crates/sui-python/README.md) for the full API reference.

## Documentation

| I want to... | Where to look |
|--------------|---------------|
| **Understand what this is** | [Start Here](docs/START_HERE.md) |
| **Get started quickly** | [examples/README.md](examples/README.md) |
| **Replay a mainnet transaction** | [Transaction Replay Guide](docs/guides/TRANSACTION_REPLAY.md) |
| **Test my Move code locally** | [Golden Flow Guide](docs/guides/GOLDEN_FLOW.md) |
| **Debug a replay failure** | [Replay Triage](docs/guides/REPLAY_TRIAGE.md) |
| **Reverse-engineer a contract** | [Obfuscated Package Analysis](examples/obfuscated_package_analysis/README.md) |
| **Look up a CLI command** | [CLI Reference](docs/reference/CLI_REFERENCE.md) |
| **Understand replay caveats** | [Limitations](docs/reference/LIMITATIONS.md) |
| **Use Python bindings** | [Python README](crates/sui-python/README.md) |
| **Understand the system internals** | [Architecture](docs/ARCHITECTURE.md) |

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
│   ├── sui-state-fetcher/  # State provider abstraction
│   └── sui-python/         # Python bindings (PyO3/maturin)
└── docs/                   # Guides and reference
```

## Limitations

The simulation layer differs from mainnet in these ways:

- **Randomness is deterministic** — Reproducible locally, different from mainnet VRF
- **Dynamic fields computed at runtime** — Some DeFi protocols traverse data structures unpredictably
- **Storage rebates approximated** — Gas refunds for deleted objects may differ by small amounts
- **Shared object initial versions** — Edge cases in initial version tracking for shared objects
- **Package upgrade linkage** — Replaying upgraded packages requires correct historical linkage table versions

For many DeFi transactions, local execution is close to mainnet behavior, but parity depends on runtime mode and data completeness. See [Limitations](docs/reference/LIMITATIONS.md) for the complete list.

## License

Apache 2.0
