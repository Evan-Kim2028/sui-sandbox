# A2A Protocol Examples

This document provides concrete, copy-paste ready examples of the A2A protocol usage for the Sui Move Interface Extractor benchmark.

All examples are based on real executions and can be validated with `smi-a2a-validate-bundle`.

## API Endpoints

The A2A green agent exposes the following HTTP endpoints:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check with binary/RPC/executor status |
| `/validate` | POST | Validate config without executing (dry-run) |
| `/schema` | GET | JSON Schema for EvalConfig |
| `/info` | GET | API version, capabilities, and limits |
| `/.well-known/agent-card.json` | GET | A2A agent card |
| `/` | POST | A2A JSON-RPC endpoint |

### Config Validation (Pre-flight Check)

Before submitting a task, validate your config:

```bash
curl -X POST http://localhost:9999/validate \
  -H "Content-Type: application/json" \
  -d '{"config": {"corpus_root": "/app/corpus", "package_ids_file": "/app/manifest.txt"}}'
```

**Response (valid):**
```json
{"valid": true, "config": {...normalized config...}, "warnings": []}
```

**Response (invalid):**
```json
{"valid": false, "error": "Invalid config: corpus_root - missing or empty", "warnings": []}
```

**Response (with unknown fields):**
```json
{"valid": true, "config": {...}, "warnings": ["Unknown config fields (will be ignored): ['typo_field']"]}
```

### Get Config Schema

```bash
curl http://localhost:9999/schema | jq .
```

Returns JSON Schema for client-side validation.

### Get API Info

```bash
curl http://localhost:9999/info | jq .
```

Returns version, capabilities, and limits (e.g., max concurrent tasks).

### Query Task Results (Partial or Complete)

```bash
# Get current status of a running task
curl http://localhost:9999/tasks/{task_id}/results | jq .
```

**Response (running):**
```json
{
  "task_id": "...",
  "status": "running",
  "started_at": 1234567890,
  "agent": "real-openai-compatible",
  "partial_metrics": {}
}
```

**Response (completed):**
```json
{
  "task_id": "...",
  "status": "completed",
  "duration_seconds": 123.45,
  "bundle": {...},
  "metrics": {
    "avg_hit_rate": 0.75,
    "total_prompt_tokens": 12543,
    "total_completion_tokens": 3421
  }
}
```

### Webhook Callbacks (Async Workflows)

Submit a task with `callback_url` to receive results via HTTP POST when complete:

```bash
curl -X POST http://localhost:9999 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "message/send",
    "params": {
      "message": {
        "messageId": "msg_123",
        "role": "user",
        "parts": [{
          "text": "{\"config\": {\"corpus_root\": \"/app/corpus\", \"package_ids_file\": \"/app/manifest.txt\", \"callback_url\": \"https://my-service.com/webhook\"}}"
        }]
      }
    }
  }'
```

When the task completes, the API will POST results to `https://my-service.com/webhook`.

### Prometheus Metrics

```bash
curl http://localhost:9999/metrics
```

Returns Prometheus-format metrics for monitoring:
- Task throughput, duration, errors
- HTTP request rates and latencies
- Active task count
- Config validation rates

See [Integration Testing Guide](INTEGRATION_TESTING.md) for Grafana dashboard setup.

## Quick Start Examples

**Minimal smoke test** (1 package, fast feedback):
```bash
cd benchmark
uv run smi-a2a-smoke \
  --scenario scenario_smi \
  --corpus-root <CORPUS_ROOT> \
  --dataset type_inhabitation_top25 \
  --samples 1
```

**Validation**:
```bash
uv run smi-a2a-validate-bundle results/a2a_smoke_response.json
```

## Smoke Test Walkthrough (Annotated)

### Step 1: Start the Scenario

The scenario manager launches both agents (green and purple):

```bash
cd benchmark
uv run smi-agentbeats-scenario scenario_smi --launch-mode current
```

This spawns:
- Green agent on port 9999 (`smi-a2a-green`)
- Purple agent on port 9998 (`smi-a2a-purple`)
- Writes PIDs to `scenario_smi/.scenario_pids.json`

### Step 2: Send a Request

The `smi-a2a-smoke` tool constructs a JSON-RPC request:

**Request Payload** (`results/a2a_request_1pkg.json`):
```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "message/send",
  "params": {
    "message": {
      "messageId": "m_one_pkg",
      "role": "user",
      "parts": [
        {
          "text": "{\"config\": {\"corpus_root\": \"<CORPUS_ROOT>\", \"package_ids_file\": \"manifests/standard_phase2_no_framework.txt\", \"samples\": 1, \"rpc_url\": \"https://fullnode.mainnet.sui.io:443\", \"simulation_mode\": \"dry-run\", \"per_package_timeout_seconds\": 90, \"max_plan_attempts\": 2, \"continue_on_error\": true, \"resume\": false }}"
        }
      ]
    }
  }
}
```

**Field-by-field explanation:**

| Field | Meaning | Example Value |
|-------|----------|---------------|
| `jsonrpc` | Protocol version (always 2.0) | `"2.0"` |
| `id` | Request identifier (client can choose) | `"1"` |
| `method` | A2A method name | `"message/send"` |
| `params.message.messageId` | Unique message ID | `"m_one_pkg"` |
| `params.message.role` | Sender role | `"user"` |
| `params.message.parts[0].text` | Config JSON (stringified) | See below |

**Config JSON structure:**
```json
{
  "corpus_root": "<CORPUS_ROOT>",
  "package_ids_file": "manifests/standard_phase2_no_framework.txt",
  "samples": 1,
  "rpc_url": "https://fullnode.mainnet.sui.io:443",
  "simulation_mode": "dry-run",
  "per_package_timeout_seconds": 90,
  "max_plan_attempts": 2,
  "continue_on_error": true,
  "resume": false
}
```

#### Config Reference

**Core Fields (Required/Common):**

| Config Key | Description | Default | Validation |
|------------|-------------|---------|------------|
| `corpus_root` | Path to bytecode corpus | **Required** | Must be non-empty |
| `package_ids_file` | Manifest file (one ID per line) | **Required** | Must be non-empty |
| `samples` | Number of packages to process | `0` (all) | >= 0 |
| `agent` | Agent type to use | `"real-openai-compatible"` | One of: `mock-empty`, `mock-planfile`, `real-openai-compatible`, `baseline-search`, `template-search` |
| `rpc_url` | Sui fullnode RPC for simulation | `"https://fullnode.mainnet.sui.io:443"` | Valid URL |
| `simulation_mode` | Transaction simulation mode | `"dry-run"` | One of: `dry-run`, `dev-inspect`, `build-only` |
| `per_package_timeout_seconds` | Wall-clock budget per package | `300.0` | > 0 |
| `max_plan_attempts` | Max PTB replanning attempts | `2` | > 0 |
| `continue_on_error` | Keep going if package fails | `true` | boolean |
| `resume` | Resume from existing output file | `true` | boolean |
| `run_id` | Custom run identifier | Auto-generated | Optional string |
| `model` | Per-request model override (takes precedence over `SMI_MODEL` env var) | `null` (use env var) | Non-empty if provided |

**P0: Production-Critical Fields:**

| Config Key | Description | Default | Validation |
|------------|-------------|---------|------------|
| `seed` | Random seed for reproducible sampling | `0` | >= 0 |
| `sender` | Sui address for tx simulation | `null` | **Required** for `dev-inspect`/`execute` modes |
| `gas_budget` | Gas budget for dry-run simulation | `10000000` (10M) | > 0 |
| `gas_coin` | Specific gas coin object ID | `null` (auto-select) | Valid object ID if provided |
| `gas_budget_ladder` | Comma-separated retry budgets on InsufficientGas | `"20000000,50000000"` | Comma-separated positive ints |
| `max_errors` | Stop run after N package errors | `25` | > 0 |
| `max_run_seconds` | Wall-clock budget for entire run | `null` (unlimited) | > 0 if provided |

**P1: Flexibility/Tuning Fields:**

| Config Key | Description | Default | Validation |
|------------|-------------|---------|------------|
| `max_planning_calls` | Max LLM calls per package (progressive exposure) | `50` | > 0 |
| `checkpoint_every` | Save partial results every N packages | `10` | > 0 |
| `max_heuristic_variants` | Max deterministic PTB variants per plan attempt | `4` | >= 1 |
| `baseline_max_candidates` | Max candidates in baseline-search mode | `25` | >= 1 |
| `include_created_types` | Include full created object type lists in output | `false` | boolean |
| `require_dry_run` | Fail if dry-run unavailable (no dev-inspect fallback) | `false` | Only valid with `simulation_mode: "dry-run"` |

#### Validation Rules

