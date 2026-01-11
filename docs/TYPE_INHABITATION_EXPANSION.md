# Type Inhabitation Expansion: Supporting Generic Functions and Witness Patterns

## Executive Summary: Implementation Status

| Phase | Feature | Status | Notes |
|-------|---------|--------|-------|
| 1 | Module-level tracing | **COMPLETED** ✅ | `target_modules_accessed` in TierBDetails |
| 2 | Function-level tracing | **COMPLETED** ✅ | Static bytecode analysis (`static_calls`) |
| 3 | Generic instantiation | **COMPLETED** ✅ | Primitives (u64) used as type args |
| 4 | Witness pattern support | **NOT IMPLEMENTED** | Requires OTW bypass + object storage |

### Results After Phases 1-3

```
Before:  Generic functions blocked at A5 ("not supported")
After:   751 tier_a_hit, 2 tier_b_hit, 190 miss (struct params)
```

**Key improvements**:
- Generic functions now attempt execution with `u64` as type argument
- Static bytecode analysis shows which target functions are called
- Module access tracing proves target package code was loaded

---

## Current Limitations

### Problem 1: Generic Functions Skipped at A5

All `liquid_staking` functions failed with "generic functions not supported yet":

```
liquid_staking::create_lst<P>      → A5 miss
liquid_staking::mint<P>            → A5 miss
liquid_staking::redeem<P>          → A5 miss
liquid_staking::refresh<P>         → A5 miss
... (all 17 functions)
```

**Root cause** in `src/benchmark/runner.rs`:
```rust
if !handle.type_parameters.is_empty() {
    report.failure_stage = Some(FailureStage::A5);
    report.failure_reason = Some("generic functions not supported yet".to_string());
    return Ok(report);
}
```

### Problem 2: Witness Pattern Required for Core Types

The LST package's key types require `create_lst<P>`:

```move
public fun create_lst<P: drop>(
    fee_config: FeeConfig,
    treasury_cap: TreasuryCap<P>,  // ← BLOCKER: requires witness
    ctx: &mut TxContext
) -> (AdminCap<P>, CollectionFeeCap<P>, LiquidStakingInfo<P>)
```

To get `TreasuryCap<P>`, you must call:
```move
public fun create_currency<P: drop>(
    witness: P,  // ← One-time witness, can only be created in module defining P
    decimals: u8,
    symbol: vector<u8>,
    name: vector<u8>,
    description: vector<u8>,
    icon_url: Option<Url>,
    ctx: &mut TxContext
): (TreasuryCap<P>, CoinMetadata<P>)
```

**The witness pattern**: `P` must be a one-time witness (OTW) type - a struct with `drop` that's only instantiated once in its module's `init` function.

### What the LLM Actually Inhabited

Instead of the core types, the LLM successfully inhabited "utility" types:

| Type | Inhabited? | Why |
|------|------------|-----|
| `cell::Cell<T>` | ✅ Yes | Simple generic, no witness needed |
| `fees::FeeConfig` | ✅ Yes | Non-generic, builder pattern |
| `LiquidStakingInfo<P>` | ❌ No | Requires `create_lst` + witness |
| `AdminCap<P>` | ❌ No | Only returned by `create_lst` |
| `CollectionFeeCap<P>` | ❌ No | Only returned by `create_lst` |

---

## Proposed Solutions

### Solution 1: Generic Function Instantiation with Synthetic Types

**Goal**: Allow the benchmark to test generic functions by providing synthetic type arguments.

**Approach**: For functions like `cell::new<T>`, automatically try common type instantiations:

```rust
// In runner.rs, instead of skipping generics:
let synthetic_types = vec![
    TypeTag::U64,
    TypeTag::U8,
    TypeTag::Bool,
    TypeTag::Address,
    // Could also try vector<u8>, etc.
];

for type_arg in synthetic_types {
    // Try executing with this type argument
    let result = harness.execute_function(module, func, vec![type_arg], args);
    if result.is_ok() {
        // Record success with this instantiation
        break;
    }
}
```

**Limitations**: Only works for unconstrained generics or common constraints like `drop`, `copy`, `store`.

