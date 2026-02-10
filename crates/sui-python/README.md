# sui-move-extractor

Python bindings for Sui Move package analysis and Walrus checkpoint replay.

Built on [sui-sandbox](../../README.md) — runs the real Sui Move VM locally via PyO3.

## Installation

### From source (requires Rust toolchain)

```bash
cd crates/sui-python
pip install maturin
maturin develop --release
```

### From PyPI

```bash
pip install sui-move-extractor
```

## Quick Start

```python
import sui_move_extractor

# Analyze the Sui framework package
info = sui_move_extractor.analyze_package(package_id="0x2")
print(f"{info['modules']} modules, {info['structs']} structs, {info['functions']} functions")

# Get the latest Walrus checkpoint and inspect it
cp = sui_move_extractor.get_latest_checkpoint()
data = sui_move_extractor.get_checkpoint(cp)
print(f"Checkpoint {cp}: {data['transaction_count']} transactions")
```

## API Reference

### Package Analysis

#### `analyze_package(*, package_id=None, bytecode_dir=None, rpc_url="https://fullnode.mainnet.sui.io:443", list_modules=False)`

Analyze a Sui Move package and return summary counts.

Provide either `package_id` (fetched via GraphQL from an RPC node) or `bytecode_dir` (local directory containing `bytecode_modules/*.mv` files), but not both.

**Returns:** `dict` with `source`, `package_id`, `modules`, `structs`, `functions`, `key_structs`, and optionally `module_names`.

```python
# Remote package
info = sui_move_extractor.analyze_package(package_id="0x2")

# Local bytecode
info = sui_move_extractor.analyze_package(
    bytecode_dir="./build/my_package/bytecode_modules",
    list_modules=True,
)
print(info["module_names"])  # ["module_a", "module_b", ...]
```

#### `extract_interface(*, package_id=None, bytecode_dir=None, rpc_url="https://fullnode.mainnet.sui.io:443")`

Extract the complete interface JSON for a Move package — all modules, structs, functions, type parameters, abilities, and fields.

**Returns:** `dict` with full interface tree.

```python
interface = sui_move_extractor.extract_interface(package_id="0x1")
for mod_name, mod_data in interface["modules"].items():
    print(f"{mod_name}: {len(mod_data.get('functions', {}))} functions")
```

### Walrus Checkpoint Replay

These functions use [Walrus](https://docs.walrus.site/) decentralized storage to fetch Sui checkpoint data. **No API keys or authentication required.**

#### `get_latest_checkpoint()`

Get the latest archived checkpoint number from Walrus.

**Returns:** `int`

```python
cp = sui_move_extractor.get_latest_checkpoint()
print(f"Latest checkpoint: {cp}")
```

#### `get_checkpoint(checkpoint)`

Fetch a checkpoint and return a summary.

**Returns:** `dict` with `checkpoint`, `epoch`, `timestamp_ms`, `transaction_count`, `transactions` (list), and `object_versions_count`.

```python
data = sui_move_extractor.get_checkpoint(239615926)
for tx in data["transactions"]:
    print(f"  {tx['digest']}: {tx['commands']} commands, {tx['input_objects']} inputs")
```

#### `walrus_analyze_replay(digest, checkpoint, *, verbose=False)`

Analyze replay state for a transaction using Walrus only. Fetches the checkpoint, extracts the transaction, and builds a complete replay state summary.

**Returns:** `dict` with `digest`, `sender`, `commands`, `inputs`, `objects`, `packages`, `modules`, `input_summary`, `command_summaries`, etc.

```python
state = sui_move_extractor.walrus_analyze_replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    239615926,
    verbose=True,
)
print(f"Commands: {state['commands']}, Objects: {state['objects']}, Packages: {state['packages']}")
```

### gRPC/Hybrid Replay

#### `analyze_replay(digest, *, rpc_url=..., source="hybrid", allow_fallback=True, prefetch_depth=3, prefetch_limit=200, auto_system_objects=True, no_prefetch=False, verbose=False)`

Analyze replay state hydration using gRPC/hybrid sources. Requires `SUI_GRPC_API_KEY` env var for gRPC access.

**Returns:** Same structure as `walrus_analyze_replay`.

```python
import os
os.environ["SUI_GRPC_API_KEY"] = "your-key"
state = sui_move_extractor.analyze_replay("DigestHere...")
```

#### `replay(digest, *, rpc_url=..., compare=False, verbose=False)`

Execute a full VM replay of a historical transaction. Shells out to the `sui-sandbox` CLI binary (must be in `PATH`).

**Returns:** `dict` with full replay results including effects comparison when `compare=True`.

```python
result = sui_move_extractor.replay("DigestHere...", compare=True)
print(f"Status: {result['status']}")
```

## Platform Support

Pre-built wheels are available for:
- Linux x86_64 (glibc 2.17+)
- Linux aarch64 (glibc 2.17+)
- macOS x86_64 (10.12+)
- macOS aarch64 (11.0+)

Building from source requires Rust 1.75+ and Python 3.9+.

## License

Apache 2.0
