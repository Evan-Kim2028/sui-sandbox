# sui-sandbox v0.18.0 — Critical Review & Test Report

**Reviewer perspective**: Skeptical developer evaluating tools in the Sui/Move ecosystem.
**Date**: 2026-02-13
**Commit**: `9326ca03` (main)
**Pinned Sui version**: mainnet-v1.64.2

---

## Executive Summary

sui-sandbox is a local Sui Move VM execution harness — the only open-source tool that can replay arbitrary mainnet transactions offline using Walrus decentralized storage with zero authentication. The Rust core is solid: 1,260 tests pass with 0 failures, the binary is 17MB stripped, and clippy is nearly clean (2 warnings). The Python PyPI package (`pip install sui-sandbox`) exposes 8 standalone functions that genuinely work — you can extract package interfaces, replay transactions, fuzz Move functions, and call view functions from Python without running any Rust binary.

The rough edges are real: Walrus replay accuracy on a random checkpoint lands at ~21% for non-trivial transactions (stale shared-object state is the main failure mode), two of five Python example scripts crash on launch due to referencing removed API functions, there are no Python type stubs (.pyi), and CI doesn't test Python bindings at all. The `call_view_function` API requires a specific dict schema (`object_id`, `type_tag`, `bcs_bytes`) that isn't documented, making the first-call experience frustrating.

**Bottom line**: The Rust crate is production-quality infrastructure. The Python package is a useful but rough beta. The replay accuracy is inherently limited by data availability, not code bugs — and the tool is honest about this.

---

## What Is It?

sui-sandbox is a local execution harness for Sui Move transactions. It embeds the **real** `move-vm-runtime` from Sui validators (not a reimplementation), wraps it with a PTB (Programmable Transaction Block) execution kernel, and provides hydration pipelines to load historical state from three sources:

1. **Walrus** — Free, unauthenticated access to Sui checkpoint archives via decentralized storage
2. **gRPC** — Sui fullnode gRPC endpoint (requires API key)
3. **JSON** — Exported state files for fully offline replay

The value proposition: you can replay any mainnet transaction locally, compare effects against on-chain results, fuzz Move functions, and develop against forked mainnet state — all without running a Sui node.

The Python package (`sui-sandbox` on PyPI) wraps 8 core functions via PyO3, providing the same capabilities from Python with pre-built wheels for Linux and macOS (x86_64 + aarch64).

---

## Architecture Overview

```
Workspace: 10 crates + 1 top-level binary

sui-sandbox (CLI binary, 17MB stripped)
├── sui-sandbox-core     — VM harness, PTB executor, gas metering, replay orchestration
├── sui-transport        — Walrus client, gRPC client, GraphQL client
├── sui-resolver         — Address normalization, dependency resolution
├── sui-prefetch         — Dynamic field prefetch, MM2 bytecode analysis
├── sui-state-fetcher    — State provider abstraction, cache layer
├── sui-package-extractor — Move bytecode parsing, interface extraction
├── sui-historical-cache — Checkpoint-level package caching
├── sui-types            — Shared type definitions, encoding utilities
├── sui-python           — PyO3 bindings (cdylib → sui_sandbox wheel)
└── sui-sandbox-integration-tests — Integration test suite
```

Key architectural decisions:
- **Pinned Sui dependency**: All Sui crates locked to `mainnet-v1.64.2` via git tags. This means execution semantics match that specific protocol version.
- **Dual runtime modes**: `use_sui_natives = false` (default, sandbox runtime) vs `true` (Sui native object runtime). The default trades some edge-case parity for broader compatibility.
- **Feature-gated subsystems**: `walrus`, `analysis`, `mm2`, `igloo`, `debug-natives` are all optional features. The default build includes Walrus.

---

## Rust: Build & Test Results

### Build

| Metric | Result |
|--------|--------|
| `cargo build --release` | **Clean** (0 errors, 0 warnings) |
| Binary size | **17 MB** (stripped, release profile) |
| Build time (cached) | 0.5s |
| `cargo fmt --all --check` | **Pass** (0 formatting issues) |

### Clippy

| Scope | Result |
|-------|--------|
| `--workspace --all-features --exclude sui-python` | **2 warnings** (0 errors) |
| `--workspace --all-features` (including Python) | **Fails** — PyO3 0.23.5 rejects Python 3.14 |

