# sui-sandbox

Python bindings for Sui Move package analysis, checkpoint replay, view function execution, and Move function fuzzing.

Built on [sui-sandbox](../../README.md) — runs the real Sui Move VM locally via PyO3.

## Installation

### From PyPI

```bash
pip install sui-sandbox
```

### From source (requires Rust toolchain)

```bash
cd crates/sui-python
pip install maturin
maturin develop --release
```

## Quick Start

```python
import sui_sandbox

# Extract the full interface of the Sui framework
interface = sui_sandbox.extract_interface(package_id="0x2")
for mod_name, mod_data in interface["modules"].items():
    print(f"{mod_name}: {len(mod_data.get('functions', {}))} functions")

# Get the latest Walrus checkpoint and inspect it
cp = sui_sandbox.get_latest_checkpoint()
data = sui_sandbox.get_checkpoint(cp)
print(f"Checkpoint {cp}: {data['transaction_count']} transactions")

# Fuzz a Move function
report = sui_sandbox.fuzz_function("0x1", "u64", "max", iterations=50)
print(f"Verdict: {report['verdict']}, successes: {report['successes']}")
```

## API Reference

### Standalone Functions

These functions work with just `pip install sui-sandbox` — no CLI binary needed.

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

```python
pkgs = sui_sandbox.fetch_package_bytecodes("0x2", resolve_deps=True)
bcs_bytes = sui_sandbox.json_to_bcs(type_str, object_json, pkgs["packages"])
```

#### `call_view_function(package_id, module, function, *, type_args=None, object_inputs=None, pure_inputs=None, child_objects=None, package_bytecodes=None, fetch_deps=True)`

Execute a Move function in the local VM with full control over object and pure inputs.

**Returns:** `dict` with:
- `success` (bool)
- `error` (string or null)
- `return_values` (per-command list of base64-encoded BCS values)
- `return_type_tags` (parallel per-command list of canonical Move type tags)
- `gas_used` (u64)

```python
result = sui_sandbox.call_view_function(
    "0x2", "clock", "timestamp_ms",
    object_inputs=[{"Clock": "0x6"}],
)
print(result["return_values"])
```

#### `fuzz_function(package_id, module, function, *, iterations=100, seed=None, sender="0x0", gas_budget=50_000_000_000, type_args=[], fail_fast=False, max_vector_len=32, dry_run=False, fetch_deps=True)`

Fuzz a Move function with randomly generated inputs.

Use `dry_run=True` to check parameter classification without executing (returns whether the function is fully fuzzable and parameter details).

**Returns:** `dict` with:
- `verdict` — `FULLY_FUZZABLE`, `NOT_FUZZABLE`, `PASS`, `FAIL`, or `MIXED`
- `iterations`, `successes`, `errors` — execution counts
- `error_categories` — grouped error summaries (when errors occur)
- `params` — parameter classification (in dry-run mode)

```python
# Dry run — check if function is fuzzable
info = sui_sandbox.fuzz_function("0x1", "u64", "max", dry_run=True)
print(f"Verdict: {info['verdict']}")

# Full fuzz run
report = sui_sandbox.fuzz_function(
    "0x1", "u64", "max",
    iterations=50, seed=42,
)
print(f"Verdict: {report['verdict']}, successes: {report['successes']}")
```

### CLI-Dependent Functions

These functions shell out to the `sui-sandbox` CLI binary, which must be built and available in `PATH`.

#### `analyze_replay(digest, *, rpc_url=..., source="hybrid", checkpoint=None, allow_fallback=True, prefetch_depth=3, prefetch_limit=200, auto_system_objects=True, no_prefetch=False, verbose=False)`

Analyze replay state hydration for a transaction.

When `checkpoint` is provided, automatically uses Walrus as the data source (no API key needed). Otherwise uses the gRPC/hybrid source (requires `SUI_GRPC_API_KEY` env var).

**Returns:** `dict` with `digest`, `sender`, `commands`, `inputs`, `objects`, `packages`, `modules`, `input_summary`, `command_summaries`, etc.

```python
# Via Walrus (no API key needed)
state = sui_sandbox.analyze_replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
)
print(f"Commands: {state['commands']}, Objects: {state['objects']}")

# Via gRPC (requires API key)
import os
os.environ["SUI_GRPC_API_KEY"] = "your-key"
state = sui_sandbox.analyze_replay("DigestHere...")
```

#### `replay(digest, *, rpc_url=..., compare=False, verbose=False)`

Execute a full VM replay of a historical transaction.

**Returns:** `dict` with full replay results including effects comparison when `compare=True`.

```python
result = sui_sandbox.replay("DigestHere...", compare=True)
print(f"Status: {result['status']}")
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
