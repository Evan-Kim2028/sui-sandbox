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
sui-sandbox replay mutate --demo              # Guided replay-mutation demo
sui-sandbox analyze package --package-id 0x2  # Package introspection
sui-sandbox analyze replay 9V3xKMnFpXyz...     # Replay-state introspection
sui-sandbox workflow init --template cetus --output workflow.cetus.json
sui-sandbox workflow validate --spec examples/data/workflow_replay_analyze_demo.json
sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --dry-run
sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --report out/workflow_report.json
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
| `replay` | Replay historical mainnet transactions and replay-mutation workflows |
| `analyze` | Package and replay-state introspection |
| `view` | Inspect modules, objects, packages |
| `bridge` | Generate `sui client` commands for real deployment |
| `test` | Test Move functions (fuzz) |
| `tools` | Utility commands (poll/stream/tx-sim/json-to-bcs) |
| `init` | Scaffold task-oriented workflow templates |
| `run-flow` | Execute deterministic YAML workflow files |
| `workflow` | Validate/run typed workflow specs (replay/analyze/command) |
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

Low-level streaming/simulation utilities are grouped under `sui-sandbox tools`:

```bash
# Poll recent transactions via GraphQL
sui-sandbox tools poll-transactions --duration 300 --interval-ms 1000

# Stream checkpoints via gRPC
sui-sandbox tools stream-transactions --endpoint https://your-provider:9000

# Simulate a PTB via gRPC
sui-sandbox tools tx-sim --ptb-spec tx.json --sender 0x...

# Convert JSON object to BCS bytes using local bytecode layouts
sui-sandbox tools json-to-bcs --type "0x2::coin::Coin<0x2::sui::SUI>" --json-file obj.json --bytecode-dir ./pkg

# Same with JSON output (includes type and size metadata)
sui-sandbox --json tools json-to-bcs --type "0x2::coin::Coin<0x2::sui::SUI>" --json-file obj.json --bytecode-dir ./pkg
```

**`tools stream-transactions` flags:**

| Flag | Description | Default |
|------|-------------|---------|
| `--endpoint <URL>` | gRPC endpoint URL | env var or public endpoint |
| `--duration <SECS>` | How long to run | `60` |
| `--output <FILE>` | Output file path (JSONL) | `transactions_stream.jsonl` |
| `--ptb-only` | Only save PTB transactions (skip system txs) | `false` |
| `-v, --verbose` | Print detailed progress | `false` |

**`tools tx-sim` flags:**

| Flag | Description | Default |
|------|-------------|---------|
| `--grpc-url <URL>` | gRPC endpoint URL | `https://archive.mainnet.sui.io:443` |
| `--sender <ADDR>` | Transaction sender address | required |
| `--mode <MODE>` | `dev-inspect`, `dry-run`, or `build-only` | `dry-run` |
| `--gas-budget <N>` | Gas budget (required for dry-run) | `10000000` |
| `--ptb-spec <PATH>` | JSON PTB spec path (use `-` for stdin) | required |
| `--bytecode-package-dir <DIR>` | Local bytecode dir for static created-object-type inference | - |

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
| `--rpc-url <URL>` | RPC URL for mainnet fetching | `https://archive.mainnet.sui.io:443` |
| `--json` | Output as JSON instead of human-readable | `false` |
| `--debug-json` | Emit structured debug diagnostics on failures | `false` |
| `-v, --verbose` | Show execution traces | `false` |

### Environment Variables

Most environment variables are centralized in:

- [ENV_VARS.md](ENV_VARS.md)

The fetch/replay path also reads a local `.env` file if present.

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

# Call a function with cached object inputs
sui-sandbox run 0x2::counter::value --arg obj-ref:0x123 --arg obj-owned:0x456

# With type arguments
sui-sandbox run 0x2::coin::zero --type-arg 0x2::sui::SUI

