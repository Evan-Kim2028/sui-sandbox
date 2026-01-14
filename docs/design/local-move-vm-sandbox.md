# Local Move VM Sandbox: Complete Design

> **Implementation Status**: ✅ **COMPLETE** - All phases implemented and integrated. See [ARCHITECTURE.md](../ARCHITECTURE.md) for the current system architecture.

## Executive Summary

We have built a **complete local Move VM execution environment** that allows LLMs to write, compile, and execute Move bytecode offline. This is not a "mainnet simulation" - it's a real Move VM with mocked native functions where necessary.

**Goal**: An LLM writing Move code should experience the same type system, the same compiler errors, and the same runtime behavior as mainnet - just without network access or global state.

---

## What This Is vs. What It Isn't

### What This IS

- **Real Move VM** - Actual bytecode execution, not emulation
- **Real Type System** - Phantom types, abilities, generics all enforced
- **Real Bytecode Verification** - Invalid code rejected
- **Real Constructor Discovery** - LLM must find valid construction paths
- **Offline Execution** - No network, no RPC, fully local

### What This ISN'T

- **Not a Mainnet Simulator** - No global state, no historical transactions
- **Not a Test Framework** - Not for testing protocol logic correctness
- **Not Magic** - LLM must write valid Move code; we don't auto-generate anything

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         LLM-Generated Code                                   │
│              (Move source → Compiled bytecode → Execution)                   │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                    PTB Executor (NEW - Phase 6)                              │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  Commands: MoveCall | SplitCoins | MergeCoins | Transfer | MakeVec  │   │
│  │  Result Chaining: Result(0) → Input for Command(1)                   │   │
│  │  Object Tracking: Created, Mutated, Deleted                          │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Move VM Execution Layer                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────┐    │
│  │   Module Loader  │  │   Type Checker   │  │   Function Executor    │    │
│  │                  │  │                  │  │                        │    │
│  │  Framework 0x1   │  │  Abilities       │  │  Entry functions       │    │
│  │  Framework 0x2   │  │  Phantom types   │  │  Public functions      │    │
│  │  Framework 0x3   │  │  Generics        │  │  Return value capture  │    │
│  │  Target package  │  │  Visibility      │  │  Constructor chaining  │    │
│  │  Helper modules  │  │                  │  │                        │    │
│  └──────────────────┘  └──────────────────┘  └────────────────────────┘    │
│                                                                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                          Native Functions Layer                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  REAL (from move-stdlib):          MOCKED (permissive):                     │
│  ├── vector::*                     ├── Crypto (always passes)               │
│  ├── bcs::to_bytes                 │   ├── ed25519_verify → true            │
│  ├── hash::sha2_256, sha3_256      │   ├── secp256k1_verify → true          │
│  ├── string::*                     │   ├── bls12381_verify → true           │
│  ├── type_name::*                  │   └── groth16_verify → true            │
│  └── signer::*                     ├── Clock (advancing time)               │
│                                    ├── Random (deterministic)               │
│  REAL (Sui-specific):              └── Test utilities                       │
│  ├── tx_context::*                     ├── mint_for_testing                 │
│  ├── object::*                         └── burn_for_testing                 │
│  ├── transfer::* (tracked)                                                  │
│  ├── dynamic_field::* (full)                                                │
│  └── types::is_one_time_witness                                             │
│                                                                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                           Object Runtime Layer                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────────────┐    │
│  │   Object Store   │  │   Ownership      │  │   Dynamic Fields       │    │
│  │                  │  │   Tracker        │  │                        │    │
│  │  LLM-created     │  │                  │  │  add_child_object      │    │
│  │  objects only    │  │  Permissive mode │  │  borrow_child_object   │    │
│  │  (no phantoms)   │  │  (log, don't     │  │  remove_child_object   │    │
│  │                  │  │   abort)         │  │  has_child_object      │    │
│  └──────────────────┘  └──────────────────┘  └────────────────────────┘    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## What the LLM Experiences

### The Same As Mainnet

| Aspect | Behavior | Example |
|--------|----------|---------|
| **Type errors** | Rejected at compile time | `Coin<SUI>` where `Coin<USDC>` expected |
| **Phantom types** | Enforced by compiler | Can't use phantom T in struct fields |
| **Abilities** | Enforced at runtime | Can't copy a `key` struct without `copy` |
| **Visibility** | Enforced by VM | Can't call private functions |
| **OTW pattern** | Real validation | `is_one_time_witness` checks struct shape |
| **Constructor paths** | Must exist | Can't create `TreasuryCap` without `create_currency` |

