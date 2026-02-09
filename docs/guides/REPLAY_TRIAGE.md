# Replay Triage Workflow

Use this when local replay fails or behaves unexpectedly.

## Decision Table

| Situation | First command | What to inspect |
|---|---|---|
| Replay fails with missing data | `sui-sandbox analyze replay <DIGEST>` | `missing_inputs`, `missing_packages`, `suggestions` |
| Replay succeeds but differs from chain | `sui-sandbox replay <DIGEST> --compare` | `comparison` section + `Execution Path` |
| Need deterministic offline debugging | `sui-sandbox replay <DIGEST> --state-json state.json` | JSON output + fixed local state file |
| Suspect hydration/flag issues | `sui-sandbox analyze replay <DIGEST> --json` | `hydration` object (`source`, fallback, prefetch, system-object flags) |

## Recommended Loop

1. Check readiness and hydration assumptions:

```bash
sui-sandbox --json analyze replay <DIGEST>
```

2. Replay with comparison enabled:

```bash
sui-sandbox --json replay <DIGEST> --compare
```

3. If data is incomplete, widen hydration:

```bash
sui-sandbox replay <DIGEST> --allow-fallback true
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP>
```

4. If dynamic-field coverage looks incomplete:

```bash
sui-sandbox replay <DIGEST> --prefetch-depth 4 --prefetch-limit 400
```

5. Freeze state for reproducible debugging:

```bash
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json
sui-sandbox --json replay <DIGEST> --state-json state.json
```

## High-Signal Fields

- `replay.execution_path`: source, fallback usage, prefetch settings, system-object flag.
- `analyze replay.hydration`: requested hydration settings used for state build.
- `analyze replay.missing_inputs` / `missing_packages`: concrete blockers.
- `analyze replay.suggestions`: next actions generated from observed gaps.

