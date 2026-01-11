# Type Inhabitation Evaluation Framework

## Overview

This document describes the evaluation framework for testing LLM capability to achieve **type inhabitation** in Sui Move packages. The framework enables LLMs to generate helper Move code that compiles against and executes with real on-chain package bytecode, all without requiring mainnet deployment or gas.

## Target Package

**Package**: LST (Liquid Staking) Package  
**Package ID**: `0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700`  
**Original Package ID**: `0xc35ee7fee75782806890cf8ed8536b52b4ba0ace0fb46b944f1155cc5945baa3`

**Modules**:
- `cell` - Generic container type `Cell<T>` with `new`, `get`, `set`, `destroy` operations
- `fees` - Fee configuration with `FeeConfigBuilder` and `FeeConfig` types
- `liquid_staking` - Core liquid staking logic
- `storage` - Validator storage management
- `events` - Event emission
- `version` - Version tracking

## Results

### Successful Type Inhabitation

The LLM (GPT 5.2) successfully:

1. **Generated valid Move code** that imports target package types:
```move
module helper_pkg::inhabitation {
    use target_pkg::cell;
    use target_pkg::fees;

    public struct Marker has drop, store { x: u64 }

    entry fun demo_cell(ctx: &mut TxContext) {
        let mut c = cell::new<Marker>(Marker { x: 1 });
        let _r = cell::get<Marker>(&c);
        cell::set<Marker>(&mut c, Marker { x: 2 });
        let _inner = cell::destroy<Marker>(c);
    }
}
```

2. **Compiled successfully** against generated source stubs

3. **Executed successfully** with the real target package bytecode

4. **Achieved tier_b_hit** with verified target package module access:
```json
{
  "function": "inhabitation::demo_cell",
  "target_modules": [
    "0xc35ee7fee75782806890cf8ed8536b52b4ba0ace0fb46b944f1155cc5945baa3::cell",
    "0xc35ee7fee75782806890cf8ed8536b52b4ba0ace0fb46b944f1155cc5945baa3::events",
    "0xc35ee7fee75782806890cf8ed8536b52b4ba0ace0fb46b944f1155cc5945baa3::fees"
  ]
}
```

## Features Implemented

### 1. Source Stub Generation

**Problem**: Move compiler requires source code to compile imports, but we only have bytecode.

**Solution**: Generate minimal `.move` source files from the bytecode interface JSON:
- Extract struct definitions with fields, abilities, and type parameters
- Extract function signatures with parameters, returns, and visibility
- Function bodies are stubs that `abort 0` (never executed - real bytecode used at runtime)

```python
def _generate_move_source_stubs(interface: dict, pkg_alias: str) -> dict[str, str]:
    """Generate Move source stub files from interface JSON."""
```

**Location**: `benchmark/scripts/e2e_one_package.py`

### 2. Native Function Name Alignment

**Problem**: VM execution failed with `MISSING_DEPENDENCY` errors.

**Root Cause**: Native function names in our mock implementations didn't match the bytecode:
- Bytecode uses: `transfer_impl`, `freeze_object_impl`, `share_object_impl`, etc.
- Our mocks used: `transfer_internal`, `freeze_object`, `share_object`, etc.

**Fix**: Updated `src/benchmark/natives.rs` to use correct `*_impl` suffixes.

### 3. Execution Tracing

**Problem**: No way to verify that target package code was actually executed.

**Solution**: Added `ExecutionTrace` to track module accesses during VM execution:

```rust
pub struct ExecutionTrace {
    pub modules_accessed: BTreeSet<ModuleId>,
}
```

The `ModuleResolver` records every module lookup, and we filter out framework modules (0x1, 0x2, 0x3) to identify target package accesses.

**Location**: `src/benchmark/vm.rs`

### 4. Target Package Validation

**Problem**: Previous validation counted helper-module-only hits as success.

**Solution**: Updated validation to require:
- `tier_b_hit` (successful execution)
- `target_modules_accessed` includes actual target package modules (not just helper/framework)

