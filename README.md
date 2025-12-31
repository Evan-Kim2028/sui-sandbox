# `sui-move-interface-extractor`

Standalone bytecode-first Rust CLI for parsing Sui Move `.mv` modules and producing deterministic, diff-friendly JSON interfaces.

It parses compiled `.mv` modules directly and emits canonical JSON for:

- corpus-level scanning/verification (index + counts + diff stability)
- bytecode-derived interfaces (`--emit-bytecode-json`) including private functions and full signature types
- rigorous comparison against RPC normalized interfaces (`--compare-bytecode-rpc`)

## Schema

- `docs/SCHEMA.md` documents the emitted JSON schemas and determinism rules.
- `docs/METHODOLOGY.md` explains the bytecode-first approach, verification loops, and limitations.
- `docs/RUNBOOK.md` has reproducible run commands.

## Dataset (bytecode corpus) location

Pass an explicit corpus root via `--bytecode-corpus-root`.

If you use the `sui-packages` dataset (a local artifact corpus), point at a corpus root like:

- `<sui-packages-checkout>/packages/mainnet_most_used`
- `<sui-packages-checkout>/packages/mainnet`

Each package dir is expected to contain `bytecode_modules/*.mv` and typically includes `metadata.json` and `bcs.json`.

## How to run

Prereqs:

- Rust toolchain (stable): https://rustup.rs
- `git`

Dataset:

```bash
git clone https://github.com/MystenLabs/sui-packages.git
```

Reproduce a `mainnet_most_used` validation run (and write a shareable summary JSON):

```bash
mkdir -p results
cargo run --release -- --bytecode-corpus-root ../sui-packages/packages/mainnet_most_used \
  --out-dir out/corpus_interface_all_1000 \
  --corpus-local-bytes-check \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency 1 \
  --emit-submission-summary results/mainnet_most_used_summary.json
```

Notes:

- `out/` is a scratch directory for large outputs (gitignored).
- `results/` is intended for small, shareable summary JSONs only.

## Repro Script

For a one-command reproduction of `mainnet_most_used`, use:

- `scripts/reproduce_mainnet_most_used.sh`

Build:

```bash
cargo build
```

Emit bytecode-derived canonical interface JSON from RPC BCS bytes:

```bash
mkdir -p out
cargo run --release -- --package-id 0x2 --emit-bytecode-json out/0x2_bytecode.json --sanity
```

Compare RPC normalized vs bytecode-derived interface (rigorous, field-by-field):

```bash
mkdir -p out
cargo run --release -- --package-id 0x2 --compare-bytecode-rpc --emit-compare-report out/0x2_compare.json
```

Emit bytecode-derived canonical interface JSON from a local artifact dir:

```bash
cargo run --release -- --bytecode-package-dir ../sui-packages/packages/mainnet_most_used/0x00/00000000000000000000000000000000000000000000000000000000000002 \
  --emit-bytecode-json out/0x2_local.json \
  --sanity
```

Run the `mainnet_most_used` corpus scan (local-only):

```bash
cargo run --release -- --bytecode-corpus-root ../sui-packages/packages/mainnet_most_used \
  --out-dir out/mainnet_most_used_local \
  --corpus-local-bytes-check \
  --concurrency 16
```

Run a corpus sample with rigorous interface compare:

```bash
cargo run --release -- --bytecode-corpus-root ../sui-packages/packages/mainnet_most_used \
  --out-dir out/corpus_interface_sample200 \
  --corpus-sample 200 --corpus-seed 1 \
  --corpus-rpc-compare --corpus-interface-compare \
  --concurrency 1
```

## Run metadata (dataset snapshot)

Corpus runs write a `run_metadata.json` file into the `--out-dir` that captures:

- start/end time (unix seconds)
- full CLI argv
- `rpc_url`
- `bytecode_corpus_root`
- best-effort `sui-packages` git HEAD (if the corpus path is inside a git checkout)

This is the “guard rail” that makes results attributable even if the underlying dataset updates.
