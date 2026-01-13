# Plausible Simulation: Implementation Checklist

## Overview

This document tracks the concrete implementation tasks for achieving "mainnet-plausible" local simulation.

---

## Phase 1: Permissive Crypto Mocks

**Goal**: Crypto verification returns success instead of aborting.

**Status**: Not Started

### Tasks

- [ ] **1.1** Update `ed25519_verify` to return `true`
  - File: `src/benchmark/natives.rs`
  - Current: `NativeResult::err(InternalGas::new(0), E_NOT_SUPPORTED)`
  - Target: `NativeResult::ok(InternalGas::new(0), smallvec![Value::bool(true)])`

- [ ] **1.2** Update `secp256k1_verify` to return `true`
  - File: `src/benchmark/natives.rs`

- [ ] **1.3** Update `secp256k1_ecrecover` to return valid pubkey bytes
  - Return: 33-byte compressed pubkey (starts with 0x02 or 0x03)

- [ ] **1.4** Update `secp256r1_verify` to return `true`

- [ ] **1.5** Update `secp256r1_ecrecover` to return valid pubkey bytes

- [ ] **1.6** Update `bls12381_min_sig_verify` to return `true`

- [ ] **1.7** Update `bls12381_min_pk_verify` to return `true`

- [ ] **1.8** Update `ecvrf_verify` to return `true`

- [ ] **1.9** Update `groth16::verify_groth16_proof_internal` to return `true`

- [ ] **1.10** Update `groth16::prepare_verifying_key_internal` to return valid struct
  - Need to understand return type structure

- [ ] **1.11** Update `hmac_sha3_256` to return 32 zero bytes
  - Valid output structure, just not cryptographically correct

- [ ] **1.12** Update `group_ops::internal_*` functions
  - These need careful analysis - some return `bool`, some return group elements

- [ ] **1.13** Update `poseidon_bn254` to return 32 zero bytes

- [ ] **1.14** Update `vdf_verify` to return `true`

- [ ] **1.15** Update `vdf_hash_to_input` to return valid bytes

- [ ] **1.16** Update `zklogin_verified_id::check_zklogin_id` to return `true`

- [ ] **1.17** Update `zklogin_verified_issuer::check_zklogin_issuer` to return `true`

### Testing

- [ ] Add unit tests for each mock
- [ ] Test with a package that uses crypto verification
- [ ] Verify no regressions in existing tests

---

## Phase 2: Clock & Randomness

**Goal**: Sensible system state values.

**Status**: Not Started

### Tasks

#### Clock

- [ ] **2.1** Create `MockClock` struct
  ```rust
  struct MockClock {
      base_ms: u64,      // Default: 1704067200000 (2024-01-01)
      tick_ms: u64,      // Default: 1000 (1 second)
      accesses: AtomicU64,
  }
  ```

- [ ] **2.2** Add `MockClock` to `MockNativeState`

- [ ] **2.3** Update `synthesize_clock()` in `vm.rs`
  - Current: Returns zeros
  - Target: Returns Clock with advancing timestamp

- [ ] **2.4** Add clock natives if missing
  - `clock::timestamp_ms` - returns advancing time

#### Randomness

- [ ] **2.5** Create `MockRandom` struct
  ```rust
  struct MockRandom {
      seed: [u8; 32],
      counter: AtomicU64,
  }
  ```

- [ ] **2.6** Update `random::random_internal` native
  - Current: Aborts
  - Target: Returns deterministic bytes based on seed + counter

- [ ] **2.7** Add configuration for random seed
  - Allow setting seed for reproducible tests

### Testing

- [ ] Test clock advancing behavior
- [ ] Test randomness determinism (same seed = same sequence)
- [ ] Test time-dependent package functions

---

## Phase 3: Test Utility Loading

**Goal**: LLM can use `mint_for_testing` and similar utilities.

**Status**: Not Started

### Research Tasks

- [ ] **3.1** Investigate framework bytecode structure
  - Do compiled frameworks include `#[test_only]` modules?
  - Where are framework bytecode files located?

- [ ] **3.2** Analyze `coin::mint_for_testing` signature
  - What module is it in?
  - What are the exact type parameters?

- [ ] **3.3** Check Move 2024 test module compilation
  - How does `sui move build --test` affect bytecode?

### Implementation Options

#### Option A: Load Test Modules

- [ ] **3.4a** Modify `LocalModuleResolver` to load test modules
- [ ] **3.5a** Rebuild framework with test modules included
- [ ] **3.6a** Add test module paths to resolver

#### Option B: Native Mocks for Test Utilities

- [ ] **3.4b** Add `coin::mint_for_testing` native
  ```rust
  fn mint_for_testing<T>(value: u64, ctx: &mut TxContext) -> Coin<T>
  ```

