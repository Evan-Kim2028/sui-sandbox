# Issue 26 Single-PR Rollout Plan

Issue: https://github.com/Evan-Kim2028/sui-sandbox/issues/26

## Objective

Deliver the full Issue 26 architecture in one PR:

- pluggable replay-state provider interface
- file-backed local replay backend
- flexible state ingestion formats (JSON/JSONL/CSV + base64/raw BCS)
- CLI import + local replay source
- Python API parity for file-oriented workflows
- validation coverage to keep regressions low

## Single-PR Scope (Implemented)

### 1. Provider Layer

- `ReplayStateProvider` abstraction is now the dependency boundary for replay hydration.
- Added `FileStateProvider` implementing `ReplayStateProvider` for cache-backed local replay.
- Existing `HistoricalStateProvider` remains unchanged for gRPC/Walrus/hybrid workflows.

### 2. Flexible Replay-State Ingestion

- Added reusable parser module that accepts:
  - strict legacy `ReplayState` JSON schema
  - extended schema with:
    - `transaction.raw_bcs` / `raw_bcs_base64`
    - object `bcs` / `bcs_base64`
    - package `bcs` / `bcs_base64`
    - `owner_type` normalization (`Shared`/`Immutable`/`AddressOwner`)
  - single-object or array-of-states JSON files
- Added reusable BCS codec utilities for transaction/package decoding.

### 3. CLI: Import + Local Replay

- New `sui-sandbox import` command:
  - `--state` for strict/extended replay-state JSON
  - `--transactions`, `--objects`, `--packages` for JSON/JSONL/CSV row imports
  - `--output` for local cache directory
- `replay` now supports:
  - `--source local`
  - `--cache-dir`
  - multi-state selection by digest for `--state-json`

### 4. Python Parity

- Added Python APIs:
  - `import_state(...)`
  - `deserialize_transaction(raw_bcs)`
  - `deserialize_package(bcs)`
- Extended `replay(...)` for file-oriented usage:
  - `state_file=...`
  - `cache_dir=...` / `source="local"`
  - optional `digest` when `state_file` contains a single state
- Added `__version__` on module.
- Added stubs and typed marker:
  - `sui_sandbox.pyi`
  - `py.typed`

### 5. CI and Quality Hardening

- CI now includes Python smoke validation for import + local replay.
- Clippy lint job excludes `sui-python` to avoid PyO3/Python 3.14 runner incompatibility.
- Updated Python example scripts to removed API replacements:
  - `walrus_analyze_replay` -> `replay(..., analyze_only=True)`
  - `analyze_package` -> `extract_interface(...)`

## Validation Matrix

- `cargo check -p sui-state-fetcher`
- `cargo test -p sui-state-fetcher`
- `cargo check -p sui-sandbox`
- `cargo test -p sui-sandbox --test sandbox_cli_tests test_import_state_file_and_replay_from_local_cache`
- `cargo test -p sui-sandbox --test sandbox_cli_tests test_replay_state_json_multi_state_select_by_digest`
- `cargo test -p sui-sandbox --test sandbox_cli_tests test_replay_and_analyze_replay_help_share_hydration_flags`
- `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo check -p sui-python`
- `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo check --workspace`

## Supporting Backlog (Post-PR)

These remain useful follow-ups but are not blockers for Issue 26 core delivery:

- #24 Python DX polish beyond core parity (deeper docs/examples/perf)
- #10 provider compatibility tracking + parity datasets
- #27 CLI doctor diagnostics for backend/env configuration
- #2 gas benchmarking/reporting alignment
- #1 richer corpus metrics and reporting
