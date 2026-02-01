# MCP Reference (sui-sandbox-mcp)

Developer-facing guide to the MCP server and tool surface.

## Quick Start

Build and run the MCP server over stdio:

```bash
cargo build --release --bin sui-sandbox-mcp
./target/release/sui-sandbox-mcp
```

For quick testing without a separate MCP client, use the CLI tool bridge:

```bash
cargo build --release --bin sui-sandbox
sui-sandbox tool get_interface --input '{"package":"0x2","module":"coin"}'
```

## MCP Client Setup

Most MCP clients accept a config entry with a command, args, and optional env vars.
Example configuration:

```json
{
  "mcpServers": {
    "sui-sandbox": {
      "command": "/path/to/target/release/sui-sandbox-mcp",
      "args": [],
      "env": {
        "SUI_SANDBOX_HOME": "~/.sui-sandbox",
        "SUI_GRPC_ENDPOINT": "https://fullnode.mainnet.sui.io:443",
        "SUI_GRPC_API_KEY": "your-api-key"
      }
    }
  }
}
```

If your MCP client cannot set env vars, you can also call `configure` after startup
to set the network or endpoints.

## Environment + Paths

`SUI_SANDBOX_HOME` controls the shared workspace (CLI + MCP):

```
~/.sui-sandbox/
├── cache/      # Global cache (per-network)
├── projects/   # MCP project workspace
└── logs/mcp/   # MCP JSONL logs
```

Optional environment variables:

- `SUI_GRAPHQL_ENDPOINT`: default GraphQL endpoint for mainnet/testnet
- `SUI_GRPC_ENDPOINT`: gRPC endpoint override (defaults to public mainnet)
- `SUI_GRPC_API_KEY`: API key for authenticated gRPC endpoints
- `SUI_DEBUG_LINKAGE`: set to `1` to log package linkage/version resolution during replay/fetch
- `SUI_DEBUG_MUTATIONS`: set to `1` to log mutation tracking details during replay
- `SUI_WALRUS_ENABLED`: set to `true` to enable Walrus checkpoint hydration
- `SUI_WALRUS_CACHE_URL`: Walrus caching server base URL (checkpoint metadata)
- `SUI_WALRUS_AGGREGATOR_URL`: Walrus aggregator base URL (checkpoint blobs)
- `SUI_WALRUS_NETWORK`: `mainnet` or `testnet` (used for Walrus defaults)
- `SUI_WALRUS_TIMEOUT_SECS`: Walrus fetch timeout in seconds (default 10)
- `SUI_WALRUS_LOCAL_STORE`: enable local filesystem object store for Walrus checkpoints
- `SUI_WALRUS_STORE_DIR`: override local store path (defaults to `$SUI_SANDBOX_HOME/walrus-store/<network>`)
- `SUI_WALRUS_FULL_CHECKPOINT_INGEST`: ingest all objects in a checkpoint into the local store
- `SUI_WALRUS_RECURSIVE_LOOKUP`: use local index to hydrate missing objects from Walrus checkpoints
- `SUI_WALRUS_RECURSIVE_MAX_CHECKPOINTS`: max checkpoints to pull per replay (default 5)

## Tool Categories

### 1) Project workflow

- `create_move_project`: create a new Move project (persisted or scratch)
- `read_move_file` / `edit_move_file`: manage source files
- `build_project` / `test_project` / `deploy_project` / `upgrade_project`
- `list_projects`, `set_active_package`

### 2) Execution

- `call_function`: single Move call
- `execute_ptb`: programmable transaction block
- `replay_transaction`: replay a mainnet transaction (uses cache + state provider)

### 3) CLI parity tools (aliases)

These mirror the `sui-sandbox` CLI commands and are easier for agents to use.

- `publish`: deploy a local Move package (CLI `publish`)
- `run`: execute `package::module::function` (CLI `run`)
- `ptb`: execute a PTB from CLI-style spec or MCP-style `execute_ptb` input
- `fetch`: alias for `load_from_mainnet`
- `replay`: alias for `replay_transaction`
- `view`: lightweight view of a module/object/packages list
- `bridge`: generate `sui client` commands (publish/call/ptb/info)
- `status`: summarize session state (packages, provider, active world)
- `clean`: reset the environment and clear object refs

