# Local Move VM Sandbox: Implementation Checklist

## Overview

This document tracks the implementation tasks for the complete Local Move VM Sandbox.

**Execution Order**: Phase 1 → 2 → 3 → 6 → 4 → 5

---

## Phase 1: Permissive Crypto Mocks
**Priority: P0 | Effort: 4 hours | Status: ✅ COMPLETE**

### Tasks

- [x] **1.1** `ed25519::ed25519_verify` → return `true`
- [x] **1.2** `ecdsa_k1::secp256k1_verify` → return `true`
- [x] **1.3** `ecdsa_k1::secp256k1_ecrecover` → return 33-byte compressed pubkey `[0x02; 33]`
- [x] **1.4** `ecdsa_k1::decompress_pubkey` → return 65-byte uncompressed pubkey
- [x] **1.5** `ecdsa_r1::secp256r1_verify` → return `true`
- [x] **1.6** `ecdsa_r1::secp256r1_ecrecover` → return 33-byte compressed pubkey
- [x] **1.7** `bls12381::bls12381_min_sig_verify` → return `true`
- [x] **1.8** `bls12381::bls12381_min_pk_verify` → return `true`
- [x] **1.9** `ecvrf::ecvrf_verify` → return `true`
- [x] **1.10** `groth16::verify_groth16_proof_internal` → return `true`
- [x] **1.11** `groth16::prepare_verifying_key_internal` → return valid PreparedVerifyingKey bytes
- [x] **1.12** `hmac::hmac_sha3_256` → return 32 zero bytes
- [x] **1.13** `poseidon::poseidon_bn254` → return 32 zero bytes
- [x] **1.14** `vdf::vdf_verify` → return `true`
- [x] **1.15** `vdf::vdf_hash_to_input` → return 32 zero bytes
- [x] **1.16** `zklogin_verified_id::check_zklogin_id` → return `true`
- [x] **1.17** `zklogin_verified_issuer::check_zklogin_issuer` → return `true`
- [x] **1.18** `group_ops::internal_validate` → return `true`
- [x] **1.19** `group_ops::internal_add` → return valid group element bytes
- [x] **1.20** `group_ops::internal_sub` → return valid group element bytes
- [x] **1.21** `group_ops::internal_mul` → return valid group element bytes
- [x] **1.22** `group_ops::internal_div` → return valid group element bytes
- [x] **1.23** `group_ops::internal_hash_to` → return valid group element bytes
- [x] **1.24** `group_ops::internal_multi_scalar_mul` → return valid group element bytes
- [x] **1.25** `group_ops::internal_pairing` → return `true`
- [x] **1.26** `group_ops::internal_convert` → return valid bytes
- [x] **1.27** `group_ops::internal_sum` → return valid group element bytes

### Files
- `src/benchmark/natives.rs` - renamed `add_abort_stubs()` to `add_permissive_crypto_mocks()`

### Testing
- [x] Unit test each mock returns expected type
- [x] Integration test with package using crypto

---

## Phase 2: Clock & Randomness
**Priority: P1 | Effort: 6 hours | Status: ✅ COMPLETE**

### Tasks

#### Clock
- [x] **2.1** Create `MockClock` struct in `natives.rs`
  ```rust
  pub struct MockClock {
      base_ms: u64,        // 1704067200000 (2024-01-01 00:00:00 UTC)
      tick_ms: u64,        // 1000 (1 second)
      accesses: AtomicU64,
  }
  ```
- [x] **2.2** Add `MockClock` to `MockNativeState`
- [x] **2.3** Implement `clock::timestamp_ms` native (if not exists)
- [x] **2.4** Update `synthesize_clock()` in `vm.rs` to use MockClock

#### Randomness
- [x] **2.5** Create `MockRandom` struct
  ```rust
  pub struct MockRandom {
      seed: [u8; 32],
      counter: AtomicU64,
  }
  ```
- [x] **2.6** Add `MockRandom` to `MockNativeState`
- [x] **2.7** Update `random::random_internal` to return deterministic bytes

### Files
- `src/benchmark/natives.rs`
- `src/benchmark/vm.rs`

### Testing
- [x] Test clock advances on each access
- [x] Test randomness is deterministic (same seed = same sequence)
- [x] Test with time-dependent package

---

## Phase 3: Test Utility Loading
**Priority: P1 | Effort: 1-2 days | Status: ✅ COMPLETE (Option B)**

### Research
- [x] **3.1** Check if framework bytecode includes test modules - No, `#[test_only]` not included
- [x] **3.2** Determine how to compile framework with `--test` flag - Not needed, using Option B
- [x] **3.3** Analyze `coin::mint_for_testing` signature and implementation - Done

### Option A: Load Test Modules (Not Used)
- [ ] **3.4a** Modify `LocalModuleResolver` to detect and load test modules
- [ ] **3.5a** Update framework loading path to include test bytecode
- [ ] **3.6a** Test that `mint_for_testing` is callable

### Option B: Native Mocks (Implemented)
- [x] **3.4b** Implement `coin::mint_for_testing` native
- [x] **3.5b** Implement `coin::burn_for_testing` native
- [x] **3.6b** Implement `balance::create_for_testing` native
- [x] **3.7b** Implement `balance::destroy_for_testing` native
- [x] **3.8b** (Bonus) Implement `balance::create_supply_for_testing` native
- [x] **3.9b** (Bonus) Implement `balance::destroy_supply_for_testing` native