# With custom sender and gas budget
sui-sandbox run 0x100::counter::increment --sender 0xABC --gas-budget 10000000
```

| Flag | Description | Default |
|------|-------------|---------|
| `--arg <VALUE>` | Arguments (auto-parsed: `42`, `true`, `0xABC`, `"string"`). Object inputs also support `obj-ref:<id>`, `obj-owned:<id>`, `obj-mut:<id>`, `obj-shared:<id>`, `obj-shared-mut:<id>` | - |
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

Object inputs are optional and must be present in the current sandbox session (`sui-sandbox fetch object <ID>`), since `run` uses the local object's BCS bytes and type tag as input.

`--json` output now includes return payloads:

```json
{
  "success": true,
  "gas_used": 1500,
  "created": ["..."],
  "mutated": ["..."],
  "deleted": ["..."],
  "events_count": 0,
  "return_values": [
    [
      "9f4a..."
    ]
  ],
  "return_type_tags": [
    [
      "0x2::coin::Coin<0x2::sui::SUI>"
    ]
  ]
}
```

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
| `checkpoints <START> <END>` | Ingest package index entries from a checkpoint range |
| `checkpoint <SEQ>` | Fetch a single Walrus checkpoint and display its summary |
| `latest-checkpoint` | Show the latest checkpoint sequence number on Walrus |

| Flag | Description |
|------|-------------|
| `--with-deps` | Recursively fetch transitive dependency packages |
| `--bytecodes` | Include base64-encoded module bytecodes in JSON output |

Behavior notes:
- `fetch package <ID>` fails if any module in the package GraphQL payload is missing `bytecode_base64`, so you do not get partial package fetches.
- `fetch package <ID> --bytecodes` includes base64-encoded module bytecodes in JSON output (useful for programmatic consumers).
- `fetch package <ID> --with-deps` resolves and fetches all transitive dependency packages (framework packages 0x1/0x2/0x3 are skipped).

**Walrus checkpoint queries:**

```bash
# Show latest checkpoint number
sui-sandbox fetch latest-checkpoint

# Fetch checkpoint summary with transaction details
sui-sandbox --json fetch checkpoint 12345
```

`fetch checkpoint` output includes transaction digests, senders, command counts, and object version counts.

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

If replay fails with `ContractAbort ... abort_code: 1` and missing runtime-object context,
switch to a different historical provider:

```bash
SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443 sui-sandbox replay <DIGEST> --source grpc --compare
```

**Replay mutate (fail -> heal orchestration):**

```bash
# Deterministic one-command demo (uses pinned fixture candidates)
sui-sandbox replay mutate --demo

# Fixture-driven mutate run
sui-sandbox replay mutate \
  --fixture examples/data/replay_mutation_fixture_v1.json \
  --replay-source walrus \
  --differential-source grpc \
  --jobs 4 \
  --retries 1 \
  --keep-going \
  --corpus-out examples/out/replay_mutation_corpus.json \
  --max-transactions 4 \
  --replay-timeout 35

# Single explicit target
sui-sandbox replay mutate \
  --digest 5WqivEXirxeLLENpZEhEdGzprwJ6yRbeVJTqJ3KkyGP5 \
  --checkpoint 239615931

# Plan-only/no-op (target validation + report contract, no replay)
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --no-op --json

# Strategy-driven mutate run (YAML/JSON)
sui-sandbox replay mutate \
  --fixture examples/data/replay_mutation_fixture_v1.json \
  --strategy examples/replay_mutate_strategies/default.yaml \
  --mutator state_input_rewire \
  --mutator state_object_version_skew \
  --oracle fail_to_heal \
  --invariant commands_executed_gt_zero \
  --minimize true \
  --minimization-mode operator-specific
