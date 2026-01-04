# Benchmark Code Architecture (`benchmark/src/smi_bench`)

This document is a maintainers’ map of the Python benchmark harness: **what lives where**, **how data flows**, and **which invariants matter** for refactors.

It is intentionally short and “source-first.” When in doubt, trust the code.

## High-level flow

Both Phase I and Phase II consume a local `sui-packages` checkout (bytecode corpus) and invoke the Rust extractor to emit a **bytecode-derived interface JSON** for each package:

- Rust CLI: `sui_move_interface_extractor --bytecode-package-dir <pkg_dir> --emit-bytecode-json -`
- Output is parsed as JSON and used as the ground truth substrate for benchmarks.

## Module map

### Corpus / dataset utilities

- `smi_bench/dataset.py`
  - Discovers packages under `--corpus-root` (expects `bytecode_modules/` + `metadata.json`).
  - Provides deterministic sampling (`seed` + FNV-1a).

### Phase I (key-struct discovery)

- `smi_bench/runner.py`
  - Orchestrates Phase I runs.
  - Extracts **truth** key types from the bytecode interface JSON (`abilities` contains `key`).
  - Builds an LLM prompt that **omits abilities** (to avoid leakage) and may truncate struct context.
  - Scores predictions with precision/recall/F1 (`smi_bench/judge.py`).

- `smi_bench/judge.py`
  - Deterministic set-matching metrics for Phase I.

### Phase II (type inhabitation)

- `smi_bench/inhabit_runner.py`
  - Orchestrates Phase II runs.
  - Targets are key structs from the same bytecode interface JSON.
  - Produces PTB plans via:
    - `baseline-search` (deterministic heuristics),
    - `real-openai-compatible` (LLM planning), or
    - `template-search` (baseline skeleton + LLM fills args).
  - Simulates transactions via Rust helper `smi_tx_sim` (dry-run/dev-inspect/build-only).
  - Scores created object types vs targets using base-type matching (`smi_bench/inhabit/score.py`).

- `smi_bench/inhabit/executable_subset.py`
  - The core deterministic "baseline-search" logic:
    - candidate selection for entry functions,
    - supported-arg construction rules,
    - shallow recursive constructor discovery,
    - prompt-oriented interface summaries (`summarize_interface`).
    - **Interface Summary Modes**: `summarize_interface()` supports four modes:
      - `entry_then_public` (default): Entry functions first, then public functions.
      - `entry_only`: Only entry functions (used by `real-openai-compatible`).
      - `names_only`: Only module + function names (no signatures).
      - `focused`: Include only specified functions (for progressive expansion).

- `smi_bench/inhabit/normalize.py`
  - Auto-corrects common LLM formatting mistakes in PTB plans before simulation.
  - Fixes: `"object"` → `"imm_or_owned_object"`, string integers/bools, missing `0x` prefixes.
  - Returns normalized PTB + list of corrections applied.

- `smi_bench/inhabit/validator.py`
  - Validates PTB causality (result references point to earlier calls).
  - Computes `causality_score` independent of execution success.
  - Performs schema validation for PTB structure.

- `smi_bench/inhabit/metrics.py`
  - Computes aggregate metrics including `planning_only_hit_rate` (excludes pure formatting failures).
  - Tracks formatting corrections vs semantic failures.

- `smi_bench/inhabit/score.py`
  - Phase II scoring: normalize type strings, compare **base types** (type args ignored).

- `smi_bench/inhabit/dryrun.py`
  - Parses dry-run responses into `exec_ok` + best-effort failure details (abort code/location).

### Agents / I/O

- `smi_bench/agents/real_agent.py`
  - OpenAI-compatible chat-completions client with retry/backoff and strict JSON parsing.
  - Outputs either a type list (Phase I) or a PTB JSON object (Phase II).

- `smi_bench/agents/mock_agent.py`
  - Deterministic mock behaviors for Phase I infrastructure testing.

- `smi_bench/json_extract.py`
  - Best-effort JSON extraction from model output (handles code fences and surrounding prose).

- `smi_bench/logging.py`
  - JSONL logging for runs (run metadata + event stream + per-package rows).

## A2A Layer

### Overview
The A2A layer wraps the benchmark harness in a standardized AgentBeats-compatible protocol.
It provides: streaming execution, artifact encapsulation, and scenario lifecycle management.