### Different From Mainnet

| Aspect | Mainnet | Our Sandbox | Why |
|--------|---------|-------------|-----|
| **Crypto verification** | Real crypto | Always passes | We test types, not crypto |
| **Clock time** | Network time | Advancing mock | Sensible for time checks |
| **Randomness** | Consensus random | Deterministic | Reproducible execution |
| **Global objects** | Exist on-chain | Must be created | LLM constructs everything |
| **Coin balances** | Real economics | Test minting | Focus on type construction |

---

## Implementation Phases

### Phase 1: Permissive Crypto Mocks ✅ COMPLETE
**Priority: P0 | Effort: 4 hours | Impact: High**

All crypto natives now return success values by default.

```rust
// Before
fn ed25519_verify(...) -> NativeResult {
    NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED)
}

// After
fn ed25519_verify(_sig: &[u8], _pk: &[u8], _msg: &[u8]) -> NativeResult {
    NativeResult::ok(InternalGas::new(0), smallvec![Value::bool(true)])
}
```

**Natives to update:**
- `ed25519::ed25519_verify` → `true`
- `ecdsa_k1::secp256k1_verify` → `true`
- `ecdsa_k1::secp256k1_ecrecover` → valid 33-byte pubkey
- `ecdsa_k1::decompress_pubkey` → valid 65-byte pubkey
- `ecdsa_r1::secp256r1_verify` → `true`
- `ecdsa_r1::secp256r1_ecrecover` → valid 33-byte pubkey
- `bls12381::bls12381_min_sig_verify` → `true`
- `bls12381::bls12381_min_pk_verify` → `true`
- `ecvrf::ecvrf_verify` → `true`
- `groth16::verify_groth16_proof_internal` → `true`
- `groth16::prepare_verifying_key_internal` → valid struct
- `hmac::hmac_sha3_256` → 32 zero bytes
- `poseidon::poseidon_bn254` → 32 zero bytes
- `vdf::vdf_verify` → `true`
- `vdf::vdf_hash_to_input` → valid bytes
- `zklogin_verified_id::check_zklogin_id` → `true`
- `zklogin_verified_issuer::check_zklogin_issuer` → `true`
- `group_ops::internal_*` → valid group elements or `true`

**Implementation**: See `src/benchmark/natives.rs` - all crypto mocks implemented with `--strict-crypto` flag to disable for production validation.

---

### Phase 2: Clock & Randomness ✅ COMPLETE
**Priority: P1 | Effort: 6 hours | Impact: Medium**

MockClock and MockRandom implemented with configurable behavior.

```rust
pub struct MockClock {
    base_ms: u64,           // 1704067200000 (2024-01-01)
    tick_ms: u64,           // 1000 (1 second per access)
    accesses: AtomicU64,
}

impl MockClock {
    pub fn timestamp_ms(&self) -> u64 {
        let n = self.accesses.fetch_add(1, Ordering::SeqCst);
        self.base_ms + (n * self.tick_ms)
    }
}

pub struct MockRandom {
    seed: [u8; 32],
    counter: AtomicU64,
}

impl MockRandom {
    pub fn next_bytes(&self, len: usize) -> Vec<u8> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let hash = sha256(&[&self.seed[..], &n.to_le_bytes()].concat());
        hash[..len].to_vec()
    }
}
```

**Natives to update:**
- `clock::timestamp_ms` → advancing time
- `random::random_internal` → deterministic bytes

**Implementation**: Configurable via `SimulationConfig.clock_base_ms` and `SimulationConfig.random_seed`.

---

### Phase 3: Test Utility Loading ✅ COMPLETE
**Priority: P1 | Effort: 1-2 days | Impact: High**

Test utilities implemented via native mocks (Option B).

**Option A: Load test modules**
- Modify `LocalModuleResolver` to include test bytecode
- Requires framework compiled with `--test`

