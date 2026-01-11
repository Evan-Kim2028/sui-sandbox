# Production Deployment Guide

## 🎯 Current Status

**✅ Complete Infrastructure:**
- Docker API with 7 endpoints (`/info`, `/validate`, `/schema`, `/health`, `/metrics`, `/tasks/{id}/results`, `/`)
- Prometheus metrics for monitoring
- Webhook callbacks for async workflows
- Partial results API for progress tracking
- Full config validation (26 fields)
- 166 passing tests

**✅ Testing Tools Ready:**
- `scripts/test_multi_model_integration.py` - Multi-model testing
- `scripts/query_benchmark_logs.py` - Results analysis & cost tracking
- Complete documentation (`docs/INTEGRATION_TESTING.md`, `TESTING_QUICKSTART.md`)

**⚠️ Corpus Required:**
- Docker container needs a properly structured corpus at `/app/corpus`
- Corpus structure: `/app/corpus/{package_id}/bytecode_modules/*.mv`
- Manifest file: `/app/corpus/manifest.txt` with one package ID per line

---

## 🚀 Quick Start (When You Have a Corpus)

### 1. Prepare Corpus

```bash
# Your corpus should look like:
corpus/
├── manifest.txt
├── 0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7/
│   └── bytecode_modules/
│       ├── module1.mv
│       └── module2.mv
├── 0x2df868f30120484cc5e900c3b8b7a04561596cf15a9751159a207930471afff2/
│   └── bytecode_modules/
│       └── *.mv
...
```

### 2. Mount Corpus in Docker

Edit `docker-compose.yml`:

```yaml
volumes:
  - /path/to/your/corpus:/app/corpus:ro
  - ./benchmark/results:/app/results
  - ./benchmark/logs:/app/logs
```

### 3. Start Services

```bash
cd /path/to/sui-move-interface-extractor

# Build and start
docker compose up -d --wait smi-bench

# Verify running
curl http://localhost:9999/info | jq .
```

### 4. Run Tests

**Option A: Real-World Test Script (NEW - Recommended)**

Fast, flexible multi-model testing with detailed analytics:

```bash
cd benchmark

# Quick dry-run test (free, ~2s per package)
uv run python3 run_real_world_test.py \
    --samples 2 \
    --models gpt-4o-mini,google/gemini-3-flash-preview \
    --simulation-mode dry-run

# Live production test (uses model APIs, ~30-120s per package)
uv run python3 run_real_world_test.py \
    --samples 5 \
    --models gpt-4o,google/gemini-3-flash-preview,google/gemini-2.5-flash-preview \
    --simulation-mode live
```

**Key Features:**
- ✅ Test any combination of models
- ✅ Mix fast and slow models (e.g., GPT-4o-mini + GPT-4o)
- ✅ Dry-run mode (mock agent, no API costs)
- ✅ Live mode (real model APIs, cost tracking)
- ✅ Detailed analytics (duration, tokens, errors, hit rate)
- ✅ Results saved to `real_world_test_results_*.json`

**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md)

---

**Option B: Multi-Model Integration Script**

Comprehensive testing with multiple scenarios:

```bash
cd benchmark

# Multi-model integration test
./scripts/test_multi_model_integration.py \
  --corpus-root /app/corpus \
  --manifest /app/corpus/manifest.txt
```

**Documentation:** [benchmark/TESTING_QUICKSTART.md](benchmark/TESTING_QUICKSTART.md)

---

**Query Results**

```bash
cd benchmark

# List all runs
./scripts/query_benchmark_logs.py list

# Filter by model
./scripts/query_benchmark_logs.py list --model gpt-4o

# Show run details
./scripts/query_benchmark_logs.py show <RUN_ID>

# Compare runs
./scripts/query_benchmark_logs.py compare <RUN_ID_1> <RUN_ID_2>

# Calculate costs
./scripts/query_benchmark_logs.py cost <RUN_ID>
```

---

## 📊 What Gets Tracked

### Per Run
- **Model**: `gpt-4o-mini`, `gpt-4o`, `claude-3-5-sonnet`, etc.
- **Duration**: Total time + per-package breakdown
- **Tokens**: Prompt & completion (for cost calculation)
- **Hit Rate**: Success metric (created types / target types)
- **Errors**: Package failures, timeouts

### Storage

**Logs**: `logs/{run_id}/`
- `run_metadata.json` - Config, timing, model info
- `events.jsonl` - Streaming event log

**Results**: `results/a2a/{run_id}.json`
- Aggregate metrics
- Per-package detailed results
- Token counts, cost estimates

---

## 🔍 Monitoring & Analysis

