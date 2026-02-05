# Self-Heal Replay Examples (Testing Only)

These examples demonstrate **self-healing replay** workflows when historical data is incomplete
(e.g., gRPC/object availability gaps). The goal is to show how the CLI can **synthesize
placeholder state** so PTBs can execute locally for investigation.

> ⚠️ **Testing-only:** Synthesized objects and dynamic-field values are **not** high-fidelity
> on-chain state. Use this to explore code paths and debug, not to validate economic behavior.

> Requires the `mm2` feature for synthesis flags.

## CLI Self-Heal Replay

```
./examples/self_heal/cli_self_heal_replay.sh <DIGEST>
```

If no digest is provided, the script uses the first entry in `selected_digests.txt`.

### What it does

1. Attempts a **strict replay** (no synthesis).
2. Replays again with:
   - `--synthesize-missing` (placeholder input objects)
   - `--self-heal-dynamic-fields` (placeholder dynamic-field values)

The script prints clear logs when synthesis is used.
