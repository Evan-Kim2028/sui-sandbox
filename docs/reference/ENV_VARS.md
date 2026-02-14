# Environment Variables

This page is the canonical environment variable reference for `sui-sandbox`.

Boolean values are parsed case-insensitively as `1`, `true`, `yes`, or `on`.
Everything else (including `0`, `false`, `off`, `no`, or an empty value) is treated as false.

## Network and Data Sources

| Variable | Default | Description |
|---|---|---|
| `SUI_GRPC_ENDPOINT` | `https://archive.mainnet.sui.io:443` | Mainnet gRPC endpoint used for standard and archival provider setups. If historical replay fails with `ContractAbort ... abort_code: 1` (runtime-object gaps), switch to another archival provider (for example `https://grpc.surflux.dev:443`). |
| `SUI_GRPC_TESTNET_ENDPOINT` | `https://fullnode.testnet.sui.io:443` | Testnet-only gRPC endpoint. |
| `SUI_GRPC_HISTORICAL_ENDPOINT` | none | Override for archive endpoint discovery when set. |
| `SUI_GRPC_ARCHIVE_ENDPOINT` | none | Alternate archive endpoint override with higher precedence than `SUI_GRPC_ENDPOINT` when no historical override is set. |
| `SURFLUX_API_KEY` | none | Enables automatic fallback to `https://grpc.surflux.dev:443` for historical fetches when no explicit endpoint is set; used as API key for that endpoint. |
| `SUI_GRPC_API_KEY` | none | API key for explicit gRPC endpoints (including endpoints from `SUI_GRPC_*`). |
| `SUI_GRAPHQL_ENDPOINT` | inferred from `--rpc-url` network | Override GraphQL endpoint for package/object queries. |
| `SUI_GRAPHQL_TIMEOUT_SECS` | `30` | GraphQL request timeout in seconds. |
| `SUI_GRAPHQL_CONNECT_TIMEOUT_SECS` | `10` | GraphQL connect timeout in seconds. |
| `SUI_GRAPHQL_CIRCUIT_BREAKER` | `true` | Enable timeout-driven GraphQL circuit breaker; when open, GraphQL calls fail fast for a cooldown window. |
| `SUI_GRAPHQL_CIRCUIT_TIMEOUT_THRESHOLD` | `2` | Consecutive timeout-like GraphQL errors required to open the circuit breaker. |
| `SUI_GRAPHQL_CIRCUIT_COOLDOWN_SECS` | `60` | Cooldown duration for an open GraphQL circuit breaker. |

Endpoint precedence:

1. `SUI_GRPC_HISTORICAL_ENDPOINT`
2. `SUI_GRPC_ARCHIVE_ENDPOINT`
3. `SURFLUX_API_KEY` (for archive mode)
4. `SUI_GRPC_ENDPOINT`
5. Built-in public archive endpoint

## Walrus

| Variable | Default | Description |
|---|---|---|
| `SUI_WALRUS_ENABLED` | auto-enabled for replay `--source hybrid|walrus`; otherwise `false` | Enable Walrus checkpoint hydration. Set `0/false/off/no` to disable Walrus auto-hydration in `--source hybrid` mode (`--source walrus` forces Walrus on). |
| `SUI_WALRUS_CACHE_URL` | network mainnet/testnet default URL | Override Walrus cache metadata endpoint. |
| `SUI_WALRUS_AGGREGATOR_URL` | network mainnet/testnet default URL | Override Walrus checkpoint blob endpoint. |
| `SUI_WALRUS_NETWORK` | `mainnet` | Select Walrus default network family for fallback URLs. |
| `SUI_WALRUS_TIMEOUT_SECS` | `10` | Timeout for per-checkpoint Walrus fetches. |
| `SUI_WALRUS_LOCAL_STORE` | `false` | Enable local filesystem Walrus object store. |
| `SUI_WALRUS_STORE_DIR` | `$SUI_SANDBOX_HOME/walrus-store/<network>` | Override local Walrus store directory. |
| `SUI_WALRUS_FULL_CHECKPOINT_INGEST` | `true` | Ingest all objects from an input/output checkpoint while hydrating local store (opt-out with falsey value). |
| `SUI_WALRUS_RECURSIVE_LOOKUP` | inherited from `SUI_WALRUS_LOCAL_STORE` | Enable recursive parent-object lookup from local Walrus indexes. |
| `SUI_WALRUS_RECURSIVE_MAX_CHECKPOINTS` | `5` | Max checkpoints scanned in recursive lookup. |
| `SUI_WALRUS_RECURSIVE_MAX_TX_STEPS` | `3` | Max transaction steps scanned during recursive lookup. |
| `SUI_WALRUS_PACKAGE_ONLY` | `false` | Restrict replay package resolution to Walrus package data only. |

## Replay, Checkpoint, and Package Resolution

