# Consolidation Candidates (2026-02-10)

This review focuses on reducing command/features that add maintenance cost but are not central to the replay-first architecture.

Status update (2026-02-10): `tools ptb-replay-harness` and `tools walrus-warmup` were removed from the main CLI surface, and the igloo loader path was removed from core `replay` command wiring while retaining source for future extraction.

## Scope and Method

Signals used:
- Code footprint (approximate LOC by command area)
- Test footprint (`tests/sandbox_cli_tests.rs` command usage)
- Docs/examples prominence (`README`, CLI reference, guides)
- Overlap with other commands

Primary data points:
- `src/bin/sandbox_cli/replay.rs` is the dominant command surface (3759 LOC).
- Non-core command areas are large:
  - `bridge.rs` (864 LOC)
  - `analyze.rs` + submodules (727 LOC + submodule set)
  - `tools/*` (2355 LOC combined)
  - `flow.rs` + `snapshot.rs` (594 LOC combined)
  - `replay/igloo.rs` (1286 LOC, feature-gated)
- CLI test command count in `tests/sandbox_cli_tests.rs`:
  - `bridge`: 29
  - `publish`: 15
  - `view`: 12
  - `ptb`: 9
  - `replay`: 8
  - `snapshot`: 4, `run-flow`: 2, `init`: 1
  - `tools`: no direct CLI integration tests

## Keep vs Consolidate vs Remove

## Keep (Core Product Surface)

- `replay`, `publish`, `run`, `ptb`, `fetch (package/object)`, `view`, `status/reset/clean`

Reason:
- These directly support the core "local VM + replay" value proposition and have strong test/docs presence.

## Consolidate (High Confidence)

### 1) Checkpoint ingestion commands

Current overlap:
- `fetch checkpoints <start> <end>` in `src/bin/sandbox_cli/fetch.rs`
- legacy warmup implementation in `src/bin/sandbox_cli/tools/walrus_warmup.rs` (no longer exposed in main CLI)

Recommendation:
- Keep one ingestion entry point only.
- Implemented: `fetch checkpoints` is now the user-facing ingestion path.

Why:
- Two commands for near-identical ingestion semantics increase docs and support burden.

### 2) PTB robustness scan utilities

Current overlap:
- legacy harness implementation in `src/bin/sandbox_cli/tools/ptb_replay_harness.rs` (no longer exposed in main CLI) vs `replay "*" --latest N --compare`

Recommendation:
- Implemented for public CLI: remove `ptb-replay-harness` from command surface.

Why:
- Harness is explicitly internal and overlaps with main replay scan workflow.

## Move Out of Core Binary (Medium Confidence)

### 3) `tools` namespace (poll/stream/tx-sim)

Current state:
- `poll-transactions`, `stream-transactions`, `tx-sim` are useful but not replay-core and currently weakly tested as CLI surface.

Recommendation:
- Split into separate binary (e.g. `sui-sandbox-tools`) or keep feature-gated and hidden from default docs.

Why:
- Keeps core CLI focused and lowers cognitive load for new users.
- Lowers risk of peripheral regressions affecting core command UX.

### 4) Flow orchestration commands (`init`, `run-flow`, `snapshot`)

Current state:
- Helpful convenience layer, but mostly wraps functionality that can be done with shell scripts + `--state-file`.

Recommendation:
- Either:
  1. move to examples/scripts and deprecate these commands, or
  2. keep only `run-flow`, deprecate `init` and `snapshot`.

Why:
- Small but non-trivial maintenance footprint; not core replay architecture.

## Feature-Flag Pruning (High Leverage)

### 5) `igloo` replay path

Current state:
- Large feature-specific code path (`replay/igloo.rs`: 1286 LOC), no longer wired into core `replay` CLI flow.

Recommendation:
- Keep source but keep it out of core replay wiring; isolate into dedicated extension/internal tooling if revived.

Why:
- Adds substantial branching and replay complexity for a specialized pipeline.

### 6) `analysis` + `mm2` defaults

Current state:
- Default features include `analysis` and `mm2`.
- `analyze objects`/MM2 workflows are valuable for advanced corpus work but not required for core replay.

Recommendation:
- Keep `walrus`, `analysis`, and `mm2` in defaults.
- Encode `analysis -> mm2` dependency so analysis builds always include MM2 capabilities.

Why:
- Preserves out-of-the-box replay + analysis capability while avoiding feature-mismatch builds.

## Remove Now (Low Risk)

### 7) Empty scaffold directory

- `crates/sui-sandbox-mcp/` currently contains no Rust source files and is not a workspace member.

Recommendation:
- Remove directory or add a clear README marker explaining intentional placeholder status.

Why:
- Dead scaffolding confuses architecture boundaries.

## Proposed Decommission Sequence

1. Keep single public checkpoint-ingestion path (`fetch checkpoints`).
2. Keep internal harness/warmup implementations out of the main CLI surface.
3. Split remaining `tools` subcommands into separate binary or opt-in feature docs.
4. Keep `analysis` + `mm2` defaults, enforce `analysis -> mm2`.
5. Keep igloo code isolated from core replay command flow.
6. Reassess `bridge` after above cuts (high test coverage indicates active value; defer hard removal until usage telemetry is available).

## Guardrails Before/After Each Removal

Run:
- `./scripts/phase1_validation.sh --quick`
- `./scripts/phase1_validation.sh` (or `SCAN_REQUIRED=1` in stable network env)
- `cargo test --test sandbox_cli_tests`

Core success criteria:
- Replay canary status parity remains intact.
- No regression of fixed classes (`LOOKUP_FAILED`, `FUNCTION_RESOLUTION_FAILURE`) in scan summaries when scan data is available.
- CLI help/docs remain consistent with surviving commands.