```python
target_accesses = [
    m for m in accessed
    if not m.startswith("0x0::") and  # helper
       not m.startswith("0x1::") and  # move-stdlib
       not m.startswith("0x2::") and  # sui-framework
       not m.startswith("0x3::")      # sui-system
]
```

**Location**: `benchmark/scripts/e2e_one_package.py` in `_validate_artifacts()`

### 5. Bundled Framework Bytecode

**Feature**: Sui framework (move-stdlib, sui-framework, sui-system) bytecode bundled at compile time.

**Version**: mainnet-v1.62.1

**Location**: `framework_bytecode/` directory, loaded via `include_bytes!` in `src/benchmark/resolver.rs`

## How to Replicate

### Prerequisites

1. Docker and Docker Compose
2. OpenRouter API key (or compatible LLM API)
3. Sui packages corpus with bytecode

### Steps

```bash
# 1. Build the Docker image
cd sui-move-interface-extractor
docker compose build smi-bench

# 2. Run E2E evaluation on a package
docker compose run --rm --entrypoint "" \
  -e SMI_E2E_REAL_LLM=1 \
  -e OPENROUTER_API_KEY=sk-or-v1-... \
  smi-bench python /app/benchmark/scripts/e2e_one_package.py \
  --corpus-root /corpus/mainnet \
  --package-id 0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700 \
  --model gpt-5.2 \
  --max-attempts 3 \
  --out-dir /app/results/my_test

# 3. Check results
cat results/my_test/*/validation_report.json

# 4. View execution trace
cat results/my_test/*/mm2_combined_benchmark_local.jsonl | \
  jq 'select(.status == "tier_b_hit") | {
    function: "\(.target_module)::\(.target_function)",
    target_modules: .tier_b_details.target_modules_accessed
  }'
```

### Key Output Files

| File | Description |
|------|-------------|
| `validation_report.json` | Pass/fail with error details |
| `mm2_combined_benchmark_local.jsonl` | Per-function tier_a/tier_b results with execution traces |
| `llm_response_attempt_N.json` | LLM-generated Move code |
| `helper_pkg/` | Generated helper package (Move.toml, sources/, deps/) |
| `target_interface.json` | Bytecode interface used for stub generation |

### Success Criteria

A successful type inhabitation requires:

1. **Compilation**: Helper code compiles against target package stubs
2. **Execution**: Helper entry function executes without abort
3. **Target Access**: Execution trace shows target package modules were loaded (not just framework/helper)

```json
{
  "ok": true,
  "errors": []
}
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         E2E Pipeline                            │
├─────────────────────────────────────────────────────────────────┤
│  1. Extract interface from target package bytecode              │
│  2. Generate source stubs from interface                        │
│  3. LLM generates helper package using target_pkg:: imports     │
│  4. Compile helper against stubs (sui move build)               │
│  5. Load helper bytecode + real target bytecode into VM         │
│  6. Execute helper entry functions                              │
│  7. Trace module accesses to verify target package execution    │
│  8. Validate tier_b_hit with target package module access       │
└─────────────────────────────────────────────────────────────────┘

                    Compilation                    Execution
                    ───────────                    ─────────
┌──────────┐    ┌──────────────┐    ┌─────────────────────────────┐
│  Stubs   │───▶│ sui move     │───▶│  VM with real bytecode:     │
│ (source) │    │ build        │    │  - Framework (0x1, 0x2, 0x3)│
└──────────┘    └──────────────┘    │  - Target package (real)    │
                      │             │  - Helper (compiled)        │
                      ▼             └─────────────────────────────┘
               ┌──────────────┐                   │
               │ Helper .mv   │                   ▼
               │ (bytecode)   │           ┌──────────────┐
               └──────────────┘           │ ExecutionTrace│
                                          │ - modules    │
                                          │   accessed   │
                                          └──────────────┘
```

## What "Tier B Hit" Actually Means

### Tier Definitions

| Tier | Name | What It Proves |
|------|------|----------------|
| **Tier A** | Compilation | Code compiles - type signatures are correct |
| **Tier B** | Execution | Code runs without abort - runtime semantics are valid |

### The Wrapper Pattern

