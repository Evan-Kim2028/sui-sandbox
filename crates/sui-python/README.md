# sui-sandbox

Python bindings for Sui Move package analysis, transaction replay, view function execution, and Move function fuzzing.

Built on [sui-sandbox](../../README.md) — runs the real Sui Move VM locally via PyO3. **All functions are standalone** and execute in-process from Python (no CLI subprocess passthrough). With published wheels, `pip install sui-sandbox` is all you need.

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

## Naming

Canonical API families now mirror CLI naming:

- `context_*` (alias-compatible with `context_prepare` / replay helpers)
- `adapter_*` (alias-compatible with `protocol_*`)
- `pipeline_*` (alias-compatible with `workflow_*`)

Backwards-compatible aliases remain available.

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

#### `doctor(*, rpc_url="https://archive.mainnet.sui.io:443", state_file=None, timeout_secs=20, include_toolchain_checks=False)`

Run native preflight checks (CLI parity for `sui-sandbox doctor`) and return a structured report.

```python
report = sui_sandbox.doctor(timeout_secs=15)
print(report["ok"], report["passed"], report["failed"])
for check in report["checks"]:
    print(check["id"], check["passed"], check["detail"])
```

#### `session_status(*, state_file=None, rpc_url="https://archive.mainnet.sui.io:443")`

Return local sandbox session status (CLI parity for `sui-sandbox status`).

```python
status = sui_sandbox.session_status()
print(status["packages_loaded"], status["objects_loaded"], status["state_file"])
```

#### `session_reset(*, state_file=None)` / `session_clean(*, state_file=None)`

- `session_reset`: reset to a clean baseline state file (CLI parity for `reset`)
- `session_clean`: remove the state file (CLI parity for `clean`)

```python
sui_sandbox.session_reset()
sui_sandbox.session_clean()
```

#### `snapshot_save(name, *, description=None, state_file=None)` / `snapshot_load(name, *, state_file=None)` / `snapshot_list()` / `snapshot_delete(name)`

Snapshot lifecycle APIs with CLI parity for `sui-sandbox snapshot save|load|list|delete`.

```python
sui_sandbox.snapshot_save("pre_test", description="before risky replay")
print(sui_sandbox.snapshot_list())
sui_sandbox.snapshot_load("pre_test")
sui_sandbox.snapshot_delete("pre_test")
```

#### `ptb_universe(*, source="walrus", latest=10, top_packages=8, max_ptbs=20, out_dir=None, grpc_endpoint=None, stream_timeout_secs=120)`

Run the checkpoint-source PTB universe engine from Python (same core engine as
the Rust `walrus_ptb_universe` example wrapper). Artifacts are written to
`out_dir` and returned in the response.

```python
run = sui_sandbox.ptb_universe(
    latest=1,
    top_packages=1,
    max_ptbs=1,
)
print(run["out_dir"])
print(run["artifacts"]["summary"])
```

#### `discover_checkpoint_targets(*, checkpoint=None, latest=None, package_id=None, include_framework=False, limit=200, walrus_network="mainnet", walrus_caching_url=None, walrus_aggregator_url=None)`

Discover replay candidates directly from checkpoint Move calls.

Use this when you want a package-first flow:
1) discover digest/checkpoint candidates,
2) prepare package context,
3) replay with your input/state data.

`walrus_network` defaults to `mainnet`; set to `testnet` or provide both custom endpoint URLs.

**Returns:** `dict` with scan summary and `targets` entries:
- `checkpoint`, `digest`, `sender`
- `package_ids`
- `move_calls` (`command_index`, `package`, `module`, `function`)

```python
targets = sui_sandbox.discover_checkpoint_targets(
    latest=5,
    package_id="0x2",
    limit=20,
)
print(f"matches={targets['matches']}")
for t in targets["targets"][:3]:
    print(t["checkpoint"], t["digest"], t["package_ids"])

# optional Walrus endpoint control (testnet/custom):
targets_testnet = sui_sandbox.discover_checkpoint_targets(
    latest=3,
    walrus_network="testnet",
)
```

#### `adapter_discover(*, protocol="generic", package_id=None, checkpoint=None, latest=None, include_framework=False, limit=200, walrus_network="mainnet", walrus_caching_url=None, walrus_aggregator_url=None)` (alias: `protocol_discover`)

