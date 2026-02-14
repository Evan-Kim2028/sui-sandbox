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

## Workflow CLI (Direct)

For workflow orchestration, use direct CLI commands:

```bash
sui-sandbox workflow auto --package-id 0x2 --output examples/out/workflow_auto/workflow.auto.2.json --force
sui-sandbox workflow validate --spec examples/out/workflow_auto/workflow.auto.2.json
sui-sandbox workflow run --spec examples/out/workflow_auto/workflow.auto.2.json --dry-run
```

## Native Margin Example (No CLI Pass-Through)

### 4) DeepBook `manager_state` (native bindings only)

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