The LLM doesn't call target package functions directly for benchmarking. Instead:

1. **LLM generates a wrapper module** (`helper_pkg::inhabitation`) that imports and uses target package types
2. **We execute the wrapper's entry functions** (e.g., `demo_cell`)
3. **The wrapper internally calls target package functions** (e.g., `cell::new<T>()`)

```
┌─────────────────────────────────────────────────────────────┐
│  Benchmark Runner executes:                                 │
│    helper_pkg::inhabitation::demo_cell(ctx)                 │
│                                                             │
│  Which internally calls:                                    │
│    target_pkg::cell::new<Marker>(...)     ← TARGET CODE     │
│    target_pkg::cell::get<Marker>(...)     ← TARGET CODE     │
│    target_pkg::cell::set<Marker>(...)     ← TARGET CODE     │
│    target_pkg::cell::destroy<Marker>(...) ← TARGET CODE     │
└─────────────────────────────────────────────────────────────┘
```

### How We Validate Target Package Access

**Module loading is our proxy for code execution.** The Move VM must load a module's bytecode before it can execute any function from that module.

We instrument the `ModuleResolver::get_module()` method to record every module lookup:

```rust
impl<'a> ModuleResolver for InMemoryStorage<'a> {
    fn get_module(&self, id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        // Record this module access
        if let Ok(mut trace) = self.trace.lock() {
            trace.modules_accessed.insert(id.clone());
        }
        self.module_resolver.get_module(id)
    }
}
```

After execution, we filter out framework modules to identify target package accesses:

```python
target_accesses = [
    m for m in accessed
    if not m.startswith("0x0::") and   # helper module (address 0x0)
       not m.startswith("0x1::") and   # move-stdlib
       not m.startswith("0x2::") and   # sui-framework  
       not m.startswith("0x3::")       # sui-system
]
```

**Validation passes only if:**
- Execution completed without abort (`tier_b_hit`)
- At least one target package module was loaded (e.g., `0xc35ee7...::cell`)

### Why Module Loading Implies Code Execution

In the Move VM:
1. **Module loading is lazy** - modules are only loaded when needed
2. **Function calls require module loading** - you cannot call `cell::new()` without loading the `cell` module
3. **Type instantiation requires module loading** - creating `Cell<T>` requires loading the module that defines `Cell`

If `0xc35ee7...::cell` appears in the trace, the VM must have:
- Resolved a function call to that module, OR
- Instantiated a type defined in that module

Either way, **target package code paths were exercised**.

## Critical Analysis: Limitations and Risks

### What This Benchmark Proves

✅ **LLM understands type signatures** - Code compiles against real interface  
✅ **LLM understands basic semantics** - Code executes without runtime errors  
✅ **LLM can instantiate generic types** - `Cell<Marker>` works with custom type parameter  
✅ **Target package code runs** - Module loading trace proves bytecode was accessed  

### What This Benchmark Does NOT Prove

#### 1. Semantic Correctness

**Risk**: Code executes but does the wrong thing.

```move
// This would pass our benchmark but is semantically wrong:
entry fun bad_demo(ctx: &mut TxContext) {
    let c = cell::new<u64>(0);
    cell::destroy<u64>(c);  // Created and immediately destroyed - pointless
}
```

**Mitigation**: We verify execution, not intent. The benchmark tests "can the LLM write code that uses these types correctly enough to run" - not "does it do something useful."

#### 2. Deep Code Path Coverage

**Risk**: Module loading doesn't mean all code paths were exercised.

```move
// If cell::new() is called, the cell module is loaded.
// But cell::get(), cell::set() might not be called.
// We only know the module was loaded, not which functions ran.
```

**Current state**: We trace module loads, not function calls. The Move VM doesn't expose function-level tracing without significant instrumentation.

**Mitigation**: The LLM-generated code typically calls multiple functions (new, get, set, destroy) which exercises more paths. But we can't guarantee coverage.

#### 3. Stateful Operations

**Risk**: Many real-world operations require persistent state.

```move
// This would fail because we don't have object storage:
public fun stake(pool: &mut StakingPool, coin: Coin<SUI>) { ... }
```

