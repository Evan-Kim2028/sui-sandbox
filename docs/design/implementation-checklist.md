# Local Move VM Sandbox: Implementation Checklist

## Overview

This document tracks the implementation tasks for the complete Local Move VM Sandbox.

**Execution Order**: Phase 1 → 2 → 3 → 6 → 4 → 5

---

## Phase 1: Permissive Crypto Mocks
**Priority: P0 | Effort: 4 hours | Status: Not Started**

### Tasks

- [ ] **1.1** `ed25519::ed25519_verify` → return `true`
- [ ] **1.2** `ecdsa_k1::secp256k1_verify` → return `true`
- [ ] **1.3** `ecdsa_k1::secp256k1_ecrecover` → return 33-byte compressed pubkey `[0x02; 33]`
- [ ] **1.4** `ecdsa_k1::decompress_pubkey` → return 65-byte uncompressed pubkey
- [ ] **1.5** `ecdsa_r1::secp256r1_verify` → return `true`
- [ ] **1.6** `ecdsa_r1::secp256r1_ecrecover` → return 33-byte compressed pubkey
- [ ] **1.7** `bls12381::bls12381_min_sig_verify` → return `true`
- [ ] **1.8** `bls12381::bls12381_min_pk_verify` → return `true`
- [ ] **1.9** `ecvrf::ecvrf_verify` → return `true`
- [ ] **1.10** `groth16::verify_groth16_proof_internal` → return `true`
- [ ] **1.11** `groth16::prepare_verifying_key_internal` → return valid PreparedVerifyingKey bytes
- [ ] **1.12** `hmac::hmac_sha3_256` → return 32 zero bytes
- [ ] **1.13** `poseidon::poseidon_bn254` → return 32 zero bytes
- [ ] **1.14** `vdf::vdf_verify` → return `true`
- [ ] **1.15** `vdf::vdf_hash_to_input` → return 32 zero bytes
- [ ] **1.16** `zklogin_verified_id::check_zklogin_id` → return `true`
- [ ] **1.17** `zklogin_verified_issuer::check_zklogin_issuer` → return `true`
- [ ] **1.18** `group_ops::internal_validate` → return `true`
- [ ] **1.19** `group_ops::internal_add` → return valid group element bytes
- [ ] **1.20** `group_ops::internal_sub` → return valid group element bytes
- [ ] **1.21** `group_ops::internal_mul` → return valid group element bytes
- [ ] **1.22** `group_ops::internal_div` → return valid group element bytes
- [ ] **1.23** `group_ops::internal_hash_to` → return valid group element bytes
- [ ] **1.24** `group_ops::internal_multi_scalar_mul` → return valid group element bytes
- [ ] **1.25** `group_ops::internal_pairing` → return `true`
- [ ] **1.26** `group_ops::internal_convert` → return valid bytes
- [ ] **1.27** `group_ops::internal_sum` → return valid group element bytes

### Files
- `src/benchmark/natives.rs` - modify `add_abort_stubs()` function

### Testing
- [ ] Unit test each mock returns expected type
- [ ] Integration test with package using crypto

---

## Phase 2: Clock & Randomness
**Priority: P1 | Effort: 6 hours | Status: Not Started**

### Tasks

#### Clock
- [ ] **2.1** Create `MockClock` struct in `natives.rs`
  ```rust
  pub struct MockClock {
      base_ms: u64,        // 1704067200000 (2024-01-01 00:00:00 UTC)
      tick_ms: u64,        // 1000 (1 second)
      accesses: AtomicU64,
  }
  ```
- [ ] **2.2** Add `MockClock` to `MockNativeState`
- [ ] **2.3** Implement `clock::timestamp_ms` native (if not exists)
- [ ] **2.4** Update `synthesize_clock()` in `vm.rs` to use MockClock

#### Randomness
- [ ] **2.5** Create `MockRandom` struct
  ```rust
  pub struct MockRandom {
      seed: [u8; 32],
      counter: AtomicU64,
  }
  ```
- [ ] **2.6** Add `MockRandom` to `MockNativeState`
- [ ] **2.7** Update `random::random_internal` to return deterministic bytes

### Files
- `src/benchmark/natives.rs`
- `src/benchmark/vm.rs`

### Testing
- [ ] Test clock advances on each access
- [ ] Test randomness is deterministic (same seed = same sequence)
- [ ] Test with time-dependent package

---

## Phase 3: Test Utility Loading
**Priority: P1 | Effort: 1-2 days | Status: Not Started**

### Research
- [ ] **3.1** Check if framework bytecode includes test modules
- [ ] **3.2** Determine how to compile framework with `--test` flag
- [ ] **3.3** Analyze `coin::mint_for_testing` signature and implementation

### Option A: Load Test Modules
- [ ] **3.4a** Modify `LocalModuleResolver` to detect and load test modules
- [ ] **3.5a** Update framework loading path to include test bytecode
- [ ] **3.6a** Test that `mint_for_testing` is callable

### Option B: Native Mocks (Fallback)
- [ ] **3.4b** Implement `coin::mint_for_testing` native
- [ ] **3.5b** Implement `coin::burn_for_testing` native
- [ ] **3.6b** Implement `balance::create_for_testing` native
- [ ] **3.7b** Implement `balance::destroy_for_testing` native

### Files
- `src/benchmark/resolver.rs` (Option A)
- `src/benchmark/natives.rs` (Option B)

### Testing
- [ ] Test minting Coin<SUI>
- [ ] Test minting Coin<T> with custom type
- [ ] Test in constructor chain

---

## Phase 6: PTB Executor
**Priority: P2 | Effort: 3-5 days | Status: Not Started**

