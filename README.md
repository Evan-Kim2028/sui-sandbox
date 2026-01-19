# sui-move-interface-extractor

A **high-fidelity local Move execution environment** for Sui. Test transactions, replay mainnet activity, and validate contract logic - all offline, with real cryptography.

## What This Is

This tool runs the **real Sui Move VM** locally, letting you:

- **Execute transactions offline** - No network, no tokens, no wallet needed
- **Test with real crypto** - Same cryptographic library as Sui validators (fastcrypto)
- **Replay mainnet transactions** - Verify your understanding of on-chain behavior
- **Explore contracts interactively** - Introspect modules, functions, and types
- **Stream real-time data** - gRPC streaming and GraphQL for mainnet data fetching

Think of it as a local Move execution sandbox with mainnet-grade fidelity.

## Quick Start

```bash
# Build
cargo build --release

# Replay a recent mainnet transaction locally
./target/release/sui_move_interface_extractor tx-replay <TRANSACTION_DIGEST>

# Interactive mode (JSON over stdin/stdout)
./target/release/sui_move_interface_extractor sandbox-exec --interactive

# List functions in a module
echo '{"action": "list_functions", "package_id": "0x2", "module": "coin"}' | \
  ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

## Getting Started: Cetus DEX Swap Replay

This walkthrough demonstrates replaying a real Cetus DEX swap transaction locally. This is a complete end-to-end example that verifies your setup works correctly.

**Transaction:** `7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp` (LEIA → SUI swap)

**Prerequisites:**

- Rust 1.75+ installed
- Network access to `archive.mainnet.sui.io:443` (for fetching historical state)

### Step 1: Build and Verify

```bash
# Clone and build
git clone https://github.com/anthropics/sui-move-interface-extractor.git
cd sui-move-interface-extractor
cargo build --release

# Verify the binary works
./target/release/sui_move_interface_extractor --help
```

### Step 2: Run the Cetus Swap Replay Test

The repository includes a pre-cached Cetus swap transaction and a comprehensive integration test:

```bash
# Run the Cetus swap replay test (fetches historical state from gRPC archive)
cargo test --test execute_cetus_swap test_replay_cetus_with_grpc_archive_data -- --nocapture
```

**Expected output:**

```text
✓ TRANSACTION REPLAYED SUCCESSFULLY WITH gRPC ARCHIVE DATA!
test test_replay_cetus_with_grpc_archive_data ... ok
```

### Step 3: Verify Your Setup (One Command)

Run the quickstart validation test to confirm everything works:

```bash
cargo test --test quickstart_validation -- --nocapture
```

This test validates:

- The cached transaction data exists and loads correctly
- gRPC archive connectivity (fetches historical object state)
- Package loading and address aliasing
- Dynamic field resolution (skip_list nodes)
- Full PTB execution with Move VM

### What's Happening Under the Hood

1. **Load cached transaction** from `.tx-cache/7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp.json`
2. **Fetch historical Pool state** from Sui's gRPC archive at the transaction-time version
3. **Pre-load dynamic field children** (skip_list nodes for tick management)
4. **Execute the PTB locally** with the real Move VM
5. **Verify success** - the swap executes identically to mainnet

### Troubleshooting

| Issue | Solution |
|-------|----------|
| `SKIP: No cache available` | The `.tx-cache/` directory should be included in the repo |
| `gRPC connection failed` | Check network connectivity to `archive.mainnet.sui.io:443` |
| `Package version check failed` | The test uses upgraded packages with address aliasing |

For detailed technical documentation, see [Case Study: Cetus LEIA/SUI Swap](docs/defi-case-study/01_CETUS_SWAP_LEIA_SUI.md).

## What's Real vs Simulated

| Component | Implementation |
|-----------|----------------|
| Move VM execution | **Real** (move-vm-runtime) |
| Type checking & abilities | **Real** |
| BCS serialization | **Real** |
| Hash functions | **Real** (sha2, sha3, keccak256, blake2b256) |
| Signature verification | **Real** (ed25519, secp256k1, secp256r1, bls12381) |
| ZK proof verification | **Real** (groth16 for BN254 and BLS12-381) |
| BLS12-381 group operations | **Real** (fastcrypto) |
| Dynamic fields | **Real** (full support) |
| Object storage | Simulated (in-memory) |
| Clock/timestamps | Configurable |
| Randomness | Deterministic (for reproducibility) |
| Gas metering | Permissive (configurable limits) |

**The rule of thumb:** Cryptographic operations are real. Storage is in-memory. If a transaction succeeds here, it will succeed on mainnet (assuming state hasn't changed).

## Use Cases

### Test Transactions Before Submitting

Dry-run your PTB locally before spending gas:

```bash
# Execute a PTB and see what would happen
echo '{
  "action": "execute_ptb",
  "commands": [
    {"MoveCall": {"package": "0x2", "module": "coin", "function": "zero", "type_arguments": ["0x2::sui::SUI"]}}
  ]
}' | ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

