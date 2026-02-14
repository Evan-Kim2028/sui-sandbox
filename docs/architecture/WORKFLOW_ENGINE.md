# Workflow Engine Contract

This note defines the typed workflow layer introduced by `sui-sandbox workflow`.

## Goal

Provide a protocol-agnostic execution contract so higher-level tools can submit one
workflow spec and rely on Rust-side replay/analyze implementations for execution.

## Layering

1. **Spec + validation (core)**  
   `crates/sui-sandbox-core/src/workflow.rs`
2. **Template planners (core)**  
   `crates/sui-sandbox-core/src/workflow_adapter.rs`
3. **CLI adapter (execution/init)**  
   `src/bin/sandbox_cli/workflow.rs`
4. **Existing engines (unchanged)**  
   `replay`, `analyze replay`, and other subcommands

The workflow adapter resolves each typed step into a deterministic `sui-sandbox` argv
sequence, then executes it. This keeps behavior aligned with existing commands while
providing a stable machine-oriented contract.

`workflow run --report <path>` writes a canonical JSON execution artifact (including
per-step argv, status, and elapsed time) for CI and orchestration layers.

`workflow init --template <name>` uses built-in template planners to emit starter
specs for:

- `generic`
- `cetus`
- `suilend`
- `scallop`

Template planners can also embed protocol context directly into generated command
steps via `workflow init --package-id ... --view-object ...`, so users can start
with package/object introspection before replay execution.

`workflow init --from-config <file>` allows the same planner inputs to be driven
from a checked-in config file for CI and reproducible team workflows.

Generated specs can be emitted as JSON or YAML (`workflow init --format yaml`), or
inferred from the output file extension.

`workflow auto --package-id <id>` adds a package-first draft adapter flow:

- probes package modules via `analyze package` (when available),
- infers template heuristically (or uses explicit override),
- validates fetched package bytecode closure (fails closed on unresolved deps),
- emits scaffold-only workflows when no digest is provided,
- emits replay-capable drafts when digest/checkpoint are supplied.

`workflow auto --best-effort` is the single escape hatch when strict closure
validation fails and you still want scaffold output.

## Current Step Kinds

- `replay`
- `analyze_replay`
- `command` (pass-through argv)

## Why This Scales

- New protocol-specific logic can compile down into these generic step kinds.
- Python bindings can remain thin pass-through wrappers that emit workflow specs.
- Rust remains the single implementation of hydration/execution semantics.

## Extension Path

When adding protocol adapters (Suilend/Cetus/Scallop/etc.), keep this shape:

1. Build adapter-specific planners outside core replay engine.
2. Emit typed workflow specs.
3. Execute via `sui-sandbox workflow run`.
4. Add new step kinds only when multiple adapters require shared behavior that cannot
   be expressed with existing commands.
