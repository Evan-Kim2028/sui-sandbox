# `sui-move-interface-extractor` runbook

This runbook documents the intended workflow for running scans/validation and keeping results reproducible as the `sui-packages` dataset evolves.

## Principles

- Treat on-chain bytecode (`bcs.moduleMap`) as the ground truth.
- Use RPC normalized interfaces as an independent cross-check (interface-level correctness).
- Prefer deterministic outputs and stable ordering so diffs are meaningful.

## Before you run

1. Ensure the bytecode dataset checkout exists:
   - `<sui-packages-checkout>/`
2. Know your corpus root:
   - `<sui-packages-checkout>/packages/mainnet_most_used` (1000 packages; mostly symlinks)
3. For RPC-heavy runs, keep concurrency low to avoid rate limiting:
   - recommended: `--concurrency 1`

## Standard runs

### A) Full 1000-package rigorous validation

```bash
cd packages/sui-move-interface-extractor
cargo run --release -- \
  --bytecode-corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir out/corpus_interface_all_1000 \
  --corpus-local-bytes-check \
  --corpus-rpc-compare \
  --corpus-interface-compare \
  --concurrency 1 \
  --retries 12 --retry-initial-ms 500 --retry-max-ms 10000 \
  --emit-submission-summary results/mainnet_most_used_summary.json
```

Inspect:

- `out/corpus_interface_all_1000/corpus_summary.json`
- `out/corpus_interface_all_1000/problems.jsonl`
- `out/corpus_interface_all_1000/run_metadata.json` (dataset attribution)
- `results/mainnet_most_used_summary.json` (sanitized, shareable)

### B) Deterministic sampling (for iteration)

```bash
cd packages/sui-move-interface-extractor
cargo run --release -- \
  --bytecode-corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir out/corpus_interface_sample200 \
  --corpus-local-bytes-check \
  --corpus-sample 200 --corpus-seed 1 \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency 1
```

This writes:

- `out/corpus_interface_sample200/sample_ids.txt`

Re-run the exact same set later using:

```bash
cargo run --release -- \
  --bytecode-corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --out-dir out/rerun_same_sample \
  --corpus-ids-file out/corpus_interface_sample200/sample_ids.txt \
  --corpus-local-bytes-check \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency 1
```

## Interpreting results

- `problems.jsonl` is a filtered subset of packages that failed any enabled check.
- When `--corpus-interface-compare` is enabled, a package is considered “interface OK” only if:
  - structs match (abilities, type params, field names/types)
  - RPC exposed functions match (visibility, entry, type params, params/returns)

Note: the rigorous compare only matches what RPC exposes (`exposedFunctions`). Bytecode-derived JSON includes private functions too; RPC does not, so those aren’t compared in corpus mode.

### Making failures actionable

In corpus mode, each `CorpusRow` may include:

- `local_bytes_check`: per-module byte integrity summary (when `--corpus-local-bytes-check` is enabled)
- `local_bytes_check_error`: why the byte check couldn't run (parse/read errors)
- `interface_compare`: summary counts
- `interface_compare_sample`: up to `--corpus-interface-compare-max-mismatches` mismatch samples (path + reason)

To include the raw values for each mismatch sample, pass:

- `--corpus-interface-compare-include-values`

## Debugging mismatches

1. Take the failing package id(s) from `problems.jsonl` (or use `--corpus-ids-file` on a hand-made list).
2. Run a single-package compare report:

```bash
cd packages/sui-move-interface-extractor
cargo run --release -- \
  --package-id <0x...> \
  --compare-bytecode-rpc \
  --emit-compare-report out/compare_<id>.json
```

3. Inspect `out/compare_<id>.json` and focus on the first mismatch paths.

## Recording “what dataset was this run against?”

Every corpus run writes `run_metadata.json` (including best-effort `sui-packages` git HEAD when available).

Optionally maintain a human-readable snapshot log here:

- `docs/DATASET_SNAPSHOTS.md`

## Performance knobs

- For RPC-heavy runs (normalized interface + rigorous compare), use `--concurrency 1` to avoid 429s.
- For local-only scans where you only need module-name checks, use:
  - `--corpus-module-names-only`
  - (not compatible with RPC compare / interface compare)
- For local-only scans where you want a strong integrity check, use:
  - `--corpus-local-bytes-check`