Example (view a module interface):

```json
{"kind":"module","module":"0x2::coin"}
```

### 4) World workflow

- `world_create`, `world_open`, `world_close`, `world_status`
- `world_read_file`, `world_write_file` (paths are relative to the world root; traversal is blocked)
- `world_list`, `world_build`, `world_deploy`
- `world_snapshot`, `world_restore`
- `world_commit`, `world_log`
- `world_templates`, `world_export`, `world_delete`

### 5) State + introspection

- `get_interface`, `search`, `get_state`, `list_packages`, `read_object`
- `load_from_mainnet`: fetch object/package into the environment
- `load_package_bytes`: load local compiled bytecode into the env
- `create_asset`: create synthetic objects/coins for testing

### 6) Configuration

Use `configure` to update runtime settings:

```json
{"action":"set_network","params":{"network":"mainnet"}}
{"action":"set_logging","params":{"enabled":true}}
{"action":"advance_clock","params":{"delta_ms":1000}}
```

## Common Input Patterns

### LLM metadata (logged)

```json
{
  "_meta": { "reason": "Inspect interface before PTB", "tags": ["analysis"] },
  "package": "0x2",
  "module": "coin"
}
```

### Object refs

Most tools return `object_ref` handles. You can reuse them later:

```json
{"object_ref":"obj_1"}
```

**Note:** object refs are process-local; they do not persist across server restarts.

### Shared object mutability

Shared inputs accept an explicit mutability flag:

```json
{"kind":"shared","object_id":"0x6","mutable":false}
```

This affects shared lock acquisition (read vs write).
Shared immutable inputs are enforced: if an immutable shared input is mutated, the execution fails.

### MCP option enums

`cache_policy` and `fetch_strategy` are validated enums:

- `cache_policy`: `default` | `bypass`
- `fetch_strategy`: `eager` | `full`

Example:

```json
{
  "digest": "...",
  "options": { "fetch_strategy": "full", "cache_policy": "bypass", "auto_system_objects": true }
}
```

`cache_policy: "bypass"` skips cache reads and writes for that call.
`auto_system_objects: true` auto-injects Clock/Random system objects when missing.

### Replay options

`replay_transaction` accepts additional options:

- `compare_effects`: compare local effects with on-chain effects (default: true)
- `prefetch_depth`: dynamic field discovery depth (default: 3)
- `prefetch_limit`: max children per parent (default: 200)
- `auto_system_objects`: auto-inject Clock/Random (default: true)
- `reconcile_dynamic_fields`: reconcile dynamic-field effects when on-chain lists omit them (default: true)
- `synthesize_missing`: if replay fails due to missing input objects, synthesize placeholders and retry (default: false)
- `self_heal_dynamic_fields`: synthesize placeholder dynamic-field values when data is missing (default: false)

Example:

```json
{
  "digest": "...",
  "options": {
    "compare_effects": true,
    "fetch_strategy": "full",
    "prefetch_depth": 3,
    "prefetch_limit": 200,
    "auto_system_objects": true,
    "reconcile_dynamic_fields": true,
    "synthesize_missing": true,
    "self_heal_dynamic_fields": true
  }
}
```

## Logging

Every tool call is logged to JSONL with inputs/outputs plus optional `_meta`:

```
$SUI_SANDBOX_HOME/logs/mcp/*.jsonl
```

## Response metadata

Every tool response includes a `state_file` field so callers can see where
session state is stored:

- MCP server: `$SUI_SANDBOX_HOME/mcp-state.json`
- CLI tool mode: the path passed via `--state-file`

## CLI Bridge (1:1 parity)

All MCP tools are callable via `sui-sandbox tool ...`:

```bash
sui-sandbox tool call_function --input '{"package":"0x2","module":"coin","function":"zero","type_args":["0x2::sui::SUI"],"args":[]}'
```

CLI parity tools are also available through `tool`, e.g.:

```bash
sui-sandbox tool status --input '{}'
sui-sandbox tool publish --input '{"path":"./my_pkg"}'
```
