# Deep Dive: Feedback Pipeline Brittleness Analysis

Following the initial analysis, I've conducted a targeted review of the `benchmark/src/smi_bench/` execution and feedback loop components (`a2a_green_agent.py`, `inhabit_runner.py`).

I have identified **6 additional brittleness areas** specifically affecting the reliability and observability of the feedback pipeline.

## 🔴 Critical: Intermediate Feedback Loss (The "All-or-Nothing" Trap)

**Location:** `inhabit_runner.py` (`run` function) and `a2a_green_agent.py` (`execute` method)

**The Issue:**
The `smi-inhabit` runner **only writes the results JSON file (`--out`) at the very end of the execution**.
- `inhabit_runner.py`: The `_write_checkpoint` call happens only after the package loop completes.
- `a2a_green_agent.py`: Relies entirely on the existence of this file (`if out_json.exists(): ...`) to generate the evaluation bundle.

**Brittleness Impact:**
If the benchmark process crashes 99% of the way through (due to OOM, timeout, or a single fatal exception), **100% of the result data is lost**. The agent reports a generic failure with zero packages processed, hiding all the useful signal from the successful attempts.

**Fix:** Enable periodic checkpointing in `inhabit_runner.py` (e.g., every N packages or T seconds) so that `a2a_green_agent.py` can pick up partial results even after a crash.

## 🟠 Medium: Environment Variable "Bleed-Through"

**Location:** `a2a_green_agent.py` -> `subprocess.create_subprocess_exec`

**The Issue:**
The agent executor does not sanitize or restrict the environment variables passed to the subprocess. It inherits the parent's environment.
Additionally, `inhabit_runner.py` proactively loads `.env` files.

**Brittleness Impact:**
- **Non-Reproducibility:** A run might succeed only because `SMI_SENDER` or `SMI_RPC_URL` was accidentally set in the agent's deployment environment.
- **Security:** If the agent process has sensitive env vars (e.g. AWS keys), they are implicitly passed to the untrusted benchmark subprocess code.

**Fix:** Explicitly define the `env` dictionary in `create_subprocess_exec`, passing only an allow-list of variables.

## 🟠 Medium: Fragile JSON Parsing from Helper Binaries

**Location:** `inhabit_runner.py` (`_run_tx_sim_via_helper`)

**The Issue:**
The code captures `stdout` from the Rust `smi_tx_sim` binary and blindly feeds it to `json.loads`.
```python
out = subprocess.check_output(cmd, text=True, ...)
data = safe_json_loads(out, ...)
```

**Brittleness Impact:**
If the Rust binary emits *any* warning, debug log, or deprecation notice to `stdout` (e.g., from a dependency update or `println!`), the JSON parsing fails (`ValueError`). This marks the package as a "system error" rather than a valid failure, noise-polluting the benchmark results.

**Fix:** Update the Rust binary to ensure structured output goes to a specific fd or file, OR make the Python parser robust to finding the JSON blob within mixed output.

## 🟠 Medium: Unbounded Memory Buffering of Subprocess Output

**Location:** `a2a_green_agent.py` (`execute` method)

**The Issue:**
```python
proc_lines: list[str] = []
async for b in proc.stdout:
    # ...
    proc_lines.append(line)
```
The agent keeps the **entire stdout history** of the benchmark run in memory to include the "tail" in the final report.

**Brittleness Impact:**
For long-running benchmarks (e.g., 24h) or verbose simulation modes, this list can grow to gigabytes, potentially causing the agent itself to OOM and crash, killing the task it was monitoring.

**Fix:** Use a `collections.deque(maxlen=2000)` to store only the relevant tail, or stream the output to a file and read the tail on demand.

## 🟡 Low: Implicit Schema Coupling

**Location:** `a2a_green_agent.py` (`_summarize_phase2_results`)

**The Issue:**
The agent extracts metrics by hardcoded key names (`planning_only_hit_rate`, `causality_success_rate`, etc.).

**Brittleness Impact:**
If `inhabit_runner.py` renames these keys or moves them (e.g. nesting them under `intelligence_metrics`), the agent will silently report `null` or missing data for these fields without raising an error. The feedback loop degrades silently.

## 🟡 Low: Buried Crash Diagnostics

**Location:** `a2a_green_agent.py`

**The Issue:**
When `smi-inhabit` crashes (non-zero exit), the error details are often buried in the last few lines of stdout. The agent bundles this into `runner_output_tail` but does not extract the *root cause* (e.g., "ImportError", "Segfault").

**Brittleness Impact:**
Automated analysis of failure rates becomes impossible because every crash looks identical ("exit code 1"). Humans must manually read the logs for every failure.

---

## Recommended Action Plan

1.  **Prioritize:** Fix the **Intermediate Feedback Loss** immediately. This is the highest value fix for robustness.
2.  **Harden:** Implement the **Output Buffering** fix (deque) and **JSON Parsing** robustness.
3.  **Sanitize:** Lock down the subprocess environment variables.
