# Phase 0 UX Baseline

This document defines baseline user journeys and metrics for the Issue #20 UX program.

## Scope

Target command surface:

- `replay`
- `publish` + `run`
- `status`/`view` state inspection

## Canonical Journeys

1. First replay attempt (new user)
- Goal: run a single replay command and understand path/result.
- Command: `sui-sandbox replay <DIGEST>`
- Success signal: user can identify execution path and next action from output.

2. Local publish-and-run loop
- Goal: publish local package and run one function.
- Commands:
  - `sui-sandbox publish <PATH> --bytecode-only --address fixture=0x100`
  - `sui-sandbox run <TARGET> ...`
- Success signal: package visible in `status` and `view packages`; run output is actionable.

3. Reproducible command flow
- Goal: execute a deterministic sequence from a file.
- Command: `sui-sandbox run-flow flow.quickstart.yaml`
- Success signal: pass/fail summary with per-step timing and failure index.

## Baseline Metrics

1. Time-to-first-success (TFS)
- Definition: elapsed time from first command invocation to first successful end-to-end flow.
- Collection: `scripts/phase0_baseline.sh`.

2. Actionable failure rate (AFR)
- Definition: fraction of failed commands that include a concrete next-step hint.
- Collection: parse stderr/stdout for hints and classify manually during Phase 0.

3. Fallback usage rate (FUR)
- Definition: fraction of replay runs that used fallback paths.
- Collection: replay `execution_path.fallback_used` JSON field.

4. Flow determinism pass rate (FDP)
- Definition: successful steps / total steps for `run-flow` in CI and local runs.
- Collection: `run-flow --json` summary.

## Phase 0 Exit Criteria

- Baseline journeys documented.
- Baseline script exists and runs locally.
- CLI tests include coverage for run-flow summary and enhanced status JSON fields.

