# Issue 26 Implementation Plan

This document defines the execution plan for issue `#26`:

- https://github.com/Evan-Kim2028/sui-sandbox/issues/26

## Goal

Make replay-state ingestion pluggable and file-oriented without regressing existing replay paths.

Core outcome:

- callers depend on an abstract replay-state provider interface
- network-backed hydration remains the default
- file-backed hydration can be introduced as a first-class backend
- Python API layers mirror the same backend model

## PR Strategy

Use small, reviewable PR slices instead of one monolithic rewrite.

### PR 26A (this branch)

Scope:

- Introduce `ReplayStateProvider` trait in `sui-state-fetcher`
- Wire `ReplayStateBuilder` to depend on the trait instead of concrete `HistoricalStateProvider`
- Provide trait implementation for `HistoricalStateProvider`

Non-goals:

- No behavior change to replay hydration
- No file import path yet
- No CLI surface changes yet

Success criteria:

- `cargo check -p sui-state-fetcher` passes
- Existing call sites continue to compile with no behavior drift

### PR 26B

Scope:

- Extend state JSON ingestion to accept practical external formats:
  - base64 BCS object bytes
  - raw transaction BCS blob
  - full package BCS blob
- Preserve compatibility with current strict format

Success criteria:

- old and new state JSON forms are both accepted
- explicit tests for base64/object/package/transaction decoding

### PR 26C

Scope:

- Add `FileStateProvider` and cache-backed local replay source
- Add import pipeline (`json`, `jsonl`, `csv`) into local replay cache
- Add replay source selector for local file/cache-backed execution

Success criteria:

- deterministic replay from imported files without network dependency
- clear error reporting for malformed input rows

### PR 26D

Scope:

- Layered Python API on top of shared Rust backend surface
- Add typed stubs, docs, and CI coverage for new Python paths

Success criteria:

- Python can run import + replay against file-backed cache
- Python quality gates from `#24` are integrated for new API paths

## Supporting Backlog Map

These open issues directly support `#26` rollout quality:

- `#24` Python bindings quality and CI:
  https://github.com/Evan-Kim2028/sui-sandbox/issues/24
- `#10` provider compatibility tracking for replay data fidelity:
  https://github.com/Evan-Kim2028/sui-sandbox/issues/10
- `#27` CLI doctor command for environment/backend diagnostics:
  https://github.com/Evan-Kim2028/sui-sandbox/issues/27

Related future enhancements (not blockers for `#26` core):

- `#2` gas benchmarking/reporting alignment
- `#1` richer corpus metrics

## Risks and Controls

Risk:

- backend abstraction adds indirection and could hide source-specific failures

Control:

- keep source-specific diagnostics attached to provider errors
- validate parity with current historical provider before enabling new defaults

Risk:

- schema-flexible import paths can introduce silent coercion bugs

Control:

- strict validation and explicit normalization reports for each imported record
- fixture-based tests for each accepted external format

## Execution Order

1. Merge PR 26A foundation abstraction.
2. Implement PR 26B extended state JSON compatibility.
3. Implement PR 26C file provider + import CLI.
4. Implement PR 26D Python parity and quality hardening.