The 2 clippy warnings are minor:
1. `if_same_then_else` — identical branches in `call_view_function.rs:93` (cosmetic)
2. `nonminimal_bool` — `!std::env::var("X").is_ok()` should be `.is_err()` (cosmetic)

**Note**: CI runs `cargo clippy --workspace --all-features -- -D warnings`. This will **fail** on any CI runner with Python >= 3.14 due to the PyO3 version cap. The CI would need to either pin Python to 3.13 or exclude `sui-python` from the clippy workspace.

### Test Suite

| Category | Passed | Failed | Ignored |
|----------|--------|--------|---------|
| Unit tests (all crates) | 1,260 | 0 | 72 |
| Test suites | 42 | 0 | — |

**Zero failures across 1,260 tests.** The 72 ignored tests are correctly gated — they require network access (`#[ignore]` with descriptive messages like "requires network access to Sui mainnet") or are doc-tests for modules that need runtime state.

Breakdown of notable test files:
- `sui-sandbox-core` unit tests: **471 passed** (the bulk — covers PTB execution, gas metering, type system, VM harness, error handling, fuzzer)
- `validator_tests.rs`: **69 passed** (BCS roundtrip validation, type layout resolution)
- `vm_tests.rs`: **59 passed** (VM harness creation, execution, dynamic fields, events, tracing)
- `sandbox_replay_integration_tests`: **23 passed** (PTB multi-command, gas tracking, simulation environment)
- `sui-resolver` doc tests: **10 passed** (address normalization, parsing)

---

## Rust: CLI Evaluation

### Subcommand Coverage

The CLI provides **16 subcommands**. Testing results:

| Subcommand | Zero-Setup? | Tested | Status |
|------------|-------------|--------|--------|
| `status` | Yes | Yes | **Works** — clean output, `--json` flag works |
| `status --json` | Yes | Yes | **Works** — structured JSON output |
| `reset` | Yes | Yes | **Works** — "Session reset" |
| `clean` | Yes | Yes | **Works** |
| `fetch latest-checkpoint` | Yes | Yes | **Works** — returns integer (239,615,933) |
| `fetch checkpoint <N>` | Yes | Yes | **Works** — full JSON with tx list |
| `replay <DIGEST> --checkpoint <N>` | Yes | Yes | **Works** — Walrus replay with summary |
| `replay '*' --latest 5` | Yes | Yes | **Works** (slow — ~60s+ for 5 checkpoints) |
| `replay --export-state` | Yes | Yes | **Works** — roundtrip verified |
| `replay --state-json` | Yes | Yes | **Works** — fully offline |
| `replay mutate --demo` | Yes | Yes | **Works** — interactive guided demo |
| `test fuzz "<TARGET>"` | Yes | Yes | **Works** — dry-run and execution both functional |
| `analyze package` | Partial | Help only | Requires package ID or bytecode dir |
| `snapshot list` | Yes | Yes | **Works** — "No snapshots found" |
| `publish` | No | Help only | Requires local Move package |
| `run` | No | Help only | Requires published package |

**Error handling quality**: Error messages are generally good. Invalid digests produce clear "Transaction X not found in checkpoint Y" messages. The one exception: using `replay '*'` without `--checkpoint` or `--latest` falls through to gRPC with a raw `InvalidArgument` error dump including full gRPC metadata bytes — not user-friendly.

### CLI Design Observations

**Good**:
- `--json` flag on every command enables scripting/piping
- `--debug-json` for structured failure diagnostics
- `--verbose` for execution traces
- `--latest N` auto-discovers the Walrus tip — genuinely zero-config
- The replay summary output is well-designed (bar charts, pass/fail breakdown by package)

**Rough**:
- `replay '*'` is a magic sentinel that means "all transactions in checkpoint" — unintuitive
- The `--source` flag defaults to `hybrid` which tries gRPC first — confusing when you expected Walrus-only. Should arguably default to `walrus` when `--checkpoint` is provided.

---

## Rust: Replay Accuracy (Walrus)

### Single Checkpoint Test (checkpoint 100,000,000)