### Replay Mainnet Transactions

Understand what a transaction did by replaying it locally:

```bash
# Replay a specific transaction
./target/release/sui_move_interface_extractor tx-replay <DIGEST>

# Replay recent transactions (validation mode)
./target/release/sui_move_interface_extractor tx-replay --recent 100 --parallel
```

### Explore Contract APIs

Discover what functions are available and how to call them:

```bash
# List all modules in a package
echo '{"action": "list_modules", "package_id": "0x2"}' | ...

# Get function signature details
echo '{"action": "get_function_info", "package_id": "0x2", "module": "coin", "function": "split"}' | ...
```

### LLM/AI Integration

The sandbox provides structured JSON errors that are easy for LLMs to parse and learn from:

```json
{
  "error": "TypeMismatch",
  "expected": "0x2::coin::Coin<0x2::sui::SUI>",
  "got": "0x2::coin::Coin<0xabc::token::TOKEN>",
  "location": "argument 2"
}
```

This enables a feedback loop: LLM builds transaction → sandbox executes → structured error → LLM adjusts → repeat.

## Data Fetching

Fetch on-chain data from Sui mainnet/testnet with multiple backends:

| Backend | Best For |
|---------|----------|
| **GraphQL** | Queries, packages, objects, transaction replay verification |
| **gRPC Streaming** | Real-time checkpoint monitoring, high throughput, historical object versions |

```rust
use sui_move_interface_extractor::data_fetcher::DataFetcher;

// Fetch from mainnet
let fetcher = DataFetcher::mainnet();
let pkg = fetcher.fetch_package("0x2")?;  // Sui framework
let txs = fetcher.fetch_recent_ptb_transactions(25)?;
```

### Real-Time Streaming

Subscribe to checkpoints as they're finalized:

```bash
# Stream transactions via gRPC
cargo run --bin stream_transactions -- --duration 60 --output stream.jsonl

# Poll via GraphQL
cargo run --bin poll_transactions -- --duration 600 --interval 1500 --output txs.jsonl
```

See [Data Fetching Guide](docs/guides/DATA_FETCHING.md) for details.

## Python Integration

Native Python bindings via PyO3 for simulation and benchmarking:

```python
from sui_sandbox import SuiSandbox

sandbox = SuiSandbox()
sandbox.load_package("0x2")
result = sandbox.execute_ptb(commands=[...])
```

The `benchmark/` directory contains the `smi_bench` Python package for LLM evaluation and type inhabitation benchmarks.

## CLI Commands

| Command | Purpose |
|---------|---------|
| `sandbox-exec` | Interactive JSON API for transaction execution |
| `tx-replay` | Replay mainnet transactions locally |
| `ptb-eval` | Evaluate PTB with automatic dependency fetching |
| `benchmark-local` | Test type synthesis capabilities |
| `stream_transactions` | gRPC real-time transaction streaming |
| `poll_transactions` | GraphQL transaction polling |

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                    Your Application                          │
│              (CLI, Scripts, LLM Orchestrator)                │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ JSON over stdin/stdout
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      Sandbox API                             │
│           execute_ptb, list_functions, replay_tx...          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                  SimulationEnvironment                       │
│        Object store, PTB execution, effects tracking         │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐
│   Move VM (Real) │ │  Data Fetching   │ │  Transaction     │
│   Bytecode exec  │ │  GraphQL/gRPC    │ │  Caching         │
│   Type checking  │ │  Mainnet data    │ │  .tx-cache/      │
└──────────────────┘ └──────────────────┘ └──────────────────┘
```

### Transaction Replay Pipeline

For newcomers wanting to understand local transaction replay, here's how the system reconstructs and re-executes mainnet transactions:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                         TRANSACTION REPLAY FLOW                              │
└─────────────────────────────────────────────────────────────────────────────┘

1. FETCH                    2. CONVERT                   3. RESOLVE
   ─────────────────────       ─────────────────────       ─────────────────────
   Sui gRPC (Surflux)          GrpcTransaction             FetchedTransaction
   ↓                           ↓                           ↓
   • Transaction by digest     • Extract sender, gas       • Fetch input objects
   • Historical objects        • Parse commands            • Get historical versions
   • Package bytecode          • Convert inputs            • Load package bytecode

4. TRANSFORM                5. EXECUTE                   6. COMPARE
   ─────────────────────       ─────────────────────       ─────────────────────
   to_ptb_commands()           PTBExecutor                 EffectsComparison
   ↓                           ↓                           ↓
   • InputValue enum           • VMHarness runs Move VM    • Created objects
   • Command enum              • Track mutations           • Mutated objects
   • Type argument parsing     • Handle mutable refs       • Deleted objects
```

