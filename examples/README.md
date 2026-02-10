# Examples - Start Here

This is the best way to learn the sui-sandbox library. Work through these examples in order.

## Learning Path

### Level 0: Walrus Checkpoint Stream (Core External Flow)

```bash
./examples/scan_checkpoints.sh
```

This is the primary external-facing example.
It streams across recent checkpoints from Walrus, replays PTBs locally, and prints
an actionable summary (success/fail rates + tags).

```bash
./examples/scan_checkpoints.sh                    # Latest 5 checkpoints
./examples/scan_checkpoints.sh 10                 # Latest 10 checkpoints
./examples/scan_checkpoints.sh --range 100..110   # Explicit checkpoint range
```

Equivalent CLI commands:

```bash
sui-sandbox replay '*' --source walrus --latest 5 --compare
sui-sandbox replay '*' --source walrus --checkpoint 239615920..239615926 --compare
```

Why this is the core example:

- Zero setup, no API key
- Exercises the real replay path on fresh mainnet activity
- Gives instant signal on replay health across multiple transactions

**Prerequisites**: None.

#### Recommended External User Flow

1. Run latest checkpoint stream scan (`./examples/scan_checkpoints.sh`).
2. If failures appear, rerun a narrower range with `--range`.
3. Drill into a specific digest with `sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --compare`.

---

### Level 0.25: Walrus PTB Universe (Mock PTB Generation)

```bash
./examples/walrus_ptb_universe.sh
```

Builds a real PTB universe from the latest Walrus checkpoints, then generates and executes
mock local PTBs from observed package/module/function usage.

```bash
./examples/walrus_ptb_universe.sh
./examples/walrus_ptb_universe.sh --latest 10 --top-packages 8 --max-ptbs 20
./examples/walrus_ptb_universe.sh --out-dir /tmp/walrus-ptb-universe
```

Artifacts written to `examples/out/walrus_ptb_universe/`:

- `universe_summary.json` (checkpoint/package/function distribution)
- `package_downloads.json` (top packages + dependency closure fetch/deploy status)
- `function_candidates.json` (which observed calls are mockable)
- `ptb_specs/*.json` and `ptb_execution_results.json` (generated PTBs + execution outcomes)

**Prerequisites**: None (Walrus + public GraphQL endpoints).

---

### Level 0.5: Single-Transaction Replay

```bash
./examples/replay.sh
```

Replay a specific mainnet transaction locally. Supports multiple data sources:

```bash
./examples/replay.sh                                        # Walrus (default, zero setup)
./examples/replay.sh --source walrus <DIGEST> <CHECKPOINT>  # Walrus with custom tx
./examples/replay.sh --source walrus '*' 100..200           # Walrus range (all txs in range)
./examples/replay.sh --source grpc <DIGEST>                 # gRPC (needs SUI_GRPC_ENDPOINT)
./examples/replay.sh --source json <STATE_FILE>             # JSON state file (any data source)
```

- **Walrus**: Zero authentication, fetches everything from decentralized checkpoint storage. Supports checkpoint ranges (`100..200`) and lists (`100,105,110`).
- **gRPC**: Standard fullnode/archive endpoint (requires `SUI_GRPC_ENDPOINT`)
- **JSON**: Load replay state from a JSON file. Bring your own data from any source.

Export state from any source, then replay offline:

```bash
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CP> --export-state state.json
sui-sandbox replay <DIGEST> --state-json state.json
```

Or use the CLI directly:

```bash
sui-sandbox replay <DIGEST> --source walrus --checkpoint <CHECKPOINT> --compare
sui-sandbox replay <DIGEST> --source grpc --compare
```

**Prerequisites**: None for Walrus or JSON. gRPC requires endpoint configuration.

---

### Level 0.75: Replay Mutation Lab (Fail -> Heal)

```bash
./target/debug/sui-sandbox replay mutate --demo
./examples/replay_mutation_lab.sh
./examples/replay_mutation_guided_demo.sh
```

