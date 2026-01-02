## `benchmark/` (benchmarks)

This folder contains two benchmarks:

- **Phase I (key-struct discovery)**: ask an LLM to predict which structs have `key`.
- **Phase II (type inhabitation)**: build a PTB plan, dry-run (ground truth) or dev-inspect (fallback), and score created key types.

It uses `sui-move-interface-extractor` as the ground-truth parser for `.mv` bytecode artifacts.

## AgentBeats / Berkeley “Green Agent”

This benchmark suite is intended to plug into AgentBeats-style evaluation (Berkeley RDI AgentX):

https://rdi.berkeley.edu/agentx-agentbeats

Phase I already has:

- deterministic targets (bytecode-derived `key` structs)
- mechanical scoring (precision/recall/F1)
- resumable batch execution over a corpus

Phase II simulates the model’s PTB plan and scores it by created object types:

- Ground truth mode: `--require-dry-run` with a funded sender (dry-run returns `objectChanges.objectType`)
- Best-effort mode: omit `--require-dry-run` and fall back to dev-inspect + bytecode proxy scoring

Phase II also supports bounded, deterministic “local adaptation” without an LLM reprompt:

- Gas retry ladder on `InsufficientGas`: `--gas-budget-ladder`
- Heuristic PTB plan variants (e.g., replace placeholder addresses with `--sender`, try small integer literals): `--max-heuristic-variants`

For Phase II dry-run, set a sender with gas in `benchmark/.env` (or pass flags explicitly):

- `SMI_SENDER=0x...` (address with at least one `Coin<SUI>` on that network)
- `SMI_GAS_COIN=0x...` (optional; pins a specific gas coin object id)

### Setup

```bash
cd benchmark
uv sync --group dev --frozen
```

If `uv` is unavailable in your environment, you can still run the tools with:

- `PYTHONPATH=src python3 -m pytest`
- `PYTHONPATH=src python3 -m smi_bench.inhabit_runner --help`
- `PYTHONPATH=src python3 -m smi_bench.inhabit_manifest --help`

Build the Rust binaries used by the benchmarks:

```bash
cd ..
cargo build --release --locked
cd benchmark
```

### Dataset

You need a local checkout of the Sui bytecode corpus:

```bash
# Shallow clone is sufficient and faster
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git
```

Benchmarks typically target the `mainnet_most_used` subset: `sui-packages/packages/mainnet_most_used`.

### Standard Benchmarks

We provide canonical benchmark manifests in `benchmark/manifests/` to ensure consistent evaluation.

**Phase II Standard Set (n=292):** `benchmark/manifests/standard_phase2_benchmark.txt`

- **Why this list?** The full corpus (~1000 packages) is mostly non-viable for simple transaction simulation (requiring complex admin caps or specific setups).
- **Origin:** This subset contains ~292 packages from `mainnet_most_used` that have been verified to expose at least one "inhabitable" public entry function compatible with the harness.
- **Usage:** Always use this list for Phase II leaderboard runs to avoid 98% rejection rates.

### Configure a real agent (optional)

Copy `benchmark/.env.example` to `benchmark/.env` and fill in:

- `SMI_API_KEY`
- `SMI_MODEL`
- `SMI_API_BASE_URL` (OpenAI-compatible base; for non-OpenAI providers)

Z.AI GLM-4.7 example:

```bash
cp .env.example .env
# set:
# If you’re on the GLM Coding Plan, use:
# SMI_API_BASE_URL=https://api.z.ai/api/coding/paas/v4
# (If you’re on Model API instead, use https://api.z.ai/api/paas/v4)
# SMI_MODEL=glm-4.7
# SMI_THINKING=enabled
# SMI_CLEAR_THINKING=true
# SMI_RESPONSE_FORMAT=json_object
# SMI_MAX_TOKENS=2048
```

Smoke test (does a single tiny API call and expects `{ "key_types": [] }` JSON):

```bash
uv run smi-bench --corpus-root <sui-packages-checkout>/packages/mainnet_most_used --smoke-agent --agent real-openai-compatible
```

Diagnostics (prints redacted config and probes `GET /models` + `POST /chat/completions`):

```bash
uv run smi-bench --corpus-root <sui-packages-checkout>/packages/mainnet_most_used --doctor-agent --agent real-openai-compatible
```

### Run (Unified Workflow)

The simplest way to benchmark an agent (or the baseline) is the `run-all` command, which executes Phase I and Phase II sequentially.

```bash
uv run smi-bench run-all \
  --corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir results/my_run_01 \
  --samples 100 \
  --sender <FUNDED_MAINNET_ADDRESS> \
  --rpc-url https://fullnode.mainnet.sui.io:443
```

This will:
1.  **Phase I**: Run Key Struct Discovery (static analysis).
2.  **Phase II Manifest**: Select viable candidates from the Phase I corpus.
3.  **Phase II Execution**: Attempt to execute transactions (using `dry-run` if sender is provided, else `build-only`).

Artifacts will be saved in `results/my_run_01/`.

### Run Phase II (Standard Benchmark)

To run the standard Phase II benchmark against the canonical 292 viable packages:

```bash
uv run smi-inhabit \
  --corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --package-ids-file manifests/standard_phase2_benchmark.txt \
  --agent real-openai-compatible \
  --out results/phase2_standard_run.json \
  --rpc-url https://fullnode.mainnet.sui.io:443 \
  --continue-on-error
```

### Run (Legacy / Modular)

You can still run individual tools for debugging or advanced use cases.

**Phase I only:**
```bash
uv run smi-phase1 \
  --corpus-root ... \
  --samples 25 \
  --agent mock-empty \
  --out results/phase1.json
```

**Phase II only:**
```bash
# 1. Generate Manifest
uv run smi-phase2-manifest \
  --corpus-root ... \
  --out-ids results/p2_ids.txt \
  --out-plan results/p2_plan.json \
  --out-report results/p2_report.json

# 2. Run Execution
uv run smi-inhabit \
  --corpus-root ... \
  --package-ids-file results/p2_ids.txt \
  --agent baseline-search \
  --simulation-mode dry-run \
  --out results/p2_exec.json
```

### Output

The runner writes a JSON report with:

- `aggregate`: average precision/recall/F1 and error count
- `packages[]`: per-package scores and telemetry (`elapsed_seconds`, `timed_out`, etc.)

Use:

```bash
python scripts/phase1_status.py results/<file>.json
python scripts/phase1_analyze.py --run-json results/<file>.json
python scripts/phase1_analyze.py --run-json results/<file>.json --show <package_id>
```

### Phase I methodology (key-struct discovery)

- Treat bytecode-derived `abilities` as ground truth.
- Hide `abilities` from the model prompt to avoid trivial extraction.
- Ask the model to output `{"key_types":[...]}`
- Score predictions against truth using precision/recall/F1.
- Expect timeouts and partial progress on large packages; the runner records `timed_out` and `error` per package and supports resume.

### Running 500 packages (first half) in batches of 5

Generate deterministic manifests (first 500 ids + remaining ids) from your local corpus:

```bash
uv run python scripts/make_mainnet_most_used_halves.py \
  --corpus-root <sui-packages-checkout>/packages/mainnet_most_used
```

Run Phase I for the first 500 ids in sequential 5-package batches (2-minute per-package timeout, resume-safe):

```bash
./scripts/run_phase1_manifest_batches.sh \
  <sui-packages-checkout>/packages/mainnet_most_used \
  results/manifests/mainnet_most_used_first500_ids.txt \
  results/phase1_first500_glm47.json
```

Check what is done vs remaining:

```bash
python scripts/manifest_remaining.py \
  --manifest results/manifests/mainnet_most_used_first500_ids.txt \
  --out-json results/phase1_first500_glm47.json
```

Later, run the remaining 500 ids by swapping the manifest:

```bash
./scripts/run_phase1_manifest_batches.sh \
  <sui-packages-checkout>/packages/mainnet_most_used \
  results/manifests/mainnet_most_used_remaining500_ids.txt \
  results/phase1_remaining500_glm47.json
```

### Phase II scaffold (type inhabitation)

`smi-inhabit` is the Phase II runner for the “PTB type inhabitation” benchmark (build PTB → dry-run/dev-inspect → score by created key types).

It runs transaction simulation via the Rust helper binary `smi_tx_sim`.

Phase II prefers **dry-run** to get transaction-ground-truth created object types from `objectChanges.objectType`.

If dry-run cannot run (most commonly because the sender has no gas coin on the target network), it can fall back to dev-inspect + a
bytecode-first proxy: scan called function bytecode for `0x2::transfer::transfer<T>` / `public_transfer<T>` / `share_object<T>` and treat those `T` as “created types”.

Offline sanity (no RPC): `smi_tx_sim --mode build-only` validates the PTB spec and emits `programmableTransactionBcsBase64`.

#### PTB spec schema (minimal)

The PTB spec is JSON:

- `{"calls":[{"target":"0xADDR::module::function","type_args":[...],"args":[...]}]}`

Supported `args` entries:

- Pure values:
  - `{"u8": 1}`, `{"u16": 1}`, `{"u32": 1}`, `{"u64": 1}`, `{"bool": true}`
  - `{"address": "0x..."}`
  - `{"vector_u8_utf8": "text"}`, `{"vector_u8_hex": "0xdeadbeef"}`
  - `{"vector_address": ["0x1","0x2"]}`
  - `{"vector_bool": [true,false]}`, `{"vector_u16": [1,2]}`, `{"vector_u32": [1,2]}`, `{"vector_u64": [1,2]}`
- Object values (resolved via RPC):
  - `{"imm_or_owned_object": "0x<OBJECT_ID>"}`
  - `{"shared_object": {"id":"0x<OBJECT_ID>","mutable": true}}`

Supported system objects:
- `0x6::clock::Clock` (shared, 0x6)
- `0x8::random::Random` (shared, 0x8)
- `0x403::deny_list::DenyList` (shared, 0x403)

#### Executable subset policy (Phase II)

Most mainnet packages cannot be executed by an arbitrary sender (they require caps, shared objects, or specific state).