**Current state**: We can only test stateless operations or operations where the helper creates and destroys objects within a single transaction.

**Mitigation**: Focus on "pure" functions and types that don't require pre-existing on-chain state.

#### 4. False Positives from Transitive Loads

**Risk**: A module might be loaded transitively without the LLM actually understanding it.

```move
// LLM calls cell::new(), which internally calls events::emit()
// The events module is loaded, but LLM didn't explicitly use it
```

**Example from our trace**:
```json
"target_modules": [
    "0xc35ee7...::cell",    // LLM explicitly used
    "0xc35ee7...::events",  // Transitively loaded by cell
    "0xc35ee7...::fees"     // Transitively loaded
]
```

**Mitigation**: This is actually fine - it shows the target package's internal dependencies work correctly. The LLM proved it can use `cell`, and the transitive loads prove the real bytecode executed.

#### 5. Native Function Mocking

**Risk**: Our native function mocks may not match real Sui behavior.

```move
// We mock transfer::transfer_impl() to do nothing
// Real Sui would update object ownership
```

**Current state**: Mocks are no-ops or simple stubs. Object ownership is not tracked. Dynamic fields ARE supported via the ObjectRuntime VM extension (see `src/benchmark/object_runtime.rs`).

**Mitigation**: For type inhabitation, this is acceptable. We're testing "can the code run" not "does it produce correct state changes."

### Confidence Assessment

| Aspect | Confidence | Notes |
|--------|------------|-------|
| **Compilation correctness** | HIGH | Move compiler is the source of truth |
| **Execution success** | HIGH | Move VM is the source of truth |
| **Target package accessed** | MEDIUM-HIGH | Module loading is reliable proxy |
| **Specific functions called** | LOW | We don't trace function calls |
| **Semantic correctness** | LOW | We verify execution, not intent |
| **State correctness** | N/A | No persistent state |

### Known False Negative Scenarios

These will FAIL our benchmark even though they might represent valid type understanding:

1. **Functions requiring object arguments** - We can't synthesize arbitrary objects
2. **Functions requiring specific state** - e.g., "pool must have balance > X"
3. **Functions with complex preconditions** - Assertions that fail with default values
4. **Generic functions without wrapper** - Direct tier_b on `cell::new<T>` fails (no T)

### Known False Positive Scenarios

These could PASS our benchmark without true understanding:

1. **Trivial wrappers** - `entry fun demo() { let _ = 1; }` that imports but doesn't use target types (caught by module trace validation)
2. **Copy-paste from docs** - LLM copies example code without understanding (still valid - code works)
3. **Transitive-only access** - Using one function that happens to load many modules (acceptable - proves integration works)

### Recommendations for Robust Evaluation

1. **Require multiple entry points** - LLM must demonstrate multiple type usages
2. **Check for meaningful operations** - Not just create/destroy but actual data manipulation
3. **Add function-level tracing** - Instrument VM to trace actual function calls (future work)
4. **Test with adversarial prompts** - Verify LLM doesn't just import without using

## Limitations

1. **Generic Functions**: Direct tier_b testing of generic functions not supported (requires type argument synthesis)
2. **Crypto Verification**: Operations using `bls12381::*`, `ecdsa_*::*`, `ed25519::*`, `groth16::*` abort with `E_NOT_SUPPORTED` (1000)
3. **Randomness/ZK**: Operations using `random::*`, `zklogin::*`, `poseidon::*` abort with `E_NOT_SUPPORTED` (1000)
4. **Object Storage**: No persistent object storage - each execution starts fresh (objects don't survive between function calls)
5. **Clock/Random**: Clock and Random system objects not fully synthesized
6. **Function-level tracing**: We trace module loads, not individual function calls

## Future Work

1. Support generic function instantiation with common type arguments
2. Add mock object storage for stateful testing
3. Expand native function coverage for more complex operations
4. Add function-level execution tracing via VM instrumentation
5. Semantic analysis of generated code (not just execution success)

---

*Last updated: January 2025*  
*Framework version: mainnet-v1.62.1*
