# Getting Started with Phase II Benchmark

This guide is the canonical entrypoint for running the Phase II (Type Inhabitation) benchmark: setup → first run → interpreting results.

## Quick start (5 minutes)

### Option A: Top 25 Dataset (Full Run)

Runs a full benchmark on the curated Top 25 dataset using GPT-5.2.

```bash
./scripts/top25_quickstart.sh
```

### Option B: E2E One-Package (LLM Pipeline)

Complete LLM-driven evaluation: Target → Helper Generation → Build → Simulation.

```bash
cd benchmark

# Offline test (no API key needed)
uv run python scripts/e2e_one_package.py \
    --corpus-root tests/fake_corpus \
    --package-id 0x1 \
    --out-dir results/my_test

# Real LLM test
export SMI_E2E_REAL_LLM=1
export OPENROUTER_API_KEY=sk-or-v1-...
uv run python scripts/e2e_one_package.py \
    --corpus-root ../sui-packages/packages/mainnet_most_used \
    --dataset type_inhabitation_top25 \
    --samples 5 \
    --model google/gemini-3-flash-preview \
    --out-dir results/e2e_run
```

### Option D: Local Benchmark (No-Chain)

Validate type inhabitation offline using the Rust `benchmark-local` command.

```bash
./target/release/sui_move_interface_extractor benchmark-local \
    --target-corpus /path/to/bytecode_modules \
    --output results.jsonl \
    --restricted-state
```

See [NO_CHAIN_TYPE_INHABITATION_SPEC.md](../docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) for details.

### Option E: Local Setup (Development)

For developers working directly on the Python/Rust source code.

```bash
cd benchmark
uv sync --group dev --frozen
cd .. && cargo build --release --locked && cd benchmark
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages
cp .env.example .env
```

---

## 1) One-time setup

### Dependencies

We use `uv` for Python dependency management and `cargo` for Rust components.

```bash
cd benchmark
uv sync --group dev --frozen

# Build Rust binaries (extractor and transaction simulator)
cd .. && cargo build --release --locked && cd benchmark
```

### Clone the corpus

```bash
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages
```

### Credentials configuration

Copy `.env.example` to `.env` and set your model credentials.

```bash
cp .env.example .env
```

Recommended (OpenRouter): one key for many models.

```env
OPENROUTER_API_KEY=sk-or-v1-...
SMI_API_BASE_URL=https://openrouter.ai/api/v1
SMI_MODEL=google/gemini-3-flash-preview

# Optional but recommended for "real" dry-runs (see note below)
SMI_SENDER=0xYOUR_FUNDED_MAINNET_ADDRESS
```

### Important: `sender` / inventory expectations

- If you run with an unfunded sender or `sender=0x0`, many packages are effectively "inventory empty".
- In that mode, it is normal to see:
  - `dry_run_ok=true` for harmless/no-op PTBs, while
  - `created_hits=0` because target types require existing objects/caps or init paths.

If your near-term goal is **framework stability**, prioritize `dry_run_ok` and timeout/error rates.

---

## 2) Choose your entrypoint

### A) Fast local run (single model)

Use this when you want to quickly iterate on benchmarking and see a JSON output file.

```bash
cd benchmark
./scripts/run_model.sh --env-file .env --model google/gemini-3-flash-preview \
  --scan-samples 100 --run-samples 5 --per-package-timeout-seconds 90
```

Model slug sanity check (avoids "no requests" surprises):

```bash
cd benchmark
./scripts/run_model.sh --help
```

### B) Multi-model comparison

Use this when you want the same workload executed across multiple models.

```bash
cd benchmark
./scripts/run_multi_model.sh --env-file .env \
  --models "google/gemini-3-flash-preview,anthropic/claude-3.5-sonnet" \
  --parallel 1 \
  --scan-samples 100 --run-samples 5 --per-package-timeout-seconds 90
```

**Notes:**

- Start with `--parallel 1` to avoid RPC rate limits; increase gradually.

---

### C) Using Datasets

Use this when you want to run benchmarks on curated package lists.

**Quick iteration with top-25 dataset:**

```bash
uv run smi-inhabit \
  --corpus-root ../../sui-packages/packages/mainnet_most_used \
  --dataset type_inhabitation_top25 \
  --samples 1 \
  --agent mock-empty \
  --out results/top25_test.json
```

