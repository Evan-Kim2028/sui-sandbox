# Architecture Overview

This document describes the architecture of the sui-move-interface-extractor project, focusing on how the components integrate to provide a comprehensive Move VM sandbox environment.

## Design Philosophy

### Single API Surface

All LLM interactions go through **`SandboxRequest`** in `sandbox_exec.rs`. This is the canonical interface:

- **One entry point**: `execute_request(SandboxRequest)` handles all operations
- **One schema**: `{"action": "list_available_tools"}` returns complete tool documentation
- **One state**: All operations share `SimulationEnvironment` state

The legacy `ToolCall`/`LlmToolkit` in `llm_tools.rs` is deprecated and should not be used for new integrations.

### Neutral and Unopinionated

The sandbox API is intentionally neutral to enable unbiased LLM evaluation:

- **No recovery hints**: Errors describe what happened, not how to fix it
- **No usage suggestions**: Tool descriptions explain capabilities, not strategies
- **No tips or guidance**: LLMs must reason about solutions independently

This ensures experiments measure actual LLM reasoning, not instruction-following.

### Stateful Execution

All operations share state through `SimulationEnvironment`:

- Loading a module makes it available for all subsequent operations
- Creating an object makes it usable in PTB execution
- State persists across requests within a session
- Use `reset` to clear state between independent tests

## System Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CLI Commands                                    │
├──────────────┬──────────────┬──────────────┬──────────────────────────────────┤
│ benchmark-   │  tx-replay   │  ptb-eval    │  sandbox-exec                   │
│ local        │              │              │                                  │
└──────┬───────┴──────┬───────┴──────┬───────┴──────────────┬─────────────────┘
       │              │              │                      │
       │              │              │                      │
       ▼              ▼              ▼                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                        SimulationEnvironment                                 │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  • Object Store (create/mutate/delete tracking)                      │   │
│  │  • Package Registry (LocalModuleResolver)                            │   │
│  │  • Clock & Random mocking for deterministic execution                │   │
│  │  • Dynamic Fields (Tables, Bags)                                     │   │
│  │  • Shared Object Locking                                             │   │
│  │  • State Persistence                                                 │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                                    ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                         PTBExecutor                                  │   │
│  │  • MoveCall, SplitCoins, MergeCoins, TransferObjects                │   │
│  │  • Publish, Upgrade, MakeMoveVec, Receive                           │   │
│  │  • Result chaining between commands                                  │   │
│  │  • TransactionEffects computation                                    │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                                    ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                          VMHarness                                   │   │
│  │  • Move VM execution                                                 │   │
│  │  • Native function dispatch                                          │   │
│  │  • Gas metering                                                      │   │
│  │  • Execution tracing                                                 │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                         │
│                                    ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                       Native Functions                               │   │
│  │  • Permissive crypto mocks (signatures pass)                        │   │
│  │  • MockClock, MockRandom for determinism                            │   │
│  │  • Dynamic field operations                                          │   │
│  │  • Event emission                                                    │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Core Components

### SimulationEnvironment (`src/benchmark/simulation.rs`)

The **SimulationEnvironment** is the central orchestrator for all sandbox operations. It provides:

- **Object Store**: Tracks all simulated objects with their types, BCS bytes, and ownership
- **Package Registry**: Manages loaded packages via `LocalModuleResolver`
- **State Management**: Clock, Random, and transaction counter for deterministic execution
- **PTB Execution**: Routes through `PTBExecutor` for Programmable Transaction Block support

Key methods:
```rust
// Create environment
let mut env = SimulationEnvironment::new()?;
let mut env = SimulationEnvironment::with_resolver(resolver)?;

// Deploy packages
env.deploy_package(modules)?;
env.deploy_package_from_mainnet("0x...")?;

// Execute PTB
let result = env.execute_ptb(inputs, commands);

// Reset state between tests (keeps loaded modules)
env.reset_state()?;
```

### PTBExecutor (`src/benchmark/ptb.rs`)

Executes Programmable Transaction Blocks with full command support:

| Command | Description |
|---------|-------------|
| `MoveCall` | Call a Move function with type args and arguments |
| `SplitCoins` | Split a coin into multiple coins |
| `MergeCoins` | Merge multiple coins into one |
| `TransferObjects` | Transfer objects to a recipient |
| `MakeMoveVec` | Create a vector from elements |
| `Publish` | Publish new package modules |
| `Upgrade` | Upgrade existing package |
| `Receive` | Receive objects from pending transfers |

The executor tracks:
- **Result chaining**: `Argument::Result(n)` references previous command outputs
- **Mutable references**: Updates propagate through command chain
- **Object lifecycle**: Created, mutated, deleted, wrapped, unwrapped objects

### LocalModuleResolver (`src/benchmark/resolver.rs`)

Loads and resolves Move modules from bytecode:

```rust
let mut resolver = LocalModuleResolver::with_sui_framework()?;
resolver.load_from_dir(&bytecode_path)?;
resolver.add_package_modules(modules)?;
```

Features:
- **Sui Framework**: Auto-loads 0x1, 0x2, 0x3 framework modules
- **Multi-package**: Supports loading packages from multiple sources
- **Address aliases**: Maps upgraded package addresses

### VMHarness (`src/benchmark/vm.rs`)

Low-level Move VM wrapper:

- Creates and manages Move VM instances
- Dispatches native function calls
- Tracks execution trace (modules accessed)
- Implements gas metering

### Native Function Mocks (`src/benchmark/natives.rs`)

Provides permissive implementations for Sui native functions:

| Category | Behavior |
|----------|----------|
| **Crypto** | Signature verification always passes |
| **Clock** | Returns configurable timestamp |
| **Random** | Returns deterministic values from seed |
| **Dynamic Fields** | Full Table/Bag support |
| **Events** | Captured for inspection |

