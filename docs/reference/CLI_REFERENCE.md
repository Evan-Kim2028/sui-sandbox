# sui-sandbox CLI Reference

The `sui-sandbox` CLI is the primary interface for local Move/Sui development.
For a full docs map, see **[docs/README.md](../README.md)**.

## Quick Reference

```bash
# Build the CLI
cargo build --release --bin sui-sandbox

# Core workflow
sui-sandbox fetch package 0x1eabed72...      # Import mainnet package
sui-sandbox publish ./my_package              # Deploy locally
sui-sandbox run 0x100::module::function       # Execute function
sui-sandbox replay 9V3xKMnFpXyz...            # Replay mainnet tx
sui-sandbox analyze package --package-id 0x2  # Package introspection
sui-sandbox analyze replay 9V3xKMnFpXyz...     # Replay-state introspection
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
| `analyze` | Package and replay-state introspection |
| `view` | Inspect modules, objects, packages |
| `bridge` | Generate `sui client` commands for real deployment |
| `tools` | Utility commands (poll/stream/tx-sim/walrus) |
| `init` | Scaffold task-oriented workflow templates |
| `run-flow` | Execute deterministic YAML workflow files |
| `snapshot` | Save/list/load/delete named session snapshots |
| `reset` | Reset in-memory session state |
| `status` | Show session state |
| `clean` | Remove session state file |

---

### Replay + Analyze Contract

`sui-sandbox replay` and `sui-sandbox analyze replay` share the same hydration flags (`--source`, `--allow-fallback`, `--auto-system-objects`, dynamic-field prefetch flags). This is intentional: you can inspect hydration behavior in `analyze replay` and execute with the same settings in `replay` without re-learning a second flag model.

Use `analyze replay` first when debugging data-quality or dependency issues, then run `replay` with the same hydration settings to validate end-to-end execution.

---

### `tools` - Utility Commands

Low-level utilities are grouped under `sui-sandbox tools`:

```bash
# Poll recent transactions via GraphQL
sui-sandbox tools poll-transactions --duration 300 --interval-ms 1000

# Stream checkpoints via gRPC
sui-sandbox tools stream-transactions --endpoint https://your-provider:9000

# Simulate a PTB via gRPC
sui-sandbox tools tx-sim --ptb-spec tx.json --sender 0x...

# PTB replay harness (Walrus feature)
sui-sandbox tools ptb-replay-harness --count 20 --max-total 100

# Warm local Walrus store (Walrus feature)
sui-sandbox tools walrus-warmup --count 50
```

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
| `--state-file <PATH>` | Session state persistence file | `~/.sui-sandbox/state.json` |
| `--rpc-url <URL>` | RPC URL for mainnet fetching | `https://fullnode.mainnet.sui.io:443` |
| `--json` | Output as JSON instead of human-readable | `false` |
| `--debug-json` | Emit structured debug diagnostics on failures | `false` |
| `-v, --verbose` | Show execution traces | `false` |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `SUI_GRPC_API_KEY` | API key for authenticated gRPC endpoints (used with `--rpc-url`) |
| `SUI_SANDBOX_HOME` | Override sandbox home for cache/logs (default: `~/.sui-sandbox`) |
| `SUI_GRAPHQL_ENDPOINT` | Default GraphQL endpoint for `fetch`/`replay` (overridden by `--graphql-url`) |
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

The CLI accepts the compact `calls` spec. See `docs/reference/PTB_SCHEMA.md`.

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
On success, replay prints **PTB-style effects** (created/mutated/deleted/events/return values).
On failure, it prints the error context when available.

**Walrus replay (recommended — zero setup):**

```bash
# Replay a specific transaction from Walrus (no API key needed)
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare

# Scan the latest 5 checkpoints (auto-discovers tip, prints summary)
sui-sandbox replay '*' --source walrus --latest 5 --compare

# Replay ALL transactions in a checkpoint range
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926

# Replay specific checkpoints (comma-separated)
sui-sandbox replay '*' --source walrus --checkpoint 239615920,239615923

# Multi-digest replay
sui-sandbox replay "digest1,digest2,digest3" --source walrus --checkpoint 239615926

# Export state for offline replay
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json

# Replay from exported JSON (completely offline, no network)
sui-sandbox replay <DIGEST> --state-json state.json
```

**gRPC replay (requires endpoint configuration):**

```bash
sui-sandbox replay <DIGEST> --source grpc --compare
sui-sandbox replay <DIGEST> --source grpc --compare --verbose
sui-sandbox replay <DIGEST> --vm-only    # Deterministic VM-only mode
```

**Data source flags:**

