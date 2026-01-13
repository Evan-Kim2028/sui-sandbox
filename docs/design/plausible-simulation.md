# Plausible Simulation: Design Philosophy

## Executive Summary

The goal is to create a **local simulation environment** that is permissive enough for LLM-generated Move code to execute successfully, while requiring the LLM to do the intellectual work of discovering and constructing all necessary objects and types.

**Key Principle**: The simulation removes artificial barriers, but the LLM must still write correct Move bytecode that constructs everything it needs.

---

## Design Philosophy

### What We Are NOT Doing

- **Not** auto-generating phantom objects for the LLM
- **Not** magically providing coins, pools, or other objects
- **Not** making the simulation "smart" about what the LLM needs
- **Not** bypassing Move's type system or safety guarantees

### What We ARE Doing

- Removing **infrastructure barriers** that prevent valid Move code from executing
- Allowing **test utilities** (like `coin::mint_for_testing`) to work
- Making **crypto verification** return success so code continues executing
- Providing **sensible defaults** for system state (clock, epoch, randomness)
- Letting the **LLM discover** patterns like phantom types, witness patterns, etc.

### The LLM's Responsibility

The LLM must:

1. **Analyze** the target package's interface and dependencies
2. **Discover** what types and objects are needed to call target functions
3. **Write Move code** that constructs those objects from scratch
4. **Chain** constructor outputs to target function inputs
5. **Handle** phantom types, witnesses, capabilities correctly

The simulation's job is simply: **if the Move code is valid, let it run**.

---

## Current State (v0.4.0)

### What Works

| Feature | Status | Notes |
|---------|--------|-------|
| Move VM execution | ✅ | Full bytecode execution |
| Framework modules | ✅ | 0x1, 0x2, 0x3 loaded |
| Basic natives | ✅ | vector, bcs, hash, string |
| TxContext synthesis | ✅ | Mocked sender, epoch, fresh_id |
| Object creation (UID) | ✅ | record_new_uid, delete_impl |
| Dynamic fields | ✅ | Full CRUD via ObjectRuntime |
| Constructor chaining | ✅ | Multi-hop producer chains |
| OTW validation | ✅ | Real is_one_time_witness check |

### What Blocks Valid Code

| Feature | Current | Impact |
|---------|---------|--------|
| Crypto verification | ❌ Aborts | Functions using signatures fail |
| Randomness | ❌ Aborts | Games, lotteries fail |
| Coin minting | ❌ Limited | No test utilities available |
| Clock access | ⚠️ Returns 0 | Time-dependent logic fails |
| Object loading | ❌ None | Can't load existing objects |
| Receiving | ❌ Aborts | receive pattern fails |

---

## Proposed Architecture: Plausible Simulation v0.5.0

### Layer Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         LLM-Generated Code                               │
│   (Discovers patterns, constructs objects, calls target functions)       │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                      Plausible Simulation Layer                          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────┐ │
│  │  Permissive     │  │   Test Utility  │  │    System State         │ │
│  │  Crypto Mocks   │  │   Enablement    │  │    Defaults             │ │
│  │                 │  │                 │  │                         │ │
│  │  - verify: true │  │  - mint_for_*   │  │  - Clock: advancing     │ │
│  │  - sign: valid  │  │  - burn_for_*   │  │  - Epoch: current       │ │
│  │  - recover: pk  │  │  - destroy_*    │  │  - Random: deterministic│ │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────┘ │
│                                                                          │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────┐ │
│  │  Object Store   │  │   Ownership     │  │    Gas Metering         │ │
│  │  (LLM-created)  │  │   (permissive)  │  │    (optional)           │ │
│  │                 │  │                 │  │                         │ │
│  │  - Tracks UIDs  │  │  - Log warnings │  │  - Unmetered default    │ │
│  │  - No phantoms  │  │  - Don't abort  │  │  - Optional limits      │ │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────┘ │
│                                                                          │
├─────────────────────────────────────────────────────────────────────────┤
│                      Move VM (existing v0.4.0)                           │
│                                                                          │
│  - LocalModuleResolver (framework + target + helper packages)            │
│  - NativeContextExtensions (ObjectRuntime for dynamic fields)            │
│  - ExecutionTrace (module access tracking)                               │
└─────────────────────────────────────────────────────────────────────────┘
```

### Component Details

#### 1. Permissive Crypto Mocks

**Goal**: Crypto operations succeed with plausible outputs, allowing code flow to continue.

```rust
// Current (v0.4.0): Aborts with E_NOT_SUPPORTED
fn ed25519_verify(...) -> NativeResult {
    NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED)
}

