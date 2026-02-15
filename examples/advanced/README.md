# Advanced Examples

These examples are intentionally separated from the core onboarding path.
They are useful for power users, debugging, and research workflows.
They are not part of the Rust/Python parity onboarding set in `examples/README.md`.

## Advanced Workflows

### Replay Mutation Lab (Native CLI)

```bash
sui-sandbox replay mutate --latest 5 --max-transactions 60 --out-dir examples/out/replay_mutation_lab
sui-sandbox replay mutate --demo --out-dir examples/out/replay_mutation_lab
```

### CLI Quickstart (Typed Workflow)

```bash
sui-sandbox pipeline validate --spec examples/data/workflow_cli_quickstart.json
sui-sandbox pipeline run --spec examples/data/workflow_cli_quickstart.json

# package-agnostic two-step replay flow
sui-sandbox context run --package-id 0x2 --digest <DIGEST> --checkpoint <CP>
sui-sandbox context run --package-id 0x2 --discover-latest 5 --analyze-only
sui-sandbox context prepare --package-id 0x2 --output examples/out/flow_context/flow_context.2.json --force
sui-sandbox context replay <DIGEST> --context examples/out/flow_context/flow_context.2.json --checkpoint <CP>
sui-sandbox context replay --context examples/out/flow_context/flow_context.2.json --discover-latest 5 --analyze-only
```

### Package Analysis

```bash
sui-sandbox analyze package --package-id <PACKAGE_ID> --list-modules --mm2
sui-sandbox --json analyze package --bytecode-dir <PACKAGE_DIR> --mm2

# corpus object classification (direct CLI)
sui-sandbox --json analyze objects --corpus-dir <CORPUS_DIR> --top 20
```

### Obfuscated Package Analysis

```bash
sui-sandbox fetch package <PACKAGE_ID> --with-deps
sui-sandbox view modules <PACKAGE_ID>
sui-sandbox view module <PACKAGE_ID>::<MODULE> --json
```

### Self-Heal Replay Workflow

```bash
sui-sandbox pipeline validate --spec examples/data/workflow_self_heal_replay_demo.json
sui-sandbox pipeline run --spec examples/data/workflow_self_heal_replay_demo.json --dry-run
# then replace REPLACE_WITH_DIGEST in the spec and run without --dry-run
```

### Walrus Digest-Specific Workflows

```bash
# direct zero-setup replay/analyze commands
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare --verbose
sui-sandbox analyze replay <DIGEST> --source walrus --checkpoint <CP> --json --verbose

# protocol-focused typed workflow specs
sui-sandbox pipeline validate --spec examples/data/workflow_walrus_cetus_swap.json
sui-sandbox pipeline run --spec examples/data/workflow_walrus_cetus_swap.json

sui-sandbox pipeline validate --spec examples/data/workflow_walrus_deepbook_orders.json
sui-sandbox pipeline run --spec examples/data/workflow_walrus_deepbook_orders.json --continue-on-error

sui-sandbox pipeline validate --spec examples/data/workflow_walrus_multi_swap_flash_loan.json
sui-sandbox pipeline run --spec examples/data/workflow_walrus_multi_swap_flash_loan.json
```

## Advanced Rust Examples

Some of these remain intentionally deep/protocol-specific, but not all.
`deepbook_margin_state` is now a thin wrapper over generic first-class Rust
historical-view orchestration helpers.
`ReplayOrchestrator` also now exposes generic batch historical-view execution
and reusable PTB return-value decoders used by advanced examples.
`fork_state` and `deepbook_spot_offline_ptb` now share generic
`environment_bootstrap` helpers for package/object hydration and local
environment setup.

### Fork Mainnet State + Custom Contract

```bash
cargo run --example fork_state
```

### DeepBook Margin Manager Historical Reconstruction

```bash
cargo run --example deepbook_margin_state
cargo run --example deepbook_timeseries
cargo run --example deepbook_json_bcs_only
```

### DeepBook Spot Offline PTB (Pool + Orders)

```bash
cargo run --example deepbook_spot_offline_ptb
```

This example fetches live DeepBook package/state once, then runs locally to:

- create a permissionless SUI/STABLECOIN pool
- create a balance manager + deposits
- place bid/ask limit orders
- query locked balances

Data and docs:

- `examples/advanced/deepbook_margin_state/README.md`
- `examples/advanced/deepbook_margin_state/data/`

Validation helper:

```bash
./scripts/rust_examples_smoke.sh
./scripts/rust_examples_smoke.sh --network
```

## Notes

- Some advanced flows require gRPC historical data.
- Core onboarding remains in `examples/README.md`.
- Power-user shell orchestration is intentionally moved out of examples to `scripts/internal/README.md`.
