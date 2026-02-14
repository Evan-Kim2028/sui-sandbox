# sui-sandbox

Python bindings for Sui Move package analysis, transaction replay, view function execution, and Move function fuzzing.

Built on [sui-sandbox](../../README.md) — runs the real Sui Move VM locally via PyO3. **All functions are standalone** — `pip install sui-sandbox` is all you need.

## Installation

### From PyPI

```bash
pip install sui-sandbox
```

### From source (requires Rust toolchain)

```bash
cd crates/sui-python
pip install maturin
# For Python 3.14 with current PyO3 constraints:
#   PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop --release
maturin develop --release
```

For maintainers/contributors, see the full build and validation workflow:
[`docs/guides/PYTHON_BINDINGS.md`](../../docs/guides/PYTHON_BINDINGS.md).

## Quick Start

```python
import sui_sandbox

# Extract the full interface of the Sui framework
interface = sui_sandbox.extract_interface(package_id="0x2")
for mod_name, mod_data in interface["modules"].items():
    print(f"{mod_name}: {len(mod_data.get('functions', {}))} functions")

# Replay a historical transaction via Walrus (no API key needed)
result = sui_sandbox.replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
)
print(f"Success: {result['local_success']}")

# Fuzz a Move function
report = sui_sandbox.fuzz_function("0x1", "u64", "max", iterations=50)
print(f"Successes: {report['outcomes']['successes']}")
```

For runnable end-to-end scripts, see:

- `python_sui_sandbox/README.md`

## API Reference

#### `extract_interface(*, package_id=None, bytecode_dir=None, rpc_url="https://fullnode.mainnet.sui.io:443")`

Extract the complete interface JSON for a Move package — all modules, structs, functions, type parameters, abilities, and fields.

Provide either `package_id` (fetched via GraphQL) or `bytecode_dir` (local directory containing `bytecode_modules/*.mv` files), but not both.

**Returns:** `dict` with full interface tree.

```python
interface = sui_sandbox.extract_interface(package_id="0x1")
for mod_name, mod_data in interface["modules"].items():
    print(f"{mod_name}: {len(mod_data.get('functions', {}))} functions")
```

#### `get_latest_checkpoint()`

Get the latest archived checkpoint number from Walrus.

**Returns:** `int`

```python
cp = sui_sandbox.get_latest_checkpoint()
print(f"Latest checkpoint: {cp}")
```

#### `get_checkpoint(checkpoint)`

Fetch a checkpoint from Walrus and return a summary.

**Returns:** `dict` with `checkpoint`, `epoch`, `timestamp_ms`, `transaction_count`, `transactions` (list), and `object_versions_count`.

```python
data = sui_sandbox.get_checkpoint(239615926)
for tx in data["transactions"]:
    print(f"  {tx['digest']}: {tx['commands']} commands, {tx['input_objects']} inputs")
```

#### `fetch_object_bcs(object_id, *, version=None, endpoint=None, api_key=None)`

Fetch a Sui object's BCS payload via gRPC (optionally pinned to a historical version).

**Returns:** `dict` with `object_id`, `version`, `type_tag`, `bcs_base64`, and owner metadata.

```python
obj = sui_sandbox.fetch_object_bcs("0x6", version=714666359)
print(obj["type_tag"], obj["version"])
```

#### `fetch_historical_package_bytecodes(package_ids, *, type_refs=None, checkpoint=None, endpoint=None, api_key=None)`

Fetch package bytecodes via `HistoricalStateProvider` with transitive dependency resolution, optionally pinned to a checkpoint.

This mirrors the Rust historical package-loading path used by advanced examples like DeepBook margin reconstruction.

**Returns:** `dict` with:

- `packages`: package ID -> list of base64 module bytecodes
- `aliases`: storage -> runtime package ID aliases
- `linkage_upgrades`: runtime -> storage upgrade map
- `package_runtime_ids`: storage -> runtime IDs
- `package_linkage`: per-package linkage tables
- `package_versions`: storage -> package version (used for alias/version-aware child lookup)
- `count`, `checkpoint`, `endpoint_used`

```python
pkgs = sui_sandbox.fetch_historical_package_bytecodes(
    [
        "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
        "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497",
    ],
    type_refs=[
        "0x2::sui::SUI",
        "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
    ],
    checkpoint=240733000,
)
print(f"Fetched {pkgs['count']} packages")
```

#### `fetch_package_bytecodes(package_id, *, resolve_deps=True)`

Fetch package bytecodes via GraphQL, optionally resolving transitive dependencies.

**Returns:** `dict` with `packages` (map of package ID to list of base64-encoded module bytecodes) and `count`.

```python
pkgs = sui_sandbox.fetch_package_bytecodes("0x2", resolve_deps=True)
print(f"Fetched {pkgs['count']} packages")
```

#### `json_to_bcs(type_str, object_json, package_bytecodes)`

Convert a Sui object JSON representation to BCS bytes using Move type layout.

**Returns:** `bytes`

#### `transaction_json_to_bcs(transaction_json)`

Convert Snowflake-style `TRANSACTION_JSON` (or canonical Sui `TransactionData` JSON)
into raw transaction BCS bytes.

