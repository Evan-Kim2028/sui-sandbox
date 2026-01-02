# Phase II Benchmark Quickstart

Run an A2A benchmark against Sui Move packages in 3 steps.

## 1. Setup (one-time)

```bash
cd benchmark
uv sync --group dev --frozen

# Build Rust binaries
cd .. && cargo build --release --locked && cd benchmark

# Clone corpus (if not already present)
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages
```

## 2. Configure API Key

Copy `.env.example` to `.env` and set your model credentials:

```bash
cp .env.example .env
```

**GLM 4.7 (Z.AI Coding API):**
```env
SMI_API_KEY=your_api_key_here
SMI_API_BASE_URL=https://api.z.ai/api/coding/paas/v4
SMI_MODEL=glm-4.7
SMI_THINKING=enabled
SMI_RESPONSE_FORMAT=json_object
SMI_CLEAR_THINKING=true
SMI_SENDER=0xYOUR_FUNDED_MAINNET_ADDRESS
```

**OpenAI:**
```env
SMI_API_KEY=sk-...
SMI_API_BASE_URL=https://api.openai.com/v1
SMI_MODEL=gpt-4o
SMI_SENDER=0xYOUR_FUNDED_MAINNET_ADDRESS
```

Verify connectivity:
```bash
uv run smi-phase1 --corpus-root ../sui-packages/packages/mainnet_most_used --doctor-agent --agent real-openai-compatible
```

## 3. Run Phase II Benchmark

**Quick test (3 packages, ~5 min):**
```bash
uv run smi-inhabit \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --package-ids-file manifests/standard_phase2_no_framework.txt \
  --agent real-openai-compatible \
  --rpc-url https://fullnode.mainnet.sui.io:443 \
  --simulation-mode dry-run \
  --continue-on-error \
  --per-package-timeout-seconds 90 \
  --max-plan-attempts 2 \
  --samples 3 \
  --out results/my_test_run.json
```

**Full benchmark (290 packages):**
```bash
uv run smi-inhabit \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --package-ids-file manifests/standard_phase2_no_framework.txt \
  --agent real-openai-compatible \
  --rpc-url https://fullnode.mainnet.sui.io:443 \
  --simulation-mode dry-run \
  --continue-on-error \
  --checkpoint-every 1 \
  --per-package-timeout-seconds 90 \
  --max-plan-attempts 2 \
  --out results/phase2_full_run.json \
  --resume
```

## 4. View Results

```bash
# Quick status
python scripts/phase2_status.py results/my_test_run.json

# Detailed metrics
python scripts/phase2_metrics.py results/my_test_run.json

# Compare runs
python scripts/phase2_leaderboard.py results/run_a.json results/run_b.json
```

## Key Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--per-package-timeout-seconds` | 120 | Max time per package (API + simulation) |
| `--max-plan-attempts` | 2 | Retry attempts on compiler errors |
| `--continue-on-error` | off | Don't abort on individual failures |
| `--checkpoint-every` | 0 | Save progress every N packages |
| `--resume` | off | Resume from existing output file |

## Manifests

- `manifests/standard_phase2_benchmark.txt` - Full 292 packages (includes framework)
- `manifests/standard_phase2_no_framework.txt` - 290 packages (excludes slow 0x2, 0x3)

## Baseline Comparison

The mechanical baseline achieves **2.6% hit rate**. Any AI agent should beat this floor.

See `baselines/v0.2.2_baseline/` for reference results.