```

**Data source flags:**

| Flag | Description |
|------|-------------|
| `--source <grpc\|walrus\|hybrid>` | Replay hydration source (default: `hybrid`) |
| `--checkpoint <SPEC>` | Walrus checkpoint: single (`239615926`), range (`100..200`), or list (`100,105,110`) |
| `--latest <N>` | Auto-discover tip and replay the latest N checkpoints (max 100) |
| `--state-json <PATH>` | Load replay state from a JSON file (no network needed) |
| `--export-state <PATH>` | Export fetched replay state as JSON before executing |

Notes:
- `--source hybrid` now auto-enables Walrus hydration by default (no `SUI_WALRUS_ENABLED=1` required).
- Set `SUI_WALRUS_ENABLED=0` to explicitly disable Walrus auto-hydration in `--source hybrid` runs (`--source walrus` still forces Walrus on).

**Behavior flags:**

| Flag | Description |
|------|-------------|
| `--compare` | Compare local effects with on-chain effects |
| `--allow-fallback` / `--fallback` | Allow fallback to secondary data sources |
| `--profile <safe\|balanced\|fast>` | Replay runtime defaults profile (default: `balanced`) |
| `--vm-only` | Disable fallback and force direct VM replay path |
| `--fetch-strategy` | Dynamic field strategy: `eager` (only accessed) or `full` (prefetch) |
| `--prefetch-depth` | Max dynamic field discovery depth (default: 3) |
| `--prefetch-limit` | Max children per parent when prefetching (default: 200) |
| `--no-prefetch` | Disable dynamic field prefetch regardless of fetch strategy |
| `--auto-system-objects <true\|false>` | Auto-inject Clock/Random system objects when missing |
| `--reconcile-dynamic-fields` | Reconcile dynamic-field effects when on-chain lists omit them |
| `--synthesize-missing` | If replay fails due to missing input objects, synthesize placeholders and retry |
| `--self-heal-dynamic-fields` | Synthesize placeholder dynamic-field values when data is missing (testing only) |

Default runtime behavior:
- Replay/PTB progress logs auto-enable in interactive TTY mode (and stay off for `--json` output) unless you explicitly set `SUI_REPLAY_PROGRESS`/`SUI_PTB_PROGRESS`.
- `--strict` and `--compare` auto-enable checkpoint-strict dynamic field reads (`SUI_DF_STRICT_CHECKPOINT=1`) unless explicitly overridden.
- Failure-only error context (`SUI_DEBUG_ERROR_CONTEXT`) auto-enables when `--verbose` or `--strict` is used.
- A startup `[replay_config]` line prints effective runtime settings and which values were auto-applied.
- GraphQL timeout circuit breaker auto-opens after repeated timeout-like failures and disables GraphQL calls for a cooldown (`SUI_GRAPHQL_CIRCUIT_*`).

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
```
`--bytecode-dir` resolves the package ID from `metadata.json` (`id`) when present,
or falls back to the directory name if metadata is unavailable.

