# Python Examples (`sui_sandbox`)

These examples use the native PyO3 extension from `crates/sui-python`.

## Setup

From repo root:

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

## Core Example Set (4 files)

### 1) Walrus checkpoint summary

```bash
python3 python_sui_sandbox/examples/01_walrus_checkpoint.py
python3 python_sui_sandbox/examples/01_walrus_checkpoint.py --checkpoint 239615926 --tx-limit 3
```

### 2) Package interface extraction

```bash
python3 python_sui_sandbox/examples/02_extract_interface.py --package-id 0x2
```

### 3) Context replay flow (native bindings)

```bash
python3 python_sui_sandbox/examples/03_context_replay_native.py --package-id 0x2 --discover-latest 1 --analyze-only
python3 python_sui_sandbox/examples/03_context_replay_native.py --package-id 0x2 --state-file examples/data/state_json_synthetic_ptb_demo.json
```

### 4) DeepBook margin state (native bindings)

```bash
python3 python_sui_sandbox/examples/04_deepbook_margin_state_native.py
```

## Canonical API Names

Primary naming now mirrors CLI naming:
- `context_prepare`, `context_replay`, `context_run`, `context_discover`
- `adapter_prepare`, `adapter_run`, `adapter_discover`
- `pipeline_validate`, `pipeline_init`, `pipeline_auto`, `pipeline_run`, `pipeline_run_inline`

Compatibility aliases remain available:
- `prepare_package_context` (`context_prepare`)
- `protocol_*` (`adapter_*`)
- `workflow_*` (`pipeline_*`)

`FlowSession` and `ContextSession` are equivalent session helpers (`ContextSession` is the naming-parity alias).

## CLI parity

Canonical CLI names:
- `context` (alias: `flow`)
- `adapter` (alias: `protocol`)
- `script` (alias: `run-flow`)
- `pipeline` (alias: `workflow`)

Example:

```bash
sui-sandbox context run --package-id 0x2 --discover-latest 5 --analyze-only
```
