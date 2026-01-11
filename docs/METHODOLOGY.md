# Methodology, Verification, and Limitations

This project is a **bytecode-first** analyzer for Sui Move packages, combined with an automated transaction inhabitation benchmark.

---

## 1. Bytecode Extraction Methodology

The authoritative source of a published package is its compiled Move bytecode (`.mv`). We parse `.mv` directly to emit a **canonical, deterministic JSON** representation of the package interface.

### Why parsing `.mv` works (first principles)
Sui Move modules compile into a deterministic binary format (“CompiledModule”) defined by Move’s bytecode spec. That binary contains the full set of declarations for a module:
- **Module identity** (address + name)
- **Structs** (abilities, type params, field names, field types, native-ness)
- **Functions** (visibility, entry, type params, parameter/return types, acquires list, native-ness)

This tool parses those tables using `move-binary-format::file_format::CompiledModule` (from MystenLabs’ Sui/Move dependency), the standard Rust implementation of the Move bytecode format.

### Verification loops (“robustness”)
We validate the extracted representation with multiple feedback loops:

- **Local bytes integrity**: Verifies that the `.mv` bytes match the `bcs.json` module map in the corpus.
- **RPC sanity**: Compares module name sets and declaration counts with what the Sui RPC reports for the same package ID.
- **Rigorous interface compare**: Performs a field-by-field comparison between RPC-normalized modules and bytecode-derived modules.

---

## 2. Benchmark Methodology (Phase II: Inhabitation)

The goal of the Phase II inhabitation benchmark is to measure an agent's ability to **construct valid transactions** that result in the creation of specific Move `key` structs (objects) defined in a package.

### Planning Intelligence Focus (Normalization)
The benchmark is designed to measure **planning and inhabitation intelligence**, not JSON formatting ability. We apply a series of automatic normalization rules to agent outputs:
- **Alias Resolution**: Automatically maps legacy or common incorrect keys like `"object"` or `"object_id"` to the supported `"imm_or_owned_object"`.
- **Type Coercion**: Converts stringified numbers/booleans to native JSON types to handle "sloppy" LLM formatting.
- **Address Padding**: Ensures all Move addresses have the `0x` prefix and are padded to the correct 32-byte hex length.

See **[PTB Schema](PTB_SCHEMA.md)** for the full technical specification of valid kinds and corrections.

### Scoring Semantics: Base-Type Matching
A "Hit" is recorded if the transaction simulation results in the creation of an object whose **Base Type** matches a target type.
- **Generic Handling**: We ignore type arguments during matching. For example, if the target is `0x2::coin::Coin<T>`, creating a `0x2::coin::Coin<0x2::sui::SUI>` counts as a success.
- **Normalization**: Both target and created types are canonicalized (address padding, case sensitivity) before comparison.

### The Mechanical Baseline (`baseline-search`)
We use a deterministic, non-LLM baseline establishing the benchmark's "floor":
1. **Candidate Selection**: Identifies all `public entry` functions.
2. **Recursive Constructor Discovery**:
   - The harness scans for "Constructor" functions (functions that return the target type).
   - **Search Depth**: The discovery is limited to **3 levels** of recursion to prevent state space explosion.
3. **PTB Chaining**: Uses Sui Programmable Transaction Blocks to chain constructors and target functions.

### Simulation Strategy & Mock Inventory
We support three simulation modes, which affect the "evidence" used for scoring:
- **`dry-run` (Strict)**: Requires a real funded account. Uses authoritative transaction effects from the Sui network.
- **`dev-inspect` (Lax)**: Executes the transaction without full signature/ownership checks.
- **`build-only` (Static)**:
  - For packages that cannot be simulated on-chain, we use a **Mock Inventory Strategy**.
  - Object IDs like `0x0`, `0x1`, etc., are provided as "filler" arguments.
  - Scoring is derived from static bytecode analysis of the called functions (checking for `transfer::transfer<T>` calls).

### 2.4 Research Invariants & Technical Decisions

Several "hidden" heuristics define the difficulty and fairness of the benchmark. These are codified in the harness to ensure reproducible results.

#### A. Recursive Search Depth (Limit: 3)
When discovering constructors for "Baseline Search," the harness performs a recursive scan of the package's public functions.
- **The Decision**: Recursion is capped at **3 levels**.
- **Rationale**: Most Sui Move patterns (e.g., Request/Policy patterns) are resolved within 2-3 steps. Deeper recursion often leads to "Solver-style" state explosion which exceeds the scope of a baseline benchmark. This cap defines the "Mechanical Floor" of the benchmark.

#### B. Base-Type Matching (Generics "Close Enough")
To score a "Hit," the harness compares the type of an object created during simulation against the target types.
- **The Rule**: Matching is performed on **Base Types** only.
- **Logic**: Any content between `<` and `>` is stripped before comparison.
- **Example**: If the target is `0x2::coin::Coin<T>`, then `0x2::coin::Coin<0x2::sui::SUI>` and `0x2::coin::Coin<0x...::usdc::USDC>` are both considered valid hits. This ensures the benchmark measures the ability to find the correct *container* logic even if specific coin types are unavailable.

#### C. Mock Inventory Strategy (Build-Only Mode)
In `build-only` mode (where no real Sui RPC is available), the harness simulates object ownership:
- **System Objects**: Standard objects like `0x6` (Clock), `0x8` (Random), and `0x403` (DenyList) are automatically provided.
- **Deterministic Mock IDs**: Required object arguments are filled with incremental IDs (e.g., `0x0...01`, `0x0...02`).
- **Gas Simulation**: A dummy gas coin is always injected at a fixed ID (`0x1234`) to satisfy the transaction builder.

