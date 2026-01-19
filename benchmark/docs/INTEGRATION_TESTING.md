# Integration Testing & Run Analysis

This document covers multi-model integration testing and analyzing benchmark run results.

## Quick Start

### 1. Run Integration Tests

```bash
cd benchmark

# Run multi-model tests
uv run python scripts/test_multi_model_integration.py \
  --corpus-root <CORPUS_ROOT> \
  --manifest manifests/datasets/type_inhabitation_top25.txt
```

### 2. Query Past Runs

```bash
# List recent runs
uv run python scripts/query_benchmark_logs.py list --limit 10

# Filter by model
uv run python scripts/query_benchmark_logs.py list --model gpt-4o

# Show detailed run info
uv run python scripts/query_benchmark_logs.py show <RUN_ID>

# Analyze package performance
uv run python scripts/query_benchmark_logs.py analyze <RUN_ID>

# Compare multiple runs
uv run python scripts/query_benchmark_logs.py compare <RUN_ID_1> <RUN_ID_2>

# Estimate cost
uv run python scripts/query_benchmark_logs.py cost <RUN_ID>
```

---

## Integration Testing

### What It Tests

The integration test script (`scripts/test_multi_model_integration.py`) runs these scenarios:

1. **Baseline**: Single package, single model (fast validation)
2. **Multi-package**: Multiple packages, same model (throughput test)
3. **Model comparison**: Same packages, different models (quality comparison)
4. **Webhook delivery**: With callback URL (async workflow test)

### Test Output

```
================================================================================
SCENARIO: baseline_gpt4o_mini_1pkg
Model: gpt-4o-mini
Samples: 1
================================================================================

⏳ Submitting task...
✓ Task submitted: task_abc123
⏳ Waiting for completion...
📊 Partial metrics:
{
  "avg_hit_rate": 0.75
}

================================================================================
✓ COMPLETED in 45.2s
Status: completed

Key Metrics:
  • Avg Hit Rate: 0.75
  • Errors: 0
  • Total Prompt Tokens: 12,543
  • Total Completion Tokens: 3,421
  • Estimated Cost: $0.0241
================================================================================
```

### Customizing Tests

Edit `scripts/test_multi_model_integration.py` to:

- Add/remove models in `TEST_CONFIGS`
- Change package IDs in `SAMPLE_PACKAGES`
- Modify test scenarios in `run_all_tests()`

---

## Log Structure

### Directory Layout

```
logs/
├── phase2_1234567890/
│   ├── run_metadata.json       # Run configuration & timing
│   └── events.jsonl            # Streaming event log
results/
└── phase2_1234567890.json      # Final results with aggregate metrics
```

### Searchable Fields

#### run_metadata.json

```json
{
  "schema_version": 1,
  "benchmark": "phase2_inhabitation",
  "started_at_unix_seconds": 1234567890,
  "agent": "real-openai-compatible",
  "model": "gpt-4o-mini",
  "seed": 42,
  "rpc_url": "https://fullnode.mainnet.sui.io:443",
  "sender": "0x...",
  "gas_budget": 10000000,
  "simulation_mode": "dry-run",
  "max_plan_attempts": 2,
  "argv": ["smi-inhabit", "--corpus-root", "..."]
}
```

#### results JSON (aggregate)

```json
{
  "schema_version": 2,
  "aggregate": {
    "avg_hit_rate": 0.75,
    "max_hit_rate": 1.0,
    "errors": 0,
    "total_prompt_tokens": 12543,
    "total_completion_tokens": 3421,
    "planning_only_hit_rate": 0.80,
    "causality_success_rate": 0.95
  },
  "packages": [...]
}
```

#### Per-Package Results

```json
{
  "package_id": "0x1::option",
  "elapsed_seconds": 15.2,
  "score": {
    "targets": 4,
    "created_hits": 3,
    "hit_rate": 0.75
  },
  "plan_attempts": 2,
  "sim_attempts": 3,
  "error": null,
  "timed_out": false
}
```

---

## Query Examples

