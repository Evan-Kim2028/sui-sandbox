# Phase 1 Action Plan: Replay DevEx and Completeness

This is the concrete follow-up to Phase 0 baseline (`2026-02-09` snapshot) for improving new-user replay flow, entry-point usability, and replay completeness validation.

## Baseline Grades (2026-02-09)

| Area | Grade | Why |
|---|---|---|
| Architecture | B- | Core VM/replay design is solid, but upgrade/linkage paths needed non-trivial fixes and stronger guardrails. |
| Usability | C+ | Powerful CLI, but first-run success path and failure interpretation still require expert context. |
| Features & Functionality | B | Walrus replay + scan are strong; remaining gaps are in consistency and edge-case handling. |
| Docs / New User Flow | C | Good content exists, but end-to-end "do this, then validate this" flow is fragmented. |

## Scope

- `replay` first-run experience (Walrus + JSON/offline).
- Replay correctness and non-regression for package upgrade/linkage behavior.
- Entry-point clarity: which commands/scripts a new user should run first.
- Validation completeness: explicit gates for "fix is done" vs "fix regressed."

## Workstreams

## WS1: New User End-to-End Flow

1. Make one canonical onboarding sequence explicit in docs:
- `examples/replay.sh` (single digest)
- `examples/scan_checkpoints.sh` (latest checkpoints)
- `analyze replay` triage loop
2. Add a single "validation command pack" for maintainers.
3. Ensure each step has expected output cues and next action.

Acceptance criteria:
- New user can get first successful replay in under 10 minutes using only README + guides.
- Docs have one canonical command sequence, not multiple competing "start here" paths.

Validation:
- `scripts/phase1_validation.sh --quick`
- Manual docs walk-through from clean env (`SUI_SANDBOX_HOME` temp dir).

## WS2: Entry Points and Tool Design

1. Keep `execution_path` JSON contract stable across replay sources.
2. Keep replay/analyze hydration flags aligned.
3. Ensure errors expose actionable direction (source, fallback, prefetch, system object settings).

Acceptance criteria:
- Replay JSON always includes `execution_path` with typed fields.
- Shared hydration flags remain in both `replay --help` and `analyze replay --help`.

Validation:
- `cargo test --test sandbox_cli_tests test_replay_json_output_execution_path_contract_from_state_json -- --nocapture`
- `cargo test --test sandbox_cli_tests test_replay_auto_system_objects_explicit_bool_true_false -- --nocapture`

## WS3: Architecture and Correctness Hardening

Implemented in current patch set:
- `execution_path` telemetry is no longer hardcoded in Walrus/JSON branches.
- Linkage debug hooks are unified across replay modes.
- Walrus dependency closure proactively fetches upgraded storage packages.
- Runtime/storage relocation now keeps runtime IDs for loader/cache consistency.
- Regression test added for self-call relocation behavior.

Remaining hardening:
1. Expand canary corpus for upgraded-package transactions.
2. Convert known failure classes into explicit issue buckets with owners.
3. Add a non-regression gate for previously fixed classes:
- `LOOKUP_FAILED`
- `FUNCTION_RESOLUTION_FAILURE`

Acceptance criteria:
- Canary transactions replay with status parity.
- Previously fixed failure classes do not reappear in latest checkpoint scan output.

Validation:
- `scripts/phase1_validation.sh` (full profile)

## WS4: Feature Gaps and Backlog

Current residual failure buckets (latest scan sample at checkpoint `239615933`):
- `ABORTED(*)` in protocol logic paths
- `FAILED_TO_DESERIALIZE_ARGUMENT`
- occasional `OTHER` (internal type mismatch)

Backlog:
1. Add per-bucket triage playbooks (root cause, reproducer command, expected mitigation).
2. Add fixture digests per bucket so failures are reproducible independent of latest tip drift.
3. Add threshold-based scan gate in CI (rate + banned regression classes).

Acceptance criteria:
- Every recurring failure bucket has a reproducible digest + documented triage command.
- Scan gate reports trend deltas (not just one-off snapshot).

Validation:
- `scripts/phase1_validation.sh` plus periodic trend capture.

## Completeness Matrix

| Problem | Fix Artifact | Validation Gate | Status |
|---|---|---|---|
| Walrus/JSON `execution_path` inconsistency | `src/bin/sandbox_cli/replay.rs` | replay JSON contract tests + phase1 script schema checks | Done |
| Missing linkage debug paths in some replay modes | `src/bin/sandbox_cli/replay.rs` | env-driven debug output across replay modes | Done |
| Upgrade dependency closure misses storage IDs | `src/bin/sandbox_cli/replay/deps.rs` | Walrus canary + latest scan non-regression | Done |
| Runtime/storage relocation breaks function cache resolution | `crates/sui-sandbox-core/src/vm.rs` | relocation unit test + canary parity replay | Done |
| First-run validation not standardized | `scripts/phase1_validation.sh` | single command pass/fail summary | Done |
| Residual abort/deserialization buckets | issue backlog + reproducible digests | bucketized scan trend checks | In progress |

## Definition of Done (Phase 1)

1. `scripts/phase1_validation.sh` full profile passes in maintainer environment.
2. Walrus canaries pass with expected status parity.
3. Scan output has no `LOOKUP_FAILED` or `FUNCTION_RESOLUTION_FAILURE`.
4. Docs point to one canonical onboarding + validation flow.
5. Residual failure buckets are documented with reproducible digests and triage steps.