### Files
- `src/benchmark/natives.rs` - Added `add_test_utility_natives()` function

### Testing
- [x] All 98 library tests pass
- [ ] Test minting Coin<SUI> (integration test pending)
- [ ] Test minting Coin<T> with custom type (integration test pending)
- [ ] Test in constructor chain (integration test pending)

---

## Phase 6: PTB Executor
**Priority: P2 | Effort: 3-5 days | Status: ✅ COMPLETE**

### Core Infrastructure
- [x] **6.1** Create `src/benchmark/ptb.rs` module
- [x] **6.2** Define `Command` enum
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
- [x] **6.3** Define `Argument` enum
  ```rust
  pub enum Argument {
      Input(u16),
      Result(u16),
      NestedResult(u16, u16),
  }
  ```
- [x] **6.4** Define `CommandResult` enum
- [x] **6.5** Define `InputValue` enum (Pure, Object)

### PTBExecutor
- [x] **6.6** Create `PTBExecutor` struct
- [x] **6.7** Implement `resolve_args()` - resolve Input/Result/NestedResult
- [x] **6.8** Implement `execute()` - main command loop
- [x] **6.9** Implement `compute_effects()` - generate TransactionEffects

### Command Implementations
- [x] **6.10** Implement `MoveCall` - wire to `execute_function_with_return`
- [x] **6.11** Implement `SplitCoins` - split Coin into multiple
- [x] **6.12** Implement `MergeCoins` - merge multiple Coins into one
- [x] **6.13** Implement `TransferObjects` - update ownership
- [x] **6.14** Implement `MakeMoveVec` - construct vector from elements
- [ ] **6.15** Implement `Publish` - load new modules (optional, deferred)
- [ ] **6.16** Implement `Upgrade` - upgrade package (optional, deferred)

### Integration
- [ ] **6.17** Add PTB executor to benchmark harness (pending integration work)
- [ ] **6.18** Add PTB output format for LLM (pending integration work)
- [x] **6.19** Export from `src/benchmark/mod.rs`

### Files
- `src/benchmark/ptb.rs` (NEW)
- `src/benchmark/mod.rs`

### Testing
- [x] Test simple MoveCall chain (Result(0) → Input) - unit tests added
- [x] Test SplitCoins - unit tests added
- [x] Test MergeCoins - unit tests added
- [x] Test multi-command PTB - unit tests added
- [ ] Test with real package scenario (pending integration)

---

## Phase 4: Object Store & Persistence
**Priority: P2 | Effort: 2-3 days | Status: ✅ COMPLETE**

### Tasks
- [x] **4.1** Create `ObjectStore` struct
  ```rust
  pub struct ObjectStore {
      objects: HashMap<ObjectID, StoredObject>,
      shared: HashSet<ObjectID>,
      pending_receives: HashMap<(ObjectID, ObjectID), Vec<u8>>,
  }
  ```
- [x] **4.2** Create `StoredObject` struct
  ```rust
  pub struct StoredObject {
      bytes: Vec<u8>,
      type_tag: TypeTag,
      owner: Owner,
      version: u64,
      deleted: bool,
  }
  ```
- [x] **4.3** Implement `record_created()`
- [x] **4.4** Implement `get()` / `get_mut()`
- [x] **4.5** Implement `mark_shared()` and `mark_immutable()`
- [x] **4.6** Implement `delete()`
- [x] **4.7** Integrate with `ObjectRuntime` (via `object_store()` / `object_store_mut()`)
- [ ] **4.8** Add object persistence across VM sessions (deferred - not needed for current use case)

### Additional Implementations
- [x] **4.9** Create `Owner` enum (Address, Shared, Immutable, Object)
- [x] **4.10** Implement `transfer()` for ownership changes
- [x] **4.11** Implement `update_bytes()` for mutations

### Files
- `src/benchmark/object_runtime.rs` - Added ObjectStore, StoredObject, Owner types

### Testing
- [x] Test object creation and retrieval
- [x] Test object mutation (transfer, version increment)
- [x] Test shared object marking
- [x] Test delete operations
- [ ] Test cross-command object access in PTB (pending integration)

---

## Phase 5: Receiving Objects
**Priority: P3 | Effort: 4 hours | Status: ✅ COMPLETE**

### Tasks
- [x] **5.1** Add `pending_receives` to ObjectStore
- [x] **5.2** Implement `send_to_object()` - stage object for receiving
- [x] **5.3** Implement `receive_impl` native using ObjectStore

### Files
- `src/benchmark/natives.rs` - Updated `receive_impl` native
- `src/benchmark/object_runtime.rs` - Added `send_to_object()`, `receive_object()`, `has_pending_receive()`

### Testing
- [x] Test basic send → receive flow (unit tests in object_runtime.rs)
- [ ] Test with package using receive pattern (integration test pending)

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
| Phase 1: Crypto Mocks | ✅ Complete | 2025-01-12 | 2025-01-12 |
| Phase 2: Clock/Random | ✅ Complete | 2025-01-12 | 2025-01-12 |
| Phase 6: PTB Executor | ✅ Complete | 2025-01-12 | 2025-01-12 |
| Phase 3: Test Utils | ✅ Complete | 2025-01-12 | 2025-01-12 |
| Phase 4: Object Store | ✅ Complete | 2025-01-12 | 2025-01-12 |
| Phase 5: Receiving | ✅ Complete | 2025-01-12 | 2025-01-12 |

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