### Real-Time Monitoring (Prometheus)

```bash
# View metrics
curl http://localhost:9999/metrics

# Key metrics:
# - smi_bench_task_duration_seconds (task performance)
# - smi_bench_task_requests_total (throughput)
# - smi_bench_active_tasks (current load)
# - smi_bench_task_errors_total (failure rate)
```

**Grafana Setup:**
1. Add Prometheus datasource pointing to `:9999/metrics`
2. Create dashboards for:
   - Task duration by model
   - Success rate trends
   - Cost per run
   - Package-level performance

### Historical Analysis

```bash
# List all runs
./scripts/query_benchmark_logs.py list

# Filter by model
./scripts/query_benchmark_logs.py list --model gpt-4o

# Detailed analysis
./scripts/query_benchmark_logs.py analyze <RUN_ID>

# Cost calculation
./scripts/query_benchmark_logs.py cost <RUN_ID>

# Compare models
./scripts/query_benchmark_logs.py compare <RUN_ID_1> <RUN_ID_2>
```

---

## 💰 Cost Tracking

Token usage automatically tracked with every run (real agents only).

**Current Pricing** (update in `scripts/query_benchmark_logs.py`):
- GPT-4o-mini: $0.15/$0.60 per 1M tokens (input/output)
- GPT-4o: $2.50/$10.00 per 1M tokens
- Claude-3.5-Sonnet: $3.00/$15.00 per 1M tokens

**Estimate Costs:**
```bash
# Single run
./scripts/query_benchmark_logs.py cost a2a_phase2_1234567890

# All runs from today
for run in results/a2a/*.json; do
  RUN_ID=$(basename $run .json)
  ./scripts/query_benchmark_logs.py cost $RUN_ID
done | jq -s 'map(.total_cost_usd) | add'
```

---

## 🧪 Testing Workflows

### 1. Quick Smoke Test (Mock Agent)

```bash
# Fast validation (no LLM calls)
curl -X POST http://localhost:9999 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "message/send",
    "params": {
      "message": {
        "messageId": "msg_smoke",
        "role": "user",
        "parts": [{
          "text": "{\"config\": {\"corpus_root\": \"/app/corpus\", \"package_ids_file\": \"/app/corpus/manifest.txt\", \"samples\": 1, \"agent\": \"mock-empty\"}}"
        }]
      }
    }
  }'
```

### 2. Model Comparison

Edit `scripts/test_multi_model_integration.py`:

```python
# Test 3 models on same packages
await tester.test_scenario("gpt4o_mini", model="gpt-4o-mini", samples=2)
await tester.test_scenario("gpt4o", model="gpt-4o", samples=2)
await tester.test_scenario("claude", model="claude-3-5-sonnet-20241022", samples=2)
```

Run:
```bash
./scripts/test_multi_model_integration.py --corpus-root /app/corpus --manifest /app/corpus/manifest.txt
```

### 3. Webhook Workflow

```bash
# Submit with callback
curl -X POST http://localhost:9999 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "message/send",
    "params": {
      "message": {
        "messageId": "msg_webhook",
        "role": "user",
        "parts": [{
          "text": "{\"config\": {\"corpus_root\": \"/app/corpus\", \"package_ids_file\": \"/app/corpus/manifest.txt\", \"callback_url\": \"https://your-service.com/webhook\"}}"
        }]
      }
    }
  }'

# Results will be POST'd to your webhook when complete
```

---

## 🐛 Troubleshooting

### Container Won't Start

```bash
# Check logs
docker compose logs smi-bench

# Rebuild
docker compose build smi-bench
docker compose up -d --wait smi-bench
```

### Task Stuck "Running"

**Possible causes:**
1. **Missing corpus** - Packages in manifest don't exist in `/app/corpus`
2. **Wrong corpus structure** - Needs `{package_id}/bytecode_modules/*.mv`
3. **API key missing** - Check SMI_MODEL env var for real agents

**Debug:**
```bash
# Check latest logs
ls -lt benchmark/logs/ | head -5

# View events
cat benchmark/logs/<run_id>/events.jsonl

# Check container filesystem
docker exec smi-bench-dev ls /app/corpus
docker exec smi-bench-dev cat /app/corpus/manifest.txt
```

### No Token Counts

Token tracking requires `agent: real-openai-compatible`. Mock agents don't track tokens.

### Wrong Cost Estimates

Update pricing in `scripts/query_benchmark_logs.py`:
```python
pricing = {
    "input_per_1k": <your_rate> / 1000,
    "output_per_1k": <your_rate> / 1000,
}
```

---