Protocol-first discovery wrapper:
- requires `package_id` for non-generic protocols

```python
targets = sui_sandbox.adapter_discover(
    protocol="deepbook",
    package_id="0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    latest=5,
    limit=20,
)
print(targets["matches"])
```

#### `pipeline_validate(spec_path)` (alias: `workflow_validate`)

Validate a typed pipeline spec (JSON or YAML) and return step counts.

```python
summary = sui_sandbox.pipeline_validate("examples/data/workflow_replay_analyze_demo.json")
print(summary["steps"], summary["replay_steps"], summary["analyze_replay_steps"])
```

#### `pipeline_init(*, template="generic", output_path=None, format=None, digest=None, checkpoint=None, include_analyze_step=True, strict_replay=True, name=None, package_id=None, view_objects=[], force=False)` (alias: `workflow_init`)

Generate a typed pipeline spec from built-in planners (`generic`, `cetus`, `suilend`, `scallop`).

```python
spec = sui_sandbox.pipeline_init(
    template="suilend",
    output_path="workflow.suilend.yaml",
    format="yaml",
    force=True,
)
print(spec["output_file"], spec["steps"])
```

#### `pipeline_auto(package_id, *, template=None, output_path=None, format=None, digest=None, discover_latest=None, checkpoint=None, name=None, best_effort=False, force=False, walrus_network="mainnet", walrus_caching_url=None, walrus_aggregator_url=None)` (alias: `workflow_auto`)

Auto-generate a package-first draft adapter pipeline with closure checks and optional replay seed discovery.

```python
# scaffold-only draft
draft = sui_sandbox.pipeline_auto("0x2", output_path="workflow.auto.2.json", force=True)
print(draft["replay_steps_included"], draft["template"])

# replay-capable draft using checkpoint discovery
draft_replay = sui_sandbox.pipeline_auto(
    "0x2",
    discover_latest=25,
    output_path="workflow.auto.2.replay.json",
    force=True,
)
print(draft_replay["replay_seed_source"], draft_replay["discovered_checkpoint"])
```

#### `pipeline_run(spec_path, *, dry_run=False, continue_on_error=False, report_path=None, rpc_url="https://archive.mainnet.sui.io:443", walrus_network="mainnet", walrus_caching_url=None, walrus_aggregator_url=None, verbose=False)` (alias: `workflow_run`)

Execute a typed pipeline spec natively from Python (no CLI passthrough).

```python
report = sui_sandbox.pipeline_run(
    "workflow.auto.2.replay.json",
    report_path="out/workflow_report.json",
)
print(report["succeeded_steps"], report["failed_steps"])
```

#### `pipeline_run_inline(spec, *, dry_run=False, continue_on_error=False, report_path=None, rpc_url="https://archive.mainnet.sui.io:443", walrus_network="mainnet", walrus_caching_url=None, walrus_aggregator_url=None, verbose=False)` (alias: `workflow_run_inline`)

Execute a typed pipeline from an in-memory Python object (no temp spec file).

Local-cache pipeline replay/analyze is supported in native mode:

```python
sui_sandbox.import_state(state="examples/data/state_json_synthetic_ptb_demo.json")
spec = {
    "version": 1,
    "defaults": {"source": "local"},
    "steps": [
        {"kind": "analyze_replay", "digest": "synthetic_make_move_vec_demo"},
        {"kind": "replay", "digest": "synthetic_make_move_vec_demo"},
    ],
}
report = sui_sandbox.pipeline_run_inline(spec)
print(report["succeeded_steps"], report["failed_steps"])  # 2, 0
```

Supported pipeline native controls include:
- replay: `profile`, `fetch_strategy`, `vm_only`, `synthesize_missing`, `self_heal_dynamic_fields`
- analyze_replay: `mm2`

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

#### `context_prepare(package_id, *, resolve_deps=True, output_path=None)` (alias: `prepare_package_context`)

Prepare a portable package context payload for replay workflows.

**Returns:** `dict` with:
- `version`, `package_id`, `with_deps` (plus compatibility mirror `resolve_deps`)
- `generated_at_ms`, `packages_fetched`, `count`
- `packages` as a v2 array of:
  - `address`
  - `modules`
  - `bytecodes` (base64)
- `count`

