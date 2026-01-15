# Benchmark Guide

This guide is the canonical entrypoint for running benchmarks: setup → first run → interpreting results.

## Overview

The benchmark system has two execution paths:

| Path | Tool | Use Case |
|------|------|----------|
| **Rust CLI** | `benchmark-local`, `tx-replay`, `ptb-eval` | Deterministic type inhabitation testing, transaction replay |
| **Python Harness** | `smi-inhabit`, scripts | LLM evaluation, multi-model comparison, Phase II benchmarks |

Both paths use the **SimulationEnvironment** for offline Move VM execution. See [ARCHITECTURE.md](ARCHITECTURE.md) for how components integrate.

## Quick start (5 minutes)

1. **Setup dependencies and corpus:**
```bash
cd benchmark
uv sync --group dev --frozen
cd .. && cargo build --release --locked && cd benchmark
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages
cp .env.example .env
```

2. **Run a high-signal Phase II sample (Gemini 3 Flash):**
```bash
./scripts/run_model.sh --env-file .env --model google/gemini-3-flash-preview \
  --scan-samples 100 --run-samples 5 --per-package-timeout-seconds 90
```

---

## Rust CLI Benchmarks

### `benchmark-local` - Type Inhabitation Testing

Test type inhabitation without any network access:

```bash
# Tier A only (fast preflight validation)
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus ../sui-packages/packages/mainnet_most_used \
  --output results.jsonl \
  --tier-a-only

# Full Tier A + B via SimulationEnvironment (recommended)
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus ../sui-packages/packages/mainnet_most_used \
  --output results.jsonl \
  --use-ptb
```

### `tx-replay` - Transaction Replay

Validate against real mainnet transactions:

```bash
# Download recent transactions
./target/release/sui_move_interface_extractor tx-replay \
  --recent 100 \
  --cache-dir .tx-cache \
  --download-only

# Replay locally
./target/release/sui_move_interface_extractor tx-replay \
  --cache-dir .tx-cache \
  --from-cache \
  --parallel
```

### `ptb-eval` - Self-Healing Evaluation

Evaluate with automatic recovery from missing packages/objects:

```bash
./target/release/sui_move_interface_extractor ptb-eval \
  --cache-dir .tx-cache \
  --max-retries 3 \
  --enable-fetching \
  --show-healing
```

Self-healing actions:
- **DeployPackage**: Fetch and deploy missing package from mainnet
- **CreateObject**: Synthesize missing object with appropriate type
- **SetupSharedObject**: Initialize shared object state

See [CLI_REFERENCE.md](CLI_REFERENCE.md) for complete command documentation.

---

## Python Benchmark Harness

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

### D) Using Datasets

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

**See [Datasets Guide](DATASETS.md)** for comprehensive guide on creating and using datasets.

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

## 4) Troubleshooting

- Rate limits (RPC/OpenRouter): reduce `--parallel` (multi-model) and/or lower `--run-samples`.
- "No requests": confirm you used the exact model id shown in `./scripts/run_model.sh --help`.
- Port conflicts (A2A): check ports 9999 (Green) / 9998 (Purple).

---

## 5) Oracle and Evaluator Architecture

The benchmark system uses a separation between **oracle** (answer key) and **evaluator** (scorer):

### Oracle

The oracle provides ground truth for type inhabitation:
- Extracts interfaces from bytecode
- Identifies target types that can be inhabited
- Provides constructor chains for complex types

### Evaluator

The evaluator scores LLM-generated solutions:
- Validates PTB structure
- Executes in SimulationEnvironment
- Compares created types against targets
- Computes hit rates and failure taxonomy

### Self-Healing Loop

The `ptb-eval` command implements a self-healing evaluation loop:

```
                    ┌────────────────────┐
                    │   Load Cached TX   │
                    └─────────┬──────────┘
                              │
                              ▼
              ┌───────────────────────────────┐
              │  Execute via SimulationEnv    │
              └───────────────┬───────────────┘
                              │
                    ┌─────────┴─────────┐
                    │                   │
                    ▼                   ▼
              ┌──────────┐        ┌──────────┐
              │ Success  │        │  Error   │
              └──────────┘        └────┬─────┘
                                       │
                                       ▼
                              ┌─────────────────┐
                              │ Diagnose Error  │
                              └────────┬────────┘
                                       │
                    ┌──────────────────┼──────────────────┐
                    │                  │                  │
                    ▼                  ▼                  ▼
            ┌─────────────┐   ┌─────────────┐   ┌─────────────┐
            │ Deploy Pkg  │   │Create Object│   │Setup Shared │
            └──────┬──────┘   └──────┬──────┘   └──────┬──────┘
                   │                 │                 │
                   └─────────────────┼─────────────────┘
                                     │
                                     ▼
                           ┌─────────────────┐
                           │  Retry Execute  │
                           └─────────────────┘
```

---

## Related documentation

- **[ARCHITECTURE.md](ARCHITECTURE.md)** - System architecture and data flows
- **[CLI_REFERENCE.md](CLI_REFERENCE.md)** - Complete CLI command reference
- **[LOCAL_BYTECODE_SANDBOX.md](LOCAL_BYTECODE_SANDBOX.md)** - Sandbox internals
- **[Insights & Reward](INSIGHTS.md)** - High-value takeaways and research value proposition
- **[Methodology](METHODOLOGY.md)** - Scoring rules and extraction logic
- **[A2A Protocol](A2A_PROTOCOL.md)** - Protocol implementation and examples
- **[Datasets Guide](DATASETS.md)** - Creating and using curated package lists