## 🔌 Offline Mode & Air-Gapped Deployment

The benchmark system supports fully offline operation for air-gapped environments.

### Local Benchmark (No-Chain)

Use the `benchmark-local` command for network-free validation:

```bash
# Inside container or on host
./target/release/sui_move_interface_extractor benchmark-local \
    --target-corpus /app/corpus/0x.../bytecode_modules \
    --output results.jsonl \
    --restricted-state
```

**Benefits:**
- No RPC access required
- No funded accounts needed
- Deterministic, reproducible results
- Fast validation (~6000 modules in <500ms for Tier A)

See [docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) for technical details.

### Framework Bytecode Caching

The Docker container includes pre-compiled Sui framework bytecode for offline helper package builds:

```
framework_bytecode/
├── sui-framework/
│   └── bytecode_modules/*.mv
├── move-stdlib/
│   └── bytecode_modules/*.mv
└── sui-system/
    └── bytecode_modules/*.mv
```

**To update framework bytecode:**
```bash
# Build fresh framework bytecode (requires Sui CLI)
sui move build --path framework_bytecode/sui-framework

# Or copy from existing Sui installation
cp -r ~/.sui/sui_config/framework/* framework_bytecode/
```

### Docker Offline Artifacts

The Docker image includes:
- **Sui CLI**: For deterministic helper package builds
- **Framework bytecode**: For offline compilation
- **Rust binaries**: Pre-built `sui_move_interface_extractor`

**Persisting artifacts between runs:**
```yaml
# docker-compose.yml
volumes:
  - ./framework_bytecode:/app/framework_bytecode:ro
  - ./benchmark/results:/app/results
  - ./benchmark/logs:/app/logs
```

### E2E One-Package (Offline)

Test the full pipeline without API keys:

```bash
cd benchmark
uv run python scripts/e2e_one_package.py \
    --corpus-root tests/fake_corpus \
    --package-id 0x1 \
    --out-dir results/offline_test
```

---

## 📁 File Locations

```
.
├── docker-compose.yml           # Service config
├── Dockerfile                   # Container image
├── benchmark/
│   ├── scripts/
│   │   ├── test_multi_model_integration.py  # Testing script
│   │   └── query_benchmark_logs.py          # Analysis tool
│   ├── docs/
│   │   ├── INTEGRATION_TESTING.md   # Full testing guide
│   │   └── A2A_EXAMPLES.md          # API examples
│   ├── TESTING_QUICKSTART.md        # Quick reference
│   ├── logs/                        # Run logs
│   │   └── {run_id}/
│   │       ├── run_metadata.json
│   │       └── events.jsonl
│   └── results/a2a/                 # Final results
│       └── {run_id}.json
└── PRODUCTION_DEPLOYMENT.md         # This file
```

---

## ✅ Production Checklist

Before deploying to production:

- [ ] Corpus mounted and validated
- [ ] Manifest file points to existing packages
- [ ] API keys configured (SMI_MODEL env var)
- [ ] Prometheus scraping configured
- [ ] Grafana dashboards created
- [ ] Log retention policy set
- [ ] Cost alerts configured
- [ ] Webhook endpoint tested (if using)
- [ ] Integration tests passing
- [ ] Backup strategy for results/logs

---

## 📚 Documentation

- **[Quick Start Guide](QUICK_START_GUIDE.md)** - 30-second quick start with all features
- **[Getting Started](benchmark/GETTING_STARTED.md)** - Benchmark introduction
- **[Testing Guide](benchmark/docs/INTEGRATION_TESTING.md)** - Complete testing reference
- **[API Examples](benchmark/docs/A2A_EXAMPLES.md)** - All endpoints with examples
- **[Quick Testing](benchmark/TESTING_QUICKSTART.md)** - 5-minute testing guide
- **[Architecture](benchmark/docs/ARCHITECTURE.md)** - System design
- **[No-Chain Spec](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md)** - Local benchmark technical spec
- **[CLI Reference](docs/CLI_REFERENCE.md)** - All CLI commands including `benchmark-local`
- **[Test Fixtures](docs/TEST_FIXTURES.md)** - Fixture organization for testing

---

## 🎯 Next Steps

1. **Obtain or build corpus** - Download/extract Move packages
2. **Validate corpus structure** - Ensure bytecode_modules directories exist
3. **Run smoke test** - Verify Docker + API working
4. **Run model comparison** - Test 2-3 models on small subset
5. **Analyze results** - Use query tool to find optimal model
6. **Set up monitoring** - Connect Grafana to metrics endpoint
7. **Production deployment** - Scale up to full corpus