**A2A Protocol Compliance & Testing:** This implementation is fully compliant with Google's A2A Protocol v0.3.0, including task cancellation, version headers, and streaming support. See [A2A_COMPLIANCE.md](A2A_COMPLIANCE.md) for detailed compliance and testing documentation.

### Components

- `smi_bench/a2a_green_agent.py` - Green agent (Phase II runner)
  - `SmiBenchGreenExecutor`: Implements AgentExecutor interface
  - Spawns `smi-inhabit` subprocesses with cancellation support
  - Streams live events via TaskUpdater
  - Returns `evaluation_bundle` artifact
  - Tailors `evaluation_bundle` from Phase II output JSON
  - **NEW:** Supports graceful task cancellation (SIGTERM → SIGKILL)
  - **NEW:** Injects A2A-Version headers via middleware
  - **NEW:** Explicit protocol_version in agent card

- `smi_bench/a2a_purple_agent.py` - Purple agent (stub)
  - `PurpleExecutor`: Echo/test harness
  - Validates A2A wiring without LLM costs
  - **NEW:** Protocol version support matching green agent

- `smi_bench/a2a_smoke.py` - Smoke test client
  - Starts scenario (optional)
  - Waits for green agent health
  - Sends minimal config
  - Extracts and prints summary

- `smi_bench/a2a_preflight.py` - Pre-flight validator
  - Checks corpus existence
  - Validates RPC reachability
  - Verifies Rust binary availability
  - Runs smoke test automatically

- `smi_bench/a2a_validate_bundle.py` - Schema validator
  - Validates `evaluation_bundle` against JSON Schema
  - Checks required fields and `spec_url`
  - Used in CI for A2A artifacts

- `smi_bench/agentbeats_run_scenario.py` - Scenario manager
  - Wraps AgentBeats ScenarioManager
  - Patches agent commands to launch local servers
  - Manages scenario lifecycle (--status, --kill)
  - Handles .env propagation to subprocesses

### Progressive Exposure (Design)

The `real-openai-compatible` agent supports a progressive exposure pattern to balance context window constraints with comprehensive interface information.

**Current Implementation Status:**
- ✅ `summarize_interface()` supports 4 modes (`entry_then_public`, `entry_only`, `names_only`, `focused`)
- ✅ Prompt includes `need_more` instruction format
- ✅ `need_more` response handling is **fully implemented** in `inhabit_runner.py`
- ✅ `--max-planning-calls` parameter exists (default: 50, recommended: 2-3)

**Workflow:**
1. Model receives initial interface summary (`mode="entry_then_public"`, `max_functions=60`)
2. If model needs more detail, returns: `{"need_more": ["0xADDR::module::function", ...], "reason": "..."}`
3. Runner re-invokes model with focused summary (`mode="focused"`, `requested_targets` from `need_more`)
4. Model returns final PTB plan

**Loop Detection:** The runner includes a safeguard to detect and break infinite `need_more` loops if a model requests the same targets multiple times.

**Tuning Parameters:**
- `--max-planning-calls`: Maximum LLM planning calls per package (higher = more progressive exposure rounds)
- `max_functions` in `summarize_interface()`: Controls initial interface chunk size
- Interface summary mode choice affects what model sees upfront

**See also:** [A2A_TUNING.md](A2A_TUNING.md) for practical tuning guidance

### A2A Protocol Flow

```
1. Orchestrator → GreenAgent: POST /rpc (JSON-RPC message/send)
2. GreenAgent → TaskStore: Create task, enqueue event
3. GreenAgent → SmiBenchGreenExecutor.execute(): Spawn smi-inhabit
4. SmiBenchGreenExecutor → subprocess: uv run smi-inhabit [...args...]
5. subprocess → stdout/stderr: Tail events → TaskUpdater.update_status()
6. subprocess → exit code: TaskUpdater.complete() or failed()
7. GreenAgent → response: Return with evaluation_bundle artifact
8. Orchestrator → parse: Extract metrics, errors, artifacts
```

### Event Streaming Mechanism

Events flow through: `subprocess.stdout` → `SmiBenchGreenExecutor` → `TaskUpdater` → `EventQueue` → A2A client

Event types:
- `status`: Task state transitions (working/completed/failed)
- `artifact`: Evaluation bundle, Phase II results, logs
- `error`: Execution failures

See `docs/A2A_EXAMPLES.md` for event field definitions and examples.

### Evaluation Bundle Schema

Path: `benchmark/docs/evaluation_bundle.schema.json`