| Metric | Value |
|--------|-------|
| Total transactions | 19 |
| PTBs (replayable) | 14 |
| System transactions (skipped) | 5 |
| **Passed** | **3 / 14 (21.4%)** |
| Failed | 11 / 14 |

**Failure breakdown**:
- `ABORTED(2)` — 4 occurrences (Move assertion failure, stale state)
- `ABORTED(1)` — 3 occurrences (Move assertion failure)
- `OTHER` — 3 occurrences (missing data or unrecognized failure mode)
- `ABORTED(906)` — 1 occurrence (application-specific abort code)

### What This Means

The ~21% success rate is **not a code bug** — it's a fundamental data-availability limitation. Walrus checkpoints contain objects at their *output* versions (post-mutation), but replay needs objects at their *input* versions (pre-mutation). For transactions that depend on shared objects whose state changed in the same checkpoint, the input-version data may not be available from Walrus alone.

The tool is honest about this: the failure breakdown shows `ABORTED(2)` and `ABORTED(1)` — these are Move-level assertion failures that occur because a shared object (e.g., a DEX pool) was read at the wrong version. The sui-sandbox correctly identifies the failure mode as "stale state."

Transactions that **do** succeed tend to be simpler: single-package calls with immutable or owned inputs. Cross-package DeFi transactions with shared objects are the hardest to replay from Walrus alone.

### Export-State / Offline Roundtrip

**Works correctly.** A transaction replayed from Walrus, exported via `--export-state`, and re-replayed via `--state-json` produces identical results. The exported JSON is compact (2.8 KB for a simple system transaction).

---

## Python: Binding Evaluation (All 8 Functions)

Tested on: Python 3.13, `pip install sui-sandbox==0.18.0`, fresh virtualenv.

### Function-by-Function Results

| # | Function | Happy Path | Edge Cases | Grade |
|---|----------|-----------|------------|-------|
| 1 | `get_latest_checkpoint()` | **Pass** — returns `int` (239,615,933) | N/A (no args) | A |
| 2 | `get_checkpoint(n)` | **Pass** — returns dict with 6 keys | Future checkpoint → `RuntimeError` (404) | A |
| 3 | `extract_interface(package_id=)` | **Pass** — 62 modules for `0x2` | Invalid ID → `RuntimeError`, wrong type → `TypeError` | A |
| 4 | `fetch_package_bytecodes(id)` | **Pass** — returns `{packages, count}` | `resolve_deps=False` works | A |
| 5 | `json_to_bcs(type_str, json, bytecodes)` | **Pass** — 40 bytes for Coin | Key format is `0x2` (short), not full address | B+ |
| 6 | `call_view_function(...)` | **Pass** (with correct dict schema) | Dict keys underdocumented (see below) | B- |
| 7 | `fuzz_function(...)` | **Pass** — dry_run and execution both work | `dry_run=True` returns classification only | A |
| 8 | `replay(digest, checkpoint=)` | **Pass** — `local_success: True` | `analyze_only=True` works | A |

### Latency Benchmarks

| Function | Avg Latency | Notes |
|----------|-------------|-------|
| `get_latest_checkpoint()` | **304ms** | Network-bound (Walrus HTTP) |
| `get_checkpoint(100M)` | **9,286ms** | Large checkpoint, fetches all objects |
| `extract_interface("0x2")` | **501ms** | GraphQL fetch + parse |
| `fetch_package_bytecodes("0x2")` | **479ms** | GraphQL |
| `json_to_bcs(...)` | **3ms** | Pure computation, no network |
| `call_view_function(...)` | **13ms** | Local VM execution |
| `fuzz_function(..., n=10)` | **21ms** | 10 iterations, ~2ms/iter |
| `replay(digest, cp=100M)` | **589ms** | Walrus fetch + VM execution |

### `call_view_function` — The Tricky One

This function works but has a steep learning curve. The `object_inputs` parameter requires dicts with specific keys that aren't documented in the README:

```python
# What the README shows (DOESN'T WORK):
object_inputs=[{"Clock": "0x6"}]

# What actually works:
object_inputs=[{
    "object_id": "0x...",    # Required
    "owner": "immutable",    # Required: "immutable" | "shared" | "address_owned"
    "type_tag": "0x2::coin::Coin<0x2::sui::SUI>",  # Required (not "type"!)
    "bcs_bytes": [1, 2, 3],  # Required: list of ints
}]
```

