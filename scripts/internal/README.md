# Internal Shell Tools

These scripts are intentionally kept out of `examples/` to keep onboarding focused.
They are maintainer/power-user helpers for workflows that are not yet first-class CLI subcommands.

## Scripts

- `scripts/internal/cli_workflow.sh`
  - Legacy multi-step CLI walkthrough.
- `scripts/internal/entry_function_practical_fuzzer.sh`
  - High-fanout practical fuzz orchestration.
- `scripts/internal/cli_package_analysis.sh`
  - Package loop helper (fetch + inspect + zero-arg entry attempts).
- `scripts/internal/cli_mm2_corpus_sweep.sh`
  - Corpus MM2 sweep with TSV reporting.
- `scripts/internal/cli_obfuscated_analysis.sh`
  - Stonker-focused obfuscated package workflow.

## Direction

Preferred user-facing surfaces are:

1. `sui-sandbox` native subcommands (`replay`, `analyze`, `workflow`, `test`).
2. Typed workflow specs under `examples/data/`.
3. Rust examples under `examples/*.rs`.

As corresponding first-class commands are added, these scripts should be retired.