Required fields (v1):
- `schema_version`: Always 1
- `spec_url`: `smi-bench:evaluation_bundle:v1`
- `benchmark`: `"phase2_inhabit"`
- `run_id`: Unique identifier (ISO timestamp-based)
- `exit_code`: Process exit code (0=success)
- `timings`: `started_at`, `finished_at`, `elapsed_seconds`
- `config`: Full Phase II configuration
- `metrics`: Aggregated results (`avg_hit_rate`, `packages_total`, etc.)
- `errors`: List of package-level errors
- `artifacts`: Paths to `results_path`, `run_metadata_path`, `events_path`

Invariants:
- `exit_code=0` ⇔ Task state = completed
- `metrics` may be empty if Phase II output is missing
- `artifacts` paths must be absolute or relative to scenario root
- `spec_url` must match schema `$id`

### Scenario Lifecycle

1. Start: `smi-agentbeats-scenario scenario_smi --launch-mode current`
   - Writes PID to `scenario_smi/.scenario_pids.json`
   - Spawns green (9999) + purple (9998) processes
   - Loads env vars from `.env` into subprocesses

2. Monitor: `smi-agentbeats-scenario scenario_smi --status`
   - Checks ports 9999/9998
   - Prints `green_9999_listening=True/False`
   - Prints `purple_9998_listening=True/False`

3. Stop: `smi-agentbeats-scenario scenario_smi --kill`
   - Reads PID from `scenario_smi/.scenario_pids.json`
   - Sends SIGTERM to scenario manager
   - Manager should terminate child processes
   - Best-effort (may leave zombie processes if manager crashed)

### Integration Points

- `scenario_smi/green_agent_card.toml`: Green agent A2A metadata
- `scenario_smi/purple_agent_card.toml`: Purple agent A2A metadata
- `scenario_smi/scenario.toml`: Scenario configuration (ports, commands)
- `.env`: API keys for agent subprocesses (SMI_API_KEY, OPENROUTER_API_KEY)

See `benchmark/GETTING_STARTED.md` for usage examples and `benchmark/docs/A2A_EXAMPLES.md` for protocol details. See `benchmark/docs/A2A_TUNING.md` for practical tuning guidance.

### Output schemas / versioning invariants

- Phase I output JSON includes `schema_version=1` (see `runner.py`).
- Phase II output JSON includes `schema_version=2` (see `inhabit_runner.py`).

## Hardening & Reliability

The benchmark harness implements several patterns to ensure reliability in long-running or distributed execution. For implementation details and usage patterns, see the [Hardening & Reliability Guide](HARDENING_GUIDE.md).

### Reliability Invariants
- **Atomic Writes**: All JSON and manifest outputs use the `atomic_write_text` / `atomic_write_json` patterns (write to `.tmp` file + rename) to prevent partial file corruption on disk full or crash.
- **Robust Reading**: `safe_read_json` and `safe_read_text` provide centralized error handling, logging, and optional retry/raise behavior.
- **JSON Recovery**: `safe_json_loads` includes heuristics to extract JSON blobs from noisy model outputs or mixed log/stdout streams.

### Subprocess Management
- **Managed Lifecycle**: The `managed_subprocess` async context manager ensures that child processes (like `smi-inhabit` or `smi_tx_sim`) are terminated (SIGTERM → SIGKILL) even if the parent task is cancelled or crashes.
- **Signal Handling**: `setup_signal_handlers` ensures that cleanup logic (like stopping Docker containers) runs on `SIGINT` and `SIGTERM`.

### Input Validation
- **Strict Parsing**: `safe_parse_int` and `safe_parse_float` clamp values to reasonable ranges and provide fallbacks with warnings instead of crashing on malformed environment variables or user inputs.
- **Pre-flight Checks**: `_run_preflight_checks` validates RPC reachability and sender funding before starting expensive LLM-based runs.

### Checkpoint Integrity
- **Checksums**: Checkpoints include an 8-character SHA-256 checksum (`_checksum` field) to detect manual edits or filesystem corruption.
- **Compatibility Checks**: `validate_checkpoint_compatibility` ensures that a resumed run matches the original configuration (agent, seed, schema version) to prevent data pollution.

## Refactor safety checklist

When refactoring:

- Keep **determinism**: sort keys / stable ordering where possible, keep sampling stable.
- Keep **scoring semantics** stable (especially Phase II base-type matching).
- Prefer `--require-dry-run` runs for comparisons/leaderboards; document any fallback logic.
- Avoid duplicating “how to call the Rust extractor” in multiple places without a clear reason.

