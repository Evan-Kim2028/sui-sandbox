## `benchmark/` (key struct target discovery)

This benchmark scores an agent on discovering **which structs have `key`** in a Sui Move package.

It uses `sui-move-interface-extractor` as the ground-truth parser for `.mv` bytecode artifacts.

### Setup

```bash
cd benchmark
uv sync --group dev
```

### Run (mock agents)

You need a local `sui-packages` checkout and a corpus root like `<sui-packages-checkout>/packages/mainnet_most_used`.

```bash
uv run smi-bench \
  --corpus-root <sui-packages-checkout>/packages/mainnet_most_used \
  --samples 25 --seed 1 \
  --agent mock-empty
```

### Output

The runner writes a small JSON report with per-package metrics and an aggregate summary.