**Key modules:**

| Module | File | Purpose |
|--------|------|---------|
| `tx_replay` | `src/benchmark/tx_replay.rs` | Transaction fetching, conversion, and replay orchestration |
| `ptb` | `src/benchmark/ptb.rs` | PTB command execution with result chaining |
| `vm` | `src/benchmark/vm.rs` | Move VM harness for sandboxed bytecode execution |

**The critical insight:** Replaying requires fetching objects at their *input* versions (before the transaction modified them), not their current versions. The `unchanged_loaded_runtime_objects` field from Surflux gRPC provides this information.

For detailed module documentation, see the doc comments in each source file.

## Installation

```bash
# Prerequisites: Rust 1.75+
git clone https://github.com/anthropics/sui-move-interface-extractor.git
cd sui-move-interface-extractor
cargo build --release

# Verify installation
./target/release/sui_move_interface_extractor --help
```

## Upgrading Sui Version

When Sui releases a new mainnet version, update this project:

```bash
# Automated upgrade (updates Cargo.toml, version constants, fetches new protos)
./scripts/update-sui-version.sh mainnet-v1.70.0

# Then manually:
# 1. Update Dockerfile SUI_VERSION
# 2. cargo build
# 3. Rebuild framework bytecode (see script output)
# 4. cargo test
```

See [src/grpc/README.md](src/grpc/README.md) for detailed version management docs.

## Documentation

| Category | Documents |
|----------|-----------|
| **Getting Started** | [Quickstart](docs/getting-started/QUICKSTART.md) · [Troubleshooting](docs/getting-started/TROUBLESHOOTING.md) |
| **Guides** | [Transaction Replay](docs/guides/TRANSACTION_REPLAY.md) · [LLM Integration](docs/guides/LLM_INTEGRATION.md) · [Data Fetching](docs/guides/DATA_FETCHING.md) · [Running Benchmarks](docs/guides/RUNNING_BENCHMARKS.md) · [Local Sandbox](docs/guides/LOCAL_BYTECODE_SANDBOX.md) |
| **Reference** | [CLI Reference](docs/reference/CLI_REFERENCE.md) · [Sandbox API](docs/reference/SANDBOX_API.md) · [Error Codes](docs/reference/ERROR_CODES.md) · [PTB Schema](docs/reference/PTB_SCHEMA.md) · [JSON Schema](docs/reference/SCHEMA.md) |
| **Methodology** | [Methodology](docs/METHODOLOGY.md) · [Insights](docs/INSIGHTS.md) · [Type Inhabitation Spec](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) |
| **Case Studies** | [Cetus Swap Replay](docs/defi-case-study/01_CETUS_SWAP_LEIA_SUI.md) · [Jackson SUI Swap](docs/defi-case-study/02_CETUS_SWAP_JACKSON_SUI.md) · [Complex TX Replay](docs/defi-case-study/03_COMPLEX_TX_REPLAY.md) |
| **Design** | [Architecture](ARCHITECTURE.md) · [Contributing](docs/CONTRIBUTING.md) · [Migration](docs/MIGRATION.md) |
| **Benchmark** | [Getting Started](benchmark/GETTING_STARTED.md) · [Datasets](benchmark/DATASETS.md) · [Architecture](benchmark/docs/ARCHITECTURE.md) |

## Limitations

- **Gas estimation is approximate** - Use `sui_dryRunTransactionBlock` RPC for exact gas
- **Randomness is deterministic** - For reproducibility, not real VRF
- **No network operations in sandbox** - Offline execution only (use DataFetcher separately)
- **VRF not implemented** - `ecvrf::*` operations are mocked

## Contributing

```bash
cargo fmt && cargo clippy && cargo test
```

## License

Apache 2.0 - see [LICENSE](LICENSE) for details.
