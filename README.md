# sui-sandbox

A **high-fidelity local Move execution environment** for Sui. Test transactions, replay mainnet activity, and validate contract logic - all offline, with real cryptography.

## What This Is

This tool runs the **real Sui Move VM** locally, letting you:

- **Execute transactions offline** - No network, no tokens, no wallet needed
- **Test with real crypto** - Same cryptographic library as Sui validators (fastcrypto)
- **Replay mainnet transactions** - Verify your understanding of on-chain behavior
- **Explore contracts interactively** - Introspect modules, functions, and types
- **Stream real-time data** - gRPC streaming and GraphQL for mainnet data fetching

Think of it as a local Move execution sandbox with mainnet-grade fidelity.

## Workspace Structure

The project is organized as a Cargo workspace with specialized crates:

| Crate | Purpose |
|-------|---------|
| **sui-sandbox** (root) | Main library with CLI, examples, and integration tests |
| **sui-sandbox-core** | Move VM simulation engine (PTB execution, transaction replay) |
| **sui-data-fetcher** | GraphQL and gRPC clients for Sui network data |
| **sui-package-extractor** | Bytecode parsing and interface extraction |
| **sui-sandbox-types** | Shared types (RetryConfig, etc.) |

```text
sui-sandbox/
├── src/                    # Main library
│   ├── benchmark/          # Core simulation engine (VMHarness, PTBExecutor)
│   ├── data_fetcher.rs     # Unified data fetching API
│   └── ...
├── examples/               # Self-contained replay examples (START HERE!)
├── crates/
│   ├── sui-sandbox-core/   # Re-exports simulation modules
│   ├── sui-data-fetcher/   # GraphQL + gRPC clients
│   ├── sui-package-extractor/  # Bytecode analysis
│   └── sui-types/          # Shared types
└── tests/                  # Integration tests
```

## Quick Start

```bash
# Build
cargo build --release

# Developer CLI - publish, run, inspect Move packages
./target/release/sui-sandbox publish ./my_package --bytecode-only
./target/release/sui-sandbox run 0x2::coin::value --arg 0x123
./target/release/sui-sandbox view module 0x2::coin

# Run a self-contained example (no cache needed!)
cargo run --example deepbook_replay

# Interactive sandbox mode (JSON over stdin/stdout)
./target/release/sui_move_interface_extractor sandbox-exec --interactive

# List functions in a module
echo '{"action": "list_functions", "package_id": "0x2", "module": "coin"}' | \
  ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

## Examples (Recommended Starting Point)

The `examples/` directory contains **self-contained, cache-free** examples that demonstrate historical transaction replay. These are the best way to get started - they fetch all data fresh via gRPC, require no pre-cached data, and show the complete workflow from transaction fetching to local execution.

### Running the Examples

```bash
# 1. Set up your Surflux API key (get one at https://surflux.dev)
echo "SURFLUX_API_KEY=your-api-key" > .env

# 2. Run any example
cargo run --example deepbook_replay    # DeepBook flash loan swaps
cargo run --example cetus_swap         # Cetus AMM swap
cargo run --example scallop_deposit    # Scallop lending deposit
cargo run --example inspect_df         # Framework module inspector (no API key needed)
```

### Available Examples

| Example | Protocol | Description |
|---------|----------|-------------|
| `deepbook_replay` | DeepBook | Flash loan swap transactions - demonstrates success/failure replay |
| `cetus_swap` | Cetus CLMM | AMM swap with dynamic field children (skip_list nodes) |
| `scallop_deposit` | Scallop | Lending protocol deposit with version-locked contracts |
| `inspect_df` | Framework | Diagnostic tool for inspecting dynamic field module bytecode |

### Why Cache-Free Examples?

Each example is **completely self-contained**:

1. **No Cache Required** - Fetches all data fresh via gRPC, no `.tx-cache/` directory needed
2. **Self-Documenting** - All helper functions are defined locally with documentation
3. **Portable** - Works on any machine with a Surflux API key
4. **Robust to Upgrades** - Follows package linkage tables to handle protocol upgrades
5. **Educational** - Step-by-step output shows exactly what's happening

### Key Techniques Demonstrated

The examples showcase the complete historical replay workflow:

```text
Step 1: Connect to Surflux gRPC
Step 2: Fetch transaction via gRPC
Step 3: Collect historical object versions (unchanged_loaded_runtime_objects)
Step 4: Fetch objects at exact historical versions
Step 5: Fetch packages with transitive dependencies (following linkage tables)
Step 6: Build transaction structure
Step 7: Build module resolver with address aliasing
Step 8: Create VM harness with correct timestamp
Step 9: Set up on-demand child fetcher for dynamic fields
Step 10: Register input objects
Step 11: Execute and compare results
```

### Example Output

```text
╔══════════════════════════════════════════════════════════════════════╗
║      DeepBook Flash Loan Replay - Pure gRPC (No Cache)               ║
╚══════════════════════════════════════════════════════════════════════╝

Step 1: Connecting to Surflux gRPC...
   ✓ Connected to Surflux gRPC

Step 2: Fetching transaction via gRPC...
   Digest: DwrqFzBSVHRAqeG4cp1Ri3Gw3m1cDUcBmfzRtWSTYFPs
   Commands: 17
   Status: Success

...

╔══════════════════════════════════════════════════════════════════════╗
║                         VALIDATION SUMMARY                           ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ Flash Loan Swap           | local: SUCCESS | expected: SUCCESS     ║
║ ✓ Flash Loan Arb            | local: FAILURE | expected: FAILURE     ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ ALL TRANSACTIONS MATCH EXPECTED OUTCOMES                           ║
╚══════════════════════════════════════════════════════════════════════╝
```

## Getting Started: Local Move Execution

This walkthrough demonstrates the core capabilities of the local Move execution sandbox.

**Prerequisites:**

- Rust 1.75+ installed

### Step 1: Build and Verify

```bash
# Clone and build
git clone https://github.com/anthropics/sui-move-interface-extractor.git
cd sui-move-interface-extractor
cargo build --release

