# Python Examples (`sui_sandbox`)

These examples use the native PyO3 extension from `crates/sui-python` (in-process native module, not CLI passthrough).

## Setup

Preferred (published wheel, no Rust toolchain needed):

```bash
pip install sui-sandbox
```

From source (repo checkout, requires Rust toolchain):

```bash
cd crates/sui-python
pip install maturin
maturin develop --release
cd ../..
```

Optional smoke check:

```bash
./scripts/python_examples_smoke.sh
./scripts/python_examples_smoke.sh --network
```

## Core Example Set (DeepBook, 3 files)

### 1) Controls: DeepBook interface extraction

```bash
python3 python_sui_sandbox/examples/02_extract_interface.py
# Optional overrides:
# BYTECODE_DIR=tests/fixture/build/fixture python3 python_sui_sandbox/examples/02_extract_interface.py
# PACKAGE_ID=0x2 RPC_URL=https://fullnode.mainnet.sui.io:443 python3 python_sui_sandbox/examples/02_extract_interface.py
```

### 2) Safety: DeepBook context preparation

```bash
python3 python_sui_sandbox/examples/03_deepbook_context_safety.py
# Optional:
# CONTEXT_PATH=examples/out/contexts/context.deepbook_margin.custom.json \
#   python3 python_sui_sandbox/examples/03_deepbook_context_safety.py
```

### 3) Power: DeepBook margin state (native bindings)

```bash
python3 python_sui_sandbox/examples/04_deepbook_margin_state_native.py
```

This example uses `historical_decode_with_schema(...)` to decode historical
view return values into a named object without manual index unpacking.

## Optional Utility Example

### Walrus checkpoint summary

```bash
python3 python_sui_sandbox/examples/01_walrus_checkpoint.py
```

## Canonical API Names

Primary naming now mirrors CLI naming:
- `context_prepare`, `context_replay`, `context_run`, `context_discover`
- `adapter_prepare`, `adapter_run`, `adapter_discover`
- `pipeline_validate`, `pipeline_init`, `pipeline_auto`, `pipeline_run`, `pipeline_run_inline`
- `session_status`, `session_reset`, `session_clean`
- `snapshot_save`, `snapshot_load`, `snapshot_list`, `snapshot_delete`
- `doctor`, `analyze_replay`, `replay_effects`, `classify_replay_result`, `dynamic_field_diagnostics`

Compatibility aliases remain available:
- `prepare_package_context` (`context_prepare`)
- `protocol_*` (`adapter_*`)
- `workflow_*` (`pipeline_*`)

`OrchestrationSession` is the canonical session helper. `FlowSession` and `ContextSession` remain compatibility aliases.

## CLI parity

Canonical CLI names:
- `context` (alias: `flow`)
- `adapter` (alias: `protocol`)
- `pipeline` (alias: `workflow`)

Compatibility CLI commands:
- `script` (alias: `run-flow`) for legacy YAML scripts
- `init` for legacy script template scaffolding

Example:

```bash
sui-sandbox context run --package-id 0x2 --discover-latest 5 --analyze-only
```