// Proposed (v0.5.0): Returns success
fn ed25519_verify(_sig: &[u8], _pk: &[u8], _msg: &[u8]) -> NativeResult {
    // Verification "passes" - allows code to continue
    NativeResult::ok(InternalGas::new(0), smallvec![Value::bool(true)])
}

fn secp256k1_ecrecover(_sig: &[u8], _msg: &[u8]) -> NativeResult {
    // Return valid-looking 33-byte compressed pubkey
    let fake_pk = vec![0x02u8; 33];
    NativeResult::ok(InternalGas::new(0), smallvec![Value::vector_u8(fake_pk)])
}
```

**Affected natives**:
- `ed25519::ed25519_verify` → returns `true`
- `ecdsa_k1::secp256k1_verify` → returns `true`
- `ecdsa_k1::secp256k1_ecrecover` → returns valid pubkey bytes
- `ecdsa_r1::secp256r1_verify` → returns `true`
- `bls12381::bls12381_min_sig_verify` → returns `true`
- `groth16::verify_groth16_proof_internal` → returns `true`

**LLM impact**: Can write code that uses signature verification. The LLM still needs to construct the signature bytes, public keys, etc. - we just don't fail the verification.

#### 2. Test Utility Enablement

**Goal**: Allow LLM to use test/debug utilities that create objects.

Many Sui packages have `#[test_only]` functions like:
- `coin::mint_for_testing<T>(value: u64, ctx: &mut TxContext): Coin<T>`
- `coin::burn_for_testing<T>(coin: Coin<T>)`
- `balance::create_for_testing<T>(value: u64): Balance<T>`

**Challenge**: These are gated by `#[test_only]` attribute.

**Solutions**:

Option A: **Include test modules in bytecode loading**
```rust
// When loading framework packages, include test modules
fn load_framework_with_tests() -> Vec<CompiledModule> {
    // Load both production and test modules
}
```

Option B: **Mock the underlying operations**
```rust
// If LLM tries to call mint_for_testing and it doesn't exist,
// provide a native that does the same thing
natives.push(("coin", "mint_for_testing", make_native(|ctx, ty_args, args| {
    // Create a Coin<T> with the requested value
    let value = pop_arg!(args, u64);
    let coin = synthesize_coin(ty_args[0], value, ctx);
    Ok(NativeResult::ok(InternalGas::new(0), smallvec![coin]))
})));
```

Option C: **LLM writes equivalent helper code**
```move
// LLM discovers it can create Balance directly and wrap it
module helper::coin_maker {
    use sui::balance::{Self, Balance};
    use sui::coin::{Self, Coin};

    public fun make_coin<T>(value: u64, ctx: &mut TxContext): Coin<T> {
        // This requires understanding Balance/Coin internals
        // But if the LLM figures it out, it should work
    }
}
```

**Recommendation**: Start with Option A (load test modules), fall back to Option B for critical utilities.

#### 3. System State Defaults

**Goal**: Provide sensible values for system state that most code will accept.

```rust
struct SystemState {
    clock: MockClock,
    epoch: u64,
    random_seed: u64,
}

struct MockClock {
    /// Base timestamp (default: 2024-01-01 00:00:00 UTC)
    base_ms: u64,
    /// Increment per access (default: 1000ms = 1 second)
    tick_ms: u64,
    /// Access counter
    accesses: AtomicU64,
}

impl MockClock {
    fn timestamp_ms(&self) -> u64 {
        let n = self.accesses.fetch_add(1, Ordering::SeqCst);
        self.base_ms + (n * self.tick_ms)
    }
}
```

**Clock behavior**:
- Starts at a reasonable timestamp (not 0)
- Advances on each access (simulates time passing)
- Most time-based assertions will pass

