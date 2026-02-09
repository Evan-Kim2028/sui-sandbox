# Examples - Start Here

This is the best way to learn the sui-sandbox library. Work through these examples in order.

## Learning Path

### Level 0: Transaction Replay

```bash
./examples/replay.sh
```

Replay a real mainnet transaction locally. Supports multiple data sources:

```bash
./examples/replay.sh                                        # Walrus (default, zero setup)
./examples/replay.sh --source walrus <DIGEST> <CHECKPOINT>  # Walrus with custom tx
./examples/replay.sh --source walrus '*' 100..200           # Walrus range (all txs in range)
./examples/replay.sh --source grpc <DIGEST>                 # gRPC (needs SUI_GRPC_ENDPOINT)
./examples/replay.sh --source json <STATE_FILE>             # JSON state file (any data source)
```

- **Walrus**: Zero authentication, fetches everything from decentralized checkpoint storage. Supports checkpoint ranges (`100..200`) and lists (`100,105,110`).
- **gRPC**: Standard fullnode/archive endpoint (requires `SUI_GRPC_ENDPOINT`)
- **JSON**: Load replay state from a JSON file. Bring your own data from any source.

Export state from any source, then replay offline:

```bash
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json
sui-sandbox replay <DIGEST> --state-json state.json
```

Or use the CLI directly:

```bash
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CHECKPOINT> --compare
sui-sandbox replay <DIGEST> --source grpc --compare
```

**Prerequisites**: None for Walrus or JSON. gRPC requires endpoint configuration.

---

### Level 0.5: Scan Latest Checkpoints

```bash
./examples/scan_checkpoints.sh       # Latest 5 checkpoints (default)
./examples/scan_checkpoints.sh 10    # Latest 10 checkpoints
```

Automatically discovers the tip checkpoint from Walrus, fetches the latest N checkpoints
in a single batched request, replays every PTB transaction, and prints a summary:

```
━━━ Batch Replay Summary ━━━
  Checkpoints scanned: 5
  Total transactions:  115 (89 PTBs, 26 system)
  Replayed:            89
  Succeeded:           82 (92.1%)
  Failed:              7

  By tag:
    app_call                 replayed=67 ok=62 fail=5
    framework_only           replayed=22 ok=20 fail=2
    shared                   replayed=45 ok=42 fail=3
━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

Or use the CLI directly:

```bash
sui-sandbox replay "*" --source walrus --latest 5 --compare
```

**Prerequisites**: None. Zero setup, no API keys.

---

### Level 1: CLI Exploration (Minimal Setup)

```bash
# Run the CLI workflow (recommended)
./examples/cli_workflow.sh
```

This shell script walks you through the CLI interface
without any additional setup. You'll learn:

- Exploring framework modules (`view`)
- Publishing a local Move package
- Executing simple functions via `run`
- Using JSON output for scripting

**Prerequisites**: None beyond the `sui-sandbox` binary.

### Self-Heal Replay (Testing Only)

Demonstrates self-healing replay when historical data is incomplete by synthesizing
placeholder inputs and dynamic-field values:

```
./examples/self_heal/README.md
```

### Package Analysis (CLI)

Includes:

- Single package analysis and no-arg entry execution attempts
- Corpus object classification via `analyze objects`
- Corpus MM2 sweep via `analyze package --bytecode-dir ... --mm2`

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

## All Examples

| Example | Level | API Key | Description |
|---------|-------|---------|-------------|
| `replay.sh` | 0 | **No** | Transaction replay (walrus/grpc/json) |
| `scan_checkpoints.sh` | 0.5 | **No** | Scan & replay latest N checkpoints with summary |
| `cli_workflow.sh` | 1 | No | CLI walkthrough |
| `package_analysis/cli_corpus_objects_analysis.sh` | 1 | No | Corpus-wide `analyze objects` summary + baseline deltas |
| `package_analysis/cli_mm2_corpus_sweep.sh` | 1 | No | Corpus MM2 regression sweep (`analyze package --mm2`) |
| `convertible_simulator` | 1 | No | Convertible vs ETH vs stable APY simulator |
| `ptb_basics` | 2 | No | Basic PTB operations (SplitCoins, TransferObjects) |

## CLI-First Replacements

Several older or experimental examples were consolidated into CLI flows:

- Protocol/package analysis → `sui-sandbox analyze package --package-id 0x...`
- Historical replay demos → `sui-sandbox replay <DIGEST> --compare`
- Walrus cache warmup → `sui-sandbox tools walrus-warmup`
- Walrus package ingest → `sui-sandbox fetch checkpoints <START> <END>`

---

## Advanced Examples (Require gRPC API Key)

The following Rust examples demonstrate advanced replay internals. They require a gRPC
archive endpoint and API key. Most users will not need these — the Walrus-based CLI
workflows above cover all common replay scenarios without any authentication.

### Setup

```bash
cp .env.example .env
# Edit .env with your endpoint and API key
```

Example `.env`:

```
SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on provider
```

### Rust Examples

| Example | Description |
|---------|-------------|
| `fork_state` | Fork mainnet state + deploy custom contracts against real DeFi protocols |
| `cetus_position_fees` | Synthetic object BCS introspection for Cetus fees |
| `cetus_swap` | Full Cetus AMM swap replay with package upgrade handling |
| `deepbook_replay` | DeepBook flash loan replay |
| `deepbook_orders` | BigVector & dynamic field replay (cancel/place limit orders) |
| `multi_swap_flash_loan` | Multi-DEX flash loan arbitrage with complex state |

```bash
cargo run --example fork_state
cargo run --example cetus_swap
cargo run --example deepbook_orders
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