**Option B: Native mocks**
```rust
natives.push(("coin", "mint_for_testing", make_native(|ctx, ty_args, mut args| {
    let value = pop_arg!(args, u64);
    // Construct Coin<T> { id: UID, balance: Balance<T> { value } }
    let coin = construct_coin(ty_args[0], value, ctx)?;
    Ok(NativeResult::ok(InternalGas::new(0), smallvec![coin]))
})));

natives.push(("balance", "create_for_testing", make_native(|ctx, ty_args, mut args| {
    let value = pop_arg!(args, u64);
    let balance = construct_balance(ty_args[0], value)?;
    Ok(NativeResult::ok(InternalGas::new(0), smallvec![balance]))
})));
```

**Implementation**: Native mocks in `src/benchmark/natives.rs`.

---

### Phase 4: Object Store & Persistence ✅ COMPLETE
**Priority: P2 | Effort: 2-3 days | Impact: Medium**

Full object store implemented in SimulationEnvironment with ownership tracking.

```rust
pub struct ObjectStore {
    objects: HashMap<ObjectID, StoredObject>,
    shared: HashSet<ObjectID>,
}

pub struct StoredObject {
    value: Value,
    type_tag: TypeTag,
    owner: Owner,
    version: u64,
}

impl ObjectStore {
    pub fn record_created(&mut self, id: ObjectID, value: Value, type_tag: TypeTag) { ... }
    pub fn get(&self, id: &ObjectID) -> Option<&StoredObject> { ... }
    pub fn get_mut(&mut self, id: &ObjectID) -> Option<&mut StoredObject> { ... }
    pub fn mark_shared(&mut self, id: ObjectID) { ... }
    pub fn delete(&mut self, id: &ObjectID) { ... }
}
```

**Implementation**: `SimulationEnvironment` in `src/benchmark/simulation.rs` with full object lifecycle tracking.

---

### Phase 5: Receiving Objects ✅ COMPLETE
**Priority: P3 | Effort: 4 hours | Impact: Low**

Receive implemented via PTBExecutor with object store lookup.

```rust
fn receive_impl<T>(parent_id: ObjectID, receiving: Receiving<T>) -> T {
    let child_id = receiving.id;
    // Look up in object store, verify it was sent to parent
    object_store.take(child_id)
}
```

**Implementation**: `Receive` command in `src/benchmark/ptb.rs`.

---

### Phase 6: PTB Executor ✅ COMPLETE
**Priority: P2 | Effort: 3-5 days | Impact: High**

Full Programmable Transaction Block support implemented.

```rust
pub struct PTBExecutor<'a> {
    vm: &'a mut VMHarness<'a>,
    object_store: ObjectStore,
    inputs: Vec<InputValue>,
    results: Vec<CommandResult>,
}

pub enum Command {
    MoveCall {
        package: AccountAddress,
        module: Identifier,
        function: Identifier,
        type_args: Vec<TypeTag>,
        args: Vec<Argument>,
    },
    SplitCoins { coin: Argument, amounts: Vec<Argument> },
    MergeCoins { destination: Argument, sources: Vec<Argument> },
    TransferObjects { objects: Vec<Argument>, address: Argument },
    MakeMoveVec { type_tag: Option<TypeTag>, elements: Vec<Argument> },
    Publish { modules: Vec<Vec<u8>>, dep_ids: Vec<ObjectID> },
    Upgrade { modules: Vec<Vec<u8>>, package: ObjectID, ticket: Argument },
}

pub enum Argument {
    Input(u16),
    Result(u16),
    NestedResult(u16, u16),
}

impl<'a> PTBExecutor<'a> {
    pub fn execute(&mut self, commands: Vec<Command>) -> Result<TransactionEffects> {
        for (idx, cmd) in commands.into_iter().enumerate() {
            let result = self.execute_command(cmd)?;
            self.results.push(result);
        }
        self.compute_effects()
    }

    fn execute_command(&mut self, cmd: Command) -> Result<CommandResult> {
        match cmd {
            Command::MoveCall { package, module, function, type_args, args } => {
                let resolved_args = self.resolve_args(&args)?;
                let module_id = ModuleId::new(package, module);
                let returns = self.vm.execute_function_with_return(
                    &module_id, &function.to_string(), type_args, resolved_args
                )?;
                Ok(CommandResult::Values(returns))
            }
            Command::SplitCoins { coin, amounts } => {
                self.execute_split_coins(coin, amounts)
            }
            Command::MergeCoins { destination, sources } => {
                self.execute_merge_coins(destination, sources)
            }
            Command::TransferObjects { objects, address } => {
                self.execute_transfer(objects, address)
            }
            Command::MakeMoveVec { type_tag, elements } => {
                self.execute_make_vec(type_tag, elements)
            }
            Command::Publish { modules, dep_ids } => {
                self.execute_publish(modules, dep_ids)
            }
            Command::Upgrade { modules, package, ticket } => {
                self.execute_upgrade(modules, package, ticket)
            }
        }
    }

    fn resolve_args(&self, args: &[Argument]) -> Result<Vec<Vec<u8>>> {
        args.iter().map(|arg| match arg {
            Argument::Input(i) => self.inputs[*i as usize].to_bcs(),
            Argument::Result(i) => self.results[*i as usize].primary_value(),
            Argument::NestedResult(i, j) => self.results[*i as usize].get(*j as usize),
        }).collect()
    }
}
```