- [ ] **3.5b** Add `coin::burn_for_testing` native

- [ ] **3.6b** Add `balance::create_for_testing` native

- [ ] **3.7b** Add `balance::destroy_for_testing` native

### Testing

- [ ] Test minting coins
- [ ] Test burning coins
- [ ] Test with package that requires Coin input

---

## Phase 4: Object Store & Tracking

**Goal**: Track LLM-created objects across function calls.

**Status**: Not Started

### Tasks

- [ ] **4.1** Design `ObjectStore` struct
  ```rust
  struct ObjectStore {
      objects: HashMap<ObjectID, StoredObject>,
      shared: HashSet<ObjectID>,
  }

  struct StoredObject {
      value: Value,
      type_tag: TypeTag,
      owner: Owner,
      version: u64,
  }
  ```

- [ ] **4.2** Integrate with `ObjectRuntime`
  - Currently handles dynamic fields
  - Extend to handle top-level objects

- [ ] **4.3** Add object persistence across VM sessions
  - Current: Each `execute_function` gets fresh state
  - Target: Objects persist across calls in same harness

- [ ] **4.4** Implement object lookup by ID
  - For functions that take object references

- [ ] **4.5** Track ownership transitions
  - `transfer::transfer_impl` updates owner
  - `transfer::share_object_impl` marks as shared

- [ ] **4.6** Implement shared object semantics
  - Version tracking
  - Concurrent access (for simulation: just allow it)

### Testing

- [ ] Test object creation and retrieval
- [ ] Test ownership tracking
- [ ] Test shared objects
- [ ] Test multi-function scenarios

---

## Phase 5: Receiving Objects

**Goal**: Support `transfer::receive` pattern.

**Status**: Not Started

### Tasks

- [ ] **5.1** Understand receive pattern
  - How does `Receiving<T>` work?
  - What state is needed?

- [ ] **5.2** Add `pending_receives` to ObjectStore
  ```rust
  pending_receives: HashMap<(ObjectID, ObjectID), StoredObject>
  // (parent_id, child_id) -> object
  ```

- [ ] **5.3** Implement `receive_impl` native
  - Look up object in pending_receives
  - Return the object value

- [ ] **5.4** Add mechanism to "send" objects to another object
  - This creates the pending receive state

### Testing

- [ ] Test basic receive flow
- [ ] Test with packages using receive pattern

---

## Infrastructure Changes

### Configuration

- [ ] **I.1** Add `SimulationConfig` struct
  ```rust
  pub struct SimulationConfig {
      pub mock_crypto_pass: bool,
      pub deterministic_random: bool,
      pub advancing_clock: bool,
      pub permissive_ownership: bool,
      pub clock_base_ms: u64,
      pub random_seed: [u8; 32],
  }
  ```

- [ ] **I.2** Thread config through VMHarness

- [ ] **I.3** Add CLI flags for simulation mode
  - `--strict` for production-like behavior
  - `--permissive` (default) for benchmarking

### Logging & Debugging

- [ ] **I.4** Add simulation trace output
  - Log when mocks return fake values
  - Log ownership warnings in permissive mode

- [ ] **I.5** Add execution summary
  - Which mocks were used
  - What objects were created
  - Ownership state at end

### Documentation

- [ ] **I.6** Document simulation modes
- [ ] **I.7** Document what mocks do
- [ ] **I.8** Add examples of LLM-friendly patterns

---

## Verification Against Mainnet

### Optional Future Work

- [ ] **V.1** Add RPC client for mainnet comparison
  - Call `sui_devInspectTransactionBlock`
  - Compare local vs mainnet results

- [ ] **V.2** Add "strict mode" test suite
  - Run same code against local sim and mainnet
  - Verify behavior matches

- [ ] **V.3** Generate compatibility report
  - Which functions pass locally but fail on mainnet?
  - Which mocks cause false positives?

---

## File Change Summary

| File | Changes |
|------|---------|
| `src/benchmark/natives.rs` | Phases 1, 2, 3b, 5 |
| `src/benchmark/vm.rs` | Phases 2, 4 |
| `src/benchmark/object_runtime.rs` | Phase 4 |
| `src/benchmark/resolver.rs` | Phase 3a |
| `src/benchmark/mod.rs` | Config struct |
| `src/cli.rs` | CLI flags |

---

## Priority Matrix

| Phase | Effort | Impact | Priority |
|-------|--------|--------|----------|
| Phase 1 (Crypto) | Low | High | P0 |
| Phase 2 (Clock/Random) | Low | Medium | P1 |
| Phase 3 (Test Utils) | Medium | High | P1 |
| Phase 4 (Object Store) | High | Medium | P2 |
| Phase 5 (Receiving) | Low | Low | P3 |

**Recommended order**: 1 → 2 → 3 → 4 → 5
