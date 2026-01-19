# Local Bytecode Sandbox

## Overview

The **Local Bytecode Sandbox** is a deterministic, offline Move VM environment that enables testing type inhabitation without deploying to mainnet, testnet, or any Sui network. It provides a controlled execution context where:

1. **External package bytecode** can be loaded and executed locally
2. **Programmable Transaction Blocks (PTBs)** can be executed with full object tracking
3. **Transaction replay** enables validating against real mainnet transactions
4. **LLM-generated helper packages** can be compiled and tested against real bytecode
5. **Type inhabitation** can be verified through actual VM execution
6. **No gas, tokens, or network access** is required

This is the core infrastructure that powers the `benchmark-local`, `tx-replay`, `ptb-eval`, and `sandbox-exec` commands.

> **Architecture Reference**: For the full system architecture including all CLI commands and data flows, see [ARCHITECTURE.md](ARCHITECTURE.md).

## What Problem Does It Solve?

### The Challenge

To evaluate whether an LLM understands Move types well enough to construct valid function calls, we need to:

1. **Compile** LLM-generated Move code against target package interfaces
2. **Execute** the compiled bytecode to verify it actually runs
3. **Validate** that the target package code paths are exercised

Traditionally, this would require:

- Publishing packages to a Sui network
- Having funded accounts for gas
- Dealing with network latency and non-determinism
- Managing testnet/devnet state

### The Solution

The Local Bytecode Sandbox eliminates all of these requirements by:

- Loading bytecode directly from `.mv` files (no deployment)
- Executing in an embedded Move VM (no network)
- Using synthetic state for required objects (no real chain state)
- Providing deterministic execution (same input = same output)

## Architecture

The sandbox has a layered architecture with **SimulationEnvironment** as the central orchestrator:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Local Bytecode Sandbox                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────┐    ┌─────────────────┐    ┌─────────────────────────┐ │
│  │ Target Package  │    │ Helper Package  │    │ Sui Framework           │ │
│  │ Bytecode (.mv)  │    │ Bytecode (.mv)  │    │ (0x1, 0x2, 0x3)         │ │
│  │ from mainnet    │    │ LLM-generated   │    │ bundled at compile time │ │
│  └────────┬────────┘    └────────┬────────┘    └───────────┬─────────────┘ │
│           │                      │                         │               │
│           └──────────────────────┼─────────────────────────┘               │
│                                  │                                         │
│                                  ▼                                         │
│                    ┌─────────────────────────────┐                         │
│                    │    LocalModuleResolver      │                         │
│                    │    (unified bytecode index) │                         │
│                    └─────────────┬───────────────┘                         │
│                                  │                                         │
│                                  ▼                                         │
│                    ┌─────────────────────────────┐                         │
│                    │   SimulationEnvironment     │ ◄── Central orchestrator│
│                    │  • Object Store             │     for PTB execution   │
│                    │  • State Management         │                         │
│                    │  • Clock/Random Mocking     │                         │
│                    └─────────────┬───────────────┘                         │
│                                  │                                         │
│                                  ▼                                         │
│                    ┌─────────────────────────────┐                         │
│                    │       PTBExecutor           │ ◄── Programmable TX     │
│                    │  • MoveCall, SplitCoins     │     Block execution     │
│                    │  • TransferObjects, etc.    │                         │
│                    └─────────────┬───────────────┘                         │
│                                  │                                         │
│                                  ▼                                         │
│                    ┌─────────────────────────────┐                         │
│                    │        VMHarness            │                         │
│                    │   (Move VM + natives)       │                         │
│                    └─────────────┬───────────────┘                         │
│                                  │                                         │
│           ┌──────────────────────┼──────────────────────┐                  │
│           │                      │                      │                  │
│           ▼                      ▼                      ▼                  │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────────┐ │
│  │ Native Functions│  │ ObjectRuntime   │  │ Execution Trace             │ │
│  │ (permissive     │  │ (dynamic fields │  │ (which modules were loaded) │ │
│  │  crypto mocks)  │  │  via VM ext.)   │  │                             │ │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────────┘ │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```text

## Key Components

### 1. SimulationEnvironment (`src/benchmark/simulation.rs`)

The **central orchestrator** for all sandbox operations. It manages:

- **Object Store**: Tracks all simulated objects with types, BCS bytes, and ownership
- **Package Registry**: Uses LocalModuleResolver for module resolution
- **State Management**: Clock, Random, and transaction counter for deterministic execution
- **PTB Routing**: Delegates to PTBExecutor for Programmable Transaction Block execution

```rust
// Create environment
let mut env = SimulationEnvironment::new()?;
let mut env = SimulationEnvironment::with_resolver(resolver)?;