- **Cross-field validation**: `require_dry_run: true` requires `simulation_mode: "dry-run"`
- **Cross-field validation**: `simulation_mode: "dev-inspect"` or `"execute"` requires `sender` to be set
- **Unknown fields**: Silently ignored (use `/validate` endpoint to check)
- **Type coercion**: Strings `"true"`, `"1"`, `"yes"` coerced to boolean `true`

### Model Override (Per-Request Model Switching)

**Use case:** Run multiple models with different configurations using a single long-running container, avoiding container restart overhead.

**Model precedence rules:**
1. **API payload** `config.model` (highest priority)
2. **Environment variable** `SMI_MODEL` (fallback)
3. **Default** (from agent code or config file)

**Example: Override model in API request**
```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "message/send",
  "params": {
    "message": {
      "messageId": "run_with_gpt4",
      "role": "user",
      "parts": [{
        "text": "{\"config\": {\"corpus_root\": \"<CORPUS_ROOT>\", \"package_ids_file\": \"manifests/standard_phase2_no_framework.txt\", \"samples\": 5, \"model\": \"openai/gpt-4-turbo\"}, \"out_dir\": \"/app/results\"}"
      }]
    }
  }
}
```

**Docker workflow:**
```bash
# Start container once (or use docker compose up -d)
docker run -d --name smi-bench -p 9999:9999 smi-bench:latest

# Run benchmark with model A
curl -X POST http://localhost:9999/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc": "2.0", "id": "1", "method": "message/send", ... "model": "gpt-4" ...}'

# Run benchmark with model B (no container restart)
curl -X POST http://localhost:9999/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc": "2.0", "id": "2", "method": "message/send", ... "model": "gemini-3-flash" ...}'
```

**Benefits:**
- **Zero startup time:** No container rebuild or restart between models
- **Port isolation:** Single port, no port management complexity
- **Efficient:** Reuses Python venv and Rust binaries across runs
- **Production-ready:** Aligns with long-running service patterns

**Limitations:**
- **Sequential execution:** Tasks run one-at-a-time (no true parallelism)
- **State sharing:** Same container memory/tmp across runs (appropriate for benchmarks)
- **Container lifecycle:** Container must be stopped manually when done (use `--cleanup` flag in `run_docker_benchmark.sh`)

### Step 3: Receive Response

The green agent returns a JSON-RPC response with three artifacts:

**Response structure** (simplified):
```json
{
  "id": "1",
  "jsonrpc": "2.0",
  "result": {
    "artifacts": [
      {
        "name": "evaluation_bundle",
        "parts": [{"kind": "text", "text": "{...}"}]
      },
      {
        "name": "phase2_results.json",
        "parts": [{"kind": "text", "text": "{...}"}]
      },
      {
        "name": "run_metadata.json",
        "parts": [{"kind": "text", "text": "{...}"}]
      }
    ],
    "history": [...],
    "id": "0bdd6112-6577-4565-80a5-2b316b97e648",
    "kind": "task",
    "status": {"state": "completed", "timestamp": "2026-01-02T02:49:34.157720+00:00"}
  }
}
```

### Step 4: Parse Evaluation Bundle

The `evaluation_bundle` artifact contains the most important summary:

**Example bundle** (extracted and formatted):
```json
{
  "schema_version": 1,
  "spec_url": "smi-bench:evaluation_bundle:v1",
  "benchmark": "phase2_inhabit",
  "run_id": "a2a_phase2_1767323740",
  "exit_code": 0,
  "timings": {
    "started_at_unix_seconds": 1767323740,
    "finished_at_unix_seconds": 1767323757,
    "elapsed_seconds": 16.33986186981201
  },
  "config": {
    "continue_on_error": true,
    "corpus_root": "<CORPUS_ROOT>",
    "max_plan_attempts": 2,
    "package_ids_file": "manifests/standard_phase2_no_framework.txt",
    "per_package_timeout_seconds": 90.0,
    "resume": false,
    "rpc_url": "https://fullnode.mainnet.sui.io:443",
    "samples": 1,
    "simulation_mode": "dry-run"
  },
  "metrics": {
    "avg_hit_rate": 0.0,
    "errors": 0,
    "packages_timed_out": 0,
    "packages_total": 1,
    "packages_with_error": 0
  },
  "errors": [],
  "artifacts": {
    "events_path": "logs/a2a_phase2_1767323740/events.jsonl",
    "results_path": "results/a2a/a2a_phase2_1767323740.json",
    "run_metadata_path": "logs/a2a_phase2_1767323740/run_metadata.json"
  }
}
```

