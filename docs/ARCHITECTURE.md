# Architecture

Technical overview of the sui-sandbox system internals.

## System Boundary

`sui-sandbox` is an execution and replay harness around the Sui Move VM.

Included in scope:

- PTB command execution semantics and kernel validation
- VM harnessing, native/runtime integration, and gas/effects accounting
- Historical state hydration from Walrus/gRPC/JSON and comparison tooling
- Local object/package stores and deterministic simulation workflows

Out of scope (fullnode/validator responsibilities):

- Consensus and checkpoint production pipelines
- Mempool/transaction admission and networking
- Authority state services and long-running node RPC behavior

## Core Components

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Your Application                            │
│                   (CLI, Scripts, LLM Orchestrator)                  │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      SimulationEnvironment                          │
│                        (simulation.rs)                              │
│                                                                     │
│  - In-memory object store                                           │
│  - Tracks events, effects, gas                                      │
│  - Provides introspection APIs                                      │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         PTBExecutor                                 │
│                           (ptb.rs)                                  │
│                                                                     │
│  - Executes Programmable Transaction Blocks                         │
│  - Chains commands (MoveCall, TransferObjects, SplitCoins, etc.)   │
│  - Tracks object mutations and results                              │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          VMHarness                                  │
│                           (vm.rs)                                   │
│                                                                     │
│  - Wraps the real Move VM (move-vm-runtime)                        │
│  - Configures simulation behavior (clock, gas, randomness)         │
│  - Dispatches native function calls                                 │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          Move VM                                    │
│                    (move-vm-runtime)                                │
│                                                                     │
│  - Bytecode execution                                               │
│  - Native function dispatch                                         │
│  - Gas metering                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

## Key Modules

| Module | File | Purpose |
|--------|------|---------|
| `SimulationEnvironment` | `crates/sui-sandbox-core/src/simulation/` | State management, object store |
| `PTBExecutor` | `crates/sui-sandbox-core/src/ptb.rs` | PTB command execution |
| `VMHarness` | `crates/sui-sandbox-core/src/vm.rs` | Move VM configuration |
| `LocalModuleResolver` | `crates/sui-sandbox-core/src/resolver.rs` | Package loading and address aliasing |
| `tx_replay` | `crates/sui-sandbox-core/src/tx_replay.rs` | Transaction replay orchestration |
| `WorkflowSpec` | `crates/sui-sandbox-core/src/workflow.rs` | Typed replay/analyze workflow contract |
| `checkpoint_discovery` | `crates/sui-sandbox-core/src/checkpoint_discovery.rs` | Shared digest/checkpoint target discovery planner |
| `workflow_planner` | `crates/sui-sandbox-core/src/workflow_planner.rs` | Shared workflow auto-inference, command planning, and profile parsing |
| `workflow_command_builder` | `crates/sui-sandbox-core/src/workflow_command_builder.rs` | Shared replay/analyze argv builders for typed workflow steps |
| `context_contract` | `crates/sui-sandbox-core/src/context_contract.rs` | Portable context JSON contract used by CLI + Python |
| `replay_support` | `crates/sui-sandbox-core/src/replay_support.rs` | Shared replay hydration/runtime wiring used by CLI + Python |
| `replay_reporting` | `crates/sui-sandbox-core/src/replay_reporting.rs` | Shared replay analysis, diagnostics, classification |
| `health` | `crates/sui-sandbox-core/src/health.rs` | Shared doctor checks + endpoint preflight logic |
| `environment_bootstrap` | `crates/sui-sandbox-core/src/environment_bootstrap.rs` | Generic package/object hydration + local environment initialization |

## PTB Control Flow (Function-Level)

### Direct `ptb` command path

1. CLI dispatch in `src/bin/sui_sandbox.rs`.
2. `src/bin/sandbox_cli/ptb.rs`:
   - `PtbCmd::execute_inner`
   - `read_ptb_spec`
   - `convert_spec`
   - `validate_ptb`
3. Harness + executor setup:
   - `state.create_harness_with_sender(...)`
   - `PTBExecutor::new`
4. Kernel execution:
   - `PTBExecutor::execute_commands`
   - command handlers (`MoveCall`, `SplitCoins`, `MergeCoins`, `TransferObjects`, `MakeMoveVec`)
5. VM invocation:
   - `VMHarness::execute_function*`
   - Move VM session with native extensions
6. Output:
   - `TransactionEffects` formatting in CLI.

### `replay` command path

1. CLI dispatch in `src/bin/sui_sandbox.rs`.
2. Replay hydration and resolver setup in `src/bin/sandbox_cli/replay.rs`.
3. Config creation via `build_simulation_config` and harness creation via `VMHarness::with_config`.
4. Replay execution via:
   - `tx_replay::replay_with_version_tracking_with_policy_with_effects`
5. Core replay execution in `crates/sui-sandbox-core/src/tx_replay.rs`:
   - PTB conversion
   - `PTBExecutor::new`
   - `PTBExecutor::execute_commands`
6. Optional effects comparison against on-chain effects.

## Transaction Replay Pipeline

```
1. FETCH                    2. PREFETCH                  3. EXECUTE
   ─────────────────────       ─────────────────────       ─────────────────────
   GrpcClient                  GroundTruthPrefetch         PTBExecutor
   ↓                           ↓                           ↓
   • Transaction by digest     • Collect object IDs        • Convert to commands
   • Transaction effects       • Fetch at historical       • Execute in Move VM
                                 versions                  • Track effects

4. COMPARE
   ─────────────────────
   EffectsComparison
   ↓
   • Created objects match?
   • Mutated objects match?
   • Status matches?
```