| Flag | Description |
|------|-------------|
| `--source <grpc\|walrus\|hybrid>` | Replay hydration source (default: `hybrid`) |
| `--checkpoint <SPEC>` | Walrus checkpoint: single (`239615926`), range (`100..200`), or list (`100,105,110`) |
| `--latest <N>` | Auto-discover tip and replay the latest N checkpoints (max 100) |
| `--state-json <PATH>` | Load replay state from a JSON file (no network needed) |
| `--export-state <PATH>` | Export fetched replay state as JSON before executing |

**Behavior flags:**

| Flag | Description |
|------|-------------|
| `--compare` | Compare local effects with on-chain effects |
| `--allow-fallback` / `--fallback` | Allow fallback to secondary data sources |
| `--vm-only` | Disable fallback and force direct VM replay path |
| `--fetch-strategy` | Dynamic field strategy: `eager` (only accessed) or `full` (prefetch) |
| `--prefetch-depth` | Max dynamic field discovery depth (default: 3) |
| `--prefetch-limit` | Max children per parent when prefetching (default: 200) |
| `--no-prefetch` | Disable dynamic field prefetch regardless of fetch strategy |
| `--auto-system-objects <true\|false>` | Auto-inject Clock/Random system objects when missing |
| `--reconcile-dynamic-fields` | Reconcile dynamic-field effects when on-chain lists omit them |
| `--synthesize-missing` | If replay fails due to missing input objects, synthesize placeholders and retry |
| `--self-heal-dynamic-fields` | Synthesize placeholder dynamic-field values when data is missing (testing only) |

**Igloo loader (feature-gated):**

When built with `--features igloo`, replay exposes an extra loader flag:

| Flag | Description |
|------|-------------|
| `--igloo-hybrid-loader` | Use igloo-mcp/Snowflake for tx/effects/packages and gRPC for objects |
| `--igloo-config <PATH>` | Path to igloo mcp service config |
| `--igloo-command <CMD>` | Override igloo executable path |

Note: `--source hybrid` and `--igloo-hybrid-loader` are different knobs.
- `--source hybrid` selects the core replay hydration source mode.
- `--igloo-hybrid-loader` switches to the optional igloo-specific loader path.

Replay output includes an **Execution Path** summary (requested/effective source, fallback usage, auto-system-object flag, dependency mode, and prefetch settings) in both human and JSON modes.

**Digest format:**

- Single digest: `At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2`
- Wildcard: `'*'` — replay all transactions in the specified checkpoint(s)

#### `analyze` - Package + Replay Readiness

Analyze is a Tier-1 tool that helps you understand **what you can execute** and
**what’s missing when replay fails**.

```bash
# Package structure + MM2 model (package id)
sui-sandbox analyze package --package-id 0x2 --list-modules --mm2

# Package structure + MM2 model (local bytecode dir)
sui-sandbox analyze package --bytecode-dir /path/to/pkg_dir --mm2

# Replay readiness: inputs, packages, and suggestions
sui-sandbox analyze replay 9V3xKMnFpXyz...

# Replay readiness with explicit fallback toggle
sui-sandbox analyze replay 9V3xKMnFpXyz... --allow-fallback false

# Corpus object classification (ownership/singleton/dynamic field heuristics)
sui-sandbox analyze objects --corpus-dir /path/to/sui-packages/packages/mainnet_most_used --top 20

# Use a built-in analysis profile
sui-sandbox analyze objects --corpus-dir /path/to/corpus --profile strict

# Use a custom profile file
sui-sandbox analyze objects --corpus-dir /path/to/corpus --profile-file ./profiles/team.yaml
```

Replay analysis outputs:
- Input summary (owned/shared/immutable)
- Command list (MoveCalls + PTB structure)
- Missing inputs / packages (if any)
- Suggested fixes (e.g., enable Walrus, use --synthesize, enable MM2)
- Hydration flag state (for example `--allow-fallback` and `--auto-system-objects`)

Objects analysis outputs:
- `object_types_discovered` and `object_types_unique`
- `profile` provenance (name/source/path + effective semantic/dynamic settings)
- Ownership counts (occurrence-weighted and unique-type)
- Party split: `party_transfer_eligible` (`key+store`) vs `party_transfer_observed_in_bytecode` (party-transfer call evidence in package bytecode)
- Singleton and dynamic-field counts
- Rare-mode examples (immutable, party, receive)

Objects profile resolution:
- Built-in profiles: `broad`, `strict`, `hybrid`
- Custom named profiles are searched in:
  - repo-local: `.sui-sandbox/analyze/profiles/<name>.yaml`
  - user-global: `${XDG_CONFIG_HOME:-~/.config}/sui-sandbox/analyze/profiles/<name>.yaml`