**Key metrics:**

| Metric | Meaning | Good Value |
|--------|----------|------------|
| `exit_code` | Process exit code (0=success) | `0` |
| `metrics.avg_hit_rate` | Average `created_hits / targets` | Higher is better |
| `metrics.errors` | Number of packages with errors | `0` |
| `metrics.packages_timed_out` | Packages that hit timeout | `0` |
| `metrics.packages_total` | Total packages processed | As expected |

**Artifact paths:**

| Path | Content |
|------|---------|
| `events_path` | Line-delimited event stream (see Event Streaming below) |
| `results_path` | Full Phase II output JSON with per-package details |
| `run_metadata_path` | Exact argv and environment used |

### Step 5: Validate

Check that the bundle conforms to the schema:

```bash
cd benchmark
uv run smi-a2a-validate-bundle results/a2a_smoke_response.json
```

Expected output:
```
valid
```

## Request/Response Reference

### Example 1: Minimal Smoke Request

**Use case:** Quick health check and protocol validation

```bash
cd benchmark
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file manifests/standard_phase2_no_framework.txt \
  --samples 1
```

**Request characteristics:**
- `samples: 1` (fastest)
- `per_package_timeout_seconds: 90` (default)
- No `--scenario` flag (assumes agents already running)

**Response characteristics:**
- `run_id` generated automatically
- `metrics.packages_total == 1`
- Artifacts written to `results/a2a/` and `logs/`

### Example 2: Standard Phase II Request

**Use case:** Full benchmark run on manifest

```bash
curl -X POST http://127.0.0.1:9999/ \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "message/send",
    "params": {
      "message": {
        "messageId": "full_manifest_run",
        "role": "user",
        "parts": [{
          "text": "{\"config\": {
            \"corpus_root\": \"/path/to/corpus\",
            \"package_ids_file\": \"manifests/standard_phase2_no_framework.txt\",
            \"rpc_url\": \"https://fullnode.mainnet.sui.io:443\",
            \"simulation_mode\": \"dry-run\",
            \"per_package_timeout_seconds\": 90,
            \"max_plan_attempts\": 2,
            \"continue_on_error\": true,
            \"resume\": false
          }}"
        }]
      }
    }
  }'
```

**Response characteristics:**
- Processes all packages in manifest (290+)
- Checkpoints written if `--checkpoint-every` set (not in A2A mode)
- `metrics.avg_hit_rate` aggregates across all packages

### Example 3: Resume Request

**Use case:** Continue interrupted run from previous output

```bash
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file manifests/standard_phase2_no_framework.txt \
  --samples 100 \
  --resume
```

**Config difference:**
- `"resume": true` in config
- Green agent reads existing `results/a2a/<run_id>.json`
- Skips already-processed packages

**Response characteristics:**
- `run_id` reused from existing output
- Only unprocessed packages are run
- Aggregate metrics computed from all (old + new)

## Event Streaming Examples

The green agent streams events as JSONL (one JSON object per line) to `logs/<run_id>/events.jsonl`.

### Typical Event Sequence

For a successful single-package run:

```json
{"agent": "real-openai-compatible", "event": "run_started", "seed": 0, "simulation_mode": "dry-run", "started_at_unix_seconds": 1767323740, "t": 1767323740}
{"api_key": "len=73 suffix=6abef7", "base_url": "https://openrouter.ai/api/v1", "event": "agent_effective_config", "model": "google/gemini-3-flash-preview", "provider": "openai_compatible", "t": 1767323741}
{"event": "package_started", "i": 1, "package_id": "0x00db9a10bb9536ab367b7d1ffa404c1d6c55f009076df1139dc108dd86608bbe", "t": 1767323741}
{"event": "sim_attempt_started", "gas_budget": 10000000, "i": 1, "package_id": "0x00db9a10bb9536ab367b7d1ffa404c1d6c55f009076df1139dc108dd86608bbe", "plan_attempt": 2, "plan_variant": "base", "sim_attempt": 1, "t": 1767323756}
{"created_hits": 0, "dry_run_ok": false, "elapsed_seconds": 15.617967750004027, "error": null, "event": "package_finished", "i": 1, "package_id": "0x00db9a10bb9536ab367b7d1ffa404c1d6c55f009076df1139dc108dd86608bbe", "plan_variant": "base", "t": 1767323756, "targets": 2, "timed_out": false}
{"avg_hit_rate": 0.0, "errors": 0, "event": "run_finished", "finished_at_unix_seconds": 1767323756, "samples": 1, "t": 1767323756}
```