**Standard Phase II benchmark:**

```bash
uv run smi-inhabit \
  --corpus-root ../../sui-packages/packages/mainnet_most_used \
  --dataset standard_phase2_benchmark \
  --samples 10 \
  --agent real-openai-compatible \
  --out results/phase2_run.json
```

**Available datasets:**

- `type_inhabitation_top25` - 25 packages for fast iteration
- `packages_with_keys` - Packages with key structs (variable count)
- `standard_phase2_benchmark` - Primary Phase II benchmark (292 packages)

**Custom manifest files:**
For custom package lists, use `--package-ids-file` with the full path:

```bash
uv run smi-inhabit \
  --corpus-root ../../sui-packages/packages/mainnet_most_used \
  --package-ids-file /path/to/my_manifest.txt \
  --agent real-openai-compatible \
  --out results/custom_run.json
```

**See `DATASETS.md`** for comprehensive guide on creating and using datasets.

---

## 3) Results-first: what to look at

The Phase II output JSON contains per-package rows and an aggregate summary.

Key fields to watch first:

- `aggregate.errors` and per-package `error` (harness/runtime failures)
- `packages[*].timed_out` (timeouts)
- `packages[*].dry_run_ok` and `packages[*].dry_run_effects_error` (execution success vs failure class)
- `packages[*].score.created_hits` (task success; may be 0 for inventory-constrained packages)

**Key distinction:**

- `dry_run_ok`: Transaction executed without aborting (runtime success)
- `created_hits`: Target types were actually created (task success)

Example: Agent calls `init_wrapper()` instead of `mint_coin()` → transaction succeeds (`dry_run_ok=true`) but creates no coins (`created_hits=0`).

Helpers:

```bash
# View run status
python scripts/phase2_status.py results/my_run.json

# Compare multiple runs (leaderboard)
python scripts/phase2_leaderboard.py results/run_a.json results/run_b.json
```

---

## 4) E2E One-Package Pipeline

The `e2e_one_package.py` script provides a complete LLM-driven evaluation pipeline that does not require HTTP API setup.

### Pipeline Stages

1. **Target Package Analysis**: Extracts bytecode interface JSON from target package
2. **LLM Helper Package Generation**: Prompts LLM to generate a Move helper package
3. **Move Build**: Compiles helper package with `sui move build`
4. **TX Simulation**: Runs local transaction simulator on helper bytecode

### Usage Examples

```bash
cd benchmark

# Single package test (offline)
uv run python scripts/e2e_one_package.py \
    --corpus-root tests/fake_corpus \
    --package-id 0x1 \
    --out-dir results/test

# Dataset-based test with real LLM
export SMI_E2E_REAL_LLM=1
uv run python scripts/e2e_one_package.py \
    --corpus-root ../sui-packages/packages/mainnet_most_used \
    --dataset type_inhabitation_top25 \
    --samples 10 \
    --model google/gemini-3-flash-preview \
    --out-dir results/e2e_top25
```

### Output Artifacts

Each run creates a directory with:

- `validation_report.json` - Overall success/failure with error details
- `mm2_mapping.json` - Function mapping results (accepted/rejected)
- `txsim_source.json` - Transaction simulation input
- `txsim_effects.json` - Transaction execution effects
- `helper_pkg/` - Generated Move helper package
- `interface.json` - Target package bytecode interface
- `prompt.json` - LLM prompt sent
- `llm_response.json` - Raw LLM response

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `SMI_E2E_REAL_LLM` | Set to "1" to use real LLM | Offline stub |
| `OPENROUTER_API_KEY` | OpenRouter API key | - |
| `SMI_MODEL` | Default model | - |
| `SMI_TEMPERATURE` | LLM temperature | 0 |
| `SMI_MAX_TOKENS` | Max response tokens | 4096 |

---

## 5) Troubleshooting

- Rate limits (RPC/OpenRouter): reduce `--parallel` (multi-model) and/or lower `--run-samples`.
- "No requests": confirm you used the exact model id shown in `./scripts/run_model.sh --help`.
- "dataset not found": Check `manifests/datasets/<name>.txt` exists.
- "benchmark-local failed": Run `cargo build --release` to build Rust binary.

---

## Related documentation

- `../docs/METHODOLOGY.md` - Scoring rules and extraction logic.
- `docs/ARCHITECTURE.md` - Maintainers' map of harness internals.
