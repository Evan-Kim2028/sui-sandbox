# Self-Heal Replay Examples (Testing Only)

These examples demonstrate **self-healing replay** workflows when historical data is incomplete
(e.g., gRPC/object availability gaps). The goal is to show how the CLI can **synthesize
placeholder state** so PTBs can execute locally for investigation.

> ⚠️ **Testing-only:** Synthesized objects and dynamic-field values are **not** high-fidelity
> on-chain state. Use this to explore code paths and debug, not to validate economic behavior.

> Requires the `mm2` feature for synthesis flags.

## Typed Workflow Self-Heal Replay

Template workflow spec:

- `examples/data/workflow_self_heal_replay_demo.json`

Run:

```bash
# 1) copy the template and replace REPLACE_WITH_DIGEST in both replay steps
cp examples/data/workflow_self_heal_replay_demo.json /tmp/workflow.self_heal.json

# 2) validate + run
sui-sandbox pipeline validate --spec /tmp/workflow.self_heal.json
SUI_SELF_HEAL_LOG=1 sui-sandbox pipeline run --spec /tmp/workflow.self_heal.json
```

For dry-run planning only:

```bash
sui-sandbox pipeline run --spec /tmp/workflow.self_heal.json --dry-run
```

## Direct CLI Equivalent

```bash
# Pass 1: baseline strict replay
sui-sandbox replay <DIGEST> --compare --fetch-strategy full --strict

# Pass 2: self-heal replay
SUI_SELF_HEAL_LOG=1 sui-sandbox replay <DIGEST> \
  --compare \
  --fetch-strategy full \
  --synthesize-missing \
  --self-heal-dynamic-fields
```

### What it does

1. Attempts a **strict replay** (no synthesis).
2. Replays again with:
   - `--synthesize-missing` (placeholder input objects)
   - `--self-heal-dynamic-fields` (placeholder dynamic-field values)

Use `SUI_SELF_HEAL_LOG=1` to print clear logs when synthesis is used.
