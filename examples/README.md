# Examples - Core Path

This directory is intentionally slimmed down for onboarding.
Start with the core flow below, then use `examples/advanced/README.md` for deeper workflows.

## Core Examples (Recommended)

Run these in order:

1. `sui-sandbox replay '*' --source walrus --latest 5 --compare`
2. `sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare`
3. `cargo run --example state_json_offline_replay`
4. `sui-sandbox workflow validate --spec examples/data/workflow_replay_analyze_demo.json` then `sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --dry-run`
5. `cargo run --example ptb_basics`

Optional add-ons (after core):

- `sui-sandbox replay mutate --demo --out-dir examples/out/replay_mutation_guided_demo`
- `sui-sandbox workflow validate --spec examples/data/workflow_cli_quickstart.json` then `sui-sandbox workflow run --spec examples/data/workflow_cli_quickstart.json`
- `sui-sandbox workflow init --template cetus --output examples/out/workflow_templates/workflow.cetus.json --force`
- `sui-sandbox workflow init --from-config examples/data/workflow_init_suilend.yaml --force`
- `sui-sandbox workflow auto --package-id 0x2 --force`
- `cargo run --example walrus_ptb_universe`

### 1) Checkpoint Stream Replay (Core External Flow)

```bash
sui-sandbox replay '*' --source walrus --latest 5 --compare
sui-sandbox replay '*' --source walrus --latest 10 --compare
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926 --compare
```

Why this is first:

- zero setup
- no API key
- validates replay behavior on fresh mainnet activity

### 2) Single-Transaction Replay

```bash
# Walrus (recommended)
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CHECKPOINT> --compare

# gRPC / hybrid
sui-sandbox replay <DIGEST> --source grpc --compare
sui-sandbox replay <DIGEST> --source hybrid --compare

# Offline JSON state
sui-sandbox replay <DIGEST> --state-json <STATE_FILE>
```

### 3) Offline Replay from Custom State JSON

```bash
cargo run --example state_json_offline_replay
cargo run --example state_json_offline_replay -- --state-json ./examples/data/state_json_synthetic_ptb_demo.json
```

This demonstrates a fully offline synthetic PTB replay flow:

- replay source is local `--state-json`
- synthetic PTB command data comes from your JSON fixture
- no network hydration required

Default fixture:

- `examples/data/state_json_synthetic_ptb_demo.json`

### 4) Guided Replay Mutation Demo

```bash
sui-sandbox replay mutate --demo --out-dir examples/out/replay_mutation_guided_demo
sui-sandbox replay mutate --demo --max-transactions 2 --out-dir examples/out/replay_mutation_guided_demo
```

This is a deterministic fail->heal walkthrough using:

- fixture: `examples/data/replay_mutation_fixture_v1.json`
- mutation strategy engine from `sui-sandbox replay mutate`
- built-in guided summary in command output and JSON report

### 5) Typed Workflow Spec (Validate + Dry-Run)

```bash
sui-sandbox workflow validate --spec examples/data/workflow_replay_analyze_demo.json
sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --dry-run
sui-sandbox workflow run --spec examples/data/workflow_replay_analyze_demo.json --report examples/out/workflow_demo/report.json --dry-run
```

Default spec:

- `examples/data/workflow_replay_analyze_demo.json`

This demonstrates the typed `workflow` contract:

- `kind: analyze_replay` step
- `kind: replay` step
- `kind: command` pass-through step
- package-id-first draft adapter generation via `workflow auto`

Use `--dry-run` for plan inspection and remove it to execute for real.

### 6) Built-In Workflow Template Generation

```bash
sui-sandbox workflow init --template generic --output examples/out/workflow_templates/workflow.generic.json --force
sui-sandbox workflow init --template cetus --output examples/out/workflow_templates/workflow.cetus.json --force
sui-sandbox workflow init --template suilend --output examples/out/workflow_templates/workflow.suilend.json --force
sui-sandbox workflow init --template scallop --output examples/out/workflow_templates/workflow.scallop.json --force

sui-sandbox workflow validate --spec examples/out/workflow_templates/workflow.suilend.json
```

### 7) Workflow Init From Config

```bash
sui-sandbox workflow init --from-config examples/data/workflow_init_suilend.yaml --force
sui-sandbox workflow validate --spec workflow.suilend.json
sui-sandbox workflow run --spec workflow.suilend.json --dry-run
```

Config example:

- `examples/data/workflow_init_suilend.yaml`

### 8) Workflow Auto Draft Adapter

```bash
sui-sandbox workflow auto --package-id 0x2 --force
sui-sandbox workflow auto --package-id 0x2 --digest <DIGEST> --checkpoint <CP> --force
sui-sandbox workflow auto --package-id 0xdeadbeef --best-effort --force
```

### 9) Walrus PTB Universe

```bash
cargo run --example walrus_ptb_universe
cargo run --example walrus_ptb_universe -- --latest 10 --top-packages 8 --max-ptbs 20
```

Artifacts are written to `examples/out/walrus_ptb_universe/`.

### 10) Local PTB Basics (Rust)

```bash
cargo run --example ptb_basics
```

No network access required.

## Core vs Advanced

| Tier | What it covers | Index |
|------|----------------|-------|
| Core | Quick onboarding + primary replay workflows | `examples/README.md` |
| Advanced | Deep replay analysis, package corpus workflows, obfuscated package analysis, DeepBook historical reconstruction, forked-state prototyping | `examples/advanced/README.md` |

## Advanced Entry Point

See `examples/advanced/README.md` for:

- replay mutation lab (`replay mutate`)
- package analysis workflows
- obfuscated package analysis
- self-heal replay workflow
- Walrus digest-specific workflows
- advanced Rust examples (`fork_state`, `deepbook_*`)
- optional maintainer-grade shell tooling in `scripts/internal/README.md`

## Python Examples

If you prefer Python bindings, use:

- `python_sui_sandbox/README.md`

It includes:

1. Walrus checkpoint summary
2. Package interface extraction
3. Replay analyze (no VM execution)
4. Typed workflow pass-through (Python -> Rust CLI)
5. Built-in workflow template pass-through
6. DeepBook `manager_state` native example (no CLI pass-through)

## Troubleshooting

### Build errors

```bash
cargo clean
cargo build --release --bin sui-sandbox
```

### gRPC endpoint issues

Set your endpoint before running gRPC/hybrid examples:

```bash
export SUI_GRPC_ENDPOINT=https://archive.mainnet.sui.io:443
# optional
export SUI_GRPC_API_KEY=...
```