### Solution 2: LLM-Generated Witness Pattern

**Goal**: Have the LLM generate a complete package that includes its own OTW.

**Approach**: Modify the prompt to guide the LLM to create witness types:

```move
module helper_pkg::my_lst {
    use sui::coin;
    use sui::tx_context::TxContext;
    use target_pkg::liquid_staking;
    use target_pkg::fees;

    /// One-time witness for our custom LST currency
    public struct MY_LST has drop {}

    /// Create an LST using our witness
    fun init(witness: MY_LST, ctx: &mut TxContext) {
        // Create the treasury cap using our witness
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9,                          // decimals
            b"MLST",                    // symbol
            b"My LST",                  // name
            b"Test LST",                // description
            option::none(),             // icon_url
            ctx
        );
        
        // Create fee config
        let fee_config = fees::to_fee_config(fees::new_builder(ctx));
        
        // Now we can call create_lst with our treasury cap!
        let (admin_cap, collection_cap, info) = liquid_staking::create_lst<MY_LST>(
            fee_config,
            treasury_cap,
            ctx
        );
        
        // Transfer or share the objects
        transfer::public_transfer(admin_cap, tx_context::sender(ctx));
        transfer::public_share_object(info);
        // ... handle other objects
    }
}
```

**Challenge**: The `init` function is only called during package publish, not during our VM execution.

### Solution 3: Mock Witness Synthesis

**Goal**: Allow the VM to synthesize witness types for testing.

**Approach**: Create a special "test mode" that can mint arbitrary witness types:

```rust
// In natives.rs, add a test-only native:
fn synthesize_witness<T: drop>() -> T {
    // UNSAFE: Only for testing! Creates a witness out of thin air.
    unsafe { std::mem::zeroed() }
}
```

**Problems**:
- Violates Move's safety guarantees
- Witnesses are supposed to be unforgeable
- Would require VM modifications

### Solution 4: Two-Phase Execution (Publish + Call)

**Goal**: Simulate the full lifecycle of package deployment.

**Approach**:
1. **Phase 1**: "Publish" the helper package, which runs `init` and creates witnesses
2. **Phase 2**: Call entry functions that use the created objects

```
┌─────────────────────────────────────────────────────────────┐
│ Phase 1: Publish                                            │
│   - Run init(witness, ctx)                                  │
│   - Creates TreasuryCap, LiquidStakingInfo, etc.           │
│   - Store objects in mock storage                           │
├─────────────────────────────────────────────────────────────┤
│ Phase 2: Execute                                            │
│   - Load objects from mock storage                          │
│   - Call mint<P>(info, coin, ctx)                          │
│   - Verify execution success                                │
└─────────────────────────────────────────────────────────────┘
```

**Implementation**:
```rust
// 1. Add mock object storage
struct MockObjectStore {
    objects: BTreeMap<ObjectID, (TypeTag, Vec<u8>)>,
}

// 2. Run init function to populate storage
fn simulate_publish(module: &CompiledModule, store: &mut MockObjectStore) {
    // Find init function
    // Execute it with synthetic witness
    // Capture created objects via transfer/share natives
}

// 3. Run entry functions with stored objects
fn execute_with_objects(func: &str, store: &MockObjectStore) {
    // Load required objects from store
    // Pass as arguments
    // Execute
}
```

---

## Implementation Roadmap

### Phase 1: Meaningful Operation Tracking (Easy)

Add to `TierBDetails`:
```rust
pub struct TierBDetails {
    pub execution_success: bool,
    pub error: Option<String>,
    pub target_modules_accessed: Option<Vec<String>>,
    // NEW:
    pub operation_count: Option<u32>,        // How many target functions called
    pub operations: Option<Vec<String>>,     // Which operations (new, get, set, destroy)
}
```

Track operations by parsing the helper module's bytecode or enhancing the trace.

### Phase 2: Function-Level Tracing (Medium)

Instrument the VM session to log function calls:

```rust
// Wrap session.execute_entry_function to trace calls
struct TracingSession<'a> {
    inner: Session<'a>,
    call_trace: Vec<FunctionCall>,
}

struct FunctionCall {
    module: ModuleId,
    function: Identifier,
    type_args: Vec<TypeTag>,
}
```

**Challenge**: Move VM doesn't expose call hooks directly. Would need to either:
- Fork move-vm-runtime to add tracing
- Use the gas meter as a proxy (it sees all operations)
- Parse the helper bytecode statically to infer calls

### Phase 3: Generic Function Instantiation (Medium)

Modify `runner.rs` to try common type instantiations:

```rust
fn try_generic_instantiations(
    handle: &FunctionHandle,
    module: &CompiledModule,
    harness: &mut VMHarness,
) -> Option<(Vec<TypeTag>, AttemptStatus)> {
    let type_params = &handle.type_parameters;
    
    // Generate candidate type arguments based on constraints
    let candidates = generate_type_candidates(type_params);
    
    for type_args in candidates {
        if let Ok(()) = harness.execute_function(..., type_args.clone(), ...) {
            return Some((type_args, AttemptStatus::TierBHit));
        }
    }
    None
}

fn generate_type_candidates(params: &[AbilitySet]) -> Vec<Vec<TypeTag>> {
    // For each param, generate types that satisfy its constraints
    // e.g., if constraints = [drop], try u64, bool, etc.
}
```

### Phase 4: Witness Pattern Support (Hard)

Full two-phase execution with mock object storage:

1. **Mock storage implementation** (`src/benchmark/storage.rs`):
```rust
pub struct MockObjectStore {
    objects: BTreeMap<ObjectID, StoredObject>,
    id_counter: u64,
}

struct StoredObject {
    type_tag: TypeTag,
    bytes: Vec<u8>,
    owner: Owner,
}

enum Owner {
    Address(AccountAddress),
    Shared,
    Immutable,
}
```

2. **Transfer native mocks** that store objects:
```rust
fn mock_transfer_impl(obj_bytes: Vec<u8>, recipient: AccountAddress) {
    // Store in MockObjectStore instead of no-op
    STORE.lock().insert(new_id(), StoredObject { ... });
}
```

3. **Publish simulation**:
```rust
fn simulate_module_publish(module: &CompiledModule, ctx: &TxContext) {
    // Find init function
    if let Some(init) = find_init_function(module) {
        // Create witness (special case for OTW pattern)
        let witness = synthesize_otw(module);
        // Execute init
        harness.execute_function("init", vec![], vec![witness, ctx]);
    }
}
```

4. **Object loading for subsequent calls**:
```rust
fn load_objects_for_function(func: &FunctionDef, store: &MockObjectStore) -> Vec<Vec<u8>> {
    // Match function params to stored objects by type
}
```

---

## Risks and Mitigations

### Risk: False Sense of Coverage

**Problem**: Even with all enhancements, we may miss important invariants.

**Mitigation**: 
- Track which specific functions were called (not just modules loaded)
- Require LLM to demonstrate "meaningful" operations (defined per package)
- Add package-specific test cases for critical paths

### Risk: Witness Synthesis Breaks Safety Model

**Problem**: Minting witnesses violates Move's security guarantees.

**Mitigation**:
- Only allow in test mode
- Document that results prove "code correctness" not "security"
- Never use synthesized witnesses in production

### Risk: Complexity Explosion

**Problem**: Two-phase execution adds significant complexity.

**Mitigation**:
- Implement incrementally
- Start with simple cases (packages with simple init)
- Add package-specific adapters for complex patterns

---

## Package-Specific Analysis: LST

### Required Call Graph

```
To inhabit LiquidStakingInfo<P>:

1. Define OTW: struct MY_LST has drop {}
2. In init(witness):
   a. coin::create_currency(witness, ...) → TreasuryCap<MY_LST>
   b. fees::new_builder() → FeeConfigBuilder
   c. fees::to_fee_config(builder) → FeeConfig
   d. liquid_staking::create_lst(fee_config, treasury_cap, ctx)
      → (AdminCap<P>, CollectionFeeCap<P>, LiquidStakingInfo<P>)
```

### What Would Success Look Like?