## CLI Commands

### `benchmark-local`

Type inhabitation testing with two tiers:

- **Tier A (Preflight)**: Type resolution, BCS serialization, layout validation
- **Tier B (Execution)**: Full VM execution

When `--use-ptb` is enabled, execution flows through SimulationEnvironment for consistent semantics.

### `tx-replay`

Transaction replay from Sui mainnet:

1. **Fetch**: Download transactions via RPC
2. **Cache**: Store as JSON for offline replay
3. **Replay**: Execute locally via SimulationEnvironment
4. **Validate**: Compare with on-chain effects

```
Mainnet RPC → TransactionFetcher → CachedTransaction → PTB Commands → SimulationEnvironment
```

### `ptb-eval`

Self-healing PTB evaluation:

1. Load cached transactions
2. Attempt execution via SimulationEnvironment
3. On failure, diagnose error (missing package, missing object, etc.)
4. Apply healing action (deploy from mainnet, create object)
5. Retry execution

Healing actions:
- `DeployPackage`: Fetch and deploy missing package
- `CreateObject`: Synthesize missing object
- `SetupSharedObject`: Initialize shared object state

### `sandbox-exec`

JSON-based interface for LLM agents:

```json
// Request
{"action": "execute_ptb", "inputs": [...], "commands": [...]}

// Response
{"success": true, "effects": {...}, "return_values": [...]}
```

Supports 30+ actions including:
- Module introspection (`list_modules`, `list_functions`, `get_function_info`)
- Type operations (`validate_type`, `encode_bcs`, `decode_bcs`)
- Execution (`execute_ptb`, `call_function`)
- Package building (`compile_move`)

## Data Flow

### Type Inhabitation Testing

```
Package Bytecode
       │
       ▼
LocalModuleResolver ──► TypeModel ──► ConstructorGraph
       │                                     │
       ▼                                     ▼
   Validator           Constructor chains found
       │                                     │
       ▼                                     ▼
Tier A: Synthesis ◄─────────────────────────┘
       │
       ▼
Tier B: Execution via SimulationEnvironment
       │
       ▼
BenchmarkReport (JSONL)
```

### Transaction Replay

```
Mainnet TX Digest
       │
       ▼
TransactionFetcher.fetch_transaction()
       │
       ▼
FetchedTransaction (parsed commands, inputs, packages)
       │
       ▼
TransactionCache.save() ──► .tx-cache/
       │
       ▼
build_ptb_from_transaction()
       │
       ▼
SimulationEnvironment.execute_ptb()
       │
       ▼
Compare: local effects vs on-chain effects
```

### LLM Sandbox Interaction

```
Python LLM Agent
       │
       ▼ (subprocess, JSON)
sandbox-exec CLI
       │
       ▼
execute_request()
       │
       ▼
SimulationEnvironment
       │
       ├──► Module introspection (resolver queries)
       ├──► PTB execution (execute_ptb)
       ├──► Object creation (create_object)
       └──► Package compilation (package_builder)
       │
       ▼
JSON Response
       │
       ▼
Python LLM Agent (parse, decide next action)
```

## Configuration

### SimulationConfig

```rust
SimulationConfig {
    mock_crypto_pass: bool,    // Crypto operations always succeed
    clock_base_ms: u64,        // Base timestamp for Clock
    random_seed: [u8; 32],     // Seed for deterministic Random
}
```

### CLI Flags

| Flag | Command | Description |
|------|---------|-------------|
| `--use-ptb` | benchmark-local | Use PTB execution via SimulationEnvironment |
| `--strict-crypto` | benchmark-local | Disable permissive crypto mocks |
| `--enable-fetching` | ptb-eval, sandbox-exec | Fetch missing packages from mainnet |
| `--interactive` | sandbox-exec | JSON lines mode for LLM agents |
| `--parallel` | tx-replay | Parallel replay with rayon |

## Error Handling

### Failure Taxonomy

| Phase | Code | Description |
|-------|------|-------------|
| Resolution | A1 | Target function not found |
| TypeCheck | A2 | Type parameter resolution failed |
| Synthesis | A3-A5 | Argument synthesis failed |
| Execution | B1 | Constructor execution failed |
| Validation | B2 | Target function aborted |

### SimulationError

```rust
enum SimulationError {
    MissingPackage { address, module },
    MissingObject { id, expected_type },
    TypeMismatch { expected, got, location },
    ContractAbort { location, code },
    ExecutionError { message },
    // ...
}
```

## File Organization

```
src/
├── benchmark/
│   ├── simulation.rs      # SimulationEnvironment - central state manager
│   ├── sandbox_exec.rs    # SandboxRequest API - canonical LLM interface
│   ├── ptb.rs             # PTBExecutor, PTBBuilder
│   ├── vm.rs              # VMHarness
│   ├── natives.rs         # Native function mocks
│   ├── resolver.rs        # LocalModuleResolver
│   ├── runner.rs          # Benchmark runner (Tier A/B)
│   ├── tx_replay.rs       # Transaction fetching/replay
│   ├── ptb_eval.rs        # Self-healing evaluation
│   ├── llm_tools.rs       # LEGACY: ToolCall/LlmToolkit (use sandbox_exec.rs)
│   ├── package_builder.rs # Move compilation
│   └── mm2/               # Type model and validation
├── args.rs                # CLI argument parsing
└── main.rs                # Command routing
```

## See Also

- [CLI_REFERENCE.md](CLI_REFERENCE.md) - Complete CLI command reference
- [LOCAL_BYTECODE_SANDBOX.md](LOCAL_BYTECODE_SANDBOX.md) - Sandbox internals
- [design/local-move-vm-sandbox.md](design/local-move-vm-sandbox.md) - Design document
