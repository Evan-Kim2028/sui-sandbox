# Troubleshooting A2A & Benchmarks

This guide covers common issues encountered when running the `smi-bench` A2A orchestration and local benchmarks.

## 1. Port Conflicts (9999 or 9998)

**Issue**: The Green or Purple agent fails to start because the port is already in use.
**Diagnostic**:
```bash
uv run smi-agentbeats-scenario scenario_smi --status
```
If `listening=True` but you don't have a running scenario manager, a stale process is holding the port.

**Solution**:
Run the enhanced kill command:
```bash
uv run smi-agentbeats-scenario scenario_smi --kill
```
If that fails, manually kill the process (Darwin/Linux):
```bash
lsof -ti:9999 | xargs kill -9
lsof -ti:9998 | xargs kill -9
```

## 2. Missing Credentials

**Issue**: Agent starts but fails to make LLM calls or RPC requests.
**Diagnostic**:
Check the `--status` output for credential status:
```bash
uv run smi-agentbeats-scenario scenario_smi --status
```
**Solution**:
Ensure `benchmark/.env` exists and contains:
- `OPENROUTER_API_KEY` (for most agents)
- `SMI_API_KEY` (if using `real-openai-compatible`)

## 3. Timeout Exceeded

**Issue**: Benchmark logs show `per-call timeout exceeded` or `sim_attempts: 0`.
**Explanation**:
Phase II has a hard wall-clock budget per package (`--per-package-timeout-seconds`). If the LLM "Thinking" time plus the simulation time exceeds this, the package fails.

**Solution**:
1. Increase the timeout: `--per-package-timeout-seconds 600`.
2. Disable "Thinking" models if using OpenRouter to reduce latency.
3. Check network latency to the Sui RPC URL.

## 4. JSON-RPC Parse Errors

**Issue**: The agent returns a parse error or `EvaluationBundle` is empty.
**Diagnostic**:
Check the events log for raw model output:
```bash
tail -f benchmark/logs/<run_id>/events.jsonl
```
**Solution**:
Often caused by the LLM returning malformed JSON or including markdown blocks (```json ... ```) when not expected. Ensure the agent prompt logic is stable.

## 5. Bytecode Extraction Failures

**Issue**: Errors related to `smi-extractor` not found or failing.
**Solution**:
The benchmark relies on the Rust binary. Ensure it is built in release mode:
```bash
cargo build --release --locked
```
The Python scripts expect the binary at `target/release/sui-move-interface-extractor`.