**Critical insight**: Objects must be fetched at their *input* versions (before the transaction modified them). The `unchanged_loaded_runtime_objects` field from gRPC provides this.

## Runtime Modes and Parity Tradeoffs

The VM harness supports two runtime/native modes:

- Default mode (`use_sui_natives = false`): custom sandbox runtime/native path (faster iteration, broad compatibility).
- Sui-native mode (`use_sui_natives = true`): uses Sui native object runtime path for highest parity.

Current CLI replay/PTB flows build from `SimulationConfig::default()` unless explicitly changed in code, so default command behavior uses the sandbox runtime path.

## CLI Boundary: Replay vs Analyze

`replay` and `analyze replay` intentionally share the same hydration contract so data loading behavior is consistent across execution and introspection flows.

- Shared hydration contract: `src/bin/sandbox_cli/replay.rs` (`ReplayHydrationArgs`)
- Hydration implementation: `src/bin/sandbox_cli/replay/hydration.rs`
- Execution path: `src/bin/sandbox_cli/replay.rs`
- Introspection path: `src/bin/sandbox_cli/analyze/replay_cmd.rs`

Design goal: keep default replay/analyze paths dependency-light and deterministic.
Legacy igloo integration source remains in-repo for internal use and future extraction work:
`src/bin/sandbox_cli/replay/igloo.rs`.

## Unified Orchestration Surfaces

Canonical user-facing orchestration commands are:

- `context` (alias: `flow`) for package-context prepare + replay execution.
- `adapter` (alias: `protocol`) for protocol-labeled wrappers over generic context flows.
- `pipeline` (alias: `workflow`) for typed multi-step replay/analyze automation.

All three surfaces consume shared core modules (planner/hydration/reporting) so Rust CLI and Python bindings stay behaviorally aligned.

## Typed Workflow Layer

`sui-sandbox pipeline` (alias: `workflow`) adds a typed orchestration layer above direct CLI invocations:

- Spec schema and validation live in `crates/sui-sandbox-core/src/workflow.rs`.
- Step execution/report loop lives in `crates/sui-sandbox-core/src/workflow_runner.rs`.
- Shared auto-planner helpers (template inference, replay profile/fetch parsing, command shaping) live in `crates/sui-sandbox-core/src/workflow_planner.rs`.
- CLI adapter lives in `src/bin/sandbox_cli/workflow.rs`.
- Native Python adapter lives in `crates/sui-python/src/lib.rs` with focused helper modules (for example `workflow_native.rs`, `workflow_api.rs`, `session_api.rs`, `transport_helpers.rs`, `replay_output.rs`, `replay_api.rs`, `replay_core.rs`) and no shell passthrough.
- Step kinds currently supported: `replay`, `analyze_replay`, and `command`.

Design intent:

- Reuse existing stable command implementations (`replay`, `analyze replay`) instead of duplicating logic.
- Keep the workflow contract protocol-agnostic so future adapters (Suilend/Cetus/Scallop/etc.) can compile into the same step model.
- Allow higher-level tooling (including Python bindings) to emit one workflow spec and delegate heavy lifting to Rust execution paths.

## Data Fetching

See [Prefetching Architecture](architecture/PREFETCHING.md) for the three-layer prefetch strategy.

| Layer | Module | Purpose |
|-------|--------|---------|
| Ground Truth | `eager_prefetch.rs` | Fetch objects from transaction effects |
| Predictive | `predictive_prefetch.rs` | Bytecode analysis for dynamic fields |
| On-Demand | `object_runtime.rs` | Fallback during execution |

## Error Handling

Errors are structured for programmatic handling:

```rust
enum SimulationError {
    MissingPackage { address, module },
    MissingObject { id, expected_type },
    ContractAbort { abort_code, module, function, message },
    TypeMismatch { expected, actual },
    DeserializationFailed { argument_index, expected_type },
    ExecutionError { message },
}
```

See [Error Codes Reference](reference/ERROR_CODES.md) for details.

## Crate Organization

```
sui-sandbox/
├── src/
│   └── bin/
│       ├── sui_sandbox.rs      # CLI entrypoint + command dispatch
│       └── sandbox_cli/        # CLI command modules (replay/analyze/etc.)
├── crates/
│   ├── sui-sandbox-core/       # Core VM and simulation with utilities
│   ├── sui-transport/          # Network layer (gRPC + GraphQL clients)
│   │   ├── graphql.rs          # GraphQL client
│   │   └── grpc/               # gRPC streaming client
│   ├── sui-prefetch/           # Strategic data loading
│   │   ├── eager_prefetch.rs   # Ground truth prefetch
│   │   ├── conversion.rs       # gRPC to FetchedTransaction
│   │   └── utilities.rs        # Prefetch utilities
│   ├── sui-resolver/           # Resolution & normalization
│   ├── sui-package-extractor/  # Bytecode analysis
│   └── sui-types/              # Shared types
└── examples/                   # Self-contained replay examples
```

## See Also

- **[Examples](../examples/README.md)** - Start here to learn the library
- [Transaction Replay Guide](guides/TRANSACTION_REPLAY.md) - End-to-end workflow
- [Prefetching Architecture](architecture/PREFETCHING.md) - Data fetching internals
- [Workflow Engine Contract](architecture/WORKFLOW_ENGINE.md) - Typed workflow orchestration layer
- [Shell-to-Rust Example Migration Plan](architecture/SHELL_TO_RUST_MIGRATION_PLAN.md) - Script consolidation roadmap
- [Limitations](reference/LIMITATIONS.md) - Known differences from mainnet
