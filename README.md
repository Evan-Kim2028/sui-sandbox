# sui-sandbox

[![Version](https://img.shields.io/badge/version-0.11.0-green.svg)](Cargo.toml)
[![Sui](https://img.shields.io/badge/sui-mainnet--v1.63.4-blue.svg)](https://github.com/MystenLabs/sui)

Local Move VM execution for Sui. Replay mainnet transactions offline with real cryptography.

## What This Does

This tool runs the **real Sui Move VM** locally, enabling you to:

- **Replay mainnet transactions** - Execute historical transactions and verify results match on-chain effects
- **Test PTBs offline** - Dry-run transactions before spending gas
- **Develop against real state** - Deploy your own contracts into a sandbox with forked mainnet protocol state
- **Explore contracts** - Introspect module interfaces and function signatures

## Quick Start

```bash
# Build
cargo build --release

# Set up gRPC configuration
cp .env.example .env
# Edit .env with your endpoint (default: https://fullnode.mainnet.sui.io:443)
# If your endpoint requires auth, set SUI_GRPC_API_KEY in .env

# Build the test fixture (required for CLI workflow example)
cd tests/fixture && sui move build && cd ../..

# Replay a DeepBook transaction
cargo run --example deepbook_replay
```

## CLI

For interactive development, use the `sui-sandbox` CLI:

```bash
cargo build --release --bin sui-sandbox

sui-sandbox fetch package 0x2                 # Import Sui framework
sui-sandbox publish ./my_package              # Deploy your code
sui-sandbox run 0x100::module::func --arg 42  # Call a function
sui-sandbox replay <TX_DIGEST> --compare      # Replay and verify
sui-sandbox bridge publish ./my_package       # Generate real deploy command
sui-sandbox tool create_move_project --input '{"name":"demo"}' # MCP tool parity
```

See [CLI Reference](docs/reference/CLI_REFERENCE.md) for all commands.

## MCP (LLM Tools)

You can use the MCP surface in two ways:

1. **CLI tool mode** (JSON in/out) — best for scripts and quick parity checks.
2. **MCP server (stdio)** — for MCP clients (Claude/GPT/etc.) to invoke tools directly.

```bash
# Build MCP server binary
cargo build --release --bin sui-sandbox-mcp

# Run MCP server over stdio (connect from your MCP client)
./target/release/sui-sandbox-mcp
```

Example tool invocation via CLI (no server required):

```bash
sui-sandbox tool call_function --input '{"package":"0x2","module":"coin","function":"zero","type_args":["0x2::sui::SUI"],"args":[]}'
```

### Cache + Logs

CLI and MCP share the same cache/log roots via `SUI_SANDBOX_HOME` (default: `~/.sui-sandbox`):

```
~/.sui-sandbox/
├── cache/      # Global cache (shared across CLI + MCP, per-network)
├── projects/   # MCP project workspace
└── logs/mcp/   # MCP JSONL logs (inputs/outputs, llm_reason, tags)
```

Add optional LLM metadata to any tool input:

```json
{
  "_meta": { "reason": "Inspect interface before PTB", "tags": ["analysis"] },
  "package": "0x2",
  "module": "coin"
}
```

## Start Here: Examples

**The best way to understand the library is through the examples.** They're ordered from simple to complex:

| Level | Example | API Key | What You'll Learn |
|-------|---------|---------|-------------------|
| 1 | `cli_workflow.sh` | No | CLI basics, no compilation needed |
| 2 | `ptb_basics` | No | Basic PTB operations (split, transfer) |
| 3 | `fork_state` | Yes | Fork mainnet state into local sandbox |
| 4 | `cetus_swap` | Yes | Full transaction replay with validation |
| 5 | `scallop_deposit` | Yes | Lending protocol replay with MM2 bytecode analysis |
| 6 | `multi_swap_flash_loan` | Yes | Complex multi-DEX arbitrage replay |

```bash
# Start with the CLI workflow (no setup required)
./examples/cli_workflow.sh

# Then try a simple code example
cargo run --example ptb_basics

# Graduate to mainnet replay
cargo run --example cetus_swap

# See MM2 predictive prefetch in action
cargo run --example scallop_deposit
```

For CLI+MCP parity examples, see **[examples/cli_mcp](examples/cli_mcp)**.
For self-healing replay demos (testing only), see **[examples/self_heal](examples/self_heal)**.

See **[examples/README.md](examples/README.md)** for detailed documentation on each example.

## How Replay Works

```
1. Fetch transaction from gRPC
2. Fetch objects at their HISTORICAL versions (before modification)
3. Fetch packages with transitive dependencies
4. Execute in local Move VM
5. Compare local effects with on-chain effects
```

**Key insight**: Objects must be fetched at their *input* versions, not current versions. The `unchanged_loaded_runtime_objects` field from gRPC provides this.

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

**Verify it yourself**: Run `cargo run --example deepbook_replay` to execute a real mainnet transaction locally and compare effects byte-for-byte against on-chain results.

## Documentation

| What you want | Where to look |
|---------------|---------------|
| **Get started** | [examples/README.md](examples/README.md) |
| **Replay transactions** | [Transaction Replay Guide](docs/guides/TRANSACTION_REPLAY.md) |
| **Understand the system** | [Architecture](docs/ARCHITECTURE.md) |
| **Debug failures** | [Limitations](docs/reference/LIMITATIONS.md) |
| **CLI commands** | [CLI Reference](docs/reference/CLI_REFERENCE.md) |
| **MCP server** | [MCP Reference](docs/reference/MCP_REFERENCE.md) |
| **Testing** | [Contributing](docs/CONTRIBUTING.md) |

## Testing

```bash
# Run unit + integration tests
cargo test

# CLI integration tests only
cargo test -p sui-sandbox --test sandbox_cli_tests
```

Tip: set `SUI_SANDBOX_HOME` to isolate cache/logs/projects during tests.

## Project Structure

```
sui-sandbox/
├── examples/               # ← START HERE
│   ├── cli_workflow.sh     # CLI demo (no setup)
│   ├── ptb_basics.rs       # Basic PTB
│   ├── fork_state.rs       # Mainnet forking
│   ├── cetus_swap.rs       # Canonical replay example
│   └── scallop_deposit.rs  # MM2 bytecode analysis
├── src/                    # Main library and CLI
├── crates/
│   ├── sui-sandbox-core/   # Core VM, PTB execution, gas metering
│   ├── sui-transport/      # Network layer (gRPC + GraphQL)
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
