# Getting Started with Phase II Benchmark

This guide is the canonical entrypoint for running the Phase II (Type Inhabitation) benchmark: setup → first run → interpreting results.

## Quick start (5 minutes)

### Option A: Docker (Recommended)
The easiest way to get started. Handles dependencies, binaries, and environment automatically.
```bash
./scripts/docker_quickstart.sh
```

### Option B: Local Setup
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

### A) Docker Quickstart (Recommended for new users)

Use this if you have Docker installed and want the easiest possible setup. It automatically configures your environment, builds the image, and runs a sample benchmark.

```bash
# From the repository root
./scripts/docker_quickstart.sh
```

**What this does:**
1. Prompts for your `OPENROUTER_API_KEY` (if not set).
2. Builds the `smi-bench` Docker image.
3. Sets up a minimal test corpus (if the real one is missing).
4. Runs a 1-package benchmark to verify the full stack.

---

### B) Fast local run (single model)

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

**See `DATASETS.md`** for comprehensive guide on creating and using datasets.

---

## 2E) Docker Multi-Model Workflow

Use this when running multiple models in production: single container, model switching via API, zero restart overhead.

### Start container once

```bash
cd benchmark
docker compose up -d --wait
```

Or with docker run:

```bash
docker build -t smi-bench:latest .
docker run -d --name smi-bench -p 9999:9999 \
  --env-file .env \
  smi-bench:latest
```

### Run benchmarks with model override

Using the enhanced `run_docker_benchmark.sh` script:

```bash
# Run with model A (default from env var)
./scripts/run_docker_benchmark.sh google/gemini-3-flash-preview 25 9999

# Run with model B (override via API, no restart)
./scripts/run_docker_benchmark.sh openai/gpt-4-turbo 25 9999 --model-override anthropic/claude-3.5-sonnet

# Check container status
./scripts/run_docker_benchmark.sh "" 0 9999 --status

# Cleanup when done
./scripts/run_docker_benchmark.sh "" 0 9999 --cleanup
```

### Batch multi-model runs

Using `run_multi_model.sh` for sequential model testing:

```bash
# Create models file
cat > models.txt <<EOF
google/gemini-3-flash-preview
openai/gpt-4-turbo
anthropic/claude-3.5-sonnet
EOF

# Run all models
./scripts/run_multi_model.sh models.txt 25 9999

# Cleanup after batch
./scripts/run_multi_model.sh models.txt 25 9999 --cleanup-after
```

### Model override in direct API calls

For manual API testing, include `model` in config:

```bash
curl -X POST http://localhost:9999/ \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "message/send",
    "params": {
      "message": {
        "messageId": "test-run",
        "role": "user",
        "parts": [{
          "text": "{\"config\": {\"corpus_root\": \"/app/corpus\", \"package_ids_file\": \"/app/manifest.txt\", \"agent\": \"real-openai-compatible\", \"samples\": 25, \"model\": \"openai/gpt-4\"}, \"out_dir\": \"/app/results\"}"
        }]
      }
    }
  }'
```

**Model precedence:**
1. API payload `config.model` (highest)
2. Environment variable `SMI_MODEL`
3. Default from agent configuration

See [docs/A2A_EXAMPLES.md](docs/A2A_EXAMPLES.md#model-override-per-request-model-switching) for detailed examples.

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

## Related documentation

- `../docs/METHODOLOGY.md` - Scoring rules and extraction logic.
- `docs/A2A_COMPLIANCE.md` - Protocol implementation details.
- `docs/A2A_EXAMPLES.md` - Concrete JSON-RPC request/response examples.