This is useful when your pipeline has transaction JSON but not transaction BCS.

**Returns:** `bytes`

#### `call_view_function(package_id, module, function, *, type_args=None, object_inputs=None, pure_inputs=None, child_objects=None, historical_versions=None, fetch_child_objects=False, grpc_endpoint=None, grpc_api_key=None, package_bytecodes=None, fetch_deps=True)`

Execute a Move function in the local VM with full control over object and pure inputs.

**Returns:** `dict` with `success`, `error`, `return_values`, `return_type_tags`, `gas_used`.

`object_inputs` entries must use:

```python
{
    "object_id": "0x...",
    "type_tag": "0x2::...",
    "bcs_bytes": [1, 2, 3],   # or bytes in Python
    "is_shared": False,       # optional
    "mutable": False,         # optional
}
```

Legacy compatibility: `owner` is also accepted as an alias for `is_shared`:
`"immutable"` / `"address_owned"` => non-shared, `"shared"` => shared.

`package_bytecodes` accepts either:

- `{"0xpackage": [b"...", b"..."]}` or `{"0xpackage": ["<base64>", ...]}`
- the full payload returned by `fetch_historical_package_bytecodes(...)`

When passing the full historical payload, dependency fetching is auto-disabled to avoid mixing in latest GraphQL dependency fetches.

To enable on-demand child-object loading (useful for `sui::versioned` wrappers):

```python
result = sui_sandbox.call_view_function(
    package_id="0x...",
    module="mod",
    function="fn",
    object_inputs=[...],
    historical_versions={"0xchild_or_known_obj": 123},
    fetch_child_objects=True,
)
```

#### `fuzz_function(package_id, module, function, *, iterations=100, seed=None, sender="0x0", gas_budget=50_000_000_000, type_args=[], fail_fast=False, max_vector_len=32, dry_run=False, fetch_deps=True)`

Fuzz a Move function with randomly generated inputs.

Use `dry_run=True` to check parameter classification without executing.

**Returns:** `dict` with `target`, `classification`, `outcomes` (successes/errors), `gas_profile`.

```python
# Dry run — check if function is fuzzable
info = sui_sandbox.fuzz_function("0x1", "u64", "max", dry_run=True)
print(f"Fuzzable: {info['classification']['is_fully_fuzzable']}")

# Full fuzz run
report = sui_sandbox.fuzz_function("0x1", "u64", "max", iterations=50, seed=42)
print(f"Successes: {report['outcomes']['successes']}")
```

#### `import_state(*, state=None, transactions=None, objects=None, packages=None, cache_dir=None)`

Import replay data from JSON/JSONL/CSV into a local replay cache.

```python
sui_sandbox.import_state(
    transactions="exports/transactions.csv",
    objects="exports/objects.jsonl",
    packages="exports/packages.csv",
    cache_dir=".sui-cache",
)
```

#### `deserialize_transaction(raw_bcs)` / `deserialize_package(bcs)`

Decode raw BCS blobs into structured JSON for debugging or preprocessing.

#### `replay(digest=None, *, rpc_url=..., source="hybrid", checkpoint=None, state_file=None, cache_dir=None, allow_fallback=True, prefetch_depth=3, prefetch_limit=200, auto_system_objects=True, no_prefetch=False, compare=False, analyze_only=False, verbose=False)`

Replay a historical Sui transaction locally with the Move VM.

Replay source modes:
- `checkpoint=...` uses Walrus (no API key needed)
- `state_file=...` replays from a local exported state file
- `source="local"` (or `cache_dir=...`) replays from imported local cache
- otherwise uses gRPC/hybrid (requires `SUI_GRPC_API_KEY`)

Use `analyze_only=True` to inspect state hydration without executing the transaction.

Use `compare=True` to compare local execution results with on-chain effects.

**Returns:** `dict` — replay results (with `local_success`, `effects`, `execution_path`, optionally `comparison`) or analysis summary (with `commands`, `inputs`, `objects`, `packages`, `input_summary`).

```python
# Analyze state hydration only (no VM execution)
analysis = sui_sandbox.replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    analyze_only=True,
)
print(f"Commands: {analysis['commands']}, Objects: {analysis['objects']}")

# Full replay via Walrus (no API key needed)
result = sui_sandbox.replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
)
print(f"Success: {result['local_success']}")

# Full replay via local state file
result = sui_sandbox.replay(state_file="exports/replay_state.json")

# Full replay via local cache import
sui_sandbox.import_state(state="exports/replay_state.json", cache_dir=".sui-cache")
result = sui_sandbox.replay(digest="DigestHere...", source="local", cache_dir=".sui-cache")

# Full replay via gRPC with comparison
import os
os.environ["SUI_GRPC_API_KEY"] = "your-key"
result = sui_sandbox.replay("DigestHere...", compare=True)
if result.get("comparison"):
    print(f"Status match: {result['comparison']['status_match']}")
```

## Platform Support

Pre-built wheels are available for:
- Linux x86_64 (glibc 2.17+)
- Linux aarch64 (glibc 2.17+)
- macOS x86_64 (10.12+)
- macOS aarch64 (11.0+)

Building from source requires Rust 1.80+ and Python 3.9+.

## License

Apache 2.0
