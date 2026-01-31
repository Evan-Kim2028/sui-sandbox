# Examples - Start Here

This is the best way to learn the sui-sandbox library. Work through these examples in order.

## Learning Path

### Level 1: MCP Tool + CLI Exploration (Minimal Setup)

```bash
# Run the MCP tool workflow (recommended)
./examples/mcp_cli_workflow.sh
```

This shell script walks you through the MCP tool interface (via `sui-sandbox tool`)
without any compilation. You'll learn:

- Exploring framework modules (`get_interface`)
- Creating/building/deploying a Move package
- Executing simple functions via `call_function`
- Using JSON output for scripting

**Prerequisites**: None beyond the `sui-sandbox` binary

If you want the classic CLI walkthrough that uses `view`, `publish`, and `run`,
use the legacy script below:

```bash
# First, build the test fixture (requires Sui CLI)
cd tests/fixture && sui move build && cd ../..

# Then run the classic CLI workflow
./examples/cli_workflow.sh
```

### New: CLI + MCP Example Suite (Parity with Rust examples)

For CLI equivalents of the Rust examples (with MCP tool parity and replay verification),
see:

```
./examples/cli_mcp/README.md
```

### New: Self-Heal Replay (Testing Only)

Demonstrates self-healing replay when historical data is incomplete by synthesizing
placeholder inputs and dynamic-field values:

```
./examples/self_heal/README.md
```

### New: Package Analysis (CLI)

Fetch a package, inspect modules, and attempt to run entry functions that need no arguments:

```
./examples/package_analysis/README.md
```

---

### Level 2: Basic PTB Operations

```bash
cargo run --example ptb_basics
```

Your first Rust example. Creates a local simulation environment and executes basic PTB commands:

- Creating a `SimulationEnvironment`
- Splitting coins with `SplitCoins`
- Transferring objects with `TransferObjects`

**Prerequisites**: Rust toolchain

---

### Level 3: Fork Mainnet State + Deploy Custom Contracts

```bash
cargo run --example fork_state
```

Fork real mainnet state and deploy your own contracts against it:

- Fetch real packages (DeepBook V3) and objects from mainnet
- Load them into a local SimulationEnvironment
- **Deploy your own Move contracts** into the sandbox
- Call real protocol code (DeepBook) and your custom code together

This is how you develop and test contracts that interact with real DeFi protocols
without deploying to mainnet or spending gas.

**Prerequisites**: gRPC endpoint, `sui` CLI (optional, for custom contract deployment)

---

### Level 3.5: Synthetic Object Introspection (Cetus)

```bash
CETUS_AUTO_DISCOVER=1 CETUS_WRITE_SYNTHETIC_TEMPLATE=1 \
  cargo run --example cetus_position_fees
```

Inspect Cetus position fees inside the sandbox using the same on-chain bytecode
the SDK calls via `devInspect`. This example supports **synthetic object BCS**:

1) Auto-discover a live position and print a `.env` template
2) Re-run with `CETUS_USE_SYNTHETIC=1` to execute locally from that snapshot

**Prerequisites**: gRPC endpoint (packages), GraphQL (auto-discovery)

---

### Level 4: Transaction Replay

```bash
cargo run --example cetus_swap
```

The core use case - replay a real mainnet transaction locally:

- Fetch historical transaction from gRPC
- Reconstruct exact input state (including dynamic fields)
- Execute locally and verify 1:1 mainnet parity
- Handles package upgrades via linkage tables

**Prerequisites**: API key

---

### Level 5: BigVector & Dynamic Fields

```bash
cargo run --example deepbook_orders
```

Replay DeepBook order transactions that use BigVector internally:

- **cancel_order** and **place_limit_order** transaction replay
- Dynamic field prefetching with `prefetch_dynamic_fields`
- Enhanced child fetcher with gRPC + GraphQL fallback
- Version validation against max lamport version

BigVector is Sui's scalable vector implementation that uses dynamic field slices.
These slices may not appear in `unchanged_loaded_runtime_objects`, requiring
on-demand fetching during execution.

**Prerequisites**: API key, understanding of Levels 1-4

---

### Level 6: Complex DeFi Replay

```bash
cargo run --example multi_swap_flash_loan
```

Advanced replay with complex state:

- Multi-DEX flash loan arbitrage
- Dynamic field prefetching
- Version-lock handling with `GenericObjectPatcher`

**Prerequisites**: API key, understanding of Levels 1-5

---

## All Examples