```json
{
  "function": "helper_pkg::my_lst::init",
  "status": "tier_b_hit",
  "tier_b_details": {
    "execution_success": true,
    "target_modules_accessed": [
      "0xc35ee7...::liquid_staking",
      "0xc35ee7...::fees",
      "0xc35ee7...::storage",
      "0xc35ee7...::events"
    ],
    "operations": [
      "fees::new_builder",
      "fees::to_fee_config", 
      "liquid_staking::create_lst"
    ],
    "objects_created": [
      { "type": "LiquidStakingInfo<MY_LST>", "owner": "shared" },
      { "type": "AdminCap<MY_LST>", "owner": "0x..." },
      { "type": "CollectionFeeCap<MY_LST>", "owner": "0x..." }
    ]
  }
}
```

---

## Detailed Feasibility Analysis

### Phase 1: Meaningful Operation Tracking - **FEASIBLE**

**What we need**: Track not just "module loaded" but "which operations were performed".

**Implementation approach**:
```rust
// Enhance ExecutionTrace in vm.rs
pub struct ExecutionTrace {
    pub modules_accessed: BTreeSet<ModuleId>,
    pub functions_called: Vec<FunctionCall>,  // NEW
}

pub struct FunctionCall {
    pub module: ModuleId,
    pub function: String,
    pub type_args: Vec<TypeTag>,
}
```

**How to capture function calls**:
The Move VM provides `MoveTraceBuilder` with `OpenFrame`/`CloseFrame` events that capture every function call with:
- Module ID
- Function name
- Type instantiation
- Parameters and return values

**Code location**: `move-trace-format` crate, `TraceEvent::OpenFrame`

**Blockers**: None. Just need to enable the `tracing` feature and pass a tracer to `execute_function_bypass_visibility`.

**Effort**: 1-2 days

---

### Phase 2: Function-Level Tracing - **FEASIBLE**

**What we need**: Full call trace showing which target package functions were called.

**Discovery**: The Move VM already supports this via `MoveTraceBuilder`:

```rust
// In session.rs (move-vm-runtime)
pub fn execute_function_bypass_visibility(
    &mut self,
    module: &ModuleId,
    function_name: &IdentStr,
    ty_args: Vec<Type>,
    args: Vec<impl Borrow<[u8]>>,
    gas_meter: &mut impl GasMeter,
    tracer: Option<&mut MoveTraceBuilder>,  // ← WE CAN PASS THIS!
) -> VMResult<SerializedReturnValues>
```

**TraceEvent types available**:
```rust
pub enum TraceEvent {
    OpenFrame { frame: Box<Frame>, gas_left: u64 },
    CloseFrame { frame_id: TraceIndex, return_: Vec<TraceValue>, gas_left: u64 },
    Instruction { type_parameters: Vec<TypeTag>, pc: u16, ... },
    Effect(Box<Effect>),
}
```

**Implementation**:
1. Add `move-trace-format` dependency with `tracing` feature
2. Create `MoveTraceBuilder` before execution
3. Pass to `execute_function_bypass_visibility`
4. Extract `OpenFrame` events to get function call list

**Blockers**: Need to enable `tracing` feature in `move-vm-runtime`. Check if this adds significant overhead.

**Effort**: 2-3 days

---

### Phase 3: Generic Instantiation - **MEDIUM FEASIBILITY**

**What we need**: For `create_lst<P>`, provide a concrete type for `P`.

**Challenge**: Type constraints. `P: drop` means we need a type with `drop` ability.

**Approach A - Synthetic types** (EASY):
```rust
// Try common primitive types that have all abilities
let candidates = vec![
    TypeTag::U64,   // has copy, drop, store
    TypeTag::Bool,  // has copy, drop, store
    TypeTag::U8,    // has copy, drop, store
];

for type_arg in candidates {
    if satisfies_constraints(type_arg, constraints) {
        try_execute(func, vec![type_arg], args);
    }
}
```

**Problem**: For `create_lst<P>`, `P` is used as `TreasuryCap<P>` which requires the witness pattern. Primitive types won't work because:
1. `coin::create_currency` asserts `is_one_time_witness(&witness)`
2. OTW check requires: struct name == MODULE_NAME (uppercase), single bool field

