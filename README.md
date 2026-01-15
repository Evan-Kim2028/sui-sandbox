# sui-move-interface-extractor

A **Move VM sandbox** that lets LLMs safely construct and test blockchain transactions without real tokens or network access.

## The Core Idea

This project solves a fundamental problem: **How can an LLM learn to interact with a blockchain through experimentation?**

The answer is a feedback loop:

```
LLM constructs transaction → Sandbox executes it → Structured error → LLM adjusts → repeat
```

The sandbox is deliberately **passive and minimal**. It doesn't orchestrate, doesn't automatically fix things, doesn't have retry limits. It just:

1. Takes a Programmable Transaction Block (PTB)
2. Executes it in a real Move VM (offline)
3. Returns structured results or errors

This lets an LLM learn Sui's semantics empirically rather than needing perfect knowledge upfront.

## The Three Pillars

### 1. Type Synthesis

*The intellectually interesting part.*

Given a Move function signature like:

```move
public fun stake<T: store + copy>(pool: &mut StakingPool, coin: Coin<T>, ctx: &mut TxContext)
```

How do you generate valid arguments to call it? The type synthesizer:

- **Inhabits generic types** - finds concrete types satisfying trait bounds (`store + copy`)
- **Constructs nested structs** - builds complex objects field-by-field using BCS serialization
- **Chains constructors** - if type A needs type B, finds a path: `create_b() → use_b_to_make_a()`
- **Handles framework types** - knows `UID`, `Balance<T>`, `Coin<T>`, `TxContext` layouts

See [`src/benchmark/mm2/type_synthesizer.rs`](src/benchmark/mm2/type_synthesizer.rs) for implementation.

### 2. The Simulation Environment

The execution core wrapping the **real Sui Move VM**:

```
┌─────────────────────────────────────────────────────────────────┐
│                    SimulationEnvironment                         │
│  • Object store management (simulated state)                    │
│  • PTB execution (MoveCall, SplitCoins, Transfer, etc.)         │
│  • Gas metering and effects tracking                            │
└─────────────────────────────────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                   move_vm_runtime::MoveVM                        │
│  • Real bytecode execution                                      │
│  • Real type checking (generics, abilities, phantoms)           │
│  • Real BCS serialization                                       │
└─────────────────────────────────────────────────────────────────┘
```

**What's real:** Type system, abilities, struct layouts, BCS, dynamic fields, all 8 PTB commands.

**What's mocked:** Crypto verification (returns true), clock/random (deterministic), gas (estimated).

### 3. Structured Error Feedback

This is what makes the sandbox *useful for LLMs*. Instead of cryptic VM panics, errors are structured and parseable:

```rust
SimulationError::MissingPackage {
    address: "0x123...",
    module: Some("my_module"),
}

SimulationError::TypeMismatch {
    expected: "0x2::coin::Coin<0x2::sui::SUI>",
    got: "0x2::coin::Coin<0xabc::token::TOKEN>",
    location: "argument 2",
}

SimulationError::ContractAbort {
    abort_code: 1,
    module: "0x2::coin",
    function: "split",
    message: None,
}
```

Errors report facts without prescribing solutions.

## Quick Start

```bash
# Build
cargo build --release

# Interactive sandbox (for LLM integration)
./target/release/sui_move_interface_extractor sandbox-exec --interactive

# Example: List available functions in a module
echo '{"action": "list_functions", "package_id": "0x2", "module": "coin"}' | \
  ./target/release/sui_move_interface_extractor sandbox-exec --input - --output -
```

## What the Sandbox Does NOT Do

| Responsibility | Who Handles It |
|----------------|----------------|
| Counting attempts/rounds | External orchestrator |
| Time limits | External orchestrator |
| Deciding when LLM is "done" | External orchestrator |
| Fetching missing dependencies | LLM explicitly calls `deploy_package_from_mainnet` |
| Modifying PTBs to fix errors | LLM reads errors and adjusts |

The sandbox is a **passive tool**. All intelligence lives in the orchestrator/LLM.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         External System                              │
│                   (LLM Orchestrator / Test Runner)                   │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   │ JSON over stdin/stdout
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        sandbox_exec.rs                               │
│                     (50+ operations via JSON API)                    │
│                                                                      │
│  execute_ptb, validate_ptb, load_module, create_object,             │
│  list_functions, get_function_info, deploy_from_mainnet...          │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      SimulationEnvironment                           │
│                        (simulation.rs)                               │
└─────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          Move VM                                     │
│                    (move-vm-runtime)                                 │
└─────────────────────────────────────────────────────────────────────┘
```

## Documentation

| Category | Documents |
|----------|-----------|
| **Getting Started** | [Quickstart](docs/getting-started/QUICKSTART.md) · [Troubleshooting](docs/getting-started/TROUBLESHOOTING.md) |
| **Guides** | [LLM Integration](docs/guides/LLM_INTEGRATION.md) · [Running Benchmarks](docs/guides/RUNNING_BENCHMARKS.md) · [Transaction Replay](docs/guides/TRANSACTION_REPLAY.md) |
| **Reference** | [CLI Reference](docs/reference/CLI_REFERENCE.md) · [Sandbox API](docs/reference/SANDBOX_API.md) · [Error Codes](docs/reference/ERROR_CODES.md) |
| **Architecture** | [System Architecture](ARCHITECTURE.md) · [Methodology](docs/METHODOLOGY.md) |

## CLI Commands

| Command | Purpose |
|---------|---------|
| `sandbox-exec` | Interactive JSON API for LLM agents |
| `benchmark-local` | Type inhabitation testing (Tier A/B validation) |
| `tx-replay` | Replay mainnet transactions locally |
| `ptb-eval` | Evaluate PTB execution with dependency fetching |

## Use Cases

### LLM Transaction Building

An LLM explores what functions are available, introspects their signatures, builds PTBs, and iterates based on structured error feedback:

```
1. list_modules()           → ["coin", "transfer", "object", ...]
2. list_functions("coin")   → ["split", "merge", "zero", ...]
3. get_function_info(...)   → signature, type params, docs
4. execute_ptb(...)         → Success or structured error
5. (If error) adjust and retry
```

### Benchmarking LLM Capabilities

Measure how well an LLM understands Move types:

```bash
cd benchmark
uv run smi-inhabit \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --dataset type_inhabitation_top25 \
  --agent real-openai-compatible
```

### Transaction Replay

Validate sandbox accuracy by replaying real mainnet transactions:

```bash
./target/release/sui_move_interface_extractor tx-replay \
  --recent 100 --cache-dir .tx-cache --parallel
```

## Installation

```bash
# Prerequisites: Rust 1.75+, Python 3.11+ with uv

# Clone and build
git clone https://github.com/your-org/sui-move-interface-extractor.git
cd sui-move-interface-extractor
cargo build --release

# Verify
./target/release/sui_move_interface_extractor --help
```

## Fidelity

**~95% accurate for type inhabitation testing.** The real Move VM ensures type checking, abilities, and struct layouts are correct. Mocked natives allow execution without real signatures or on-chain state.

**Not suitable for:** Production security validation, cryptographic correctness, or precise gas estimation.

## Contributing

See [AGENTS.md](AGENTS.md) for development guidelines.

```bash
cargo fmt && cargo clippy && cargo test
```

## License

MIT License - see [LICENSE](LICENSE) for details.