| Variable | Default | Description |
|---|---|---|
| `SUI_CHECKPOINT_LOOKUP_FORCE_REMOTE` | `false` | Force remote checkpoint lookup even when local caches are available. |
| `SUI_CHECKPOINT_LOOKUP_REMOTE` | `true` | Enable remote checkpoint lookup; set false to use local indexes only. |
| `SUI_CHECKPOINT_LOOKUP_GRAPHQL` | `true` | Include GraphQL in checkpoint lookup path. |
| `SUI_CHECKPOINT_LOOKUP_GRPC` | `true` | Include gRPC in checkpoint lookup path. |
| `SUI_CHECKPOINT_LOOKUP_SELF_TEST` | `false` | Enable checkpoint lookup self-check behavior. |
| `SUI_PACKAGE_LOOKUP_GRAPHQL` | `true` | Enable GraphQL lookup path for package versions. |
| `SUI_OBJECT_FETCH_CONCURRENCY` | `16` | Max parallel object fetch requests in replay hydration. |
| `SUI_PACKAGE_FETCH_CONCURRENCY` | `8` | Max parallel package/dependency fetch steps per frontier round. |
| `SUI_PACKAGE_FETCH_PARALLEL` | `true` | Enable frontier-parallel package dependency resolution; set false to force serial package fetch behavior. |

## Replay and Replay Debug Controls

| Variable | Default | Description |
|---|---|---|
| `SUI_REPLAY_PROGRESS` | auto-enabled in interactive TTY replay (except `--json`) | Print replay progress logs when truthy. |
| `SUI_PTB_PROGRESS` | auto-enabled in interactive TTY replay (except `--json`) | Print PTB execution command progress when truthy. |
| `SUI_DEBUG_LINKAGE` | `false` | Emit linkage/module-resolution diagnostics. |
| `SUI_DEBUG_MUTATIONS` | `false` | Emit mutation-tracking and PTB mutation debug traces. |
| `SUI_DEBUG_TIMING` | `false` | Emit replay timing measurements. |
| `SUI_DEBUG_CHECKPOINT_LOOKUP` | `false` | Emit detailed checkpoint-lookup traces. |
| `SUI_DEBUG_DATA_GAPS` | `false` | Emit dynamic field data gap diagnostics. |
| `SUI_DEBUG_WALRUS` | `false` | Emit Walrus fetch/ingest diagnostics. |
| `SUI_DEBUG_ALIAS_REWRITE` | `false` | Emit alias rewriting diagnostics during replay. |
| `SUI_DEBUG_ERROR_CONTEXT` | auto-enabled on failures when `--verbose` or `--strict`; otherwise `false` | Emit rich VM error context and state snapshots on execution failures. |
| `SUI_DISABLE_VERSION_PATCH` | `false` | Disable protocol-version-based object patching. |
| `SUI_ALLOW_PLACEHOLDER_CREATED_IDS` | `false` | Enable synthetic placeholder object IDs from return values. |
| `SUI_DF_STRICT_CHECKPOINT` | `true` when replay uses `--strict` or `--compare`; otherwise `false` | Enforce checkpoint-bounded dynamic-field reads (skip latest-version fallbacks). |
| `SUI_DF_ENUM_LIMIT` | `1000` | Upper bound for dynamic-field enumeration calls. |
| `SUI_DF_MISS_BACKOFF_MS` | `250` | Initial backoff in milliseconds for repeated dynamic-field misses. |
| `SUI_STATE_DF_PREFETCH_TIMEOUT_SECS` | `30` | Timeout for state prefetch of dynamic-field descendants. |
| `SUI_DUMP_TX_OBJECTS` | `false` | Print transaction object counts during fetch/debug runs when set. |
| `SUI_DUMP_RUNTIME_OBJECTS` | `false` | Print runtime-object counts during replay fetch when set. |
| `SUI_CHECK_OBJECT_ID` | none | Filter debug output to a single object ID. |
| `SUI_DUMP_PACKAGE_MODULES` | none | Comma-separated list of package IDs to dump module names for. |
| `SUI_CHECK_ALIAS` | none | Resolve and print alias target for a single address. |
| `SUI_DUMP_MODULE_FUNCTIONS` | none | Dump public function names for `<ADDR>::<module>` in a single package module. |

## Runtime, Paths, and I/O

| Variable | Default | Description |
|---|---|---|
| `SUI_SANDBOX_HOME` | `~/.sui-sandbox` | Main sandbox state directory used for caches and runtime artifacts. |
| `SUI_FRAMEWORK_PATH` | `~/.sui-framework-cache/mainnet-v1.64.2` | Override framework cache root. |
| `SUI_LLM_LOG_DIR` | `~/.sui-llm-logs` | Override LLM session log directory. |
| `SUI_SANDBOX_BIN` | current executable path | Override sandbox binary path used by replay-mutation orchestration. |

## Internal/Advanced

| Variable | Default | Description |
|---|---|---|
| `SANDBOX_TRACE_RELOCATE` | `false` | Enable verbose module relocation diagnostics in resolver/VM. |
| `IGLOO_MCP_SERVICE_CONFIG` | none | Primary path to MCP service config for replay/igloo integration. |
| `IGLOO_MCP_CONFIG` | none | Fallback MCP config path (legacy alias). |

## `.env` and docs alignment

- Keep a local `.env` with these variables as needed; the CLI replay hydration path reads `.env` from repository ancestors.
- `docs/CONTRIBUTING.md` and `docs/reference/CLI_REFERENCE.md` now point to this file for canonical values.