Phase II therefore records per-package failures and supports two modes:

- Ground truth mode: `--require-dry-run` (needs a funded sender on the target network)
- Best-effort mode: omit `--require-dry-run` (falls back to proxy scoring)

Phase II output includes per-package robustness flags:

- `ptb_parse_ok`: PTB spec was present and parseable
- `tx_build_ok`: tx simulation helper ran successfully
- `dry_run_ok`: dry-run succeeded (ground truth)
- `dev_inspect_ok`: dev-inspect fallback succeeded
- `dry_run_error`: captured dry-run failure reason (when falling back)

#### Generating a Viability Report

To see *why* packages are rejected (e.g., "has type params", "needs unsupported object"), run the manifest generator with a report path:

```bash
uv run smi-phase2-manifest \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --out-ids results/manifests/phase2_ids.txt \
  --out-plan results/manifests/phase2_plan.json \
  --out-report results/manifests/phase2_viability_report.json
```

The report JSON contains detailed rejection statistics under `stats.rejection_reasons_counts` and sample failures in `rejected_samples`.

#### Running the Baseline Search

To establish a deterministic performance floor, run the `baseline-search` agent. It exhaustively tries all "runnable" candidate functions found by the analysis logic.

```bash
uv run smi-inhabit \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --package-ids-file results/manifests/phase2_ids.txt \
  --samples 25 --seed 0 \
  --agent baseline-search \
  --baseline-max-candidates 50 \
  --rpc-url https://fullnode.mainnet.sui.io:443 \
  --sender <FUNDED_MAINNET_ADDRESS> \
  --simulation-mode dry-run \
  --out results/phase2_baseline.json
```

```bash
uv run smi-phase2-manifest \
  --corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-ids results/manifests/phase2_executable_ids.txt \
  --out-plan results/manifests/phase2_executable_plans.json \
  --out-report results/manifests/phase2_executable_report.json

uv run smi-inhabit \
  --corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --package-ids-file results/manifests/phase2_executable_ids.txt \
  --samples 25 --seed 0 \
  --agent mock-planfile \
  --plan-file results/manifests/phase2_executable_plans.json \
  --rpc-url https://fullnode.mainnet.sui.io:443 \
  --sender <FUNDED_MAINNET_ADDRESS> \
  --gas-budget 10000000 \
  --gas-budget-ladder 20000000,50000000 \
  --gas-coin <OPTIONAL_GAS_COIN_OBJECT_ID> \
  --simulation-mode dry-run \
  --max-plan-attempts 2 \
  --include-created-types \
  --continue-on-error \
  --per-package-timeout-seconds 120 \
  --require-dry-run \
  --out results/phase2_two_packages.json
```

Notes:

- When `--package-ids-file` is set, Phase II treats `--samples` as a batch size over the manifest order (works with `--resume`).
- `--gas-coin` is optional; if omitted, `smi_tx_sim` picks the first `Coin<SUI>` found for `--sender`.
- For local-only scaffolding runs, use `--simulation-mode build-only` (no RPC).

Quick status / per-package analysis:

```bash
python scripts/phase2_status.py results/phase2_two_packages.json
python scripts/phase2_analyze.py results/phase2_two_packages.json
python scripts/phase2_analyze.py results/phase2_two_packages.json --show <package_id>
python scripts/phase2_metrics.py results/phase2_two_packages.json
```

Compare multiple Phase II runs (leaderboard):

```bash
python scripts/phase2_leaderboard.py results/phase2_run_a.json results/phase2_run_b.json
```

### Dev workflow

```bash
./scripts/lint.sh
```

### Logs (JSONL)

Both Phase I and Phase II can write a per-run log directory under `benchmark/logs/` containing:

- `run_metadata.json`
- `events.jsonl` (progress + timings)
- `packages.jsonl` (one row per package)

Disable logging with `--no-log`, or set a custom location with `--log-dir`.

Tail events while a run is executing:

```bash
python scripts/tail_events.py logs/<run_id>/events.jsonl --follow
```

If a run is interrupted, you can compute remaining manifest ids from `packages.jsonl`:

```bash
python scripts/manifest_remaining_from_jsonl.py \
  --manifest results/manifests/mainnet_most_used_first500_ids.txt \
  --packages-jsonl logs/<run_id>/packages.jsonl
```

### Phase II “harder tier”

The default Phase II manifest is intentionally conservative (single-call, auto-filled args).
You can generate a slightly harder manifest that also allows common system inputs like:

- `&Clock` (shared object `0x6`)
- `Coin<SUI>` (select a non-gas `Coin<SUI>` owned by `--sender`)

```bash
cd ..
PYTHONPATH=benchmark/src python3 -m smi_bench.inhabit_manifest \
  --corpus-root ../sui-package-benchmark/.local/research/sui-packages/packages/mainnet_most_used \
  --out-ids benchmark/results/manifests/phase2_executable_ids_n1000_harder_v1.txt \
  --out-plan benchmark/results/manifests/phase2_executable_plans_n1000_harder_v1.json \
  --out-report benchmark/results/manifests/phase2_executable_report_n1000_harder_v1.json
```