// Deploy packages
env.deploy_package(modules)?;

// Execute PTB
let result = env.execute_ptb(inputs, commands)?;

// Reset state between tests (keeps loaded modules)
env.reset_state()?;
```text

### 2. PTBExecutor (`src/benchmark/ptb.rs`)

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

### 3. LocalModuleResolver (`src/benchmark/resolver.rs`)

Loads and indexes bytecode from multiple sources:

- **Sui Framework**: Bundled at compile time (0x1 move-stdlib, 0x2 sui-framework, 0x3 sui-system)
- **Target Packages**: Loaded from local `.mv` files (real mainnet bytecode)
- **Helper Packages**: Compiled from LLM-generated Move source

```rust
let mut resolver = LocalModuleResolver::with_sui_framework()?;
resolver.load_from_dir(&bytecode_path)?;
resolver.add_package_modules(modules)?;
```text

### 4. VMHarness (`src/benchmark/vm.rs`)

Low-level Move VM wrapper that handles:

- Module loading from the resolver
- Native function registration
- VM extension support (for ObjectRuntime)
- Execution trace capture
- Gas metering

```rust
pub struct VMHarness<'a> {
    vm: MoveVM,
    storage: InMemoryStorage<'a>,
    // ...
}
```text

### 5. Native Functions (`src/benchmark/natives.rs`)

Implements Sui framework native functions with **permissive mocks** for testing:

| Category | Behavior | Examples |
|----------|----------|----------|
| **Real** | Actual implementation | `vector::*`, `bcs::*`, `hash::sha2_256` |
| **Mock** | Return placeholder values | `tx_context::sender` → `0x0` |
| **Permissive Crypto** | Always succeed | `ed25519::verify` → `true` |
| **Mock Clock** | Configurable timestamp | `clock::timestamp_ms` → config value |
| **Mock Random** | Deterministic from seed | `random::*` → seeded values |
| **VM Extension** | Full impl via ObjectRuntime | `dynamic_field::*` |

> **Note**: Crypto mocks are permissive by default (signatures always verify). Use `--strict-crypto` flag to disable this for production validation.

### 6. ObjectRuntime (`src/benchmark/object_runtime.rs`)

VM extension that enables dynamic field operations:

- Stores objects in a HashMap keyed by (parent, child_id)
- Wraps values in `GlobalValue` for proper reference semantics
- Supports add, borrow, borrow_mut, remove, and has operations

### 7. Execution Trace (`src/benchmark/vm.rs`)

Records which modules are loaded during execution:

```rust
pub struct ExecutionTrace {
    pub modules_accessed: BTreeSet<ModuleId>,
}
```text

This proves that target package code was actually exercised, not just framework code.

## Type Inhabitation Evaluation

### What We're Measuring

The sandbox measures **type inhabitation success**: can an LLM understand Move types well enough to construct values that satisfy those types and execute functions that use them?

This is different from semantic correctness. We're asking:

- Does the LLM understand struct layouts and field types?
- Can it chain constructors to build complex types?
- Does the generated code pass the Move type checker at runtime?

### Two-Tier Evaluation

| Tier | Name | What It Proves |
|------|------|----------------|
| **Tier A** | Preflight | Types resolve, BCS serialization works, layouts are valid |
| **Tier B** | Execution | Code runs in the VM without aborting |

**Tier B hit** = The LLM successfully inhabited the types. The code compiled, the types checked, and execution completed.

### Constructor Chaining

Many Sui types can only be created through constructor functions. The sandbox supports:

1. **Direct synthesis**: Primitives, vectors, simple structs
2. **Single-hop constructors**: Types created by calling one constructor
3. **Constructor chaining**: Multi-level dependencies (e.g., TreasuryCap requires OTW)

```rust
enum ConstructorChainEntry {
    Intermediate(ConstructorInfo),  // Dependency - result stored by type
    FinalParam { param_idx: usize, ctor: ConstructorInfo },  // Target param
}
```text

## Usage

### CLI Commands

The sandbox powers four CLI commands:

| Command | Description |
|---------|-------------|
| `benchmark-local` | Tier A/B type inhabitation testing |
| `tx-replay` | Fetch and replay mainnet transactions |
| `ptb-eval` | Evaluate PTBs with self-healing |
| `sandbox-exec` | Interactive sandbox for LLM agents |

### `benchmark-local` - Type Inhabitation Testing

```bash
# Tier A only (fast)
sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode \
  --output results.jsonl \
  --tier-a-only