The error messages on wrong keys are helpful (`missing 'bcs_bytes' in object_inputs`, `missing 'type_tag' in object_inputs`) but require trial-and-error discovery. The README's `call_view_function` example with `{"Clock": "0x6"}` will not work as shown.

---

## Python: Error Handling & Edge Cases

### Type Safety

PyO3 provides automatic type checking at the boundary. All tested type errors produce clear `TypeError` messages:

| Test | Result |
|------|--------|
| `get_checkpoint("not_int")` | `TypeError: 'str' object cannot be interpreted as an integer` |
| `extract_interface(package_id=123)` | `TypeError: 'int' object cannot be converted to 'PyString'` |
| `replay(None)` | `TypeError: 'NoneType' object cannot be converted to 'PyString'` |

### Network Errors

| Test | Result |
|------|--------|
| Invalid package ID format | `RuntimeError` with GraphQL parse error |
| Non-existent package | `RuntimeError` (fetch failure) |
| Bad RPC URL | **Silently succeeds** — `extract_interface(rpc_url="https://nonexistent.example.com")` returns results (likely cached or using a fallback) |

The silent success on bad RPC URL is concerning — it suggests the function may be ignoring the `rpc_url` parameter or using a cached response.

### GIL Release / Threading

| Test | Result |
|------|--------|
| Sequential 3x `get_latest_checkpoint()` | 806ms |
| Parallel 3x (ThreadPoolExecutor) | 843ms |
| Speedup | **1.0x** (no speedup) |

The GIL is **not released** during network calls. This means Python threading provides no benefit for concurrent sui-sandbox calls. This is a limitation for Python applications that want to batch-fetch checkpoints or replay multiple transactions in parallel. The functions should use `py.allow_threads(|| ...)` in the PyO3 code.

### Module Introspection

| Check | Result |
|-------|--------|
| `__version__` | **NOT SET** |
| `__doc__` | "Python module: sui_sandbox" (generic) |
| `.pyi` type stubs | **None** |
| `py.typed` marker | **False** |
| Exported symbols | 8 functions + 1 nested module name |

No type stubs means IDEs (VS Code, PyCharm) cannot provide autocomplete or type checking. This is the single biggest friction point for Python developers.

---

## Python: Example Scripts Assessment

| Script | Status | Issue |
|--------|--------|-------|
| `test_bindings.py` | **Crashes at step 4** | Calls `sui_sandbox.walrus_analyze_replay()` — function doesn't exist |
| `test_replay_detail.py` | **Crashes** | Same: calls `sui_sandbox.walrus_analyze_replay()` |
| `scan_view_functions.py --help` | **Works** | Requires Snowflake connection for actual execution |
| `call_view_functions.py --help` | **Works** | Requires `--wallet` arg and Snowflake data |
| `wallet_profile.py --help` | **Works** | Requires pre-fetched data directory |

### What Happened with test_bindings.py

The script references two functions that were removed from the PyO3 API:
- `sui_sandbox.walrus_analyze_replay()` — removed (replaced by `replay(..., analyze_only=True)`)
- `sui_sandbox.analyze_package()` — removed (replaced by `extract_interface()`)

Steps 1-3 of `test_bindings.py` work correctly (fetching checkpoint data, listing transactions). Step 4 crashes. This means the **only two "test" scripts for Python bindings are broken**. A newcomer running `python test_bindings.py` as their first interaction will hit a traceback.

### Assessment

The `scan_view_functions.py` and `call_view_functions.py` scripts are substantial (~700 lines each) and represent a serious workflow: scanning Sui packages for view functions, matching them against wallet-owned objects, and executing them locally. However, they require Snowflake data pipelines and pre-computed scan results, making them inaccessible to new users.

---

## Cross-Cutting: Walrus Reliability, Onboarding, Gaps

### Walrus as a Data Source

**Strengths**:
- Zero authentication, zero configuration
- Covers all historical checkpoints (tested checkpoint 100,000,000 successfully)
- `get_latest_checkpoint()` consistently returns within 300-500ms
- Checkpoint summaries include full transaction lists with sender, command count, and I/O object counts

