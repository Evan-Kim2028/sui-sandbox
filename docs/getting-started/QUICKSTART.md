# Quickstart

Get the local Move execution environment running in minutes.

## Prerequisites

- Rust toolchain (1.75+)
- ~2GB disk space for dependencies
- Surflux API key (for historical transaction replay) - get one at https://surflux.dev

## Installation

```bash
# Clone and build
git clone https://github.com/anthropics/sui-move-interface-extractor.git
cd sui-move-interface-extractor
cargo build --release

# Set up API key for examples (optional but recommended)
echo "SURFLUX_API_KEY=your-api-key" > .env

# Add to PATH (optional)
export PATH="$PATH:$(pwd)/target/release"
```

## Your First Commands

### 1. Run a Self-Contained Example (Recommended)

The fastest way to see the tool in action - **no cache needed**:

```bash
# DeepBook flash loan replay (demonstrates success + failure cases)
cargo run --example deepbook_replay

# Cetus AMM swap with dynamic field fetching
cargo run --example cetus_swap
```

These examples fetch all data fresh via gRPC and demonstrate the complete replay workflow.

### 2. Replay a Mainnet Transaction

For ad-hoc transaction replay:

```bash
# Pick any transaction from https://suiscan.xyz and replay it locally
./target/release/sui_move_interface_extractor tx-replay <TRANSACTION_DIGEST>
```

This fetches the transaction, its dependencies, and replays it in the local Move VM.

### 2. Explore a Module

```bash
# List functions in the Sui coin module
echo '{"action": "list_functions", "package_id": "0x2", "module": "coin"}' | \
  ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

### 3. Execute a Simple Transaction

```bash
# Create a zero-value SUI coin
echo '{
  "action": "execute_ptb",
  "commands": [
    {"MoveCall": {
      "package": "0x2",
      "module": "coin",
      "function": "zero",
      "type_arguments": ["0x2::sui::SUI"]
    }}
  ]
}' | ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

## Interactive Mode

For ongoing work, use interactive mode:

```bash
./target/release/sui_move_interface_extractor sandbox-exec --interactive
```

Then send JSON commands one per line. The sandbox maintains state between commands.

## Common Tasks

| I want to... | Command |
|--------------|---------|
| Replay a transaction | `tx-replay <DIGEST>` |
| List available actions | `{"action": "list_available_tools"}` |
| Load a package from mainnet | `{"action": "deploy_package_from_mainnet", "address": "0x..."}` |
| Check current clock | `{"action": "get_clock"}` |
| Execute a PTB | `{"action": "execute_ptb", "commands": [...]}` |

## Troubleshooting

### "Package not found" errors

The sandbox loads the Sui framework (0x1, 0x2, 0x3) by default. For other packages:

```bash
echo '{"action": "deploy_package_from_mainnet", "address": "0x<PACKAGE_ID>"}' | \
  ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

### Build errors

```bash
cargo clean && cargo build --release
```

### protoc errors

If you see protoc-related build errors, install protobuf:

```bash
# Ubuntu/Debian
sudo apt-get install protobuf-compiler

# macOS
brew install protobuf
```

## Next Steps

- **[Examples README](../../examples/README.md)** - Start here! Self-contained replay examples
- [Transaction Replay Guide](../guides/TRANSACTION_REPLAY.md) - Deep dive into replaying mainnet transactions
- [Sandbox API Reference](../reference/SANDBOX_API.md) - All available actions
- [CLI Reference](../reference/CLI_REFERENCE.md) - Command-line options
- [Architecture](../ARCHITECTURE.md) - Workspace structure and module organization