### Progressive Exposure (Design Note)

**Important:** The progressive exposure feature with `need_more` requests is documented below for completeness. **Response handling is not yet implemented** in the current version. See [ARCHITECTURE.md](ARCHITECTURE.md#progressive-exposure-design) for current implementation status and [A2A_TUNING.md](A2A_TUNING.md) for practical tuning guidance.

**Intended `need_more` Workflow (when fully implemented):**

When a model needs more interface details than initially provided, it can request specific functions:

```json
{
  "need_more": [
    "0x2::coin::mint",
    "0x2::coin::transfer",
    "0xPACKAGE::module::create_wrapper"
  ],
  "reason": "Need to understand mint/transfer entry points and wrapper initialization"
}
```

The runner would then:
1. Extract requested target strings from the `need_more` array
2. Re-invoke the model with a focused interface summary using `mode="focused"`
3. Include only the requested functions with full signatures

**Example Progressive Exposure Sequence:**

*Call 1 (Initial Request):*
```json
{"event": "package_started", "package_id": "0x...", "t": 1767323741}
```

Model returns (first attempt):
```json
{
  "need_more": [
    "0x2::coin::mint",
    "0x2::coin::transfer"
  ],
  "reason": "Need mint and transfer functions to create coins"
}
```

*Call 2 (Focused Details):*
The runner re-invokes the model with:
```python
summarize_interface(
    interface_json,
    max_functions=60,
    mode="focused",
    requested_targets={"0x2::coin::mint", "0x2::coin::transfer"}
)
```

Model returns (final PTB plan):
```json
{
  "calls": [
    {
      "target": "0x2::coin::mint",
      "type_args": ["0x2::sui::SUI"],
      "args": [
        {"u64": 1000000000}
      ]
    }
  ]
}
```

*Call 3 (Simulation):*
```json
{"event": "sim_attempt_started", "gas_budget": 10000000, "plan_attempt": 1, "planning_call": 2, "t": 1767323756}
```

**Note:** When progressive exposure is fully implemented, additional event types will track `need_more` requests and focused summary generation. The `--max-planning-calls` parameter controls how many LLM calls are allowed per package (default: 50, recommended: 2-3 for production).

### Event Types

| Event Type | Meaning | Fields |
|------------|----------|---------|
| `run_started` | Benchmark started | `seed`, `simulation_mode`, `started_at_unix_seconds` |
| `agent_effective_config` | Agent config resolved | `api_key` (truncated), `base_url`, `model`, `provider` |
| `package_started` | Package processing started | `i` (index), `package_id` |
| `plan_attempt_harness_error` | Planning failed | `error`, `package_id`, `i` |
| `sim_attempt_started` | Simulation started | `gas_budget`, `plan_attempt`, `plan_variant`, `sim_attempt` |
| `sim_attempt_harness_error` | Simulation failed | `error`, `package_id`, `plan_attempt`, `sim_attempt` |
| `package_finished` | Package completed | `created_hits`, `dry_run_ok`, `elapsed_seconds`, `targets`, `timed_out` |
| `run_finished` | Benchmark finished | `avg_hit_rate`, `errors`, `finished_at_unix_seconds`, `samples` |

### Parsing Events Programmatically

```bash
cd benchmark

# Count packages processed
grep "event.*package_finished" logs/a2a_phase2_*/events.jsonl | wc -l

# Extract errors
grep "error" logs/a2a_phase2_*/events.jsonl | jq -r '.event, .error'

# Watch live events
tail -f logs/a2a_phase2_*/events.jsonl | jq -r '.event, .t'
```

## Common Patterns

### Pattern 1: Batch Processing with Checkpoints

**Scenario:** Run 100 packages, writing results after every 10

```bash
# Note: Checkpoints are handled by smi-inhabit directly,
# not via A2A config. For A2A mode, all packages run in one session.
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file manifests/standard_phase2_no_framework.txt \
  --samples 100
```

**Verification:**
```bash
# Check how many packages completed
uv run python scripts/phase2_analyze.py results/a2a/<run_id>.json
```

### Pattern 2: Error Recovery

**Scenario:** Continue processing even if some packages fail

```bash
cd benchmark
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file manifests/standard_phase2_no_framework.txt \
  --samples 50 \
  --per-package-timeout-seconds 90
```

**Config:**
- `"continue_on_error": true` (default in examples)

**Response:**
- `metrics.errors` will be > 0 if any package fails
- `metrics.packages_total` includes both successful and failed packages
- `evaluation_bundle.errors` list contains per-package errors

### Pattern 3: Progressive Timeout Adjustment

**Scenario:** Shorter timeout for known-fast packages, longer for complex ones

```bash
cd benchmark

# Fast packages (simple protocols)
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file manifests/simple_packages.txt \
  --samples 50 \
  --per-package-timeout-seconds 30

# Complex packages (DeFi, AMMs)
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file manifests/complex_packages.txt \
  --samples 50 \
  --per-package-timeout-seconds 300
```

### Pattern 4: Debug Mode for Single Package

**Scenario:** Investigate why a specific package fails

1. Create one-line manifest:
```bash
cd benchmark
printf "%s\n" 0x00db9a10bb9536ab367b7d1ffa404c1d6c55f009076df1139dc108dd86608bbe > debug_one_pkg.txt
```

2. Run with extended timeout:
```bash
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --package-ids-file debug_one_pkg.txt \
  --per-package-timeout-seconds 300
```

3. Inspect events:
```bash
cat logs/a2a_phase2_*/events.jsonl | jq -r '.event, .error, .package_id'
```

4. Check full error in `results/a2a/<run_id>.json`:
```bash
jq '.packages[0].error' results/a2a/<run_id>.json
```

## Debugging with Examples

### Extract Failing Request/Response from Logs

**Step 1:** Find the run ID from the failure time:
```bash
ls -lt logs/ | head
# Look for logs/a2a_phase2_*/ matching your failure time
```

**Step 2:** Read the events:
```bash
cat logs/a2a_phase2_1767323740/events.jsonl | jq -s
```

**Step 3:** Identify the failure event:
```bash
cat logs/a2a_phase2_1767323740/events.jsonl | grep "error" | jq
```

**Example error event:**
```json
{
  "error": "max planning calls exceeded",
  "event": "plan_attempt_harness_error",
  "i": 1,
  "package_id": "0x00db9a10bb9536ab367b7d1ffa404c1d6c55f009076df1139dc108dd86608bbe",
  "t": 1767323751
}
```

### Compare Successful vs Failed Runs

**Step 1:** Run a known-good package:
```bash
cd benchmark
printf "%s\n" <SIMPLE_PACKAGE_ID> > good_pkg.txt
uv run smi-a2a-smoke --package-ids-file good_pkg.txt
```

**Step 2:** Run the failing package:
```bash
printf "%s\n" <FAILING_PACKAGE_ID> > bad_pkg.txt
uv run smi-a2a-smoke --package-ids-file bad_pkg.txt
```

**Step 3:** Compare events:
```bash
diff <(cat logs/good_run/events.jsonl) <(cat logs/bad_run/events.jsonl)
```

**Look for:**
- Different `plan_variant` values
- Presence/absence of `plan_attempt_harness_error` vs `sim_attempt_harness_error`
- Different `elapsed_seconds` (timeout vs normal completion)

### Validate Local Changes Against Known-Good Payloads

**Step 1:** Save a known-good response:
```bash
cp results/a2a_smoke_response.json results/known_good_response.json
```

**Step 2:** Make your changes to the green agent code

**Step 3:** Re-run the same request:
```bash
cd benchmark
uv run smi-a2a-smoke --corpus-root <CORPUS_ROOT> --samples 1
```

**Step 4:** Validate the new response:
```bash
uv run smi-a2a-validate-bundle results/a2a_smoke_response.json
```

**Step 5:** Compare metrics:
```bash
# Known good
jq '.result.artifacts[0].parts[0].text | fromjson | .metrics' results/known_good_response.json

# New response
jq '.result.artifacts[0].parts[0].text | fromjson | .metrics' results/a2a_smoke_response.json
```

**Metrics to watch:**
- `avg_hit_rate` (should be stable or better)
- `errors` (should not increase)
- `packages_timed_out` (should not increase)

## Related Documentation

- [GETTING_STARTED.md](../GETTING_STARTED.md) - Quick start guide
- [ARCHITECTURE.md](ARCHITECTURE.md) - A2A Layer design details and progressive exposure implementation status
- [A2A_TUNING.md](A2A_TUNING.md) - Practical tuning guidance for progressive exposure, interface modes, and cost optimization
- [evaluation_bundle.schema.json](evaluation_bundle.schema.json) - Full schema definition