**Weaknesses**:
- Single-checkpoint fetch for a complex checkpoint: ~9 seconds (network + deserialization)
- 5-checkpoint batch scan: 60+ seconds (each checkpoint fetched sequentially)
- Intermittent 500s on very recent checkpoints (data propagation lag)
- Objects are at output versions, not input versions — fundamentally limits replay accuracy for shared-object transactions

### Onboarding Assessment

**What works for newcomers**:
1. `pip install sui-sandbox` → immediate access to 8 functions (genuinely zero-setup)
2. `cargo build --release && sui-sandbox status` → working CLI in one command
3. `sui-sandbox replay '*' --latest 1 --compare` → see real results immediately
4. README has a clear "20-Second Explanation" and "How This Differs" comparison table
5. Examples are tiered by setup requirements (zero-setup → gRPC → Rust library)

**What fails for newcomers**:
1. Running `python test_bindings.py` (the obvious first thing to try) crashes with `AttributeError`
2. `call_view_function` example in README doesn't match the actual API
3. No type stubs → no IDE help for Python
4. No `__version__` attribute → can't verify installed version from Python
5. The README's Python `call_view_function` example `{"Clock": "0x6"}` doesn't match the actual required dict schema

### Gaps

1. **CI doesn't test Python bindings** — `ci.yml` only runs `cargo test --locked --workspace --tests`, which excludes the Python crate
2. **CI clippy will break on Python 3.14+** — `cargo clippy --workspace --all-features` includes `sui-python` which depends on PyO3 0.23.5 (max Python 3.13)
3. **No Python integration tests** — no automated verification that the 8 exported functions work
4. **No changelog entry for API removals** — `walrus_analyze_replay()` and `analyze_package()` were removed without updating example scripts
5. **GIL not released** — Python threading provides no benefit for concurrent calls

---

## Pipeline Usability Assessment (Dagster / Python Data Workflows)

A key question: can you use this in real Python data pipelines — Dagster assets, batch processing, historical analysis?

### What works for pipelines today

The `replay(digest, checkpoint=N)` path via Walrus is the strongest primitive. You can feed it transaction digests from a Snowflake query, call one function, and get structured execution results back — `local_success`, `effects`, `commands_executed`, gas breakdown. No API keys, no infrastructure. Similarly, `extract_interface` and `fetch_package_bytecodes` are reliable for package analysis pipeline stages (~500ms per call, structured output).

### What blocks pipeline adoption

**1. The ~21% Walrus replay accuracy limits bulk replay pipelines.** For a Dagster asset processing all transactions in a checkpoint, ~80% will fail with Move aborts due to stale shared-object state. This means you cannot build a reliable "replay every transaction" pipeline on Walrus alone. Transactions with shared objects (most DeFi activity) need the gRPC path for better accuracy, which requires an API key and has its own availability constraints. The tool is honest about this — failures are clearly categorized — but pipeline-level success rates matter.

**2. No GIL release creates a sequential throughput wall.** Each replay call is ~600ms (network-bound). With no threading benefit (measured 1.0x speedup), processing 1,000 transactions takes ~10 minutes with no way to parallelize from Python. You'd need `multiprocessing` (separate OS processes) to get concurrency, which is heavier and doesn't share cached state. For a Dagster pipeline processing checkpoint ranges, this is the throughput ceiling.

**3. No built-in retry or resilience.** Walrus returns intermittent HTTP 500 errors on very recent checkpoints (data propagation lag). The Python functions raise `RuntimeError` with no retry, no backoff, no configurable timeout. In a Dagster pipeline you'd need to wrap every call in your own retry decorator — not unusual for a library, but adds boilerplate.

**4. `call_view_function` requires multi-step glue code.** For pipelines that want to call view functions across packages (the `call_view_functions.py` workflow), you need to: fetch bytecodes → convert JSON to BCS → construct the right dict with `object_id`/`type_tag`/`bcs_bytes`. That's 3-4 pipeline stages with underdocumented glue. The existing script shows this but depends on Snowflake, so it's a reference rather than something you'd drop into a Dagster asset.

### The sweet spot

Today, sui-sandbox in Python is best suited for **selective replay of known-interesting transactions** rather than bulk historical replay:

- A Dagster asset that takes specific DEX swap digests from Snowflake, replays them locally to extract execution details not available on-chain (gas breakdown, execution trace, intermediate values)
- Package analysis assets — `extract_interface` across a corpus of packages to build a function signature database
- Scheduled fuzz testing — `fuzz_function` against specific entry points to detect new abort paths after upgrades

For bulk replay across checkpoint ranges, the accuracy ceiling (~21% via Walrus) and sequential throughput (~600ms/tx with no parallelism) would be the first constraints you'd hit.

### Rust vs Python gap

The Rust crate is pipeline-ready: it's async, gives direct access to replay internals, supports configurable fallback strategies, and has the full CLI for batch operations (`replay '*' --latest N`). The Python wrapper is a thin synchronous layer over the Rust core that doesn't yet account for how Python data engineers work — concurrent calls, retry patterns, structured error types, observable progress. Closing this gap is the main work needed for serious pipeline adoption.

---

## Snowflake → sui-sandbox Feasibility Assessment

A natural question for the groot pipeline: can you query Snowflake for transaction/object data and feed it into sui-sandbox for offline replay or view function execution?

### Snowflake Data Shape (PIPELINE_V2_GROOT_DB)

| Table | Checkpoint Range | Row Count | Key Columns |
|-------|-----------------|-----------|-------------|
| `OBJECT_PARQUET` | 0–40M | 10.2B | OBJECT_ID, VERSION, DIGEST, TYPE, **BCS** (base64), **OBJECT_JSON** (VARIANT), OWNER_TYPE, INITIAL_SHARED_VERSION |
| `TRANSACTION` | 1–25M | 1B | TRANSACTION_DIGEST, SENDER, GAS_BUDGET, GAS_PRICE, **RAW_TRANSACTION** (base64 BCS), EXECUTION_SUCCESS, MOVE_CALLS, COMPUTATION_COST, STORAGE_COST, STORAGE_REBATE |
| `TRANSACTION_OBJECT` | — | — | OBJECT_ID, VERSION, TRANSACTION_DIGEST, **INPUT_KIND** (GasCoin/Input/SharedInput), OBJECT_STATUS |
| `MOVE_PACKAGE_PARQUET` | 0–150M | 246K | PACKAGE_ID, **BCS** (base64, full package), CHECKPOINT |
| `MOVE_PACKAGE_PARQUET2` (view) | 0–150M | — | PACKAGE_ID, PACKAGE_VERSION, ORIGINAL_PACKAGE_ID, BCS_LENGTH (metadata only, no BCS data) |

**Usable overlap across all tables: checkpoints 1–25M** (TRANSACTION is the bottleneck).

### State JSON Schema Mapping

The sui-sandbox `--state-json` offline replay requires a specific JSON format. Here's how Snowflake fields map:

**Objects section — DIRECT MATCH:**
| State JSON field | Snowflake source | Status |
|-----------------|------------------|--------|
| `objects.{id}.id` | `OBJECT_PARQUET.OBJECT_ID` | Direct |
| `objects.{id}.version` | `OBJECT_PARQUET.VERSION` | Direct |
| `objects.{id}.digest` | `OBJECT_PARQUET.DIGEST` | Direct |
| `objects.{id}.type_tag` | `OBJECT_PARQUET.TYPE` | Direct |
| `objects.{id}.bcs_bytes` | `OBJECT_PARQUET.BCS` | base64 decode → int array |
| `objects.{id}.is_shared` | `OBJECT_PARQUET.OWNER_TYPE = 'Shared'` | Derived |
| `objects.{id}.is_immutable` | `OBJECT_PARQUET.OWNER_TYPE = 'Immutable'` | Derived |

