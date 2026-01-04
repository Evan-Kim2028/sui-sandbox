# Real-World Production Testing Guide

## ⚡ Fastest Way: New Real-World Test Script

**Recommended Entry Point:** `benchmark/run_real_world_test.py`

Submit benchmark tasks via HTTP API, test multiple models, get detailed analytics - all from a single command line.

### 30-Second Quick Test

```bash
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 1 \
    --models gpt-4o-mini,google/gemini-3-flash-preview \
    --simulation-mode dry-run
```

**What happens:**
- ✅ Starts 2 model tests sequentially
- ✅ Submits tasks via HTTP API
- ✅ Executes benchmark (dry-run mode, no API costs)
- ✅ Collects metrics (duration, errors, hit rate, tokens)
- ✅ Saves results to `real_world_test_results_*.json`
- ✅ Saves logs to `logs/` and results to `results/a2a/`

**Documentation:**
- 📖 Complete guide: [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md)
- 📊 Available models, Docker usage, analytics, workflows

---

## Traditional API Testing

This guide shows how to run real-world tests on mainnet packages using multiple models via direct HTTP API calls.

---

## Step 1: Verify Docker is Running

```bash
cd /Users/evandekim/Documents/learning_move/packages/sui-move-interface-extractor
docker compose ps
```

Expected output:
```
NAME           STATUS          PORTS
smi-bench-dev  Up (healthy)    0.0.0.0:9999->9999/tcp
```

If not running:
```bash
docker compose up -d smi-bench
```

---

## Step 2: Verify API Health

```bash
curl -s http://localhost:9999/health | jq '.status'
```

Expected: `"ok"`

---

## Step 3: Check Available Packages

```bash
docker exec smi-bench-dev cat /app/corpus/manifest.txt
```

This shows which mainnet packages are available for testing.

---

## Step 4: Run Real-World Tests

### Quick Test (Dry-Run, Fast)

Test with 2 packages, 3 models, mock agent (no API costs):

```bash
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 2 \
    --models gpt-4o-mini,google/gemini-3-flash-preview,google/gemini-2.5-flash-preview \
    --simulation-mode dry-run
```

**Expected Duration:** ~10-20 seconds total
**Cost:** $0 (dry-run mode, mock agent)

---

### Production Test (Live, Real Models)

Test with 2 packages, 3 models, live mode (uses real model APIs):

```bash
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 2 \
    --models gpt-4o,google/gemini-3-flash-preview,google/gemini-2.5-flash-preview \
    --simulation-mode live
```

**Expected Duration:** ~5-15 minutes (depending on model speed)
**Cost:** $0.05-$0.50 (depending on models and packages)

---

### Custom Test

Test with specific models, custom sample count:

```bash
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 5 \
    --models gpt-4o,claude-sonnet-4-5-20250929 \
    --simulation-mode dry-run \
    --verbose
```

---

## Available Models

| Model ID | Name | Provider | Speed | Cost | Use Case |
|-----------|-------|----------|--------|----------|
| `gpt-4o` | GPT-4o | Medium | $$ | Top quality |
| `gpt-4o-mini` | GPT-4o-mini | Fast | $ | Fast iteration |
| `google/gemini-3-flash-preview` | Gemini Flash 3 | Fast | $ | Latest features |
| `google/gemini-2.5-flash-preview` | Gemini Flash 2.5 | Very Fast | $ | Quickest results |
| `claude-sonnet-4-5-20250929` | Claude Sonnet 4.5 | Medium | $$$ | Highest quality |

---

## Output & Analytics

### What You'll See

1. **Real-time Progress:**
   ```
   [16:30:15] API is healthy
   [16:30:15] Found 2 packages in manifest
   [16:30:15] Testing packages: 0x00, 0x01

   [16:30:20] Submitting task with GPT-4o-mini
   [16:30:20] Task submitted: abc-123-def
   [16:30:25]   [5s] Status: running, Run ID: a2a_phase2_1767545420
   [16:30:35]   [15s] Status: completed
   [16:30:35] ✅ Task completed in 15.3s
   ```

