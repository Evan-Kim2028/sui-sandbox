# Golden Flow: Context/Adapter/Pipeline

This is the recommended, repeatable CLI workflow for local development.
It centers on canonical orchestration surfaces:

- `context` (generic package-first replay)
- `adapter` (protocol-labeled wrapper over context)
- `pipeline` (typed multi-step orchestration)

## 1) Prepare a deterministic workspace

```bash
export SUI_SANDBOX_HOME=/tmp/sui-sandbox-demo
mkdir -p "$SUI_SANDBOX_HOME"
```

## 2) Discover replay candidates for a package

```bash
sui-sandbox context discover --latest 5 --package-id 0x2
```

Pick one `<DIGEST>` and `<CP>` from the output.

## 3) Prepare a reusable context artifact

```bash
sui-sandbox context prepare --package-id 0x2 --output "$SUI_SANDBOX_HOME/contexts/context.2.json" --force
```

## 4) Replay with the prepared context

```bash
sui-sandbox context replay <DIGEST> \
  --context "$SUI_SANDBOX_HOME/contexts/context.2.json" \
  --checkpoint <CP> \
  --compare
```

## 5) If replay fails, run hydration/readiness analysis

```bash
sui-sandbox analyze replay <DIGEST> --checkpoint <CP> --json
```

## 6) Protocol-labeled wrapper (optional)

```bash
sui-sandbox adapter run \
  --protocol deepbook \
  --package-id 0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b \
  --discover-latest 5 \
  --analyze-only
```

## 7) Typed automation flow (optional)

```bash
sui-sandbox pipeline init --template generic --output examples/out/workflow_templates/workflow.generic.json --force
sui-sandbox pipeline validate --spec examples/out/workflow_templates/workflow.generic.json
sui-sandbox pipeline run --spec examples/out/workflow_templates/workflow.generic.json --dry-run
```

## Notes

- `flow`, `protocol`, and `workflow` remain compatibility aliases.
- Use `--state-json` for fully offline deterministic replay fixtures.
- Use archive-capable endpoints for historical hydration. Default archive endpoint is `https://archive.mainnet.sui.io:443`.