```python
ctx = sui_sandbox.context_prepare("0x2", output_path="context.json")
print(ctx["count"])
```

#### `adapter_prepare(*, protocol="generic", package_id=None, resolve_deps=True, output_path=None)` (alias: `protocol_prepare`)

Protocol-first prepare wrapper (non-generic protocols require `package_id`):

```python
ctx = sui_sandbox.adapter_prepare(
    protocol="deepbook",
    package_id="0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    output_path="context.deepbook.json",
)
print(ctx["package_id"], ctx["count"])
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

#### `historical_view_from_versions(*, versions_file, package_id, module, function, required_objects, type_args=[], package_roots=[], type_refs=[], fetch_child_objects=True, grpc_endpoint=None, grpc_api_key=None)`

Generic historical view execution helper.

- loads object versions/checkpoint from snapshot JSON
- hydrates required historical objects via gRPC
- hydrates checkpoint-pinned package closure
- executes the specified Move function locally

```python
out = sui_sandbox.historical_view_from_versions(
    versions_file="examples/data/deepbook_margin_state/deepbook_versions_240733000.json",
    package_id="0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    module="margin_manager",
    function="manager_state",
    required_objects=[...],
    type_args=["0x2::sui::SUI", "<USDC_TYPE>"],
)
print(out["success"], out["gas_used"])
print(out["raw"]["return_values"])
```

When archive endpoints miss runtime objects, the result includes a retry `hint`.

Historical replay also attempts dynamic-field auto-hydration by default. Optional env controls:
`SUI_HISTORICAL_AUTO_HYDRATE_DYNAMIC_FIELDS`, `SUI_HISTORICAL_DYNAMIC_FIELD_DEPTH`,
`SUI_HISTORICAL_DYNAMIC_FIELD_LIMIT`, `SUI_HISTORICAL_DYNAMIC_FIELD_MAX_OBJECTS`,
`SUI_HISTORICAL_DYNAMIC_FIELD_PARENT_SCAN_LIMIT`, `SUI_HISTORICAL_DYNAMIC_FIELD_LOG`.

#### `historical_series_from_points(*, points, package_id, module, function, required_objects, type_args=[], package_roots=[], type_refs=[], fetch_child_objects=True, schema=None, command_index=0, grpc_endpoint=None, grpc_api_key=None, max_concurrency=1)`

Execute the same historical view request across multiple checkpoint/version points.

- `points`: list of `{checkpoint, versions, label?, metadata?}`
- `schema`: optional decode schema (same shape as `historical_decode_with_schema`)
- returns `{request, points, summary, runs}` with per-point raw output and optional decoded fields

```python
points = [
    {"checkpoint": 240732600, "versions": {"0x6": 129466613}, "label": "day-1"},
    {"checkpoint": 240733000, "versions": {"0x6": 129466871}, "label": "day-2"},
]
series = sui_sandbox.historical_series_from_points(
    points=points,
    package_id="0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    module="margin_manager",
    function="manager_state",
    required_objects=[...],
    type_args=["0x2::sui::SUI", "<USDC_TYPE>"],
    schema=[{"index": 2, "name": "risk_ratio_pct", "type_hint": "u64", "scale": 1e7}],
    max_concurrency=4,
)
print(series["summary"])
```

#### `historical_series_from_files(*, request_file, series_file, schema_file=None, command_index=0, grpc_endpoint=None, grpc_api_key=None, max_concurrency=1)`

File-driven historical-series runner (parity with Rust CLI `context historical-series`).

- `request_file`: JSON/YAML `HistoricalViewRequest`
- `series_file`: JSON/YAML points payload (`points` or `daily_snapshots`)
- `schema_file`: optional JSON/YAML list of decode schema fields
- returns `{request, points, runs, summary}`

```python
report = sui_sandbox.historical_series_from_files(
    request_file="examples/data/deepbook_margin_state/manager_state_request.json",
    series_file="examples/data/deepbook_margin_state/position_b_daily_timeseries.json",
    schema_file="examples/data/deepbook_margin_state/manager_state_schema.json",
    max_concurrency=4,
)
print(report["summary"])
```

#### `historical_decode_return_u64(result, *, command_index=0, value_index)`

Decode a single little-endian `u64` return value from `historical_view_from_versions` output.
The decoder accepts either the full result object or `result["raw"]`.

Returns `None` when execution failed or index is missing.

```python
risk_ratio = sui_sandbox.historical_decode_return_u64(out, value_index=2)
current_px = sui_sandbox.historical_decode_return_u64(out, value_index=11)
```

#### `historical_decode_return_u64s(result, *, command_index=0)`

Decode all command return values into `u64` slots where possible.
The decoder accepts either the full result object or `result["raw"]`.

Returns `None` when execution failed or no command return values are available.

```python
vals = sui_sandbox.historical_decode_return_u64s(out) or []
print(vals[2], vals[11])  # risk_ratio, current_price
```

#### `historical_decode_returns_typed(result, *, command_index=0)`

Decode command return values into typed JSON using `return_type_tags` when available.
The decoder accepts either the full result object or `result["raw"]`.

Returns `None` when execution failed or no command return values are available.

```python
typed = sui_sandbox.historical_decode_returns_typed(out) or []
for item in typed:
    print(item["index"], item["type_tag"], item["value"])
