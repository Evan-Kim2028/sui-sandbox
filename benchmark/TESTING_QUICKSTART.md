# Testing Quick Start

## 🚀 Ready to Test? Follow These Steps

### 1. Start the Docker API

```bash
cd benchmark
docker compose up -d smi-bench

# Verify it's running
curl http://localhost:9999/info | jq .
```

### 2. Run Multi-Model Integration Tests

```bash
# Quick test (1-2 packages, 2-3 models)
./scripts/test_multi_model_integration.py \
  --corpus-root <YOUR_CORPUS_PATH> \
  --manifest manifests/datasets/type_inhabitation_top25.txt

# This will:
# ✓ Test GPT-4o-mini baseline (fast, cheap)
# ✓ Test multiple packages
# ✓ Compare GPT-4o vs GPT-4o-mini
# ✓ Save results to test_results_<timestamp>.json
```

### 3. Query Past Runs

```bash
# List all runs
./scripts/query_benchmark_logs.py list

# Filter by model
./scripts/query_benchmark_logs.py list --model gpt-4o

# Show detailed results
./scripts/query_benchmark_logs.py show <RUN_ID>

# Analyze per-package performance
./scripts/query_benchmark_logs.py analyze <RUN_ID>

# Compare two models
./scripts/query_benchmark_logs.py compare <RUN_ID_1> <RUN_ID_2>

# Calculate cost
./scripts/query_benchmark_logs.py cost <RUN_ID>
```

### 4. Monitor with Prometheus

```bash
# View raw metrics
curl http://localhost:9999/metrics

# Key metrics:
# - smi_bench_task_duration_seconds (how long tasks take)
# - smi_bench_task_errors_total (error rates)
# - smi_bench_active_tasks (current load)
# - smi_bench_http_requests_total (API traffic)
```

---

## 📊 What Gets Tracked?

### Per Run
- **Model used**: `gpt-4o-mini`, `gpt-4o`, `claude-3-5-sonnet`, etc.
- **Duration**: Total time and per-package time
- **Tokens**: Prompt + completion tokens (for cost tracking)
- **Hit rate**: How many target types were created
- **Errors**: Package failures and timeouts

### Searchable Logs

**Location**: `logs/<run_id>/` and `results/a2a/<run_id>.json`

**Find runs by**:
- Model name
- Date/timestamp
- Package ID
- Error type
- Hit rate threshold

---

## 💰 Cost Tracking

Token usage is automatically tracked. Estimate costs:

```bash
./scripts/query_benchmark_logs.py cost <RUN_ID>
```

**Output:**
```json
{
  "run_id": "a2a_phase2_1234567890",
  "model": "gpt-4o-mini",
  "total_tokens": 15964,
  "total_cost_usd": 0.0039
}
```

Update pricing in `scripts/query_benchmark_logs.py` for accurate estimates.

---

## 🔍 Example Workflows

### Test 3 Models on Same Packages

Edit `scripts/test_multi_model_integration.py`:

```python
# In run_all_tests():
await self.test_scenario("gpt4o_mini", model="gpt-4o-mini", samples=2)
await self.test_scenario("gpt4o", model="gpt-4o", samples=2)
await self.test_scenario("claude", model="claude-3-5-sonnet-20241022", samples=2)
```

### Find Best Model for Your Corpus

```bash
# Run tests
./scripts/test_multi_model_integration.py --corpus-root ... --manifest ...

# Compare results
RUN_IDS=$(ls results/a2a/*.json | tail -3 | xargs -n1 basename | sed 's/.json//')
./scripts/query_benchmark_logs.py compare $RUN_IDS | jq .
```

### Track Costs Over Time

```bash
# Get all runs from last 7 days
find results/a2a -name "*.json" -mtime -7 | while read f; do
  RUN_ID=$(basename $f .json)
  ./scripts/query_benchmark_logs.py cost $RUN_ID
done | jq -s 'map(.total_cost_usd) | add'
```

---

## 🐛 Troubleshooting

### "Task not found" when querying results

The task may have completed before the results were cached. Check `results/a2a/` for the output file.

### No token counts in results

Token tracking only works with `agent: real-openai-compatible`. Mock agents don't track tokens.

### Docker container not responding

```bash
# Check container logs
docker compose logs smi-bench

# Restart
docker compose restart smi-bench

# Rebuild if code changed
docker compose build smi-bench && docker compose up -d smi-bench
```

---

## 📚 Full Documentation

- **[Integration Testing Guide](docs/INTEGRATION_TESTING.md)** - Complete testing reference
- **[A2A Examples](docs/A2A_EXAMPLES.md)** - API endpoint documentation
- **[Architecture](docs/ARCHITECTURE.md)** - System design

---

## ✨ Next Steps

1. **Run your first test** - Use the multi-model script
2. **Set up monitoring** - Connect Prometheus/Grafana to `/metrics`
3. **Analyze results** - Use query tool to find best model for your use case
4. **Optimize costs** - Track token usage and switch models based on performance/cost ratio
