# sui-sandbox CLI Reference

The `sui-sandbox` CLI is the primary interface for local Move/Sui development.
For MCP server usage and tool input details, see **[MCP Reference](MCP_REFERENCE.md)**.

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

# MCP tool workflow (JSON in/out)
sui-sandbox tool get_interface --input '{"package":"0x2","module":"coin"}'
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
| `tool` | Invoke MCP tools directly (JSON in/out) |
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
| `--state-file <PATH>` | Session state persistence file | `~/.sui-sandbox/state.bin` (legacy); `~/.sui-sandbox/mcp-state.json` for `tool` |
| `--rpc-url <URL>` | RPC URL for mainnet fetching | `https://fullnode.mainnet.sui.io:443` |
| `--json` | Output as JSON instead of human-readable | `false` |
| `-v, --verbose` | Show execution traces | `false` |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `SUI_GRPC_API_KEY` | API key for authenticated gRPC endpoints (used with `--rpc-url`) |
| `SUI_SANDBOX_HOME` | Override sandbox home for MCP cache/projects/logs (default: `~/.sui-sandbox`) |
| `SUI_GRAPHQL_ENDPOINT` | Default GraphQL endpoint for MCP tools and legacy `fetch`/`replay` (overridden by `--graphql-url`) |
| `SUI_DEBUG_LINKAGE` | Set to `1` to log package linkage/version resolution during replay and fetch |
| `SUI_DEBUG_MUTATIONS` | Set to `1` to log mutation tracking details during replay |
| `SUI_WALRUS_ENABLED` | Set to `true` to enable Walrus checkpoint hydration (input/output objects) |
| `SUI_WALRUS_CACHE_URL` | Walrus caching server base URL (checkpoint metadata) |
| `SUI_WALRUS_AGGREGATOR_URL` | Walrus aggregator base URL (checkpoint blobs) |
| `SUI_WALRUS_NETWORK` | `mainnet` or `testnet` (used for Walrus defaults) |
| `SUI_WALRUS_TIMEOUT_SECS` | Walrus fetch timeout in seconds (default 10) |
| `SUI_WALRUS_LOCAL_STORE` | Enable local filesystem object store for Walrus checkpoints |
| `SUI_WALRUS_STORE_DIR` | Override local store path (defaults to `$SUI_SANDBOX_HOME/walrus-store/<network>`) |
| `SUI_WALRUS_FULL_CHECKPOINT_INGEST` | Ingest all objects in a checkpoint into the local store (default: true when local store enabled) |
| `SUI_WALRUS_RECURSIVE_LOOKUP` | Use local index to hydrate missing objects from Walrus checkpoints (default: true when local store enabled) |
| `SUI_WALRUS_RECURSIVE_MAX_CHECKPOINTS` | Max checkpoints to pull per replay when doing recursive lookup (default 5) |

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
| `--gas-budget <N>` | Gas budget (0 = default metered budget) | `0` |

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
      "type_args": ["0x2::sui::SUI"],
      "args": []
    },
    {
      "target": "0x100::my_module::process",
      "args": [{"result": 0}]
    }
  ],
  "inputs": [
    {"u64": 1000},
    {"shared_object": {"id": "0x6", "mutable": false}},
    {"imm_or_owned_object": "0x123"}
  ]
}
```

#### `fetch` - Import Mainnet State

Fetch packages and objects from Sui mainnet into your local session.

By default this command uses the GraphQL endpoint inferred from `--rpc-url`. Override it with
`SUI_GRAPHQL_ENDPOINT` if you are using a non-standard network or proxy.

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

Replay uses the GraphQL endpoint inferred from `--rpc-url` unless `SUI_GRAPHQL_ENDPOINT` is set.

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
| `--fetch-strategy` | Dynamic field strategy: `eager` (only accessed) or `full` (prefetch) |
| `--prefetch-depth` | Max dynamic field discovery depth (default: 3) |
| `--prefetch-limit` | Max children per parent when prefetching (default: 200) |
| `--auto-system-objects` | Auto-inject Clock/Random system objects when missing |
| `--reconcile-dynamic-fields` | Reconcile dynamic-field effects when on-chain lists omit them |
| `--synthesize-missing` | If replay fails due to missing input objects, synthesize placeholders and retry |
| `--self-heal-dynamic-fields` | Synthesize placeholder dynamic-field values when data is missing (testing only) |

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

---

#### `tool` - MCP Tools (JSON in/out)

Invoke MCP tools directly from the CLI. This is the recommended way to exercise
the MCP server surface without running a separate server process.

Tool-specific options:

| Flag | Description |
|------|-------------|
| `--network <NAME>` | Network name (e.g., mainnet, testnet). Persists across tool runs. |
| `--graphql-url <URL>` | GraphQL endpoint (defaults to `SUI_GRAPHQL_ENDPOINT` or network default). |

```bash
# Inspect a module interface
sui-sandbox tool get_interface --input '{"package":"0x2","module":"coin"}'

# Create a Move project (persisted)
sui-sandbox tool create_move_project --input '{"name":"demo_pkg","persist":true}'

# Build and deploy
sui-sandbox tool build_project --input '{"project_id":"<id>"}'
sui-sandbox tool deploy_project --input '{"project_id":"<id>"}'

# Upgrade a project locally (records upgrade in project registry)
sui-sandbox tool upgrade_project --input '{"project_id":"<id>"}'

# Execute a function
sui-sandbox tool call_function --input '{"package":"0x2","module":"coin","function":"zero","type_args":["0x2::sui::SUI"],"args":[]}'

# Use object refs returned from previous calls
sui-sandbox tool call_function --input '{"package":"0x2","module":"coin","function":"value","args":[{"object_ref":"obj_1"}]}'
```

**Notes:**

- The tool command uses a separate state file by default: `~/.sui-sandbox/mcp-state.json`
- Use `--state-file` to override the state location (shared across tool invocations)
- If `SUI_SANDBOX_HOME` is set, default state files live under that directory
- MCP tool logs are written as JSONL under `$SUI_SANDBOX_HOME/logs/mcp`
- Mainnet fetch/replay uses a shared cache under `$SUI_SANDBOX_HOME/cache/<network>`
- Execution outputs include `object_ref` handles inside `effects.object_changes` for easy chaining.
- MCP option enums (validated): `fetch_strategy` = `eager|full`, `cache_policy` = `default|bypass`
- Inputs may include an optional `_meta` block for LLM reasoning/logging:

```json
{
  "_meta": { "reason": "Inspect interface to build PTB", "request_id": "req-123" },
  "package": "0x2",
  "module": "coin"
}
```

#### Running the MCP Server (stdio)

Use this when you want an MCP client (Claude/GPT/etc.) to call tools directly:

```bash
cargo build --release --bin sui-sandbox-mcp
./target/release/sui-sandbox-mcp
```

The server uses the same `SUI_SANDBOX_HOME` directories for cache, projects,
and logs. State is kept in memory per server process, while projects and logs
are persisted on disk.

### Session Persistence

The `sui-sandbox` maintains session state across commands:

- **Loaded packages** remain available between commands
- **Fetched objects** are cached locally
- **Last sender** is remembered for convenience

State is stored in `~/.sui-sandbox/state.bin` by default for legacy commands. The `tool`
command uses `~/.sui-sandbox/mcp-state.json` unless overridden with `--state-file`.

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
