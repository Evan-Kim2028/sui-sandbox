# Quick Start Guide - Real-World Testing & Docker

**The fastest way to test Sui Move interface extraction with multiple models via Docker API.**

---

## 🎯 What's New? (Latest Updates)

### ✅ Real-World Testing Script
**File:** `benchmark/run_real_world_test.py`

Submit benchmark tasks via HTTP API, test multiple models, get detailed analytics - all from a single command.

### ✅ Docker Fluid Usage
Start, stop, restart, monitor - all from standard Docker commands. Logs and results persist across restarts.

### ✅ Model Mixing
Test any combination of models in a single run:
- GPT-4o, GPT-4o-mini (OpenAI)
- Gemini Flash 3, Flash 2.5 (Google)
- Claude Sonnet 4.5 (Anthropic)

### ✅ Fast Iteration Loop
- **Dry-run mode:** No API costs, ~2 seconds per package
- **Live mode:** Uses real models, ~30-120 seconds per package
- Switch between modes instantly with `--simulation-mode` flag

### ✅ Detailed Analytics
- Duration per model
- Token usage (prompt + completion)
- Hit rate (success rate)
- Error tracking
- Cost estimation

---

## 🚀 30-Second Quick Start

### Step 1: Start Docker (5 seconds)
```bash
cd /path/to/sui-move-interface-extractor
docker compose up -d smi-bench

# Verify healthy
curl -s http://localhost:9999/health | jq '.status'
# Expected: "ok"
```

### Step 2: Run Fast Test (10 seconds)
```bash
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 1 \
    --models gpt-4o-mini,google/gemini-2.5-flash-preview \
    --simulation-mode dry-run
```

**This will:**
- ✅ Submit tasks to API
- ✅ Test 2 models sequentially
- ✅ Execute benchmark (dry-run, mock agent)
- ✅ Collect metrics (duration, errors, hit rate)
- ✅ Save results to `results/a2a/`
- ✅ Save logs to `logs/`

### Step 3: View Results (5 seconds)
```bash
# View summary
cat real_world_test_results_*.json | jq '.summary'

# View detailed metrics
cat benchmark/results/a2a/*.json | jq '.metrics'

# View event logs
cat benchmark/logs/*/events.jsonl | jq .
```

**Done!** You've successfully tested the system in under a minute.

---

## 📊 Model Comparison Testing

### Compare Multiple Models
```bash
uv run python3 run_real_world_test.py \
    --samples 2 \
    --models gpt-4o,google/gemini-3-flash-preview,google/gemini-2.5-flash-preview,claude-sonnet-4-5-20250929 \
    --simulation-mode dry-run
```

**Output:**
```
-----------------------------------------------------------------------
Model                          Status     Packages   Hit Rate   Errors   Duration
-----------------------------------------------------------------------
GPT-4o                         ✅          2          0.00       0        45.2s
Gemini Flash 3                 ✅          2          0.00       1        44.8s
Gemini Flash 2.5               ✅          2          0.00       0        43.1s
Claude Sonnet 4.5              ✅          2          0.05       0        47.3s
-----------------------------------------------------------------------
```

### Available Models

| Model ID | Name | Speed | Cost | Best For |
|-----------|-------|--------|-----------|
| `gpt-4o` | Medium | $$ | Top quality |
| `gpt-4o-mini` | Fast | $ | Quick iteration |
| `google/gemini-3-flash-preview` | Fast | $ | Latest features |
| `google/gemini-2.5-flash-preview` | Very Fast | $ | Quickest results |
| `claude-sonnet-4-5-20250929` | Medium | $$$ | Highest quality |

---

## 🐳 Docker Fluid Usage

### Basic Commands

```bash
# Start container (preserves logs/results)
docker compose up -d smi-bench

# Stop container
docker compose stop smi-bench

# Restart container (preserves running state)
docker compose restart smi-bench

# Stop and remove container
docker compose down
```

### Log Monitoring

```bash
# Real-time logs
docker logs -f smi-bench-dev

# Last 100 lines
docker logs smi-bench-dev --tail 100

# Last hour
docker logs smi-bench-dev --since 1h

# Filter by run ID
docker logs smi-bench-dev | grep "a2a_phase2_1767545099"
```