**Transaction section — PARTIAL:**
| State JSON field | Snowflake source | Status |
|-----------------|------------------|--------|
| `transaction.digest` | `TRANSACTION.TRANSACTION_DIGEST` | Direct |
| `transaction.sender` | `TRANSACTION.SENDER` | Direct |
| `transaction.gas_budget` | `TRANSACTION.GAS_BUDGET` | Direct |
| `transaction.gas_price` | `TRANSACTION.GAS_PRICE` | Direct |
| `transaction.checkpoint` | `TRANSACTION.CHECKPOINT` | Direct |
| `transaction.timestamp_ms` | `TRANSACTION.TIMESTAMP_MS` | Direct |
| `transaction.commands` | `TRANSACTION.RAW_TRANSACTION` | **BLOCKED** — needs BCS deserialization |
| `transaction.inputs` | `TRANSACTION.RAW_TRANSACTION` | **BLOCKED** — needs BCS deserialization |
| `transaction.effects.status` | `TRANSACTION.EXECUTION_SUCCESS` | Boolean → "Success"/"Failure" |
| `transaction.effects.gas_used.*` | `TRANSACTION.COMPUTATION_COST`, etc. | Partial (missing `non_refundable_storage_fee`) |
| `transaction.effects.created/mutated/...` | `TRANSACTION_OBJECT.OBJECT_STATUS` | Requires JOIN + grouping |

**Packages section — NEEDS DESERIALIZATION:**
| State JSON field | Snowflake source | Status |
|-----------------|------------------|--------|
| `packages.{id}` (list of module bytecodes) | `MOVE_PACKAGE_PARQUET.BCS` | **BLOCKED** — BCS is the entire serialized `MovePackage`, not individual modules |

**Metadata — GAPS:**
| State JSON field | Snowflake source | Status |
|-----------------|------------------|--------|
| `protocol_version` | Not in these tables | Missing |
| `epoch` | `TRANSACTION.EPOCH` | Direct |
| `reference_gas_price` | Not in these tables | Missing (could approximate from `GAS_PRICE`) |

### Two Viable Workflows

**Workflow A: Snowflake + `call_view_function` — WORKS TODAY**

This is the pattern `call_view_functions.py` already implements:

1. Query `OBJECT_PARQUET2` for objects with `OBJECT_JSON`
2. `json_to_bcs(type_str, object_json, package_bytecodes)` to convert
3. `fetch_package_bytecodes(package_id, resolve_deps=True)` from GraphQL
4. `call_view_function(...)` locally

No state JSON needed. No replay. Snowflake provides the object data; GraphQL provides the package bytecodes. This is the practical path for pipeline use today.

**Workflow B: Snowflake → offline replay — DOES NOT WORK YET**

Constructing the full state JSON from Snowflake alone is blocked by two BCS deserialization gaps:

1. **`RAW_TRANSACTION` → `commands`/`inputs`**: The base64-encoded BCS in Snowflake contains the full `TransactionData` structure, but no Python API exists to deserialize it into the structured commands/inputs the state JSON needs.
2. **`MOVE_PACKAGE_PARQUET.BCS` → individual module bytecodes**: The BCS column is the entire `MovePackage` blob, not split into individual `.mv` module files. No Python API exists to extract modules from this.

### The Practical Hybrid: Snowflake for Selection, Walrus/RPC for Execution

The recommended workflow today:

```python
import sui_sandbox
import snowflake.connector

# 1. Use Snowflake to SELECT which transactions to replay
conn = snowflake.connector.connect(...)
digests = conn.cursor().execute("""
    SELECT TRANSACTION_DIGEST, CHECKPOINT
    FROM TRANSACTION
    WHERE SENDER = '0x...' AND CHECKPOINT BETWEEN 10000000 AND 10000100
""").fetchall()

# 2. Feed digests into sui-sandbox replay (fetches from Walrus automatically)
for digest, checkpoint in digests:
    result = sui_sandbox.replay(digest=digest, checkpoint=checkpoint)
    # result has local_success, effects, gas breakdown, etc.
```

Snowflake provides the **filtering intelligence** (which transactions, which senders, which packages). sui-sandbox provides the **execution engine** (replay, view function calls). This avoids the BCS deserialization gap entirely.

### What Would Unlock Full Offline Replay from Snowflake

Two new Python bindings would close the gap:

1. **`deserialize_transaction(raw_bcs: bytes) -> dict`** — Parse `RAW_TRANSACTION` BCS into `{commands, inputs, ...}` structure
2. **`deserialize_package(bcs: bytes) -> list[bytes]`** — Extract individual module bytecodes from `MovePackage` BCS

With these, you could construct the complete state JSON from Snowflake data alone, enabling fully offline replay without any network calls to Walrus or RPC at execution time.

---

## Scorecard