Scans recent Walrus transactions and searches for a practical fail->heal replay path:

- **Baseline pass**: constrained hydration (`--fetch-strategy eager --no-prefetch --allow-fallback false`)
- **Heal pass**: full hydration + synthesis (`--synthesize-missing --self-heal-dynamic-fields`)

The lab records the first transaction where baseline fails but heal succeeds, then exports
the winning replay state and writes a concise report.

For a deterministic, one-command onboarding flow, use the guided demo wrapper:

```bash
./examples/replay_mutation_guided_demo.sh
# Equivalent CLI entrypoint
sui-sandbox replay mutate --demo
```

This uses a pinned fixture candidate set (`examples/data/replay_mutation_fixture_v1.json`)
to make replay target selection deterministic across runs while still replaying real Walrus data.

```bash
./examples/replay_mutation_lab.sh --latest 10 --max-transactions 80
./examples/replay_mutation_lab.sh --digest <DIGEST> --checkpoint <CP>
./examples/replay_mutation_lab.sh --digest At8M8D7QoW3HHXUBHHvrsdhko8hEDdLAeqkZBjNSKFk2 --checkpoint 239615926
./examples/replay_mutation_lab.sh --fixture examples/data/replay_mutation_fixture_v1.json --max-transactions 4
./examples/replay_mutation_guided_demo.sh --max-transactions 2
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --max-transactions 4
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --replay-source walrus --jobs 4 --retries 1
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --keep-going --differential-source grpc --corpus-out examples/out/replay_mutation_corpus.json
sui-sandbox replay mutate --digest 5WqivEXirxeLLENpZEhEdGzprwJ6yRbeVJTqJ3KkyGP5 --checkpoint 239615931
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --no-op --json
sui-sandbox replay mutate --fixture examples/data/replay_mutation_fixture_v1.json --strategy examples/replay_mutate_strategies/default.yaml
```

`examples/replay_mutation_lab.sh` is now a thin wrapper around the native CLI engine (`sui-sandbox replay mutate`), so behavior and artifacts stay aligned with the primary command surface.

Strategy config is optional but recommended for reusable plans:

- `examples/replay_mutate_strategies/default.yaml`
- Supports: mutators, oracles, invariants, scoring, minimization mode (`state-diff`, `operator-specific`, `none`).
- Concrete state mutators now include: `state_drop_required_object`, `state_input_rewire`,
  `state_object_version_skew`, `state_shared_object_substitute`, `state_pure_type_aware`,
  `state_pure_signature_aware`.

Artifacts are written under `examples/out/replay_mutation_lab/run_<timestamp>/`:

- `candidate_pool.json`
- `attempts.json`
- `report.json`
- `attempt_*/baseline_stdout.json` + `heal_stdout.json`
- `winning_state.json` (when a fail->heal pair is found)

Guided demo artifacts are written under `examples/out/replay_mutation_guided_demo/run_<timestamp>/`.

**Prerequisites**: None.

---

### Level 0.9: Entry Function Practical Fuzzer

```bash
./examples/entry_function_practical_fuzzer.sh
```

Builds a practical function target set from recent Walrus MoveCall activity, then runs
replay-backed baseline vs heal passes per target:

- **Baseline pass**: constrained hydration (`--fetch-strategy eager --no-prefetch --allow-fallback false`)
- **Heal pass**: by default only when baseline fails (`--heal-mode on-failure`)
- **Checkpoint pre-ingest**: by default ingests the scanned checkpoint window into local Walrus indexes before replay.
- **Parallel replay**: runs with bounded worker pool (`--replay-jobs`, default `2`)
- **Phased pipeline** (default): Phase A broad baseline triage, then Phase B focused deep replay on shortlisted failures (`--phase-mode phased`)
- **Mutation stage**: replay mutation operators over Phase B targets plus oracle/invariant evaluation (`--mutation-budget`, `--mutation-jobs`)
  - Flag/profile operators: `baseline_repeat`, `strict_vm`, `heal_aggressive`, `heal_no_prefetch`
  - State-json mutation operators: `state_pure_type_aware`, `state_pure_signature_aware`, `state_shared_object_substitute`, `state_object_version_skew`, `state_input_rewire`
  - State-json operators export replay state once per candidate, then run local `--state-json --vm-only` mutations (minimal network overhead after export)
  - `state_pure_signature_aware` uses local transaction-input inference by default (no extra package RPC fan-out), and is enriched by module metadata only when `--metadata-lookup` is enabled
  - Oracle includes transport-vs-VM plane separation, per-operator signal scores, and top-ranked operator findings per target
  - Stability mode repeats each operator (`--stability-runs`) and reports unstable operators
  - Automatic minimization shrinks interesting state mutations to minimal diffs (`--no-minimize`, `--minimize-max-trials`)
  - Finding fingerprints deduplicate repeated issues into `findings_index.json`

Default practical profile is network-light:

- Uses Walrus for checkpoint/transaction discovery.
- Disables GraphQL lookup fallback during replay (`SUI_CHECKPOINT_LOOKUP_GRAPHQL=0`, `SUI_PACKAGE_LOOKUP_GRAPHQL=0`), preferring gRPC fallback where needed.
- Disables package/module metadata lookup by default to avoid extra `fetch package` + `view module` fan-out.

```bash
./examples/entry_function_practical_fuzzer.sh --latest 10 --max-transactions 150 --max-targets 25
./examples/entry_function_practical_fuzzer.sh --latest 10 --max-targets 30 --replay-jobs 4
./examples/entry_function_practical_fuzzer.sh --phase-mode phased --phase-a-timeout 12 --phase-a-targets 90 --max-targets 30
./examples/entry_function_practical_fuzzer.sh --phase-mode single --max-targets 30
./examples/entry_function_practical_fuzzer.sh --mutation-budget 5 --mutation-jobs 4
./examples/entry_function_practical_fuzzer.sh --max-targets 120 --mutation-budget 40 --stability-runs 3 --replay-jobs 8 --mutation-jobs 8
./examples/entry_function_practical_fuzzer.sh --no-typed-mutators
./examples/entry_function_practical_fuzzer.sh --no-minimize
./examples/entry_function_practical_fuzzer.sh --minimize-max-trials 20
./examples/entry_function_practical_fuzzer.sh --no-mutations
./examples/entry_function_practical_fuzzer.sh --heal-mode always
./examples/entry_function_practical_fuzzer.sh --include-public --max-targets 30
./examples/entry_function_practical_fuzzer.sh --metadata-lookup --include-public
./examples/entry_function_practical_fuzzer.sh --replay-timeout 20 --out-dir /tmp/entry-fuzzer
```

Artifacts are written under `examples/out/entry_function_practical_fuzzer/run_<timestamp>/`:

- `function_universe.json` (observed package/module/function target universe)
- `checkpoint_ingest.json` (Walrus checkpoint ingest prepass status)
- `phase_a_attempts.json` (Phase A baseline triage outcomes)
- `phase_b_selection.json` (which candidates were promoted to Phase B)
- `attempts.json` (per-target metadata + replay outcome)
- `mutation_results.json` (operator-level mutation outcomes)
- `oracle_report.json` (recovery/regression/timeout-resolution findings + operator signal ranking + stability/plane-shift metrics)
- `invariant_violations.json` (violated invariants for quick triage)
- `minimization_results.json` (before/after mutation diff counts and minimized state paths)
- `findings_index.json` (fingerprinted deduplicated findings with counts/examples)
- `coverage.json` (replay coverage and status distributions)
- `failure_clusters.json` (grouped failure signatures)
- `interesting_successes.json` (fail->heal outcomes)
- `report.json` and `README.md`
- `raw/*.json` (cached replay stdout/stderr snapshots, including `mut_state_export_*` and `state_mut_*` state mutation artifacts)