**Approach B - LLM wrapper** (CURRENT):
The LLM already handles this! It generates:
```move
module helper_pkg::inhabitation {
    struct MY_TYPE has drop { dummy: bool }
    
    entry fun demo() {
        let x = target_pkg::generic_func<MY_TYPE>(...);
    }
}
```

**Limitation**: This only works in the wrapper. Direct benchmark of `create_lst<P>` still fails at A5.

**Recommendation**: 
- For benchmark runner: Add synthetic type instantiation for simple generics
- For complex patterns (OTW): Rely on LLM wrapper approach

**Effort**: 3-5 days

---

### Phase 4: Witness Pattern Support - **CHALLENGING BUT POSSIBLE**

**The Core Problem**:

To call `create_lst<P>`, we need `TreasuryCap<P>`, which requires:
1. A type `P` that passes `is_one_time_witness` check
2. Calling `coin::create_currency(witness, ...)` consumes the witness

**OTW Requirements** (from `sui-move-natives/src/types.rs`):
```rust
fn is_otw_struct(struct_layout: &MoveStructLayout, type_tag: &TypeTag) -> bool {
    // 1. Has exactly one bool field
    let has_one_bool_field = matches!(struct_layout.0.as_slice(), [MoveTypeLayout::Bool]);
    
    // 2. Struct name == module name (uppercase)
    // 3. No generic type parameters
    matches!(type_tag, TypeTag::Struct(struct_tag) if
        has_one_bool_field &&
        struct_tag.name == struct_tag.module.to_uppercase() &&
        struct_tag.type_params.is_empty()
    )
}
```

**Option A: Mock `is_one_time_witness` to always return true**

```rust
// In natives.rs
natives.push((
    "types",
    "is_one_time_witness",
    make_native(|_ctx, _ty_args, _args| {
        // UNSAFE: Always return true for testing
        Ok(NativeResult::ok(InternalGas::new(0), smallvec![Value::bool(true)]))
    }),
));
```

**Risks**:
- Violates security model (witnesses should be unforgeable)
- Could mask real bugs in LLM-generated code
- Documented as "test mode only"

**Verdict**: Acceptable for type inhabitation testing. Document clearly.

**Option B: Simulate package publish with init execution**

**How Sui runs init**:
1. Package is published
2. Runtime finds `init` function
3. If init takes `(OTW, TxContext)`, runtime creates the OTW instance
4. Runtime calls `init(otw_instance, ctx)`

**Implementation**:
```rust
fn simulate_publish(module: &CompiledModule, ctx_bytes: Vec<u8>) -> Result<MockObjectStore> {
    // 1. Find init function
    let init_func = find_function(module, "init")?;
    
    // 2. Check if it takes OTW parameter
    let params = get_params(init_func);
    let needs_otw = is_otw_param(&params[0], module);
    
    // 3. If OTW needed, synthesize it
    let mut args = vec![];
    if needs_otw {
        // Create struct with single bool field = true (Sui convention)
        let otw_bytes = bcs::to_bytes(&true)?;
        args.push(otw_bytes);
    }
    args.push(ctx_bytes);
    
    // 4. Execute init
    harness.execute_function(module.self_id(), "init", vec![], args)?;
    
    // 5. Collect created objects from transfer natives
    Ok(harness.get_created_objects())
}
```

**Challenges**:
1. Need to track objects created during init (modify transfer natives)
2. Need to store objects for later use
3. Need to match object types to function parameters

**Option C: Two-transaction simulation**

```
TX1: Publish helper package
  - Runs init(OTW, ctx)
  - Creates LiquidStakingInfo<MY_LST>, AdminCap<MY_LST>, etc.
  - Objects stored in MockObjectStore

TX2: Call entry functions
  - Load objects from MockObjectStore
  - Execute mint<MY_LST>(info, coin, ctx)
```

**Required Changes**:

1. **Mock object storage**:
```rust
pub struct MockObjectStore {
    objects: BTreeMap<ObjectID, StoredObject>,
}

struct StoredObject {
    type_tag: TypeTag,
    bytes: Vec<u8>,
    owner: Owner,
}
```

