# Architecture

Technical overview of the sui-sandbox system internals.

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
| `SimulationEnvironment` | `benchmark/simulation.rs` | State management, object store |
| `PTBExecutor` | `benchmark/ptb.rs` | PTB command execution |
| `VMHarness` | `benchmark/vm.rs` | Move VM configuration |
| `ModuleBytecodeResolver` | `benchmark/resolver.rs` | Package loading and address aliasing |
| `tx_replay` | `benchmark/tx_replay.rs` | Transaction replay orchestration |

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
├── src/                        # Main library
│   ├── benchmark/              # Core simulation engine
│   └── data_fetcher.rs         # Unified data fetching API
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
- [Sandbox API Reference](reference/SANDBOX_API.md) - API details
- [Limitations](reference/LIMITATIONS.md) - Known differences from mainnet