| Example | Level | API Key | Description |
|---------|-------|---------|-------------|
| `mcp_cli_workflow.sh` | 1 | No | MCP tool workflow (recommended) |
| `cli_workflow.sh` | 1 | No | Classic CLI walkthrough |
| `ptb_basics` | 2 | No | Basic PTB operations (SplitCoins, TransferObjects) |
| `fork_state` | 3 | Yes | Fork mainnet state locally |
| `cetus_position_fees` | 3.5 | Yes | Cetus position fee fetch (live + synthetic objects) |
| `cetus_swap` | 4 | Yes | Cetus AMM swap replay (canonical replay example) |
| `deepbook_replay` | 4 | Yes | DeepBook flash loan replay |
| `deepbook_orders` | 5 | Yes | DeepBook order replay (BigVector handling) |
| `multi_swap_flash_loan` | 6 | Yes | Multi-DEX arbitrage replay |

---

## Setup

### For Levels 1-2 (No API Key)

Just run the examples:

```bash
cargo run --example ptb_basics
```

### For Levels 3-6 (Requires gRPC Access)

1. Configure your gRPC endpoint in a `.env` file:

```bash
cp .env.example .env
# Edit .env with your endpoint and API key (if required by your provider)
```

Example `.env`:

```
SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on provider
```

2. Run the example:

```bash
cargo run --example fork_state
```

---

## Key Concepts

### SimulationEnvironment

The local Move VM that executes PTBs:

```rust
let mut env = SimulationEnvironment::new()?;

// Pre-loaded with Sui Framework (0x1, 0x2, 0x3)
// Can load additional packages from mainnet
// Tracks gas usage and object mutations
```

### PTB (Programmable Transaction Block)

Sui transactions are expressed as PTBs:

```rust
let commands = vec![
    Command::MoveCall { package, module, function, type_arguments, arguments },
    Command::SplitCoins { coin, amounts },
    Command::TransferObjects { objects, address },
];

let result = env.execute_ptb(inputs, commands)?;
```

### Transaction Replay

Fetch a historical transaction and re-execute it:

```rust
// 1. Fetch transaction and state
let provider = HistoricalStateProvider::mainnet().await?;
let state = provider.fetch_replay_state(&digest).await?;

// 2. Get historical versions from transaction effects
let historical_versions = get_historical_versions(&state);

// 3. Prefetch dynamic fields recursively (checkpoint snapshot when available)
let prefetched = if let Some(cp) = state.checkpoint {
    prefetch_dynamic_fields_at_checkpoint(&graphql, &grpc, &rt, &historical_versions, 3, 200, cp)
} else {
    prefetch_dynamic_fields(&graphql, &grpc, &rt, &historical_versions, 3, 200)
};

// 4. Set up child fetcher for on-demand object loading
let child_fetcher = create_enhanced_child_fetcher_with_cache(
    grpc, graphql, historical_versions, prefetched, patcher, state.checkpoint, cache
);
harness.set_child_fetcher(child_fetcher);

// 5. Execute locally and compare
let result = replay_with_objects_and_aliases(&transaction, &mut harness, &objects, &aliases)?;
assert!(result.local_success);
```

### BigVector Handling

Some protocols (like DeepBook) use BigVector internally. BigVector slices may not appear
in `unchanged_loaded_runtime_objects`. Handle this with:

```rust
// 1. Prefetch discovers children via GraphQL (checkpoint snapshot when available)
let prefetched = if let Some(cp) = state.checkpoint {
    prefetch_dynamic_fields_at_checkpoint(&graphql, &grpc, &rt, &versions, 3, 200, cp)
} else {
    prefetch_dynamic_fields(&graphql, &grpc, &rt, &versions, 3, 200)
};

// 2. Enhanced child fetcher validates versions
// If object.version <= max_lamport_version, it's safe to use
let child_fetcher = create_enhanced_child_fetcher_with_cache(...);
```

---

## Troubleshooting

### "API key not configured" or connection errors

Create a `.env` file with your gRPC configuration:

```bash
cp .env.example .env
# Edit .env with your endpoint and API key
```

### Build errors

```bash
# Clean and rebuild
cargo clean && cargo build --examples

# If protoc errors, install protobuf:
# Ubuntu: sudo apt-get install protobuf-compiler
# macOS: brew install protobuf
```

### "sui CLI not found" (fork_state only)

The custom contract deployment in `fork_state` requires the Sui CLI. Install from <https://docs.sui.io/guides/developer/getting-started/sui-install>

The example still runs without it - just skips the custom contract part.

---

## Next Steps

After completing these examples:

- **[Transaction Replay Guide](../docs/guides/TRANSACTION_REPLAY.md)** - Deep dive into replay mechanics
- **[Architecture](../docs/ARCHITECTURE.md)** - Understand system internals
- **[Limitations](../docs/reference/LIMITATIONS.md)** - Known differences from mainnet