**Randomness**:
```rust
fn random_internal(seed_from_chain: &[u8]) -> Vec<u8> {
    // Deterministic "random" based on call count
    let n = RANDOM_COUNTER.fetch_add(1, Ordering::SeqCst);
    sha256(&[seed_from_chain, &n.to_le_bytes()].concat())
}
```

#### 4. Object Store (LLM-Created Only)

**Goal**: Track objects the LLM creates, don't auto-generate anything.

```rust
struct ObjectStore {
    /// Objects created during execution (by UID)
    objects: HashMap<ObjectID, StoredObject>,
    /// Shared object registry
    shared: HashMap<ObjectID, SharedObjectMeta>,
}

struct StoredObject {
    value: Value,
    type_tag: TypeTag,
    owner: Owner,
    version: u64,
}

impl ObjectStore {
    /// Called when LLM code creates a new object
    fn record_created(&mut self, id: ObjectID, value: Value, type_tag: TypeTag) {
        self.objects.insert(id, StoredObject {
            value,
            type_tag,
            owner: Owner::AddressOwner(self.sender),
            version: 1,
        });
    }

    /// Called when LLM code tries to load an object
    fn get(&self, id: ObjectID) -> Option<&StoredObject> {
        // Only return objects the LLM actually created
        // NO phantom generation
        self.objects.get(&id)
    }
}
```

**Key point**: If the LLM tries to reference an object it didn't create, the operation fails. The LLM must construct everything.

#### 5. Permissive Ownership

**Goal**: Track ownership for debugging but don't abort on violations.

```rust
enum OwnershipMode {
    /// Abort on ownership violations (production-like)
    Strict,
    /// Log warning, continue execution (benchmarking)
    Permissive,
}

impl OwnershipTracker {
    fn check_transfer(&self, obj_id: ObjectID, from: Owner, to: Owner) -> Result<()> {
        match self.mode {
            OwnershipMode::Strict => {
                if self.get_owner(obj_id) != from {
                    return Err(OwnershipViolation);
                }
            }
            OwnershipMode::Permissive => {
                if self.get_owner(obj_id) != from {
                    log::warn!("Ownership violation: {} not owned by {:?}", obj_id, from);
                    // Continue anyway
                }
            }
        }
        self.set_owner(obj_id, to);
        Ok(())
    }
}
```

---

## Implementation Roadmap

### Phase 1: Permissive Crypto (Low Effort, High Impact)

**Scope**: Change crypto natives from abort to success.

**Files to modify**:
- `src/benchmark/natives.rs` - Update `add_abort_stubs()` to return success values

**Effort**: ~2-4 hours

**Impact**: Unblocks all packages using signature verification, ZK proofs, etc.

### Phase 2: Clock & Randomness (Low Effort, Medium Impact)

**Scope**: Implement advancing clock and deterministic randomness.

**Files to modify**:
- `src/benchmark/natives.rs` - Add clock natives, random natives
- `src/benchmark/vm.rs` - Add SystemState to VMHarness

**Effort**: ~4-6 hours

**Impact**: Unblocks time-dependent and random-dependent code.

### Phase 3: Test Utility Loading (Medium Effort, High Impact)

**Scope**: Load `#[test_only]` modules from framework packages.

**Files to modify**:
- `src/benchmark/resolver.rs` - Include test modules in framework loading
- May need to rebuild framework bytecode with test modules

**Effort**: ~1-2 days

**Impact**: LLM can use `mint_for_testing`, `burn_for_testing`, etc.

### Phase 4: Object Store & Tracking (Medium Effort, Medium Impact)

**Scope**: Track objects created during execution, enable cross-function persistence.

**Files to modify**:
- `src/benchmark/object_runtime.rs` - Extend to full object store
- `src/benchmark/vm.rs` - Integrate object persistence across calls

**Effort**: ~2-3 days

**Impact**: LLM can create objects in one function, use them in another.

### Phase 5: Receiving Objects (Low Effort, Low Impact)

**Scope**: Implement `receive_impl` to work with object store.

**Files to modify**:
- `src/benchmark/natives.rs` - Implement receive_impl

**Effort**: ~4-6 hours

**Impact**: Unblocks receiving pattern (niche but important for some packages).

---

## Feasibility Analysis with Move Model 2

### What MM2 Provides