**Prerequisites**: None.

---

### Level 1: CLI Exploration (Minimal Setup)

```bash
# Run the CLI workflow (recommended)
./examples/cli_workflow.sh
```

This shell script walks you through the CLI interface
without any additional setup. You'll learn:

- Exploring framework modules (`view`)
- Publishing a local Move package
- Executing simple functions via `run`
- Using JSON output for scripting

**Prerequisites**: None beyond the `sui-sandbox` binary.

### Self-Heal Replay (Testing Only)

Demonstrates self-healing replay when historical data is incomplete by synthesizing
placeholder inputs and dynamic-field values:

```
./examples/self_heal/README.md
```

### Package Analysis (CLI)

Includes:

- Single package analysis and no-arg entry execution attempts
- Corpus object classification via `analyze objects`
- Corpus MM2 sweep via `analyze package --bytecode-dir ... --mm2`

```
./examples/package_analysis/README.md
```

---

### Level 2: Basic PTB Operations

```bash
cargo run --example ptb_basics
```

Your first Rust example. Creates a local simulation environment and executes basic PTB commands:

- Creating a `SimulationEnvironment`
- Splitting coins with `SplitCoins`
- Transferring objects with `TransferObjects`

**Prerequisites**: Rust toolchain

---

## All Examples

| Example | Level | API Key | Description |
|---------|-------|---------|-------------|
| `scan_checkpoints.sh` | 0 | **No** | Core flow: stream replay over Walrus checkpoints |
| `walrus_ptb_universe.sh` | 0.25 | **No** | Build observed Walrus PTB universe, generate mock PTBs, execute locally |
| `replay.sh` | 0.5 | **No** | Single-transaction replay (walrus/grpc/json) |
| `replay_mutation_lab.sh` | 0.75 | **No** | Discover fail->heal replay cases with synthetic hydration passes |
| `replay_mutation_guided_demo.sh` | 0.75 | **No** | One-command deterministic replay mutation walkthrough |
| `entry_function_practical_fuzzer.sh` | 0.9 | **No** | Replay-backed practical fuzzer over observed Walrus function targets |
| `cli_workflow.sh` | 1 | No | CLI walkthrough |
| `package_analysis/cli_corpus_objects_analysis.sh` | 1 | No | Corpus-wide `analyze objects` summary + baseline deltas |
| `package_analysis/cli_mm2_corpus_sweep.sh` | 1 | No | Corpus MM2 regression sweep (`analyze package --mm2`) |
| `convertible_simulator` | 1 | No | Convertible vs ETH vs stable APY simulator |
| `ptb_basics` | 2 | No | Basic PTB operations (SplitCoins, TransferObjects) |

## CLI-First Replacements

Several older or experimental examples were consolidated into CLI flows:

- Protocol/package analysis → `sui-sandbox analyze package --package-id 0x...`
- Historical replay demos → `sui-sandbox replay <DIGEST> --compare`
- Walrus package ingest / checkpoint preload → `sui-sandbox fetch checkpoints <START> <END>`

---

## Advanced Examples (Require gRPC API Key)

The following Rust examples demonstrate advanced replay internals. They require a gRPC
archive endpoint and API key. Most users will not need these — the Walrus-based CLI
workflows above cover all common replay scenarios without any authentication.

### Setup

```bash
cp .env.example .env
# Edit .env with your endpoint and API key
```

Example `.env`:

```
SUI_GRPC_ENDPOINT=https://fullnode.mainnet.sui.io:443
SUI_GRPC_API_KEY=your-api-key-here  # Optional, depending on provider
```

### Rust Examples

| Example | Description |
|---------|-------------|
| `fork_state` | Fork mainnet state + deploy custom contracts against real DeFi protocols |
| `cetus_position_fees` | Synthetic object BCS introspection for Cetus fees |
| `cetus_swap` | Full Cetus AMM swap replay with package upgrade handling |
| `deepbook_replay` | DeepBook flash loan replay |
| `deepbook_orders` | BigVector & dynamic field replay (cancel/place limit orders) |
| `multi_swap_flash_loan` | Multi-DEX flash loan arbitrage with complex state |

