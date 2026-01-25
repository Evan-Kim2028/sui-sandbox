# sui-sandbox CLI Reference

The `sui-sandbox` CLI is the primary interface for local Move/Sui development.

## Quick Reference

```bash
# Build the CLI
cargo build --release --bin sui-sandbox

# Core workflow
sui-sandbox fetch package 0x1eabed72...      # Import mainnet package
sui-sandbox publish ./my_package              # Deploy locally
sui-sandbox run 0x100::module::function       # Execute function
sui-sandbox replay 9V3xKMnFpXyz...            # Replay mainnet tx
sui-sandbox view module 0x2::coin             # Inspect interface
sui-sandbox bridge publish ./my_package       # Generate real deploy command
```

## Commands

| Command | Description |
|---------|-------------|
| `publish` | Deploy Move packages to local sandbox |
| `run` | Execute a single Move function call |
| `ptb` | Execute Programmable Transaction Blocks |
| `fetch` | Import packages/objects from mainnet |
| `replay` | Replay historical mainnet transactions |
| `view` | Inspect modules, objects, packages |
| `bridge` | Generate `sui client` commands for real deployment |
| `status` | Show session state |
| `clean` | Reset session |

---

## Developer CLI (`sui-sandbox`)

The `sui-sandbox` binary is a developer-focused CLI for local Move/Sui development. It provides an ergonomic interface for publishing packages, executing functions, fetching mainnet state, and replaying transactions—all with persistent session state.

### Installation

```bash
cargo build --release --bin sui-sandbox
# Binary available at: ./target/release/sui-sandbox
```

### Global Options

| Flag | Description | Default |
|------|-------------|---------|
| `--state-file <PATH>` | Session state persistence file | `.sui-sandbox/state.bin` |
| `--rpc-url <URL>` | RPC URL for mainnet fetching | `https://fullnode.mainnet.sui.io:443` |
| `--json` | Output as JSON instead of human-readable | `false` |
| `-v, --verbose` | Show execution traces | `false` |

### Commands

#### `publish` - Deploy Move Packages

Compile and publish Move packages to the local sandbox.

```bash
# Publish from pre-compiled bytecode
sui-sandbox publish ./my_package --bytecode-only --address my_pkg=0x100

# Compile with sui move build (requires sui CLI)
sui-sandbox publish ./my_package --address my_pkg=0x100

# Assign specific address to published package
sui-sandbox publish ./my_package --bytecode-only --assign-address 0xCAFE
```

| Flag | Description |
|------|-------------|
| `--bytecode-only` | Skip compilation, use existing `bytecode_modules/` |
| `--address <NAME=ADDR>` | Named address assignments (repeatable) |
| `--assign-address <ADDR>` | Assign package to specific address |
| `--dry-run` | Don't persist to session state |

#### `run` - Execute Functions

Execute a single Move function call.

```bash
# Call a function with arguments
sui-sandbox run 0x2::coin::value --arg 0x123

# With type arguments
sui-sandbox run 0x2::coin::zero --type-arg 0x2::sui::SUI

# With custom sender and gas budget
sui-sandbox run 0x100::counter::increment --sender 0xABC --gas-budget 10000000
```

| Flag | Description | Default |
|------|-------------|---------|
| `--arg <VALUE>` | Arguments (auto-parsed: `42`, `true`, `0xABC`, `"string"`) | - |
| `--type-arg <TYPE>` | Type arguments (e.g., `0x2::sui::SUI`) | - |
| `--sender <ADDR>` | Sender address | `0x0` |
| `--gas-budget <N>` | Gas budget (0 = unlimited) | `0` |

**Argument Parsing:**

Arguments are auto-detected by format:

- Numbers: `42`, `100u64`
- Booleans: `true`, `false`
- Addresses: `0xABC...`
- Strings: `"hello"`
- Bytes: `b"data"`

#### `ptb` - Execute PTBs

Execute Programmable Transaction Blocks from JSON specifications.

```bash
# Execute from JSON file
sui-sandbox ptb --spec transaction.json --sender 0xABC

# With gas budget
sui-sandbox ptb --spec tx.json --sender 0x1 --gas-budget 10000000
```

| Flag | Description |
|------|-------------|
| `--spec <PATH>` | Path to PTB JSON specification |
| `--sender <ADDR>` | Transaction sender address |
| `--gas-budget <N>` | Gas budget |

**PTB JSON Schema:**

```json
{
  "calls": [
    {
      "target": "0x2::coin::zero",
      "type_arguments": ["0x2::sui::SUI"],
      "arguments": []
    },
    {
      "target": "0x100::my_module::process",
      "arguments": [{"Result": 0}]
    }
  ],
  "inputs": [
    {"Pure": {"type": "u64", "value": 1000}}
  ]
}
```

#### `fetch` - Import Mainnet State

Fetch packages and objects from Sui mainnet into your local session.

```bash
# Fetch a package
sui-sandbox fetch package 0x1eabed72c53feb73726a1bde7d5cce9c4c2fd8dc3a8b9b1234567890abcdef

# Fetch an object
sui-sandbox fetch object 0x6  # Clock object

# Verbose output with module details
sui-sandbox fetch package 0x2 --verbose
```

| Subcommand | Description |
|------------|-------------|
| `package <ID>` | Fetch and load a package with all modules |
| `object <ID>` | Fetch an object's current state |

#### `replay` - Transaction Replay

Replay historical mainnet transactions locally with optional effects comparison.