The current Move Model 2 (MM2) infrastructure gives us:

1. **Type Analysis**: Full type resolution, generic instantiation
2. **Constructor Graph**: Mapping from types to functions that produce them
3. **Producer Chains**: Multi-hop construction paths
4. **Function Signatures**: Parameter types, return types, visibility

### What MM2 Can Support

| Plausible Sim Feature | MM2 Support | Notes |
|-----------------------|-------------|-------|
| Crypto mocks | ✅ N/A | Pure native changes, no MM2 needed |
| Clock/Random | ✅ N/A | Pure native changes |
| Test utilities | ⚠️ Partial | Need to analyze test module signatures |
| Object tracking | ✅ Good | MM2 knows type layouts for serialization |
| Ownership | ⚠️ Partial | MM2 doesn't track runtime ownership |

### Gaps to Fill

1. **Test module discovery**: MM2 needs to see `#[test_only]` functions
2. **Capability pattern analysis**: Detecting witness/capability requirements
3. **Object lifecycle**: Tracking create → use → transfer → destroy

### Conclusion

**Yes, this is doable with MM2.** The core infrastructure exists. Main work is:
- Native function updates (Phases 1-2)
- Framework loading changes (Phase 3)
- ObjectRuntime extension (Phase 4)

---

## Success Metrics

### Quantitative

- **Package coverage**: % of packages where at least one target succeeds
- **Function coverage**: % of target functions that can be called
- **Execution rate**: % of LLM-generated code that executes without abort

### Qualitative

- LLM discovers phantom type patterns without hints
- LLM constructs complex objects (Coins, Pools) from primitives
- LLM handles witness patterns (OTW, capabilities)

### Baseline vs Target

| Metric | v0.4.0 (Current) | v0.5.0 (Target) |
|--------|------------------|-----------------|
| Package coverage | ~60% | ~85% |
| Crypto package support | 0% | ~90% |
| Time-dependent functions | ~10% | ~80% |
| Random-dependent functions | 0% | ~90% |

---

## Open Questions

1. **Test module bytecode**: Do we have test modules compiled, or need to rebuild?

2. **Framework version**: Which Sui framework version to target? (affects test utilities)

3. **Capability synthesis**: Should we allow LLM to create TreasuryCap, etc.?
   - Pro: Enables more function calls
   - Con: Unrealistic (you can't create TreasuryCap in real usage)

4. **Strict mode**: Should we have a flag to run in "strict" mode for validation?

5. **Mainnet comparison**: Should we add RPC calls to compare local vs mainnet results?

---

## Appendix: Native Function Inventory

### Currently Implemented (v0.4.0)

```
move-stdlib (0x1): vector, bcs, hash, string, type_name, debug, signer
sui-framework (0x2):
  - tx_context: all natives ✅
  - object: borrow_uid, delete_impl, record_new_uid ✅
  - transfer: transfer_impl, freeze_object_impl, share_object_impl ✅
  - event: emit ✅
  - dynamic_field: full CRUD ✅
  - address: from_bytes, to_u256, from_u256 ✅
  - types: is_one_time_witness ✅
  - hash: blake2b256, keccak256 ✅
```

### Needs Permissive Mocks (v0.5.0)

```
sui-framework (0x2):
  - bls12381: bls12381_min_sig_verify, bls12381_min_pk_verify
  - ecdsa_k1: secp256k1_ecrecover, decompress_pubkey, secp256k1_verify
  - ecdsa_r1: secp256r1_ecrecover, secp256r1_verify
  - ed25519: ed25519_verify
  - ecvrf: ecvrf_verify
  - groth16: verify_groth16_proof_internal, prepare_verifying_key_internal
  - hmac: hmac_sha3_256
  - group_ops: all internal_* functions
  - poseidon: poseidon_bn254
  - vdf: vdf_verify, vdf_hash_to_input
  - zklogin_verified_id: check_zklogin_id
  - zklogin_verified_issuer: check_zklogin_issuer
  - random: random_internal
  - clock: (need to add proper clock natives)
```

### Needs Implementation (v0.5.0)

```
sui-framework (0x2):
  - transfer: receive_impl (currently aborts)
  - config: read_setting_impl (currently aborts)
```