# Full Tier A + B validation via SimulationEnvironment
sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode \
  --output results.jsonl \
  --use-ptb  # Use PTB execution path
```text

### `tx-replay` - Transaction Replay

Fetch and replay real mainnet transactions locally:

```bash
# Download recent transactions
sui_move_interface_extractor tx-replay \
  --recent 100 \
  --cache-dir .tx-cache \
  --download-only

# Replay from cache
sui_move_interface_extractor tx-replay \
  --cache-dir .tx-cache \
  --from-cache \
  --parallel
```text

### `ptb-eval` - Self-Healing Evaluation

Evaluate cached transactions with automatic error recovery:

```bash
sui_move_interface_extractor ptb-eval \
  --cache-dir .tx-cache \
  --max-retries 3 \
  --enable-fetching  # Fetch missing packages from mainnet
```text

### `sandbox-exec` - LLM Agent Interface

JSON-based interface for AI agents:

```bash
# Interactive mode
sui_move_interface_extractor sandbox-exec --interactive

# Example request
echo '{"action": "execute_ptb", "inputs": [...], "commands": [...]}' | \
  sui_move_interface_extractor sandbox-exec --input - --output -
```text

### E2E Pipeline (with LLM)

```bash
# LLM generates helper package, sandbox validates execution
python scripts/e2e_one_package.py \
  --corpus-root /path/to/packages \
  --package-id 0x... \
  --model google/gemini-2.0-flash-001 \
  --out-dir results/
```text

See [CLI_REFERENCE.md](CLI_REFERENCE.md) for complete command documentation.

## Tradeoffs and Limitations

### What Works Well

| Feature | Status | Notes |
|---------|--------|-------|
| Type system | Full | Complete Move type checking |
| BCS serialization | Full | Real implementation |
| Vector operations | Full | Real move-stdlib |
| Dynamic fields | Full | Via ObjectRuntime VM extension |
| Constructor chaining | Full | Single-hop + multi-level |
| Execution tracing | Full | Module-level granularity |

### What's Mocked

| Feature | Behavior | Impact |
|---------|----------|--------|
| `tx_context::sender` | Returns `0x0` | Sender-based logic uses placeholder |
| `transfer::*` | No-op | No ownership tracking |
| Object persistence | Per-session only | Objects don't survive between calls |
| `event::emit` | No-op | Events not captured |

### What's Configurable

| Feature | Default | Flag | Notes |
|---------|---------|------|-------|
| Crypto verification | Permissive (always pass) | `--strict-crypto` | Disable for production validation |
| Clock timestamp | Configurable base | Config | Set via `SimulationConfig.clock_base_ms` |
| Random seed | Configurable | Config | Set via `SimulationConfig.random_seed` |

### What's Unsupported

| Feature | Behavior | Why |
|---------|----------|-----|
| Shared object locking | Simplified | No full consensus model (partial support in PTBExecutor) |
| Epochs/checkpoints | Not modeled | Would require chain state |
| Object versioning | Not tracked | Simplified object store |

## Interpreting Results

### Failure Taxonomy (Primary Metric)

The key metric is **failure distribution by stage**, not a single pass rate. Each failure stage reveals different information about LLM capability:

| Stage | Name | What Failure Indicates |
|-------|------|------------------------|
| **A1** | Target Resolution | Function/module doesn't exist in bytecode |
| **A2** | Type Layout | Unknown struct, recursive type, or unresolvable generic |
| **A3** | Type Synthesis | No constructor path to create required type |
| **A4** | (Reserved) | — |
| **A5** | Type Parameters | Generic type parameter bounds violation |
| **B1** | Constructor Execution | Dependency constructor aborted |
| **B2** | Target Execution | Function aborted during execution |

### Why Failure Distribution Matters

A single "pass rate" obscures important distinctions:

- **A3 failures** (no constructor) indicate a **synthesizability ceiling**—the sandbox can't create certain types regardless of LLM capability
- **B2 failures from unsupported natives** (error 1000) are **expected boundaries**, not LLM failures
- **B2 failures from assertions** indicate the LLM generated code that violates runtime invariants

For researchers evaluating LLMs, the question is: **where in the taxonomy do failures cluster?**

### Example Interpretation

```text
Benchmark results for package X:
  A1: 0%   → All targets found (good corpus)
  A2: 3%   → Some types unresolvable (complex generics)
  A3: 8%   → Constructor ceiling (need deeper chaining?)
  B1: 2%   → Constructor runtime issues
  B2: 12%  → Execution failures
    - 9% unsupported natives (expected)
    - 3% assertion failures (LLM issue)

  Tier B hits: 75%