### Core Infrastructure
- [ ] **6.1** Create `src/benchmark/ptb.rs` module
- [ ] **6.2** Define `Command` enum
  ```rust
  pub enum Command {
      MoveCall { package, module, function, type_args, args },
      SplitCoins { coin, amounts },
      MergeCoins { destination, sources },
      TransferObjects { objects, address },
      MakeMoveVec { type_tag, elements },
      Publish { modules, dep_ids },
      Upgrade { modules, package, ticket },
  }
  ```
- [ ] **6.3** Define `Argument` enum
  ```rust
  pub enum Argument {
      Input(u16),
      Result(u16),
      NestedResult(u16, u16),
  }
  ```
- [ ] **6.4** Define `CommandResult` enum
- [ ] **6.5** Define `InputValue` enum (Pure, Object)

### PTBExecutor
- [ ] **6.6** Create `PTBExecutor` struct
- [ ] **6.7** Implement `resolve_args()` - resolve Input/Result/NestedResult
- [ ] **6.8** Implement `execute()` - main command loop
- [ ] **6.9** Implement `compute_effects()` - generate TransactionEffects

### Command Implementations
- [ ] **6.10** Implement `MoveCall` - wire to `execute_function_with_return`
- [ ] **6.11** Implement `SplitCoins` - split Coin into multiple
- [ ] **6.12** Implement `MergeCoins` - merge multiple Coins into one
- [ ] **6.13** Implement `TransferObjects` - update ownership
- [ ] **6.14** Implement `MakeMoveVec` - construct vector from elements
- [ ] **6.15** Implement `Publish` - load new modules (optional)
- [ ] **6.16** Implement `Upgrade` - upgrade package (optional)

### Integration
- [ ] **6.17** Add PTB executor to benchmark harness
- [ ] **6.18** Add PTB output format for LLM
- [ ] **6.19** Export from `src/benchmark/mod.rs`

### Files
- `src/benchmark/ptb.rs` (NEW)
- `src/benchmark/mod.rs`

### Testing
- [ ] Test simple MoveCall chain (Result(0) → Input)
- [ ] Test SplitCoins
- [ ] Test MergeCoins
- [ ] Test multi-command PTB
- [ ] Test with real package scenario

---

## Phase 4: Object Store & Persistence
**Priority: P2 | Effort: 2-3 days | Status: Not Started**

### Tasks
- [ ] **4.1** Create `ObjectStore` struct
  ```rust
  pub struct ObjectStore {
      objects: HashMap<ObjectID, StoredObject>,
      shared: HashSet<ObjectID>,
  }
  ```
- [ ] **4.2** Create `StoredObject` struct
  ```rust
  pub struct StoredObject {
      value: Value,
      type_tag: TypeTag,
      owner: Owner,
      version: u64,
  }
  ```
- [ ] **4.3** Implement `record_created()`
- [ ] **4.4** Implement `get()` / `get_mut()`
- [ ] **4.5** Implement `mark_shared()`
- [ ] **4.6** Implement `delete()`
- [ ] **4.7** Integrate with `ObjectRuntime`
- [ ] **4.8** Add object persistence across VM sessions

### Files
- `src/benchmark/object_runtime.rs`
- `src/benchmark/vm.rs`

### Testing
- [ ] Test object creation and retrieval
- [ ] Test object mutation
- [ ] Test shared object marking
- [ ] Test cross-command object access in PTB

---

## Phase 5: Receiving Objects
**Priority: P3 | Effort: 4 hours | Status: Not Started**

### Tasks
- [ ] **5.1** Add `pending_receives` to ObjectStore
- [ ] **5.2** Implement `send_to_object()` - stage object for receiving
- [ ] **5.3** Implement `receive_impl` native using ObjectStore

### Files
- `src/benchmark/natives.rs`
- `src/benchmark/object_runtime.rs`

### Testing
- [ ] Test basic send → receive flow
- [ ] Test with package using receive pattern

---

## Configuration & Infrastructure

### SimulationConfig
- [ ] **I.1** Create config struct
  ```rust
  pub struct SimulationConfig {
      pub mock_crypto_pass: bool,    // default: true
      pub advancing_clock: bool,     // default: true
      pub deterministic_random: bool,// default: true
      pub permissive_ownership: bool,// default: true
      pub clock_base_ms: u64,        // default: 1704067200000
      pub random_seed: [u8; 32],     // default: zeros
  }
  ```
- [ ] **I.2** Thread config through VMHarness
- [ ] **I.3** Add CLI flags for config options

### Documentation
- [ ] **I.4** Update README with sandbox documentation
- [ ] **I.5** Add examples of LLM usage patterns
- [ ] **I.6** Document what is mocked vs real

---

## Progress Tracking

| Phase | Status | Started | Completed |
|-------|--------|---------|-----------|
| Phase 1: Crypto Mocks | Not Started | - | - |
| Phase 2: Clock/Random | Not Started | - | - |
| Phase 3: Test Utils | Not Started | - | - |
| Phase 6: PTB Executor | Not Started | - | - |
| Phase 4: Object Store | Not Started | - | - |
| Phase 5: Receiving | Not Started | - | - |

---

## Verification Checklist

After all phases complete:

### Type System (must be 100% accurate)
- [ ] Phantom types enforced correctly
- [ ] Abilities (key, store, copy, drop) enforced
- [ ] Generic instantiation works
- [ ] Visibility rules enforced
- [ ] OTW validation works

### Execution Coverage
- [ ] Crypto-using functions execute
- [ ] Time-dependent functions execute
- [ ] Random-using functions execute
- [ ] Multi-hop constructor chains work
- [ ] PTB command sequences work

### LLM Experience
- [ ] LLM can discover phantom types by error
- [ ] LLM can find OTW pattern requirements
- [ ] LLM can chain constructors
- [ ] LLM can write valid PTBs
