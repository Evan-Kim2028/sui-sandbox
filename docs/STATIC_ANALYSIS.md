# High-Fidelity Static Analysis Engine

The `smi_tx_sim` tool includes a robust static analysis engine powered by **Move Model 2**. This allows the benchmark to predict created object types without requiring an active network connection or funded gas coins.

## Core Capabilities

### 1. Call Graph Traversal
Unlike simple grep-based scans, the engine performs a recursive walk of the function call graph. 
- **Depth-Limited:** Traversal is limited to a depth of 10 to prevent infinite loops in recursive Move logic.
- **Context-Aware:** It identifies calls to core Sui transfer functions (`0x2::transfer::transfer`, `public_transfer`, `share_object`) and records the types being moved.

### 2. Generic Type Substitution
The engine correctly handles Move generics by tracking type parameters across call boundaries.
- **Example:** If `create_and_transfer<T>(...)` is called with `MyCoin`, the engine correctly identifies that a `0x2::coin::Coin<0x...::MyCoin>` is created, even if the transfer happens deep within a library function.
- **Nested Generics:** Supports complex nested types like `TreasuryCap<Wrapper<MyCoin>>`.

### 3. Loop & Recursion Protection
The engine maintains a `visited` set of `(Module, Function, TypeArgs)` tuples. This ensures that:
- **Mutual Recursion:** Circular dependencies (e.g., `A -> B -> A`) are detected and terminated gracefully.
- **Efficiency:** Redundant paths are pruned, ensuring analysis completes in milliseconds even for complex DeFi packages.

## Execution Modes

The engine is primarily used in **`build-only`** mode, but it also provides a "Static Ground Truth" for other modes.

| Mode | Use Case | Analysis Source |
| :--- | :--- | :--- |
| `build-only` | Offline benchmarking / Local iteration | **Static Engine only** |
| `dev-inspect` | Low-fidelity on-chain check (no ownership) | RPC + Static Engine (merged) |
| `dry-run` | High-fidelity on-chain check (ground truth) | RPC execution effects |

## Verification & Trust
The engine is systematically verified via:
- **Rust Unit Tests:** Direct assertions on substitution and recursion logic in `src/bin/smi_tx_sim.rs`.
- **CLI Integration Tests:** Automated runs against `tests/fixture/` to ensure the binary emits correct JSON.
- **Stress Fixtures:** A dedicated Move module (`tests/fixture/sources/stress_tests.move`) that exercises edge cases in generics and recursion.

## Technical Details (Implementation)
The implementation leverages:
- `move_model_2`: For building the package environment.
- `move_stackless_bytecode_2`: For normalized instruction scanning.
- `BTreeSet` visited tracking: To handle type-instantiated function unique identifying.

See `analyze_created_types` in `src/bin/smi_tx_sim.rs` for the core logic.