### Find All GPT-4o Runs

```bash
uv run python scripts/query_benchmark_logs.py list --model gpt-4o | grep gpt-4o
```

### Compare Model Performance

```bash
# Get run IDs for two different models
RUN_GPT4O_MINI=$(ls results/ | grep gpt4o_mini | head -1 | sed 's/.json//')
RUN_GPT4O=$(ls results/ | grep gpt4o | grep -v mini | head -1 | sed 's/.json//')

# Compare
uv run python scripts/query_benchmark_logs.py compare $RUN_GPT4O_MINI $RUN_GPT4O
```

### Calculate Total Cost for a Day

```bash
# Find all runs from today
TODAY=$(date +%Y-%m-%d)
cd results

for run in *.json; do
  RUN_ID=$(basename $run .json)
  uv run python ../scripts/query_benchmark_logs.py cost $RUN_ID
done | jq -s 'map(.total_cost_usd) | add'
```

### Find Slowest Packages

```bash
RUN_ID=<your_run_id>
uv run python scripts/query_benchmark_logs.py analyze $RUN_ID | \
  jq '.per_package_stats | sort_by(.elapsed_seconds) | reverse | .[0:5]'
```

---

## Metrics & Observability

### Prometheus Metrics

Available at `GET /metrics`:

```
# Task metrics
smi_bench_task_requests_total{agent_type="real-openai-compatible",simulation_mode="dry-run"} 5
smi_bench_task_duration_seconds_bucket{agent_type="...",simulation_mode="...",status="success",le="60"} 3
smi_bench_active_tasks 1

# HTTP metrics
smi_bench_http_requests_total{method="POST",endpoint="/validate",status_code="200"} 10
smi_bench_http_request_duration_seconds_sum{method="GET",endpoint="/info"} 0.023
```

### Grafana Dashboards

Example queries:

**Average Task Duration by Model:**

```promql
rate(smi_bench_task_duration_seconds_sum[5m]) / 
rate(smi_bench_task_duration_seconds_count[5m])
```

**Task Success Rate:**

```promql
sum(rate(smi_bench_task_requests_total{status="success"}[5m])) / 
sum(rate(smi_bench_task_requests_total[5m]))
```

**Active Tasks:**

```promql
smi_bench_active_tasks
```

---

## Cost Tracking

### Model Pricing (as of 2024)

| Model | Input (per 1M tokens) | Output (per 1M tokens) |
|-------|----------------------|------------------------|
| gpt-4o-mini | $0.15 | $0.60 |
| gpt-4o | $2.50 | $10.00 |
| claude-3-5-sonnet | $3.00 | $15.00 |

Update pricing in `scripts/query_benchmark_logs.py`:

```python
pricing = {
    "input_per_1k": 0.15 / 1000,   # Input cost per token
    "output_per_1k": 0.60 / 1000,  # Output cost per token
}
```

### Cost Estimation Example

```bash
$ uv run python scripts/query_benchmark_logs.py cost a2a_phase2_1234567890
{
  "run_id": "a2a_phase2_1234567890",
  "model": "gpt-4o-mini",
  "prompt_tokens": 12543,
  "completion_tokens": 3421,
  "total_tokens": 15964,
  "input_cost_usd": 0.0018814,
  "output_cost_usd": 0.0020526,
  "total_cost_usd": 0.003934
}
```

---

## Troubleshooting

### No runs found

```bash
# Check directories exist
ls -la logs/
ls -la results/a2a/

# Verify log format
cat logs/<RUN_ID>/run_metadata.json | jq .
```

### Token counts missing

Token tracking requires `--agent real-openai-compatible`. Mock agents don't track tokens.

### Cost estimates incorrect

Update pricing in `query_benchmark_logs.py` to match your model's actual rates.

---

## Next Steps

1. **Set up monitoring**: Connect Prometheus/Grafana to `/metrics`
2. **Automate testing**: Run integration tests in CI/CD
3. **Cost alerts**: Set budget thresholds based on token usage
4. **Performance baselines**: Track hit rate trends over time