```text

This tells a richer story than "75% pass rate":

- The sandbox has an ~11% synthesizability ceiling (A2+A3)
- 9% of functions use crypto/random (expected B2)
- Only 3% represent actual LLM type understanding failures

### Tier B Hit

A **Tier B hit** means:

1. All argument types were successfully synthesized (directly or via constructors)
2. BCS serialization round-tripped correctly
3. The Move VM executed the function without aborting
4. Target package modules were loaded (verified via execution trace)

This proves the LLM understood the types well enough to construct valid inhabitants.

## Design Principles

### 1. Determinism First

Same bytecode + same input = same output. No randomness, no network calls, no system time dependencies.

### 2. Real VM, Real Type System

We use the actual Move VM from `move-vm-runtime`. Type checking is real, not simulated.

### 3. Minimal Mocking

Only mock what's necessary:

- Native functions that require external state
- Transaction context fields
- Object lifecycle operations

Everything else uses real implementations.

### 4. Verification via Tracing

Don't trust that code executed correctly—verify it by tracing module loads. If target package modules weren't loaded, the LLM didn't actually exercise the target code.

## Files

### Core Sandbox

| File | Purpose |
|------|---------|
| `src/benchmark/simulation.rs` | **SimulationEnvironment** - Central orchestrator |
| `src/benchmark/ptb.rs` | **PTBExecutor** - Programmable Transaction Block execution |
| `src/benchmark/vm.rs` | VMHarness, InMemoryStorage, ExecutionTrace |
| `src/benchmark/natives.rs` | Native function implementations (permissive mocks) |
| `src/benchmark/object_runtime.rs` | Dynamic field VM extension |
| `src/benchmark/resolver.rs` | LocalModuleResolver - Module loading and resolution |

### CLI Commands

| File | Purpose |
|------|---------|
| `src/benchmark/runner.rs` | `benchmark-local` - Benchmark orchestration |
| `src/benchmark/tx_replay.rs` | `tx-replay` - Transaction fetching and replay |
| `src/benchmark/ptb_eval.rs` | `ptb-eval` - Self-healing PTB evaluation |
| `src/benchmark/sandbox_exec.rs` | `sandbox-exec` - LLM sandbox interface |

### Supporting Infrastructure

| File | Purpose |
|------|---------|
| `src/benchmark/constructor_map.rs` | Constructor discovery and chaining |
| `src/benchmark/validator.rs` | Type layout resolution |
| `src/benchmark/llm_tools.rs` | LLM tool definitions for sandbox-exec |
| `src/benchmark/package_builder.rs` | Move package compilation |
| `src/benchmark/mm2/` | Type model and validation |

## See Also

- [ARCHITECTURE.md](ARCHITECTURE.md) - Full system architecture and data flows
- [CLI_REFERENCE.md](CLI_REFERENCE.md) - Complete CLI command reference
- [NO_CHAIN_TYPE_INHABITATION_SPEC.md](NO_CHAIN_TYPE_INHABITATION_SPEC.md) - Technical specification
- [METHODOLOGY.md](METHODOLOGY.md) - Scoring and research methodology
