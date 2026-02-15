# Unification Target

This document defines the "fully integrated" architecture target for orchestration
surfaces across Rust CLI and Python bindings.

## Canonical Surfaces

- `context` (alias: `flow`)
- `adapter` (alias: `protocol`)
- `pipeline` (alias: `workflow`)

These are the only first-class orchestration surfaces. Compatibility surfaces
(`script` / `run-flow`) remain legacy mode.

## Invariants

1. Core-first behavior
   - Shared workflow planning logic lives in `sui-sandbox-core`.
   - Shared replay diagnostics/reporting/classification lives in `sui-sandbox-core`.
2. Rust/Python parity
   - Python bindings are native wrappers over shared Rust core modules.
   - Alias APIs (`pipeline_*` and `workflow_*`, `adapter_*` and `protocol_*`) remain behaviorally equivalent.
3. No internal CLI probe shelling
   - Workflow auto/discovery/package-probe paths execute natively (no recursive `current_exe` invocation).
4. Stable onboarding
   - Rust and Python example sets stay aligned with the core onboarding path.

## Execution Gates

Changes are considered integrated only when all gates pass:

1. Build gates
   - `cargo check -p sui-sandbox-core`
   - `cargo check -p sui-sandbox`
   - `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo check -p sui-python --tests`
2. Test gates
   - `cargo test -p sui-sandbox workflow -- --test-threads=1`
   - `cargo test -p sui-sandbox-core workflow_planner -- --test-threads=1`
3. Example smoke gates
   - `./scripts/rust_examples_smoke.sh`
   - `./scripts/python_examples_smoke.sh`

## Current Integrated Modules

- `crates/sui-sandbox-core/src/workflow.rs`
- `crates/sui-sandbox-core/src/workflow_runner.rs`
- `crates/sui-sandbox-core/src/workflow_planner.rs`
- `src/bin/sandbox_cli/workflow.rs`
- `crates/sui-python/src/workflow_native.rs`
- `crates/sui-python/src/workflow_api.rs`
- `crates/sui-python/src/session_api.rs`
- `crates/sui-python/src/transport_helpers.rs`
- `crates/sui-python/src/replay_output.rs`
- `crates/sui-python/src/replay_api.rs`
- `crates/sui-python/src/replay_core.rs`
- `src/bin/sandbox_cli/workflow/native_exec.rs`
- `src/bin/sandbox_cli/flow/context_io.rs`
