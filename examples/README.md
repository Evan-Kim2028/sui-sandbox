# Examples - Core Onboarding Set

This index is intentionally small. It tracks a single Rust/Python path with
focused examples and one protocol-rich Rust demo.

Canonical CLI names are:
- `context` (alias: `flow`)
- `adapter` (alias: `protocol`)
- `pipeline` (alias: `workflow`)

Compatibility CLI commands:
- `script` (alias: `run-flow`) for legacy YAML flow files
- `init` for legacy flow template scaffolding

## Core Set

### 1) Walrus checkpoint summary

Rust CLI:

```bash
sui-sandbox fetch latest-checkpoint
sui-sandbox --json fetch checkpoint 239615926
```

Python:

```bash
python3 python_sui_sandbox/examples/01_walrus_checkpoint.py
```

### 2) Package interface extraction

Rust CLI:

```bash
sui-sandbox --json analyze package --package-id 0x2 --list-modules
```

Python:

```bash
python3 python_sui_sandbox/examples/02_extract_interface.py
# Optional override:
# BYTECODE_DIR=tests/fixture/build/fixture python3 python_sui_sandbox/examples/02_extract_interface.py
```

### 3) Context replay flow

Rust CLI:

```bash
sui-sandbox context run --package-id 0x2 --digest <DIGEST> --checkpoint <CP>
sui-sandbox context run --package-id 0x2 --state-json examples/data/state_json_synthetic_ptb_demo.json
```

Rust example binary:

```bash
cargo run --example state_json_offline_replay
```

Python:

```bash
python3 python_sui_sandbox/examples/03_context_replay_native.py
# Optional overrides:
# PACKAGE_ID=0x2 DIGEST=<DIGEST> CHECKPOINT=<CP> python3 python_sui_sandbox/examples/03_context_replay_native.py
```

### 4) DeepBook margin state

Rust:

```bash
cargo run --example deepbook_margin_state
```

Core API: `sui_sandbox_core::orchestrator::ReplayOrchestrator::execute_historical_view_from_versions(...)`

Python:

```bash
python3 python_sui_sandbox/examples/04_deepbook_margin_state_native.py
```

### 5) DeepBook margin time series (Rust)

Rust:

```bash
cargo run --example deepbook_timeseries
```

Optional override:

```bash
TIMESERIES_FILE=examples/data/deepbook_margin_state/position_b_daily_timeseries.json \
REQUEST_FILE=examples/data/deepbook_margin_state/manager_state_request.json \
SCHEMA_FILE=examples/data/deepbook_margin_state/manager_state_schema.json \
MAX_CONCURRENCY=4 \
  cargo run --example deepbook_timeseries
```

Core API: `sui_sandbox_core::orchestrator::ReplayOrchestrator::execute_historical_series_from_files_with_options(...)`

CLI equivalent:

```bash
sui-sandbox context historical-series \
  --request-file examples/data/deepbook_margin_state/manager_state_request.json \
  --series-file examples/data/deepbook_margin_state/position_b_daily_timeseries.json \
  --schema-file examples/data/deepbook_margin_state/manager_state_schema.json \
  --max-concurrency 4
```

Historical view/series execution now auto-hydrates dynamic-field wrapper objects by default.
Useful tuning knobs:

```bash
SUI_HISTORICAL_AUTO_HYDRATE_DYNAMIC_FIELDS=0   # disable auto-hydration
SUI_HISTORICAL_DYNAMIC_FIELD_DEPTH=2            # recursive parent depth
SUI_HISTORICAL_DYNAMIC_FIELD_LIMIT=64           # per-parent dynamic field scan cap
SUI_HISTORICAL_DYNAMIC_FIELD_MAX_OBJECTS=512    # max prefetched wrapper objects
SUI_HISTORICAL_DYNAMIC_FIELD_PARENT_SCAN_LIMIT=512  # parent-ID discovery cap from object JSON
SUI_HISTORICAL_DYNAMIC_FIELD_LOG=1              # emit hydration diagnostics
```

### 6) DeepBook spot offline PTB (Rust)

Rust:

```bash
cargo run --example deepbook_spot_offline_ptb
```

This regular example demonstrates a full protocol PTB flow (pool creation +
orders) using first-class orchestrator/bootstrap helpers.

## Additional Rust Advanced Example

The PTB universe engine remains available as an advanced Rust example:

```bash
cargo run --example walrus_ptb_universe -- --latest 1 --top-packages 1 --max-ptbs 1
```

Core engine location: `crates/sui-sandbox-core/src/ptb_universe.rs`.

## Smoke Checks

```bash
./scripts/rust_examples_smoke.sh
./scripts/python_examples_smoke.sh
```