```

#### `historical_decode_with_schema(result, schema, *, command_index=0)`

Decode command return values into a named object using a field schema:
- `index`: return tuple index
- `name`: output key
- `type_hint`: optional decode override (`u64`, `address`, `vector<u8>`, `utf8`, `hex`, `base64`, ...)
- `scale`: optional numeric divisor

Returns `None` when execution failed or no command return values are available.
The decoder accepts either the full result object or `result["raw"]`.

```python
schema = [
    {"index": 2, "name": "risk_ratio_pct", "type_hint": "u64", "scale": 1e7},
    {"index": 11, "name": "current_price", "type_hint": "u64", "scale": 1e6},
]
decoded = sui_sandbox.historical_decode_with_schema(out, schema) or {}
print(decoded.get("risk_ratio_pct"), decoded.get("current_price"))
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

#### `replay(digest=None, *, rpc_url=..., source="hybrid", checkpoint=None, state_file=None, context_path=None, cache_dir=None, profile=None, fetch_strategy=None, vm_only=False, allow_fallback=True, prefetch_depth=3, prefetch_limit=200, auto_system_objects=True, no_prefetch=False, compare=False, analyze_only=False, synthesize_missing=False, self_heal_dynamic_fields=False, analyze_mm2=False, verbose=False)`

Replay a historical Sui transaction locally with the Move VM.

Replay source modes:
- `checkpoint=...` uses Walrus (no API key needed)
- `state_file=...` replays from a local exported state file
- `source="local"` (or `cache_dir=...`) replays from imported local cache
- otherwise uses gRPC/hybrid (requires `SUI_GRPC_API_KEY`)

Use `analyze_only=True` to inspect state hydration without executing the transaction.
Use `analyze_mm2=True` with `analyze_only=True` to include MM2 model diagnostics.
Use `profile="safe"|"balanced"|"fast"` to tune runtime env defaults.
Use `fetch_strategy="eager"|"full"` (`eager` implies `no_prefetch=True`).
Use `vm_only=True` to force direct VM-path behavior (disables fallback).

Use `compare=True` to compare local execution results with on-chain effects.
Use `synthesize_missing=True` to retry replay with synthetic bytes for missing object inputs.
Use `self_heal_dynamic_fields=True` to enable dynamic field child fetchers during VM execution.

**Returns:** `dict` — replay envelope with:
- `local_success`, `execution_path`, `commands_executed`
- full replay fields (`effects`, optional `comparison`) when `analyze_only=False`
- `analysis` summary when `analyze_only=True`

For backwards compatibility, analyze summary keys (`commands`, `inputs`, `objects`, `packages`, etc.) are also exposed at top level in analyze-only mode.