### Health & Metrics

```bash
# Check API health
curl -s http://localhost:9999/health | jq .

# Prometheus metrics
curl -s http://localhost:9999/metrics | grep smi_bench

# Key metrics:
# - smi_bench_task_duration_seconds: Execution time
# - smi_bench_task_errors_total: Error count
# - smi_bench_active_tasks: Current load
# - smi_bench_http_requests_total: API requests
```

### Resource Monitoring

```bash
# Live stats
docker stats smi-bench-dev

# Check disk usage
docker exec smi-bench-dev du -sh /app/logs /app/results
```

### File Persistence

**Files are mounted from host:**
```
./benchmark/logs/        → /app/logs        (Docker)
./benchmark/results/     → /app/results      (Docker)
```

**This means:**
- ✅ Logs survive Docker restarts
- ✅ Results can be accessed from host
- ✅ No data loss on container updates

---

## 🎯 Dry-Run vs Live Mode

### Dry-Run Mode (Fast, Free)

```bash
--simulation-mode dry-run
```

**Characteristics:**
- Uses mock agent (no LLM API calls)
- No API costs
- ~2 seconds per package
- Perfect for:
  - Quick iteration
  - Debugging
  - Testing corpus structure
  - Validating config

**When to use:**
- Initial setup
- Corpus testing
- Debugging errors
- Before committing to live runs

### Live Mode (Real, Paid)

```bash
--simulation-mode live
```

**Characteristics:**
- Uses real LLM API calls
- API costs apply
- ~30-120 seconds per package
- Perfect for:
  - Production runs
  - Model comparison
  - Real-world evaluation

**When to use:**
- After dry-run works
- Comparing model performance
- Production evaluation
- Final benchmark results

---

## 📈 Analytics & Results

### What Gets Collected

**Per Model:**
- ✅ Execution duration
- ✅ Packages processed
- ✅ Hit rate (success rate)
- ✅ Error count
- ✅ Prompt tokens
- ✅ Completion tokens

**Per Package:**
- ✅ Individual execution time
- ✅ Errors (if any)
- ✅ PTB generation attempts
- ✅ Simulation status

### Viewing Results

**Via host files:**
```bash
# List recent results
ls -lt benchmark/results/a2a/

# View specific result
cat benchmark/results/a2a/<run_id>.json | jq .

# Key fields:
# - aggregate.avg_hit_rate
# - aggregate.total_prompt_tokens
# - aggregate.errors
# - packages[].error
# - packages[].elapsed_seconds
```

**Via Docker:**
```bash
# List results
docker exec smi-bench-dev ls -lt /app/benchmark/results/a2a/

# View result
docker exec smi-bench-dev cat /app/benchmark/results/a2a/<run_id>.json | jq .
```

### Query Tool

Use the built-in query tool for analysis:

```bash
cd benchmark

# List all runs
uv run python3 scripts/query_benchmark_logs.py list --limit 10

# Show run details
uv run python3 scripts/query_benchmark_logs.py show <run_id>

# Calculate costs (for live mode)
uv run python3 scripts/query_benchmark_logs.py cost <run_id>

# Compare runs
uv run python3 scripts/query_benchmark_logs.py compare <run_id1> <run_id2>
```

---

## 🔄 Common Workflows

### Workflow 1: Quick Validation (1 minute)
```bash
# Start Docker
docker compose up -d smi-bench

# Run dry-run test
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 1 \
    --models gpt-4o-mini \
    --simulation-mode dry-run

# Check logs
docker logs smi-bench-dev --tail 20
```

### Workflow 2: Model Comparison (5-10 minutes)
```bash
# Test 3 models, 2 packages each
uv run python3 run_real_world_test.py \
    --samples 2 \
    --models gpt-4o-mini,google/gemini-3-flash-preview,gpt-4o \
    --simulation-mode live

# Wait for completion
# Check results in real_world_test_results_*.json
```

### Workflow 3: Scale Testing (30+ minutes)
```bash
# Test with many packages
uv run python3 run_real_world_test.py \
    --samples 10 \
    --models gpt-4o-mini \
    --simulation-mode live

# Monitor progress
docker logs -f smi-bench-dev

# Check resource usage
docker stats smi-bench-dev
```