#### D. Normalization Rules (The "Fairness" Layer)
The harness applies strict "massaging" to LLM outputs via `normalize.py`:
- **Alias Fixes**: `object` and `object_id` → `imm_or_owned_object`.
- **Address Formatting**: Automatic `0x` prefixing and 32-byte hex padding.
- **Type Coercion**: Converting `"100"` (string) to `100` (int) for integer-kind arguments.
This ensures models are scored on their **Move comprehension** rather than their ability to adhere to strict JSON types.

#### E. Loop Detection & Force Planning
When using **Progressive Exposure** (via `need_more` requests), the harness monitors for repetitive model behavior.
- **The Valve**: If a model requests the exact same set of functions in two consecutive calls, the harness identifies an infinite loop.
- **The Correction**: The harness terminates the discovery loop and re-invokes the model with a "Force Plan" instruction, requiring it to provide a best-effort PTB plan using the currently available context. This ensures every package attempt reaches a terminal state and protects the researcher's API budget.

---

## 3. Benchmark Difficulty Classification

To provide deeper insight into model performance, we categorize packages by their **Inhabitation Difficulty**. Scoring a "Hit" on a Level 3 package represents significantly higher semantic reasoning than a Level 1 package.

| Difficulty | Classification | Logic Requirements | Example Pattern |
|------------|----------------|--------------------|-----------------|
| **Level 1** | Pure / Simple | Single `public entry` call with primitive arguments. | `mint(amount: u64)` |
| **Level 2** | Object-Aware | Requires existing objects from inventory or system (Clock, Random). | `stake(coin: Coin, clock: &Clock)` |
| **Level 3** | Multi-Step | Requires recursive constructor discovery (2-3 steps). | `create_cap()` → `mint(cap)` |
| **Level 4** | Generic-Heavy | Requires correctly filling multiple complex type parameters. | `swap<T1, T2>(pool, coin)` |

Researchers should correlate `avg_hit_rate` with these levels to identify if an agent struggles with **Logic Depth** (Level 3) or **Signature Complexity** (Level 4).

---

## 4. Local Bytecode Sandbox Methodology

The **Local Bytecode Sandbox** (`benchmark-local` command) provides offline type inhabitation testing. See [LOCAL_BYTECODE_SANDBOX.md](LOCAL_BYTECODE_SANDBOX.md) for architecture.

### Failure Taxonomy (Primary Metric)

The key metric for LLM evaluation is **failure distribution by stage**, not a single pass rate. Each stage reveals different information:

| Stage | Name | What Failure Indicates |
|-------|------|------------------------|
| **A1** | Target Resolution | Function/module doesn't exist in bytecode |
| **A2** | Type Layout | Unknown struct, recursive type, or unresolvable generic |
| **A3** | Type Synthesis | No constructor path to create required type |
| **A5** | Type Parameters | Generic type parameter bounds violation |
| **B1** | Constructor Execution | Dependency constructor aborted |
| **B2** | Target Execution | Function aborted (assertion or unsupported native) |

### Interpreting Failure Distribution

A single pass rate obscures critical distinctions:

- **A3 failures** indicate a **synthesizability ceiling**—types the sandbox cannot create regardless of LLM capability
- **B2 failures with error 1000** are **expected boundaries** (crypto, randomness)—not LLM failures
- **B2 assertion failures** indicate actual LLM type understanding issues

For researchers: focus on *where failures cluster* rather than aggregate pass rates.

### Synthesizability Ceiling

Some types cannot be synthesized offline:
- Types requiring multi-hop constructor chains beyond current depth
- Types depending on existing chain state (shared objects)
- Types using unsupported natives (signatures, randomness)

This ceiling is a property of the sandbox, not LLM capability. Report it separately.

---

## 5. Limitations and Edge Cases

- **Private Visibility**: Our bytecode extractor captures **private** functions, which help identify constructors that RPC-based tools might miss.
- **Inventory Dependency**: Many functions require existing objects. The benchmark results depend on the sender's on-chain inventory.
- **Generic Type Arguments**: The baseline heuristic fills type params with `0x2::sui::SUI`, which may not always be appropriate.
- **Simulation Strictness**: We prefer strict `dry-run` simulation for "official" scoring to ensure transaction ground-truth.

---

## 6. Platform Integration (AgentBeats)

This framework is designed as a **"Green Agent" substrate** for the Berkeley RDI [AgentBeats](https://rdi.berkeley.edu/agentx-agentbeats) evaluation ecosystem.

### Why Bytecode-First for AgentBeats?
AgentBeats requires a verifiable, fair baseline for scoring agents. By using bytecode abilities as the source of truth, we ensure:
1. **No Human Labeling Bias**: The target set is derived mechanically from the blockchain's own declarations.
2. **Fair Comparison**: All agents are evaluated against the same canonical JSON interface, which remains stable across different Sui RPC providers.

### Execution Layer
The framework provides an A2A-compliant server that handles the "Benchmarking Lifecycle":
- **Discovery**: Phase I evaluates key-struct identification.
- **Action**: Phase II evaluates the logic of the generated PTB (Programmable Transaction Block).

---

## Related Documentation

- **[Insights & Reward](INSIGHTS.md)** - High-value takeaways and research value proposition.
- **[Benchmark Guide](BENCHMARK_GUIDE.md)** - Walkthrough for running benchmarks.
- **[CLI Reference](CLI_REFERENCE.md)** - Rust CLI commands.
- **[A2A Protocol](A2A_PROTOCOL.md)** - Integration and tuning details.
- **[JSON Schema](SCHEMA.md)** - Interface and result schemas.
