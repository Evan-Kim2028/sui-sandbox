# Sui Sandbox Examples

These examples demonstrate how to use the local Move VM sandbox for testing and simulating Sui transactions.

## Quick Start

**Start here if you're new!** The examples are ordered from simplest to most advanced.

### 0. CLI Workflow (No API Key, No Compilation)

```bash
# Interactive CLI walkthrough - best for quick exploration
./examples/cli_workflow.sh

# Or use the CLI directly:
sui-sandbox view module 0x2::coin
sui-sandbox run 0x2::coin::zero --type-arg 0x2::sui::SUI
```

### 1. Simple (No API Key Required)

```bash
# Coin transfer - basic PTB operations
cargo run --example coin_transfer
```

### 2. Intermediate (Requires API Key)

```bash
# Fork mainnet state into local sandbox
cargo run --example fork_state
```

### 3. Advanced (Transaction Replay)

```bash
# Replay real DeFi transactions
cargo run --example cetus_swap
cargo run --example deepbook_replay
cargo run --example multi_swap_flash_loan
cargo run --example scallop_deposit
```

## Available Examples

| Example | Difficulty | API Key | Description |
|---------|------------|---------|-------------|
| `cli_workflow.sh` | Beginner | No | Shell script demonstrating CLI usage |
| `coin_transfer` | Beginner | No | Split and transfer SUI coins locally |
| `fork_state` | Intermediate | Yes | Fork mainnet state and execute PTBs |
| `cetus_swap` | Advanced | Yes | Replay Cetus AMM swap transaction |
| `deepbook_replay` | Advanced | Yes | Replay DeepBook flash loan transactions |
| `multi_swap_flash_loan` | Advanced | Yes | Flash loan arbitrage across multiple DEXes |
| `scallop_deposit` | Advanced | Yes | Replay Scallop lending deposit |

## Setup

### For Basic Examples (No API Key)

Just run the example:

```bash
cargo run --example coin_transfer
```

### For Advanced Examples (Requires gRPC Endpoint)

1. Configure your gRPC endpoint in a `.env` file in the project root:

```bash
# Option 1: Generic configuration (recommended)
SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443
SUI_GRPC_API_KEY=your_api_key_here

# Option 2: Use Sui public endpoints (no API key needed, limited features)
SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443

# Option 3: Legacy Surflux variables (still supported)
SURFLUX_API_KEY=your_api_key_here
```

1. For Surflux, get an API key from [Surflux](https://surflux.dev)

2. Run the example:

```bash
cargo run --example fork_state
```

## Example Descriptions

### cli_workflow.sh (Beginner)

A shell script that walks through the `sui-sandbox` CLI:

- Exploring framework modules (`view modules`, `view module`)
- Checking session status
- Publishing a test package
- Executing functions
- Using JSON output for scripting
- Cleaning up session state

This is the fastest way to explore the sandbox without writing any code. Run it with:

```bash
./examples/cli_workflow.sh
```

Or use the CLI interactively:

```bash
# Build the CLI
cargo build --release --bin sui-sandbox

# Explore modules
./target/release/sui-sandbox view module 0x2::coin

# Execute a function
./target/release/sui-sandbox run 0x2::coin::zero --type-arg 0x2::sui::SUI

# When ready to deploy, generate sui client commands
./target/release/sui-sandbox bridge publish ./my_package

# See all commands
./target/release/sui-sandbox --help
```

### coin_transfer (Beginner)

Demonstrates basic PTB (Programmable Transaction Block) operations:

- Creating a simulation environment
- Creating test SUI coins
- Splitting coins using `SplitCoins` command
- Transferring coins using `TransferObjects` command

No network access required - runs entirely locally.

### fork_state (Intermediate)

Demonstrates "forking" real mainnet state:

- Connect to Surflux gRPC to fetch mainnet data
- Fetch packages (Move Stdlib, Sui Framework, DeepBook V3)
- Fetch objects (DeepBook registry and pools)
- Load state into local sandbox
- Deploy a custom Move contract
- Execute PTBs against the forked state

This is useful for:

- Testing transactions before submitting on-chain
- Debugging failed transactions
- Exploring "what-if" scenarios
- Developing against real protocol state

### Transaction Replay Examples (Advanced)

These examples demonstrate replaying real historical transactions:

- Fetch transaction data from mainnet
- Reconstruct exact historical state
- Execute locally and compare results

See each example's source code for detailed documentation.

## File Structure

```text
examples/
├── README.md                   # This file
├── cli_workflow.sh            # CLI workflow demonstration (shell script)
├── coin_transfer.rs           # Simple coin operations (no API key)
├── fork_state.rs              # Fork mainnet state (requires API key)
├── fork_state_helper/         # Custom Move contract for fork_state
│   ├── Move.toml
│   └── sources/manager.move
├── cetus_swap.rs              # Cetus AMM replay
├── deepbook_replay.rs         # DeepBook replay
├── multi_swap_flash_loan.rs   # Multi-DEX flash loan
├── scallop_deposit.rs         # Scallop lending replay
└── common/
    └── mod.rs                 # Shared utilities
```

## Example Status

| Example | Status | Notes |
|---------|--------|-------|
| `cli_workflow.sh` | ✅ Works | Shell script, no dependencies |
| `coin_transfer` | ✅ Works | No dependencies |
| `fork_state` | ✅ Works | Requires API key |
| `cetus_swap` | ✅ Works | Matches on-chain |
| `deepbook_replay` | ✅ Works | Both success and failure cases |
| `multi_swap_flash_loan` | ✅ Works | Uses GenericObjectPatcher for version-lock |
| `scallop_deposit` | ⚠️ Partial | Version-lock bypassed, BCS issue remains |

## Key Concepts

### PTB (Programmable Transaction Block)

Sui transactions are expressed as PTBs containing:

- **Inputs**: Objects and pure values
- **Commands**: Operations like `MoveCall`, `SplitCoins`, `TransferObjects`
- **Arguments**: References to inputs or previous command results

### SimulationEnvironment

The local Move VM that executes PTBs:

- Pre-loaded with Sui Framework and Move Stdlib
- Can load additional packages from mainnet
- Tracks gas usage and object mutations

### State Forking

"Forking" means taking a snapshot of on-chain state at a specific point in time and loading it locally. This allows testing against real protocol state without affecting the chain.

## Troubleshooting

### "API key / endpoint not configured"

Create a `.env` file with your gRPC configuration (see Setup section above). The examples support multiple configuration options:

- `SUI_GRPC_ENDPOINT` + `SUI_GRPC_API_KEY` (recommended)
- `SURFLUX_API_KEY` (legacy, still supported)
- No API key for public Sui endpoints (limited functionality)

### "sui CLI not found" (fork_state)

The custom contract deployment requires the `sui` CLI. Install it from https://docs.sui.io/guides/developer/getting-started/sui-install

The example will still work, just without the custom contract.

### Compilation errors

Make sure you're on a recent Rust version:

```bash
rustup update
cargo build --examples
```