```bash
cargo run --example fork_state
cargo run --example cetus_swap
cargo run --example deepbook_orders
```

---

## Key Concepts

### SimulationEnvironment

The local Move VM that executes PTBs:

```rust
let mut env = SimulationEnvironment::new()?;

// Pre-loaded with Sui Framework (0x1, 0x2, 0x3)
// Can load additional packages from mainnet
// Tracks gas usage and object mutations
```

### PTB (Programmable Transaction Block)

Sui transactions are expressed as PTBs:

```rust
let commands = vec![
    Command::MoveCall { package, module, function, type_arguments, arguments },
    Command::SplitCoins { coin, amounts },
    Command::TransferObjects { objects, address },
];

let result = env.execute_ptb(inputs, commands)?;
```

### Transaction Replay

Fetch a historical transaction and re-execute it:

```rust
// 1. Fetch transaction and state
let provider = HistoricalStateProvider::mainnet().await?;
let state = provider.fetch_replay_state(&digest).await?;

// 2. Get historical versions from transaction effects
let historical_versions = get_historical_versions(&state);

// 3. Prefetch dynamic fields recursively (checkpoint snapshot when available)
let prefetched = if let Some(cp) = state.checkpoint {
    prefetch_dynamic_fields_at_checkpoint(&graphql, &grpc, &rt, &historical_versions, 3, 200, cp)
} else {
    prefetch_dynamic_fields(&graphql, &grpc, &rt, &historical_versions, 3, 200)
};

// 4. Set up child fetcher for on-demand object loading
let child_fetcher = create_enhanced_child_fetcher_with_cache(
    grpc, graphql, historical_versions, prefetched, patcher, state.checkpoint, cache
);
harness.set_child_fetcher(child_fetcher);

// 5. Execute locally and compare
let result = replay_with_objects_and_aliases(&transaction, &mut harness, &objects, &aliases)?;
assert!(result.local_success);
```

### BigVector Handling

Some protocols (like DeepBook) use BigVector internally. BigVector slices may not appear
in `unchanged_loaded_runtime_objects`. Handle this with:

```rust
// 1. Prefetch discovers children via GraphQL (checkpoint snapshot when available)
let prefetched = if let Some(cp) = state.checkpoint {
    prefetch_dynamic_fields_at_checkpoint(&graphql, &grpc, &rt, &versions, 3, 200, cp)
} else {
    prefetch_dynamic_fields(&graphql, &grpc, &rt, &versions, 3, 200)
};

// 2. Enhanced child fetcher validates versions
// If object.version <= max_lamport_version, it's safe to use
let child_fetcher = create_enhanced_child_fetcher_with_cache(...);
```

---

## Troubleshooting

### "API key not configured" or connection errors

Create a `.env` file with your gRPC configuration:

```bash
cp .env.example .env
# Edit .env with your endpoint and API key
```

### Build errors

```bash
# Clean and rebuild
cargo clean && cargo build --examples

# If protoc errors, install protobuf:
# Ubuntu: sudo apt-get install protobuf-compiler
# macOS: brew install protobuf
```

### "sui CLI not found" (fork_state only)

The custom contract deployment in `fork_state` requires the Sui CLI. Install from <https://docs.sui.io/guides/developer/getting-started/sui-install>

The example still runs without it - just skips the custom contract part.

---

## Next Steps

After completing these examples:

- **[Transaction Replay Guide](../docs/guides/TRANSACTION_REPLAY.md)** - Deep dive into replay mechanics
- **[Architecture](../docs/ARCHITECTURE.md)** - Understand system internals
- **[Limitations](../docs/reference/LIMITATIONS.md)** - Known differences from mainnet
