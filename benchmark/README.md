## `benchmark/` (key struct target discovery)

This benchmark scores an agent on discovering **which structs have `key`** in a Sui Move package.

It uses `sui-move-interface-extractor` as the ground-truth parser for `.mv` bytecode artifacts.

### Setup

```bash
cd benchmark
uv sync --group dev --frozen
```

### Configure a real agent (optional)

Copy `benchmark/.env.example` to `benchmark/.env` and fill in:

- `SMI_API_KEY`
- `SMI_MODEL`
- `SMI_API_BASE_URL` (OpenAI-compatible base; for non-OpenAI providers)

Smoke test (does a single tiny API call and expects `[]` JSON):

```bash
uv run smi-bench --corpus-root <sui-packages-checkout>/packages/mainnet_most_used --smoke-agent --agent real-openai-compatible
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