2. **Transfer natives that store objects**:
```rust
natives.push((
    "transfer",
    "share_object_impl",
    make_native(move |ctx, ty_args, mut args| {
        let obj = pop_arg!(args, Vec<u8>);
        let type_tag = ctx.type_to_type_tag(&ty_args[0])?;
        STORE.lock().insert(new_id(), StoredObject {
            type_tag,
            bytes: obj,
            owner: Owner::Shared,
        });
        Ok(NativeResult::ok(InternalGas::new(0), smallvec![]))
    }),
));
```

3. **Object loading for function calls**:
```rust
fn load_objects_for_function(
    func: &FunctionDef,
    module: &CompiledModule,
    store: &MockObjectStore,
) -> Vec<Vec<u8>> {
    let params = get_params(func, module);
    params.iter().filter_map(|p| {
        let type_tag = resolve_to_type_tag(p)?;
        store.find_by_type(&type_tag).map(|obj| obj.bytes.clone())
    }).collect()
}
```

**Effort**: 1-2 weeks for full implementation

---

## Critical Risks and Mitigations

### Risk 1: OTW Bypass Creates False Positives

**Scenario**: LLM generates invalid OTW that would fail on-chain but passes our mock.

**Mitigation**:
- Keep OTW mock as "test mode only"
- Document that OTW validation is bypassed
- Consider adding static analysis to verify OTW struct shape

### Risk 2: Object Storage Complexity

**Scenario**: Object ownership, borrowing, and lifecycle are complex to simulate.

**Mitigation**:
- Start with simple shared objects only
- Don't track ownership transfers
- Focus on "can we call the function" not "is the result correct"

### Risk 3: Init Function Side Effects

**Scenario**: Init functions may do things we can't simulate (e.g., call other packages).

**Mitigation**:
- Load all transitive dependencies
- Mock external calls that we can't handle
- Document limitations

### Risk 4: Type Matching Complexity

**Scenario**: Finding the right object to pass to a function is non-trivial.

**Example**: `mint<P>(&mut LiquidStakingInfo<P>, Coin<SUI>, ...)` needs:
- The specific `LiquidStakingInfo<MY_LST>` from init
- A `Coin<SUI>` (need to synthesize)

**Mitigation**:
- Match by type tag
- Synthesize simple types (Coin with zero balance)
- Skip functions that need types we can't provide

---

## Recommended Implementation Order

### Phase 1: Meaningful Operations (Week 1)
- Add function call tracking to ExecutionTrace
- Enable move-trace-format
- Report which target package functions were called

### Phase 2: Function Tracing (Week 1-2)
- Pass MoveTraceBuilder to execute_function
- Parse OpenFrame events
- Add to TierBDetails output

### Phase 3: Generic Instantiation (Week 2-3)
- Add synthetic type candidates
- Try multiple instantiations
- Report successful instantiation

### Phase 4: Witness Pattern (Week 3-4)
- Mock is_one_time_witness (test mode)
- Add MockObjectStore
- Modify transfer natives to store objects
- Implement init simulation
- Add object loading for subsequent calls

---

## Summary

| Enhancement | Difficulty | Value | Prerequisite |
|-------------|------------|-------|--------------|
| Meaningful operation tracking | Easy | Medium | None |
| Function-level tracing | Easy | High | move-trace-format |
| Generic instantiation | Medium | High | Type constraint solver |
| Witness pattern support | Hard | Critical | Mock storage + publish sim |

**Recommended order**: 1 → 2 → 3 → 4

The witness pattern is the key blocker for testing "real" DeFi packages. Without it, we can only test utility types, not core protocol types.

**Key Discovery**: The Move VM already has tracing support via `MoveTraceBuilder`. Phases 1-2 are much easier than initially thought.

**Critical Decision Point**: Phase 4 requires mocking `is_one_time_witness` which bypasses Sui's security model. This is acceptable for type inhabitation testing but must be documented and never used for security auditing.

---

*Last updated: January 2025*