2. **Detailed Summary:**
   ```
   ==================================================================
   REAL-WORLD TEST SUMMARY
   ==================================================================

   Models Tested: 3
   Successful: 3
   Failed: 0
   Total Packages: 6
   Total Errors: 0
   Total Duration: 45.8s (0.8 min)

   ----------------------------------------------------------------------
   Model                          Status     Packages   Hit Rate   Errors   Duration
   ----------------------------------------------------------------------
   GPT-4o-mini                   ✅          2          0.00       0         15.3s
   Gemini Flash 3                  ✅          2          0.00       0         16.2s
   Gemini Flash 2.5                ✅          2          0.00       0         14.3s
   ----------------------------------------------------------------------
   ```

3. **Detailed Metrics:**
   ```
   📊 GPT-4o-mini:
      Duration: 15.3s
      Packages: 2
      Hit Rate: 0.0000
      Errors: 0
      Prompt Tokens: 0
      Completion Tokens: 0
      Run ID: a2a_phase2_1767545420
      Results: /app/benchmark/results/a2a/a2a_phase2_1767545420.json
      Logs: /app/benchmark/logs/a2a_phase2_1767545420/
   ```

4. **Results File:**
   ```bash
   real_world_test_results_20260104_163045.json
   ```

---

## Viewing Detailed Results

### Via Docker Logs
```bash
# Real-time logs
docker logs -f smi-bench-dev

# Filter for specific run
docker logs smi-bench-dev | grep "a2a_phase2_1767545420"
```

### Via Results Files
```bash
# List recent results
docker exec smi-bench-dev ls -lt /app/benchmark/results/a2a/

# View specific result
docker exec smi-bench-dev cat /app/benchmark/results/a2a/a2a_phase2_1767545420.json | jq .
```

### Via Event Logs
```bash
# List run directories
docker exec smi-bench-dev ls -lt /app/benchmark/logs/

# View events
docker exec smi-bench-dev cat /app/benchmark/logs/a2a_phase2_1767545420/events.jsonl
```

### Via Query Tool
```bash
cd benchmark

# List all runs
uv run python3 scripts/query_benchmark_logs.py list --limit 10

# Show run details
uv run python3 scripts/query_benchmark_logs.py show a2a_phase2_1767545420

# Calculate costs (for real models)
uv run python3 scripts/query_benchmark_logs.py cost a2a_phase2_1767545420
```

---

## Experimentation

### Comparing Models

Test the same packages with different models:

```bash
# Fast test to compare models
uv run python3 run_real_world_test.py \
    --samples 3 \
    --models gpt-4o,google/gemini-3-flash-preview,claude-sonnet-4-5-20250929 \
    --simulation-mode dry-run

# Compare the results
# Check:
#   - Duration per model
#   - Hit rate per model
#   - Error rate per model
```

### Scaling Up

Test with more packages:

```bash
# Medium scale (5 packages)
uv run python3 run_real_world_test.py \
    --samples 5 \
    --models gpt-4o-mini,google/gemini-3-flash-preview

# Large scale (10 packages)
uv run python3 run_real_world_test.py \
    --samples 10 \
    --models gpt-4o
```

### Testing New Packages

Add packages to corpus:

```bash
# Add new package
cp -r /path/to/new-package /Users/evandekim/Documents/learning_move/packages/sui-move-interface-extractor/benchmark/data/corpus/0x00/new_package_id/
cd /Users/evandekim/Documents/learning_move/packages/sui-move-interface-extractor/benchmark/data/corpus/0x00/new_package_id/bytecode_modules
# ... copy .mv files ...

# Add metadata.json
echo '{"id":"new_package_id","name":"New Package"}' > metadata.json

# Add to manifest
echo "new_package_id" >> manifest.txt

# Restart container to pick up new packages
docker compose restart smi-bench
```