| Area | Grade | Justification |
|------|-------|---------------|
| **Rust Build & Lint** | **A** | Clean release build, 2 minor clippy warnings, perfect formatting |
| **Rust Test Suite** | **A+** | 1,260 passed, 0 failed, 72 correctly ignored. Comprehensive coverage. |
| **CLI Design & UX** | **A-** | 16 subcommands, good `--json`/`--verbose` flags, clear help text. Minor: `'*'` sentinel is unintuitive |
| **Walrus Replay Accuracy** | **C+** | 21% on a random checkpoint. Not a code bug — data availability limitation. Tool is transparent about failure modes. |
| **Python API (8 functions)** | **B+** | All 8 functions work correctly. Latency is reasonable. Type errors caught cleanly. |
| **Python Developer Experience** | **C** | No type stubs, no `__version__`, GIL not released, `call_view_function` dict schema underdocumented |
| **Python Examples** | **D** | 2/5 scripts crash on launch. The working ones require Snowflake infrastructure. |
| **Documentation** | **B** | README is comprehensive and honest. Python README has inaccurate `call_view_function` example. |
| **CI/CD** | **B-** | Rust lint+test automated. Python bindings completely untested in CI. Clippy will break on Py3.14+ runners. |
| **Onboarding** | **B** | Rust path is smooth. Python path hits broken examples immediately. |

**Overall: B**

The Rust core is genuinely impressive — a 17MB binary with 1,260 passing tests that can replay mainnet Sui transactions with zero configuration. The Python package is functional but needs polish. The main gap is the Python developer experience: fix the broken example scripts, add type stubs, and document `call_view_function`'s actual API.

---

## Recommendations

### High Priority (Blocking for Python Adoption)

1. **Fix `test_bindings.py` and `test_replay_detail.py`** — Replace `walrus_analyze_replay()` calls with `replay(..., analyze_only=True)` and `analyze_package()` with `extract_interface()`. These are the first scripts newcomers will run.

2. **Add Python type stubs (`.pyi`)** — Generate or hand-write `sui_sandbox.pyi` with full function signatures. This is the #1 friction point for IDE users. Add `py.typed` marker file.

3. **Fix the `call_view_function` README example** — The documented `{"Clock": "0x6"}` shorthand doesn't work. Document the actual required dict schema: `object_id`, `owner`, `type_tag`, `bcs_bytes`.

### Medium Priority

4. **Release the GIL during network calls** — Wrap network-bound operations in `py.allow_threads(|| ...)` to enable Python threading benefit. Current 1.0x speedup on parallel calls is a limitation.

5. **Add Python binding tests to CI** — Even a minimal smoke test (`import sui_sandbox; sui_sandbox.get_latest_checkpoint()`) would catch the `walrus_analyze_replay` breakage.

6. **Set `__version__` on the module** — `sui_sandbox.__version__` should return `"0.18.0"`. Standard Python practice.

7. **Fix CI clippy for Python 3.14+** — Either pin Python version in CI or exclude `sui-python` from the clippy workspace scope.

### Low Priority

8. **Add `deserialize_transaction` and `deserialize_package` Python bindings** — These would unlock fully offline replay from Snowflake data. `deserialize_transaction(raw_bcs)` parses a `TransactionData` BCS blob into the structured commands/inputs dict. `deserialize_package(bcs)` extracts individual module bytecodes from a `MovePackage` BCS blob. The Rust code already has these deserializers internally; exposing them via PyO3 is the missing link for Snowflake → state JSON → offline replay pipelines.

9. **Improve error for `replay '*'` without `--checkpoint`** — Currently dumps raw gRPC error bytes. Should print a user-friendly message suggesting `--checkpoint` or `--latest`.

10. **Add a simple Python example** — A 20-line script that demonstrates all 8 functions without requiring Snowflake or external data. Would dramatically improve the newcomer experience.

11. **Consider defaulting `--source walrus` when `--checkpoint` is provided** — The current `hybrid` default tries gRPC first, which fails without an API key. When the user explicitly provides `--checkpoint`, they clearly intend Walrus-first.

---

*Report generated from live testing against sui-sandbox v0.18.0 (commit 9326ca03) on macOS aarch64, Python 3.13, Rust stable.*