```bash
# Replay a transaction
sui-sandbox replay 9V3xKMnFpXyz...

# Compare local execution with on-chain effects
sui-sandbox replay 9V3xKMnFpXyz... --compare

# Verbose output showing execution details
sui-sandbox replay 9V3xKMnFpXyz... --compare --verbose
```

| Flag | Description |
|------|-------------|
| `--compare` | Compare local effects with on-chain effects |

#### `view` - Inspect State

View modules, objects, and packages in your session.

```bash
# View a module's interface
sui-sandbox view module 0x2::coin

# List all modules in a package
sui-sandbox view modules 0x2

# View an object
sui-sandbox view object 0x123

# List all loaded packages
sui-sandbox view packages
```

| Subcommand | Description |
|------------|-------------|
| `module <PATH>` | Show module interface (structs, functions) |
| `modules <PACKAGE>` | List all modules in a package |
| `object <ID>` | Show object details |
| `packages` | List all loaded user packages |

#### `status` - Session Status

Show current session state including loaded packages and configuration.

```bash
sui-sandbox status
sui-sandbox status --json
```

#### `clean` - Reset Session

Remove the session state file to start fresh.

```bash
sui-sandbox clean
```

#### `bridge` - Transition to Sui Client

Generate `sui client` commands for deploying to real networks (testnet/mainnet). This is a helper for transitioning out of the sandbox - it generates the commands, you run them.

```bash
# Generate publish command
sui-sandbox bridge publish ./my_package
# Output: sui client publish ./my_package --gas-budget 100000000

# Generate call command
sui-sandbox bridge call 0x2::coin::zero --type-arg 0x2::sui::SUI
# Output: sui client call --package 0x2 --module coin --function zero ...

# Generate PTB command from sandbox spec
sui-sandbox bridge ptb --spec my_transaction.json
# Output: sui client ptb --move-call ... --gas-budget 10000000
```

**Subcommands:**

| Subcommand | Description |
|------------|-------------|
| `publish <PATH>` | Generate `sui client publish` command |
| `call <TARGET>` | Generate `sui client call` command |
| `ptb --spec <FILE>` | Convert sandbox PTB spec to `sui client ptb` command |
| `info` | Show transition workflow and deployment guide |

**Options:**

| Flag | Description | Default |
|------|-------------|---------|
| `--gas-budget <MIST>` | Gas budget for the transaction | 100M (publish), 10M (call/ptb) |
| `--quiet` | Skip prerequisites and notes, show only command | `false` |
| `-v, --verbose` | Show advanced info (protocol version, error handling tips) | `false` |

**Example Workflow:**

```bash
# 1. Test in sandbox
sui-sandbox publish ./my_package --bytecode-only
sui-sandbox run 0x100::my_module::init

# 2. When ready, generate deployment command
sui-sandbox bridge publish ./my_package

# 3. Follow the output instructions
sui client switch --env testnet
sui client publish ./my_package --gas-budget 100000000
```

**Transition Guide:**

```bash
# Get the full transition workflow
sui-sandbox bridge info

# Get verbose info including protocol version and error handling
sui-sandbox bridge info --verbose
```

**Notes:**

- The bridge detects sandbox addresses (like `0x100`, `0xdeadbeef`) and warns you to replace them
- Generated commands include prerequisites (network switch, faucet)
- Use `--json` for machine-readable output
- This is a helper, not a replacement for `sui client`
- Use `bridge info --verbose` to see protocol version and common abort codes

### Session Persistence

The `sui-sandbox` maintains session state across commands:

- **Loaded packages** remain available between commands
- **Fetched objects** are cached locally
- **Last sender** is remembered for convenience

State is stored in `.sui-sandbox/state.bin` by default. Use `--state-file` to customize.

### Workflow Example

A typical development workflow:

```bash
# 1. Fetch a protocol from mainnet
sui-sandbox fetch package 0x1eabed72c53feb73...  # Cetus CLMM

# 2. Publish your package that interacts with it
sui-sandbox publish ./my_strategy --bytecode-only --address my_strategy=0x200

# 3. Test your functions
sui-sandbox run 0x200::strategy::calculate_swap \
  --arg 1000000 \
  --type-arg 0x2::sui::SUI

# 4. Execute a full PTB
sui-sandbox ptb --spec my_swap.json --sender 0xABC

# 5. View what happened
sui-sandbox view packages
sui-sandbox status
```

### JSON Output Mode

All commands support `--json` for machine-readable output:

```bash
# Get module info as JSON
sui-sandbox --json view module 0x2::coin

# Publish and capture result
result=$(sui-sandbox --json publish ./my_pkg --bytecode-only)
package_addr=$(echo "$result" | jq -r '.package_address')
```

### Error Handling

Errors are reported with context:

```
Error: Function 0x2::coin::nonexistent not found

  Module 0x2::coin exists but has no function 'nonexistent'.
  Available public functions:
    - value
    - zero
    - split
    - ...
```

With `--json`, errors include structured information:

```json
{
  "error": "FunctionNotFound",
  "module": "0x2::coin",
  "function": "nonexistent",
  "available": ["value", "zero", "split"]
}
```

---

## See Also

- [Architecture](../ARCHITECTURE.md) - System internals
- [Transaction Replay Guide](../guides/TRANSACTION_REPLAY.md) - Detailed replay workflow
- [Limitations](LIMITATIONS.md) - Known differences from mainnet
- [Examples](../../examples/README.md) - Working code examples