# Verify the binary works
./target/release/sui_move_interface_extractor --help
```

### Step 2: Run the Core Tests

```bash
# Run the sandbox replay integration tests
cargo test --test sandbox_replay_integration_tests -- --nocapture

# Run the state persistence tests
cargo test --test state_persistence_tests -- --nocapture
```

**Expected output:**

```text
test test_simulation_environment_create_coin ... ok
test test_ptb_split_coins ... ok
test test_ptb_merge_coins ... ok
...
test result: ok. 17 passed; 0 failed; 0 ignored
```

### Step 3: Interactive Sandbox

Start the interactive sandbox to explore Move modules:

```bash
./target/release/sui_move_interface_extractor sandbox-exec --interactive
```

Then send JSON commands:

```json
{"action": "list_functions", "package_id": "0x2", "module": "coin"}
{"action": "get_function_info", "package_id": "0x2", "module": "coin", "function": "value"}
```

### What's Happening Under the Hood

1. **Load Sui framework** - The real Move bytecode from Sui's standard library
2. **Create simulation environment** - A local Move VM with configurable state
3. **Execute PTB commands** - SplitCoins, MergeCoins, MoveCall, TransferObjects
4. **Track effects** - Objects created, mutated, deleted, and events emitted

### Data Fetching

The library supports fetching data from Sui mainnet via GraphQL:

```rust
use sui_move_interface_extractor::data_fetcher::DataFetcher;

let fetcher = DataFetcher::mainnet();
let package = fetcher.fetch_package("0x2")?;  // Sui framework
let object = fetcher.fetch_object("0x6")?;    // Clock object
```

### Troubleshooting

| Issue | Solution |
|-------|----------|
| `SKIP: No cache available` | The `.tx-cache/` directory should be included in the repo |
| `gRPC connection failed` | Check network connectivity to `archive.mainnet.sui.io:443` |
| `Package version check failed` | The test uses upgraded packages with address aliasing |

For detailed technical documentation, see [DeFi Case Studies](docs/defi-case-study/README.md).

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

## Key Entry Points

### For New Users: Start with Examples

The `examples/` directory contains **self-contained, cache-free** examples - the best way to understand the system:

```bash
cargo run --example deepbook_replay   # DeepBook flash loan replay
cargo run --example cetus_swap        # Cetus AMM swap replay
cargo run --example multi_swap_flash_loan  # Flash loan arbitrage
```

Each example demonstrates the complete workflow from data fetching to local execution.

### Core Capabilities

| Capability | Module | Description |
|------------|--------|-------------|
| **Transaction Replay** | `benchmark::tx_replay` | Replay historical mainnet transactions locally |
| **PTB Execution** | `benchmark::ptb` | Execute Programmable Transaction Blocks |
| **Move VM Harness** | `benchmark::vm` | Full Move VM with Sui native functions |
| **Simulation Environment** | `benchmark::simulation` | Stateful sandbox with object tracking |
| **Data Fetching** | `data_fetcher` | Unified GraphQL + gRPC API |
| **gRPC Streaming** | `grpc` | Real-time checkpoint streaming (via Surflux) |
| **GraphQL Queries** | `graphql` | Object/package/transaction queries |

### API Quick Reference

```rust
use sui_move_interface_extractor::{
    // Core simulation
    benchmark::{SimulationEnvironment, VMHarness, PTBExecutor},

    // Data fetching
    data_fetcher::DataFetcher,
    graphql::GraphQLClient,
    grpc::GrpcClient,
};

// Fetch data from mainnet
let fetcher = DataFetcher::mainnet();
let package = fetcher.fetch_package("0x2")?;

// Create simulation environment
let mut env = SimulationEnvironment::new()?;
env.load_package(package)?;

// Execute PTB
let result = env.execute_ptb(&commands, &inputs)?;
```

## CLI Commands

| Command | Purpose |
|---------|---------|
| `sui-sandbox` | **Developer CLI** for local Move development (publish, run, PTB, fetch, replay) |
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
| **Getting Started** | [Quickstart](docs/getting-started/QUICKSTART.md) |
| **Guides** | [Transaction Replay](docs/guides/TRANSACTION_REPLAY.md) · [LLM Integration](docs/guides/LLM_INTEGRATION.md) · [Data Fetching](docs/guides/DATA_FETCHING.md) · [Local Sandbox](docs/guides/LOCAL_BYTECODE_SANDBOX.md) |
| **Reference** | [CLI Reference](docs/reference/CLI_REFERENCE.md) · [Sandbox API](docs/reference/SANDBOX_API.md) · [Error Codes](docs/reference/ERROR_CODES.md) · [PTB Schema](docs/reference/PTB_SCHEMA.md) · [JSON Schema](docs/reference/SCHEMA.md) |
| **Methodology** | [Methodology](docs/METHODOLOGY.md) · [Type Inhabitation Spec](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) |
| **Case Studies** | [Overview](docs/defi-case-study/README.md) · [Cetus AMM](docs/defi-case-study/CETUS.md) · [DeepBook CLOB](docs/defi-case-study/DEEPBOOK.md) · [Lending Protocols](docs/defi-case-study/LENDING_PROTOCOLS.md) · [Bluefin Perpetuals](docs/defi-case-study/BLUEFIN_PERPETUALS.md) |
| **Design** | [Architecture](docs/ARCHITECTURE.md) · [Contributing](docs/CONTRIBUTING.md) · [Migration](docs/MIGRATION.md) |

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