```python
# Analyze state hydration only (no VM execution)
analysis = sui_sandbox.replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    analyze_only=True,
    analyze_mm2=True,
)
print(f"Commands: {analysis['analysis']['commands']}, Objects: {analysis['analysis']['objects']}")
print(f"MM2 ok: {analysis['analysis'].get('mm2_model_ok')}")

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

#### `analyze_replay(...)` (alias: `replay_analyze(...)`)

First-class hydration/readiness analysis wrapper (equivalent to `replay_transaction(..., analyze_only=True)`).

```python
analysis = sui_sandbox.analyze_replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    analyze_mm2=True,
)
print(analysis["analysis"]["missing_inputs"])
print(analysis["analysis"]["suggestions"])
```

#### `replay_effects(...)`

Execution-focused replay wrapper that returns effects/comparison/diagnostics plus a classification summary.

```python
out = sui_sandbox.replay_effects(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    compare=True,
)
print(out["local_success"], out["effects"]["gas_used"])
print(out["classification"]["category"])
```

#### `classify_replay_result(result)`

Classify replay output into structured categories (`missing_input_objects`, `archive_data_gap`, `move_abort`, ...).

```python
result = sui_sandbox.replay("DigestHere...", checkpoint=239615926)
classification = sui_sandbox.classify_replay_result(result)
print(classification["category"], classification["retryable"])
```

#### `dynamic_field_diagnostics(...)`

Analyze hydration-only replay twice (baseline `no_prefetch=True` vs prefetch-enabled)
for the same digest/checkpoint and report dynamic-field related deltas.

```python
diag = sui_sandbox.dynamic_field_diagnostics(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    prefetch_depth=3,
    prefetch_limit=200,
)
print(diag["likely_dynamic_field_dependency"])
print(diag["delta"]["objects_added_by_prefetch"])
print(diag["recommendations"])
```

#### `context_replay(digest=None, *, checkpoint=None, discover_latest=None, discover_package_id=None, source=None, state_file=None, context_path=None, cache_dir=None, walrus_network="mainnet", ..., profile=None, fetch_strategy=None, vm_only=False, analyze_only=False, synthesize_missing=False, self_heal_dynamic_fields=False, analyze_mm2=False, rpc_url=...)` (alias: `replay_transaction`)

Compact replay helper with source inference:
- if `checkpoint` is set and `source` omitted, defaults to `walrus`
- if `cache_dir` is set and `source` omitted, defaults to `local`
- otherwise defaults to `hybrid`
- if `discover_latest` is set, auto-discovers a digest/checkpoint for `discover_package_id`

```python
out = sui_sandbox.context_replay(
    digest="At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
    context_path="context.json",
)
print(out["local_success"])

# Auto-discover a replay target from latest checkpoints
out = sui_sandbox.context_replay(
    discover_latest=5,
    discover_package_id="0x2",
    context_path="context.json",
)
print(out["digest"], out["local_success"])
```

#### `adapter_run(digest=None, *, protocol="generic", package_id=None, resolve_deps=True, context_path=None, checkpoint=None, discover_latest=None, source=None, state_file=None, cache_dir=None, walrus_network="mainnet", ..., profile=None, fetch_strategy=None, vm_only=False, analyze_only=False, synthesize_missing=False, self_heal_dynamic_fields=False, analyze_mm2=False, rpc_url=...)` (alias: `protocol_run`)

One-call protocol flow: prepare context + replay.

```python
out = sui_sandbox.adapter_run(
    digest="At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    protocol="deepbook",
    package_id="0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    checkpoint=239615926,
    analyze_only=True,
)
print(out["local_success"], out["analysis"]["commands"])

# Protocol-first auto-discovery + replay
out = sui_sandbox.adapter_run(
    protocol="deepbook",
    package_id="0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
    discover_latest=5,
    analyze_only=True,
)
print(out["digest"], out["analysis"]["commands"])
```

#### `OrchestrationSession` (aliases: `FlowSession`, `ContextSession`)

In-memory two-step context helper for interactive usage:
- `prepare(...)`
- `replay(...)`
- `load_context(...)` / `save_context(...)`

```python
session = sui_sandbox.OrchestrationSession()
session.prepare("0x2")
out = session.replay(
    "At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2",
    checkpoint=239615926,
)
print(out["local_success"])

# Auto-discover digest/checkpoint using prepared package context
out = session.replay(discover_latest=5, analyze_only=True)
print(out["digest"], out["analysis"]["commands"])
```

## Platform Support

Pre-built wheels are available for:
- Linux x86_64 (glibc 2.17+)
- Linux aarch64 (glibc 2.17+)
- macOS x86_64 (10.12+)
- macOS aarch64 (11.0+)
- Windows x86_64 (MSVC)

Building from source requires Rust 1.80+ and Python 3.9+.

## License

Apache 2.0