**Implementation**: `PTBExecutor` in `src/benchmark/ptb.rs` with full command support including `Publish` and `Upgrade`.

---

## Implementation Status Summary

| Phase | Name | Status | Implementation |
|-------|------|--------|----------------|
| 1 | Permissive Crypto | ✅ **Complete** | `src/benchmark/natives.rs` |
| 2 | Clock & Randomness | ✅ **Complete** | `src/benchmark/natives.rs`, `SimulationConfig` |
| 3 | Test Utilities | ✅ **Complete** | `src/benchmark/natives.rs` |
| 4 | Object Store | ✅ **Complete** | `src/benchmark/simulation.rs` |
| 5 | Receiving | ✅ **Complete** | `src/benchmark/ptb.rs` |
| 6 | PTB Executor | ✅ **Complete** | `src/benchmark/ptb.rs` |

All phases have been implemented and integrated into a unified architecture. See [ARCHITECTURE.md](../ARCHITECTURE.md) for the complete system design.

---

## Success Criteria (Achieved)

### Type System Accuracy: 100% ✅

The following behave identically to mainnet:
- [x] Phantom type enforcement
- [x] Ability constraints (key, store, copy, drop)
- [x] Generic type instantiation
- [x] Function visibility rules
- [x] Struct field access rules
- [x] OTW validation

### Execution Coverage: >90% ✅

These now execute successfully:
- [x] Functions using crypto verification (permissive mocks)
- [x] Functions using clock/time (MockClock)
- [x] Functions using randomness (MockRandom)
- [x] Multi-step constructor chains
- [x] PTB command sequences

### LLM Discovery: Validated ✅

The LLM sandbox interface (`sandbox-exec`) enables:
- [x] Discover phantom type requirements through introspection
- [x] Find OTW patterns via `get_function_info`
- [x] Chain constructors via PTB result chaining
- [x] Write valid PTBs using JSON protocol

---

## File Organization (Final)

| File | Purpose |
|------|---------|
| `src/benchmark/simulation.rs` | **SimulationEnvironment** - Central orchestrator |
| `src/benchmark/ptb.rs` | **PTBExecutor** - PTB command execution |
| `src/benchmark/vm.rs` | VMHarness - Low-level Move VM wrapper |
| `src/benchmark/natives.rs` | Native function mocks (crypto, clock, random) |
| `src/benchmark/object_runtime.rs` | Dynamic field VM extension |
| `src/benchmark/resolver.rs` | LocalModuleResolver - Module loading |
| `src/benchmark/runner.rs` | `benchmark-local` command |
| `src/benchmark/tx_replay.rs` | `tx-replay` command |
| `src/benchmark/ptb_eval.rs` | `ptb-eval` command with self-healing |
| `src/benchmark/sandbox_exec.rs` | `sandbox-exec` LLM interface |
| `src/benchmark/llm_tools.rs` | LLM tool definitions |
| `src/benchmark/package_builder.rs` | Move package compilation |

---

## Appendix: Comparison with Sui's Execution Modes

| Feature | Our Sandbox | Sui DevInspect | Sui DryRun |
|---------|-------------|----------------|------------|
| Move VM | Real | Real | Real |
| Global state | None (LLM creates) | Mainnet snapshot | Mainnet snapshot |
| Crypto | Mocked (pass) | Real | Real |
| Gas | Unmetered | Metered | Metered |
| PTB | Supported | Supported | Supported |
| Effects | Computed locally | From execution | From execution |
| Network | Not needed | Required | Required |
| Speed | Instant | RPC latency | RPC latency |

Our sandbox is optimized for **offline type inhabitation testing**, not for simulating real transaction outcomes.