```bash
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

For runnable corpus workflows, see `examples/advanced/package_analysis/README.md`.

Quick workflow:
1. `analyze package` for a single package/module debugging pass.
2. `analyze objects` on corpus for baseline and trend diffs.
3. Use `party_transfer_eligible` vs `party_transfer_observed_in_bytecode` to spot latent capabilities not exercised in package code.
4. Run MM2 corpus sweep (`scripts/internal/cli_mm2_corpus_sweep.sh`) as a reliability check.

Behavior notes:
- `analyze package --package-id` fails if any module in the fetched package is missing `bytecode_base64` in GraphQL data, so you do not get partial interface output.

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

#### `workflow` - Typed Workflow Specs

Run typed JSON/YAML workflow specs for replay/analyze automation. This is the
forward-compatible path for protocol adapters and higher-level orchestration.

```bash
sui-sandbox workflow init --template generic --output workflow.generic.json
sui-sandbox workflow init --template suilend --output workflow.suilend.json
sui-sandbox workflow init --template suilend --format yaml --output workflow.suilend.yaml
sui-sandbox workflow init --template cetus --output workflow.cetus.json --package-id 0x2 --view-object 0x6
sui-sandbox workflow init --from-config examples/data/workflow_init_suilend.yaml --force
sui-sandbox workflow auto --package-id 0x2 --output workflow.auto.pkg2.json
sui-sandbox workflow auto --package-id 0x2 --digest <DIGEST> --checkpoint <CHECKPOINT> --output workflow.auto.pkg2.replay.yaml --format yaml
sui-sandbox workflow validate --spec examples/data/workflow_replay_analyze_demo.json
sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --dry-run
sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --report out/workflow_report.json
```

Supported step kinds:

- `replay`
- `analyze_replay`
- `command` (argv pass-through to `sui-sandbox`)

`workflow auto` flags:

By default, `workflow auto` validates package bytecode dependency closure and
fails closed with `AUTO_CLOSURE_INCOMPLETE` when unresolved packages remain.
Use `--best-effort` to emit a scaffold anyway.

| Flag | Description | Default |
|------|-------------|---------|
| `--package-id <ID>` | Package id for draft adapter generation | required |
| `--template <NAME>` | Optional template override (`generic`, `cetus`, `suilend`, `scallop`) | inferred from module names, fallback `generic` |
| `--output <PATH>` | Output workflow spec path | `workflow.auto.<template>.<package_suffix>.<fmt>` |
| `--format <FORMAT>` | Output spec format: `json`, `yaml` | inferred from `--output` extension, else `json` |
| `--digest <DIGEST>` | Include replay/analyze replay steps in draft | scaffold-only when omitted |
| `--checkpoint <N>` | Checkpoint for replay/analyze replay steps | template default (when `--digest` is set) |
| `--name <NAME>` | Override generated workflow name | `auto_<template>_<package_suffix>` |
| `--best-effort` | Emit scaffold even if dependency-closure probe fails | `false` |
| `--force` | Overwrite existing output file | `false` |

`workflow init` flags:

| Flag | Description | Default |
|------|-------------|---------|
| `--from-config <PATH>` | Load workflow init options from JSON/YAML file | - |
| `--template <NAME>` | Built-in template: `generic`, `cetus`, `suilend`, `scallop` | `generic` |
| `--output <PATH>` | Output workflow spec path | `workflow.<template>.json` (or `.yaml` when `--format yaml`) |
| `--format <FORMAT>` | Output spec format: `json`, `yaml` | inferred from `--output` extension, else `json` |
| `--digest <DIGEST>` | Seed digest for replay/analyze steps | built-in demo digest |
| `--checkpoint <N>` | Checkpoint for replay/analyze steps | built-in demo checkpoint |
| `--no-analyze` | Skip `analyze_replay` step generation | `false` |
| `--no-strict` | Disable strict replay in generated spec | `false` |
| `--name <NAME>` | Override generated workflow name | - |
| `--package-id <ID>` | Add `analyze package --package-id <ID>` command step | - |
| `--view-object <ID>` | Add `view object <ID>` command step (repeatable) | - |
| `--force` | Overwrite existing output file | `false` |

`workflow run` flags:

| Flag | Description | Default |
|------|-------------|---------|
| `--spec <PATH>` | Workflow spec file (JSON or YAML) | required |
| `--dry-run` | Print resolved commands without executing | `false` |
| `--continue-on-error` | Continue after failed steps | `false` |
| `--report <PATH>` | Write workflow run JSON report to file | - |

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

#### `test` - Move Function Testing

Test Move functions with automated input generation.

```bash
# Fuzz a single function with 500 random inputs
sui-sandbox test fuzz 0x100::math::add -n 500

# Fuzz all callable functions in a module
sui-sandbox test fuzz 0x100::math --all-functions -n 100

# Reproducible run with fixed seed
sui-sandbox test fuzz 0x100::math::add -n 1000 --seed 42

# Dry-run: analyze function signature without executing
sui-sandbox test fuzz 0x100::math::add --dry-run

# Stop on first error
sui-sandbox test fuzz 0x100::math::add -n 1000 --fail-fast
```

**`test fuzz` flags:**

| Flag | Description | Default |
|------|-------------|---------|
| `-n, --iterations <N>` | Number of fuzz iterations | `100` |
| `--seed <N>` | Random seed for reproducibility | random |
| `--sender <ADDR>` | Sender address | `0x0` |
| `--gas-budget <N>` | Gas budget per execution | `50000000000` |
| `--type-arg <TYPE>` | Type arguments (repeatable) | - |
| `--fail-fast` | Stop on first abort/error | `false` |
| `--dry-run` | Analyze signature only, don't execute | `false` |
| `--all-functions` | Fuzz all callable functions in the module | `false` |
| `--max-vector-len <N>` | Maximum vector length for generated inputs | `32` |

Phase 1 supports pure-argument-only functions (bool, integers, address, vectors, strings).
Functions requiring object inputs are analyzed and reported as not yet fuzzable.

---

### Session Persistence

The `sui-sandbox` maintains session state across commands:

- **Loaded packages** remain available between commands
- **Fetched objects** are cached locally
- **Last sender** is remembered for convenience

State is stored in `~/.sui-sandbox/state.json` by default for all commands unless
overridden with `--state-file`.

Legacy `state.bin` files are intentionally not auto-migrated in current releases.
Export to JSON from your legacy toolchain and import that JSON if you need to preserve older state.

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
