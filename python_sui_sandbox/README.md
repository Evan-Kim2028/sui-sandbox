# Python Examples (sui_sandbox)

These examples target the PyO3 extension in `crates/sui-python`.

## Setup

From repo root:

```bash
cd crates/sui-python
pip install maturin
maturin develop --release
cd ../..
```

If your default Python is `3.14`, build in a `3.13` virtualenv (current PyO3 compatibility limit):

```bash
cd crates/sui-python
python3.13 -m venv .venv313
. .venv313/bin/activate
pip install maturin
maturin develop --release
cd ../..
```

Optional smoke check:

```bash
./scripts/python_examples_smoke.sh
# include network execution for examples 01 and 03
./scripts/python_examples_smoke.sh --network
# override interpreter explicitly
PYTHON_BIN=python3.13 ./scripts/python_examples_smoke.sh
```

## Core Examples (Start Here)

### 1) Walrus checkpoint summary

```bash
python3 python_sui_sandbox/examples/01_walrus_checkpoint.py
python3 python_sui_sandbox/examples/01_walrus_checkpoint.py --checkpoint 239615926 --tx-limit 3
```

### 2) Package interface extraction

```bash
python3 python_sui_sandbox/examples/02_extract_interface.py --package-id 0x2
```

### 3) Replay analyze (no VM execution)

```bash
python3 python_sui_sandbox/examples/03_replay_analyze.py
python3 python_sui_sandbox/examples/03_replay_analyze.py --digest <DIGEST> --checkpoint <CP>
```

## Optional Workflow Pass-Through Examples

Use these after the core 3 examples if you want typed workflow orchestration from Python.

### 4) Typed workflow pass-through (Python -> Rust CLI)

```bash
python3 python_sui_sandbox/examples/04_workflow_passthrough.py
python3 python_sui_sandbox/examples/04_workflow_passthrough.py --run
```

This example intentionally keeps Python thin:

- Python builds a workflow JSON spec.
- Rust CLI (`sui-sandbox workflow`) performs validation and execution.
- The run report is emitted as JSON for downstream tooling.

### 5) Built-in workflow template pass-through

```bash
python3 python_sui_sandbox/examples/05_workflow_init_template.py --template cetus
python3 python_sui_sandbox/examples/05_workflow_init_template.py --template suilend --package-id 0x2 --view-object 0x6 --view-object 0x8
python3 python_sui_sandbox/examples/05_workflow_init_template.py --from-config examples/data/workflow_init_suilend.yaml
```

This flow delegates planning to Rust:

- Python calls `workflow init` with template + optional protocol context.
- Rust emits the typed spec.
- Python calls `workflow validate` and `workflow run`.

### 6) Workflow auto from package id (Python -> Rust CLI)

```bash
python3 python_sui_sandbox/examples/07_workflow_auto_from_package.py --package-id 0x2
python3 python_sui_sandbox/examples/07_workflow_auto_from_package.py --package-id 0x2 --digest <DIGEST> --checkpoint <CP>
python3 python_sui_sandbox/examples/07_workflow_auto_from_package.py --package-id 0xdeadbeef --best-effort
```

This example mirrors the Rust `workflow_auto_from_package` flow and runs:

- `workflow auto`
- `workflow validate`
- `workflow run --dry-run` (or `--run`)

## Native Margin Example (No CLI Pass-Through)

### 7) DeepBook `manager_state` (native bindings only)

```bash
python3 python_sui_sandbox/examples/06_deepbook_margin_state_native.py
python3 python_sui_sandbox/examples/06_deepbook_margin_state_native.py --grpc-endpoint https://grpc.surflux.dev:443
```

This path mirrors the Rust DeepBook flow with Python bindings only:

1. Load versions JSON snapshot.
2. Fetch historical object BCS via `fetch_object_bcs(...)`.
3. Fetch checkpoint-pinned package bytecodes/dependencies via `fetch_historical_package_bytecodes(...)`.
4. Execute `call_view_function(...)` with on-demand child-object fetch enabled (`fetch_deps=False` because deps are already in the historical package payload).

## DeepBook Spot Pool + Orders

For now, the full multi-step offline PTB flow (create permissionless pool, deposit, place orders, then query state) is available as a Rust example:

```bash
cargo run --example deepbook_spot_offline_ptb
```

Current Python bindings expose single-call `call_view_function(...)`, but not a multi-command PTB builder with cross-command object chaining yet.

## Margin State Direction

To keep Rust and Python margin logic aligned 1:1, use the same phase model in both languages:

1. Load snapshot spec (object IDs + versions + call target).
2. Hydrate object BCS and package bytecode.
3. Execute `manager_state` view call.
4. Decode and format return values.

This keeps behavior equivalent while still allowing each language to stay readable and explicit.