### Workflow 4: Iterative Development
```bash
# 1. Make code changes
vim benchmark/src/smi_bench/your_file.py

# 2. Rebuild Docker
docker compose build smi-bench

# 3. Restart (preserves logs/results)
docker compose restart smi-bench

# 4. Run dry-run test (fast)
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 1 \
    --models gpt-4o-mini \
    --simulation-mode dry-run

# 5. Check logs
docker logs smi-bench-dev --tail 30
```

---

## 📚 Additional Documentation

| Document | Description | When to Read |
|-----------|-------------|---------------|
| **README.md** | Main project overview | First time setup |
| **PRODUCTION_DEPLOYMENT.md** | Complete production guide | Deploying to production |
| **REAL_WORLD_TEST_GUIDE.md** | Detailed testing guide | Running real tests |
| **benchmark/TESTING_QUICKSTART.md** | Testing tools reference | Using query tools |
| **docs/BENCHMARK_GUIDE.md** | Comprehensive benchmark guide | Understanding benchmarks |

---

## 🆘 Troubleshooting

### API Not Responding
```bash
# Check if container is running
docker ps | grep smi-bench

# Check container logs
docker logs smi-bench-dev --tail 50

# Restart container
docker compose restart smi-bench
```

### Task Timeout
```bash
# Increase timeout (default is 600s per task)
uv run python3 run_real_world_test.py \
    --timeout 1200 \
    --samples 1 \
    --models gpt-4o-mini \
    --simulation-mode live
```

### Model API Errors

**Missing credentials?**
```bash
# Set environment variables in .env
OPENROUTER_API_KEY=sk-or-v1-...
GOOGLE_API_KEY=AIza...
ANTHROPIC_API_KEY=sk-ant-...
```

**Rate limiting?**
- Reduce concurrent tasks
- Switch to faster model
- Increase timeout

### Corpus Issues

**Packages not found?**
```bash
# Check corpus structure
docker exec smi-bench-dev find /app/corpus -name "*.mv" | head -10

# Check manifest
docker exec smi-bench-dev cat /app/corpus/manifest.txt
```

**Metadata missing?**
```bash
# Ensure each package has metadata.json
docker exec smi-bench-dev find /app/corpus -name "metadata.json"
```

---

## 📦 Production Deployment

For production deployment, see:

```bash
# Production deployment guide
cat PRODUCTION_DEPLOYMENT.md
```

Key production considerations:
- ✅ Corpus mounted read-only
- ✅ Results volume persistent
- ✅ Logs backed up
- ✅ Prometheus metrics collected
- ✅ Health checks configured
- ✅ Error monitoring active

---

## ✅ What's Supported

### HTTP API (A2A Protocol v0.3.0)
- ✅ `POST /` - Submit benchmark task
- ✅ `GET /tasks/{id}/results` - Query task status
- ✅ `GET /health` - Health check
- ✅ `GET /info` - API information
- ✅ `GET /schema` - JSON schema
- ✅ `GET /validate` - Config validation
- ✅ `GET /metrics` - Prometheus metrics

### Benchmark Execution
- ✅ Corpus loading (MystenLabs/sui-packages structure)
- ✅ Mock agent (dry-run, no API costs)
- ✅ Real agent (live, uses model APIs)
- ✅ Multiple providers (OpenAI, Google, Anthropic)
- ✅ Subprocess management
- ✅ Graceful shutdown (SIGINT/SIGTERM)

### Monitoring & Analytics
- ✅ Prometheus metrics (duration, errors, tokens)
- ✅ Structured JSON logs (events.jsonl)
- ✅ Run metadata (config, timing, metrics)
- ✅ Cost tracking (token usage)
- ✅ Query tool (list, show, compare, cost)

### Docker Integration
- ✅ Container builds and runs reliably
- ✅ All endpoints accessible
- ✅ File persistence (logs, results)
- ✅ Easy start/stop/restart
- ✅ Log monitoring
- ✅ Resource monitoring

---

**Ready to test! Start with the 30-second quick start above.** 🚀