---

## Mixing Models in Experiments

You can mix any models in a single test:

```bash
# Mix fast and slow models
uv run python3 run_real_world_test.py \
    --models gpt-4o-mini,google/gemini-2.5-flash-preview,gpt-4o,claude-sonnet-4-5-20250929 \
    --simulation-mode dry-run

# This will:
# 1. Test 2 packages with GPT-4o-mini (fast, cheap)
# 2. Test 2 packages with Gemini 2.5 Flash (fast, cheap)
# 3. Test 2 packages with GPT-4o (medium, more expensive)
# 4. Test 2 packages with Claude Sonnet 4.5 (high quality, most expensive)
```

This is great for:
- Comparing model performance
- Finding the best model for your use case
- Testing cost vs. quality tradeoffs

---

## Docker Fluid Usage

### Start/Stop/Restart

```bash
# Start
docker compose up -d smi-bench

# Stop
docker compose stop smi-bench

# Restart (preserves logs/results)
docker compose restart smi-bench

# Stop and remove
docker compose down
```

### Accessing Logs

```bash
# Follow real-time logs
docker logs -f smi-bench-dev

# Last 100 lines
docker logs smi-bench-dev --tail 100

# Logs for last hour
docker logs smi-bench-dev --since 1h
```

### Resource Monitoring

```bash
# Check container stats
docker stats smi-bench-dev

# Check container health
curl -s http://localhost:9999/health | jq .

# Check API metrics
curl -s http://localhost:9999/metrics | grep smi_bench
```

---

## Troubleshooting

### API Not Responding
```bash
# Check if container is running
docker ps | grep smi-bench

# Check container logs
docker logs smi-bench-dev --tail 20

# Restart container
docker compose restart smi-bench
```

### Task Timeout
```bash
# Increase timeout
uv run python3 run_real_world_test.py \
    --timeout 1200 \
    --samples 2 \
    --models gpt-4o \
    --simulation-mode live
```

### Model API Errors

Check if you have proper API credentials:

```bash
# OpenAI (for gpt-4o, gpt-4o-mini)
echo $OPENAI_API_KEY

# Google (for gemini models)
echo $GOOGLE_API_KEY

# Anthropic (for claude models)
echo $ANTHROPIC_API_KEY
```

If credentials are missing, add them to `.env`:

```bash
# In benchmark/.env
OPENAI_API_KEY=sk-...
GOOGLE_API_KEY=AIza...
ANTHROPIC_API_KEY=sk-ant-...
```

---

## Best Practices

1. **Start with Dry-Run Mode**
   - Test configuration without API costs
   - Verify everything works before going live

2. **Start with Small Samples**
   - Begin with 1-2 packages
   - Scale up once you're confident

3. **Use Fast Models First**
   - gpt-4o-mini, gemini-2.5-flash-preview for quick iteration
   - Switch to gpt-4o, claude for final production runs

4. **Monitor Costs**
   - Track token usage per model
   - Compare cost vs. quality tradeoffs

5. **Save Results**
   - Run results are saved as JSON
   - Use query tool to analyze later

6. **Keep Logs**
   - Logs contain detailed execution information
   - Essential for debugging issues

---

## Next Steps

After running successful tests:

1. **Analyze Results**
   ```bash
   uv run python3 scripts/query_benchmark_logs.py list
   uv run python3 scripts/query_benchmark_logs.py show <run_id>
   ```

2. **Compare Runs**
   ```bash
   uv run python3 scripts/query_benchmark_logs.py compare <run_id1> <run_id2>
   ```

3. **Scale Up**
   - Test with more packages
   - Test with production corpus (50+ packages)
   - Automate runs via scripts

4. **Production Deployment**
   - Set up monitoring (Prometheus + Grafana)
   - Configure alerts for failed tasks
   - Set up log rotation

---

**You're ready to run real-world production tests!**

Start with the quick test (dry-run) and experiment from there.