- `--profile-file` bypasses lookup and loads that file directly.
- CLI overrides:
  - `--semantic-mode <broad|strict|hybrid>`
  - `--dynamic-lookback <N>`

Analyze command UX notes:
- Subcommand aliases: `package|pkg`, `replay|tx`, `objects|objs|corpus`.
- `analyze package` requires exactly one source: `--package-id` or `--bytecode-dir`.
- `analyze replay` exposes explicit boolean controls:
  - `--allow-fallback <true|false>` (alias: `--fallback`)
  - `--auto-system-objects <true|false>`

Custom profile YAML shape:
```yaml
name: my-team
extends: strict
dynamic:
  lookback: 30
  include_wrapper_apis: true
```

Switching behavior:
- One-off run: pass `--profile <name>` or `--profile-file <path>`.
- Inspect effective settings via JSON `profile` output (`name`, `source`, `path`, and resolved dynamic knobs).

Party split interpretation notes:
- `party_transfer_eligible` is ability-based (`key + store`) and approximates transfer eligibility at the type level.
- `party_transfer_observed_in_bytecode` is call-site-based and tracks whether package bytecode appears to invoke party-transfer helpers.
- A large eligibility/observed gap usually means latent capability: the type can be party-transferred, but package Move code does not commonly perform it directly.
- `observed_in_bytecode = 0` is not proof of impossibility; PTB-level usage outside package bytecode can still party-transfer `key + store` objects.

Dynamic field interpretation notes:
- `dynamic_field_types` is semantic/call-site based: it tracks object types with nearby UID-borrow flow into `0x2::dynamic_field` / `0x2::dynamic_object_field` API calls.
- This is conservative for wrapper-style usage (for example table/bag helpers) and may undercount those patterns.

For runnable corpus workflows, see `examples/package_analysis/README.md`.

Quick workflow:
1. `analyze package` for a single package/module debugging pass.
2. `analyze objects` on corpus for baseline and trend diffs.
3. Use `party_transfer_eligible` vs `party_transfer_observed_in_bytecode` to spot latent capabilities not exercised in package code.
4. Run MM2 corpus sweep (`examples/package_analysis/cli_mm2_corpus_sweep.sh`) as a reliability check.

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

Status includes package/object/module counts, dynamic field count, sender, rpc URL, and current state file path.

#### `snapshot` - Snapshot Lifecycle

Save, list, load, and delete named snapshots of local session state.

```bash
# Save current session state
sui-sandbox snapshot save baseline

# List snapshots
sui-sandbox snapshot list

# Restore a snapshot
sui-sandbox snapshot load baseline

# Delete a snapshot
sui-sandbox snapshot delete baseline
```

#### `reset` - Reset Session In-Memory

Reset current in-memory session while keeping CLI configuration defaults.

```bash
sui-sandbox reset
```

Use `clean` when you specifically want to remove the state file from disk.

#### `init` - Scaffold Workflow

Create a task-oriented flow template for reproducible local execution.

```bash
sui-sandbox init --example quickstart --output-dir .
```

#### `run-flow` - Execute Workflow File

Run deterministic YAML workflows where each step is one `sui-sandbox` argv list.

```bash
sui-sandbox run-flow flow.quickstart.yaml
sui-sandbox run-flow flow.quickstart.yaml --dry-run
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
| `ptb --spec <FILE>` | Convert PTB spec to `sui client ptb` command |
| `info` | Show transition workflow and deployment guide |

**Options:**

| Flag | Description | Default |
|------|-------------|---------|
| `--gas-budget <MIST>` | Gas budget for the transaction | 100M (publish), 10M (call/ptb) |
| `--quiet` | Skip prerequisites and notes, show only command | `false` |
| `-v, --verbose` | Show advanced info (protocol version, error handling tips) | `false` |

Note: `bridge ptb` currently supports **MoveCall-only** PTBs.

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

### Session Persistence

The `sui-sandbox` maintains session state across commands:

- **Loaded packages** remain available between commands
- **Fetched objects** are cached locally
- **Last sender** is remembered for convenience

State is stored in `~/.sui-sandbox/state.json` by default for all commands unless
overridden with `--state-file`.

If you have an older binary state file (`state.bin`), it will be read and
automatically migrated to JSON on the next successful run.

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
- [Replay Triage Workflow](../guides/REPLAY_TRIAGE.md) - Fast diagnose/fix loop for replay failures
- [Limitations](LIMITATIONS.md) - Known differences from mainnet
- [Examples](../../examples/README.md) - Working code examples
