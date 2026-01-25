# Known Limitations

This document describes known limitations and differences between the local Move VM sandbox and the actual Sui network. Understanding these limitations is important for:

- Interpreting simulation results
- Designing test scenarios
- Understanding when local execution may differ from on-chain behavior

## Table of Contents

1. [PTB Execution Edge Cases](#ptb-execution-edge-cases) *(7 issues fixed, 4 fully mitigated, 0 remaining - all verified against Sui source)*
2. [Gas Model](#gas-model)
3. [Object Runtime](#object-runtime)
4. [Cryptographic Operations](#cryptographic-operations)
5. [Time and Randomness](#time-and-randomness)
6. [Transaction Context](#transaction-context)
7. [Dynamic Fields](#dynamic-fields)
8. [Package Upgrades](#package-upgrades)
9. [Receiving Objects](#receiving-objects)
10. [Native Functions](#native-functions)

---

## PTB Execution Edge Cases

The PTB (Programmable Transaction Block) executor has several edge cases where local simulation may diverge from on-chain behavior. These are particularly relevant when commands operate on `Result` or `NestedResult` arguments rather than direct `Input` arguments.

**Note:** All remaining issues have been verified against the official Sui source code in `sui-execution/latest/sui-adapter/src/programmable_transactions/`. References to specific files and line numbers are provided for each issue.

### ~~SplitCoins Result Arguments Not Updated~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** When `SplitCoins` operates on a coin from a previous command's `Result` or `NestedResult`, the balance reduction was not persisted back to the Result storage.

**Fix:** Added `update_arg_bytes()` helper method that handles Input, Result, and NestedResult arguments uniformly. SplitCoins now uses this method to update coin balances for all argument types.

**Sui Source Reference:** `execution.rs:375` - `context.restore_arg::<Mode>(&mut argument_updates, coin_arg, Value::Object(obj))?;`

### ~~MergeCoins Result Destination Not Updated~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** When `MergeCoins` destination was a `Result` or `NestedResult`, the merged balance was not persisted back.

**Fix:** MergeCoins now uses `update_arg_bytes()` to update the destination balance for all argument types.

**Sui Source Reference:** `execution.rs:429-433` - `context.restore_arg::<Mode>(&mut argument_updates, target_arg, Value::Object(target))?;`

### ~~MergeCoins Result Sources Not Zeroed~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** When `MergeCoins` deleted source coins from `Result` or `NestedResult`, they were not zeroed and could potentially be reused (double-spend).

**Fix:** MergeCoins now zeros source coin bytes for all argument types using `update_arg_bytes()`, preventing double-spend of Result-based sources.

**Sui Source Reference:** `context.rs:535` - `by_value_arg` uses `val_opt.take().unwrap()` which sets the value to `None`, consuming ownership and preventing reuse.

### ~~Input Not Synced After MoveCall Mutation~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** When a MoveCall mutated an Input object via mutable reference, `apply_mutable_ref_outputs` updated the Result but the original Input was never updated.

**Fix:** `apply_mutable_ref_outputs` now uses the unified `update_arg_bytes()` method which properly handles all argument types including Inputs.

**Sui Source Reference:** `execution.rs:570-601` - `write_back_results` calls `context.restore_arg::<Mode>(argument_updates, arguments[arg_idx], value)?;` for each mutable reference output.

### ~~NestedResult Bounds Check Silent Failure~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** When updating a `NestedResult` with an out-of-bounds index, the update silently failed without error.

**Fix:** The new `update_arg_bytes()` method returns proper errors when indices are out of bounds, preventing silent failures.

**Sui Source Reference:** Sui uses `assert_invariant!` macros throughout `context.rs` for bounds checking and validity (e.g., lines 673-676, 703-706).

### ~~Unknown Coin Type Defaults to SUI~~ **FULLY MITIGATED**

**Status:** ✅ Fully Mitigated | **Verified Against Sui Source:** ✅

**Original Issue:** When `SplitCoins` couldn't determine the coin type, it fell back to `Coin<SUI>`.

**Solution Implemented:**
- Added `FunctionSignature` lookup in `resolver.rs` to inspect compiled bytecode BEFORE execution
- `signature_token_to_type_tag()` converts Move bytecode signatures to `TypeTag`, including:
  - All primitives (u8, u64, bool, address, etc.)
  - Structs via `Datatype` handle lookup
  - Generic instantiations via `DatatypeInstantiation` with type parameter substitution
  - Type parameters resolved from MoveCall type arguments
- `resolve_function_return_types()` returns expected return types for any function
- `execute_move_call()` now pre-computes return types and pairs them with values via `TypedValue`

**Technical Details:**
- Code locations: `resolver.rs:signature_token_to_type_tag()`, `resolver.rs:resolve_function_return_types()`, `ptb.rs:execute_move_call()`
- Type resolution uses `CompiledModule::function_handles`, `signatures`, and `datatype_handles`
- Falls back gracefully if signature lookup fails (empty return type list)

**Sui Source Comparison:**
- Sui uses `LoadedFunctionInfo` with return types resolved at load time
- Our approach achieves the same result by looking up function signatures before execution

**Impact:** Full type tracking now works for all commands including MoveCall returns.

### ~~Object ID Inference Fragile~~ **FULLY MITIGATED**

**Status:** ✅ Fully Mitigated | **Verified Against Sui Source:** ✅

**Original Issue:** The `get_object_id_and_type_from_arg` function assumed the first 32 bytes of any Result value are the object ID.

**Solution Implemented:**
- `TypedValue` now carries type information with Result values from function signature lookup
- MoveCall returns are paired with pre-computed types from `resolve_function_return_types()`
- `get_object_id_and_type_from_arg` checks TypedValue's `type_tag` for proper type identification

**Technical Details:**
- Function signature lookup provides return types BEFORE execution
- `TypedValue::new(bytes, Some(type_tag))` pairs bytes with known types
- All built-in commands (SplitCoins, MergeCoins, MakeMoveVec, Receive) preserve types
- MoveCall returns now have proper types from bytecode introspection

**Sui Source Comparison:**
- Sui uses typed values (`ObjectValue` struct in `execution_value.rs`) with explicit type info
- Our solution achieves similar type tracking via function signature lookup

**Impact:** Types are now known for all command results, including MoveCall returns.

### ~~Version/Digest Not Tracked~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** Object versions were not incremented when objects were mutated, and digests were not computed.

**Solution Implemented:**
- Added `TrackedObject` struct with `version`, `is_modified`, `owner`, and `digest` fields
- Added `ObjectVersionInfo` struct to capture version changes (input/output version and digest)
- Added `VersionChangeType` enum (Created, Mutated, Deleted, Wrapped)
- Added version tracking fields to `PTBExecutor`:
  - `track_versions`: Enable/disable version tracking
  - `input_object_versions`: Track input object versions
  - `input_object_digests`: Track input object digests
  - `lamport_timestamp`: Transaction version for output objects
- Added `compute_object_versions()` method that computes versions and digests at transaction end
- `TransactionEffects` now includes optional `object_versions` and `lamport_timestamp` fields

**Technical Details:**
- Version tracking is opt-in via `set_track_versions(true)` for backwards compatibility
- Input versions are registered via `register_input_version()` or `register_input_version_and_digest()`
- Lamport timestamp is set via `set_lamport_timestamp()`
- All modified objects get lamport_timestamp as output_version
- Digests computed using Blake2b256 hash of object bytes
- Deleted/wrapped objects get marker digest (all zeros)

**Sui Source Comparison:**
- Sui uses `LoadedRuntimeObject` struct with `version: SequenceNumber` and `is_modified: bool`
- Our `TrackedObject` mirrors this with additional type and digest tracking
- Sui's `update_version_and_previous_tx()` assigns lamport_timestamp to all modified objects
- Our `compute_object_versions()` does the same

**Impact:** Version-dependent logic now works correctly when version tracking is enabled.

### ~~Abort Code Extraction Fragile~~ **FIXED**

**Status:** ✅ Fixed | **Verified Against Sui Source:** ✅

**Original Issue:** Abort codes were extracted from error messages using string parsing, which was fragile and could fail with non-standard error formats.

**Solution Implemented:**
- Added `StructuredAbortInfo` struct in `vm.rs` that captures abort info directly from `VMError`
- Added `StructuredVMError` struct to preserve full error details from the Move VM
- Added `ExecutionResult` enum that can be Success or Failure with structured error info
- Added `execute_function_with_structured_error()` method to `VMHarness` that preserves VMError structure
- Updated `PTBExecutor::execute_move_call()` to use structured error extraction
- Added `build_abort_info_from_structured_error()` helper that uses direct VMError field access
- String parsing fallback is kept for backwards compatibility

**Technical Details:**
- `VMError::major_status()` provides the StatusCode (e.g., ABORTED)
- `VMError::sub_status()` provides the abort code directly (no parsing needed)
- `VMError::location()` provides module ID where abort occurred
- `VMError::offsets()` provides function index and instruction offset
- `VMError::exec_state()` provides full stack trace if available

**Sui Source Comparison:**
- Sui uses `convert_vm_error_impl()` in `error.rs:15-80` for structured error conversion
- Our implementation now mirrors this approach by capturing VMError fields directly

**Impact:** Abort codes are now reliably extracted without fragile string parsing.

---

## Additional PTB Edge Cases (Analysis Complete)

The following edge cases were identified through comprehensive analysis of the Sui source code. These represent potential areas where the sandbox differs from on-chain behavior. Most are LOW severity and do not affect typical usage patterns.

### Validation Edge Cases

#### Function Visibility Validation

**Severity:** HIGH (Security)
**Status:** ⚠️ Not Implemented

**Description:** The sandbox does not validate that called functions are `public` or `entry`. Sui enforces this at execution time.

**Sui Source:** `execution.rs` validates function visibility before allowing calls.

**Impact:** Could allow calling private functions in simulation that would fail on-chain.

**Recommendation:** Add visibility check before function execution.

#### Type Argument Validation

**Severity:** MEDIUM
**Status:** ⚠️ Not Implemented

**Description:** Type arguments passed to generic functions are not validated for arity or constraint satisfaction.

**Sui Source:** `context.rs` validates type parameter counts and constraints.

**Impact:** Invalid generic instantiations may succeed locally but fail on-chain.

#### Private Generics Verification

**Severity:** MEDIUM (Security)
**Status:** ⚠️ Not Implemented

**Description:** Sui prevents external callers from instantiating types with `phantom` or private type parameters. This is not enforced in the sandbox.

**Sui Source:** Validator checks in `sui-types/src/type_input.rs`.

**Impact:** Could allow creating invalid type instantiations.

#### Parameter Type Matching

**Severity:** MEDIUM
**Status:** ⚠️ Not Implemented

**Description:** Move function parameter types are not validated against provided argument types before execution.

**Sui Source:** `context.rs:load_value_impl()` validates types match expected.

**Impact:** Type mismatches caught late in execution rather than at command level.

### Command-Specific Edge Cases

#### TransferObjects Incomplete Validation

**Severity:** LOW
**Status:** ⚠️ Partial Implementation

**Description:** TransferObjects validation doesn't fully verify:
- Object capabilities (is the object transferable?)
- Store ability requirements
- Recipient address validity

**Impact:** Some invalid transfers may succeed locally.

#### SplitCoins Amount Validation

**Severity:** LOW
**Status:** ⚠️ Not Checked

**Description:** SplitCoins doesn't pre-validate that the sum of splits doesn't exceed balance.

**Sui Source:** Checked at Move level, but Sui may catch this earlier.

**Impact:** Error messaging may differ from on-chain.

#### MakeMoveVec Type Uniformity

**Severity:** LOW
**Status:** ✅ Implemented

**Description:** All elements in MakeMoveVec must have the same type.

**Current Status:** Partially validated via explicit type argument.

### Object State Edge Cases

#### Shared Object Mutability Tracking

**Severity:** MEDIUM
**Status:** ⚠️ Partial Implementation

**Description:** Shared objects have specific mutability rules:
- Can only be mutated once per transaction
- Must be accessed in a specific order

**Sui Source:** `object_runtime/mod.rs` tracks shared object access patterns.

**Impact:** Complex shared object patterns may behave differently.

#### Frozen Object Detection

**Severity:** LOW
**Status:** ⚠️ Not Implemented

**Description:** Objects can be "frozen" (immutable) but this isn't tracked in the sandbox.

**Impact:** Mutations to frozen objects would succeed locally but fail on-chain.

#### Wrapped Object Access

**Severity:** MEDIUM
**Status:** ⚠️ Partial Implementation

**Description:** Objects wrapped inside other objects have special access rules.

**Sui Source:** `context.rs` tracks wrapped vs unwrapped object state.

**Impact:** Some wrapped object operations may behave differently.

### Execution Flow Edge Cases

#### Abort Location Precision

**Severity:** LOW
**Status:** ⚠️ Approximate

**Description:** Abort locations (module, function, instruction) are extracted from error messages rather than structured data.

**Impact:** Abort location reporting may be less precise.

#### Gas Charge Points

**Severity:** LOW
**Status:** ⚠️ Not Implemented

**Description:** Sui charges gas at specific points during execution. The sandbox doesn't replicate these exact charge points.

**Impact:** Gas-related aborts may occur at different points.

#### Recursive Depth Limits

**Severity:** LOW
**Status:** ⚠️ Not Implemented

**Description:** Sui enforces stack depth limits during execution.

**Sui Source:** Move VM enforces `MAX_CALL_STACK_DEPTH`.

**Impact:** Deeply recursive calls may succeed locally but fail on-chain.

### Type System Edge Cases

#### Ability Constraints

**Severity:** MEDIUM
**Status:** ⚠️ Partial Implementation

**Description:** Type abilities (copy, drop, store, key) affect what operations are valid. Not all ability checks are enforced.

**Impact:** Some ability-violating operations may succeed locally.

#### Reference Semantics

**Severity:** LOW
**Status:** ✅ Handled by Move VM

**Description:** Reference borrowing rules are enforced by the Move VM, not the PTB executor.

**Impact:** None - Move VM handles this correctly.

---

## Gas Model

### Accurate Gas Metering (v0.9.0+)

**Status:** Gas metering is now Sui-compatible and enabled by default.

**Details:**
- `AccurateGasMeter` with per-instruction costs matching Sui's cost tables
- Storage tracking for read/write/delete charges
- Native function gas costs from `native_costs.rs`
- Protocol-version-aware cost tables
- Computation bucketing matching Sui's gas model

**Configuration:**
```rust
// Gas metering is enabled by default
let config = SimulationConfig::default();

// To disable gas metering for unlimited execution
let config = SimulationConfig::default().without_gas_metering();
```

### Storage Rebate Approximation

**Limitation:** Storage rebates are approximated rather than precisely calculated.

**Details:**
- On-chain storage rebates depend on the exact object size and historical storage price when the object was created
- The sandbox uses a formula: `object_size_bytes * 100 * storage_price * 0.99` (99% refundable)
- For precise rebate tracking, use `SimulationConfig::with_storage_price()` to set the storage price

**Impact:** Gas cost comparisons may show small differences in storage rebate values.

---

## Object Runtime

### Initial Shared Object Version

**Limitation:** Shared object initial versions are tracked but may not match on-chain exactly for dynamically created shared objects.

**Details:**
- When creating a shared object during simulation, the initial shared version is set to the object's creation version
- For replayed transactions, the initial shared version comes from the transaction input data

**Impact:** Shared object version checks in Move code should work correctly for most cases.

### Object Existence Checks

**Limitation:** The sandbox requires all input objects to be provided upfront.

**Details:**
- Missing objects will cause an explicit error (not silent failures)
- Dynamic field children discovered during execution require pre-fetching
- There's no on-chain state oracle for discovering objects at runtime

**Impact:** Transaction replay requires complete object data in the cache.

---

## Cryptographic Operations

Cryptographic operations use **fastcrypto**, the same library used by Sui validators. Most crypto is real, not mocked.

### Signature Verification (Real)

| Algorithm | Status | Notes |
|-----------|--------|-------|
| Ed25519 | **Real** | Full verification via fastcrypto |
| ECDSA secp256k1 | **Real** | Verify and ecrecover supported |
| ECDSA secp256r1 | **Real** | Verify and ecrecover supported |
| BLS12-381 | **Real** | min_sig and min_pk variants |
| Groth16 (ZK-SNARKs) | **Real** | BLS12-381 and BN254 curves |
| ECVRF | **Mocked** | Always returns `true` |

**Details:**
- Invalid signatures return `false` (not abort), matching mainnet behavior
- Public key recovery (`ecrecover`) performs real cryptographic recovery
- Groth16 proof verification is fully implemented for both curve types

**Impact:** Signature-dependent code behaves identically to mainnet. Only ECVRF verification is mocked (always passes).

### Hash Functions (Real)

| Function | Status |
|----------|--------|
| blake2b256 | **Real** |
| keccak256 | **Real** |
| sha2_256 | **Real** |
| sha3_256 | **Real** |

**Details:**
- All hash functions produce correct outputs matching on-chain behavior
- Object ID derivation uses proper hashing

**Impact:** None - hash functions work correctly.

---

## Time and Randomness

### Clock Behavior

**Limitation:** The mock clock can be frozen or advancing, differing from on-chain.

**Details:**
- On-chain, `Clock::timestamp_ms()` is fixed for the entire transaction
- By default, the sandbox uses an "advancing" clock (each access increments)
- For replay, set `tx_timestamp_ms` to freeze the clock at the transaction's timestamp

**Recommendation:** Always set `tx_timestamp_ms` for transaction replay to match on-chain behavior.

### Random Number Generation

**Limitation:** Random values are deterministic by default.

**Details:**
- `sui::random::Random` uses a configurable seed
- The same seed produces the same sequence of random values
- On-chain randomness comes from the blockchain's random beacon

**Impact:** Random-dependent code will produce reproducible results locally but different values on-chain.

---

## Transaction Context

### Sender Address Default

**Limitation:** The default sender is the zero address (`0x0`).

**Details:**
- When not explicitly configured, `tx_context::sender()` returns `0x0`
- This is intentional - most testing doesn't need a specific sender
- Use `SimulationConfig::with_sender_address()` to set a specific sender
- With `permissive_ownership: true` (default), ownership checks are relaxed

**Recommendation:** For transaction replay, always use `with_sender_address()` to set the correct sender.

### Protocol Version

**Limitation:** Protocol version affects feature availability.

**Details:**
- Default protocol version is 74 (recent mainnet)
- Some features are version-gated (e.g., `is_feature_enabled` checks version >= 60)
- Use `SimulationConfig::with_protocol_version()` for specific versions

**Impact:** Older protocol versions may have different feature sets.

---

## Dynamic Fields

### Dynamic Field Traversal

**Limitation:** Dynamic fields computed at runtime cannot be pre-fetched.

**Details:**
- Some DeFi protocols (Cetus, Turbos) use skip_list data structures
- These compute tick indices at runtime during traversal
- The sandbox cannot predict which dynamic fields will be accessed

**Example:** A Cetus swap traverses: `head(0) → 481316 → 512756 → tail(887272)`. If the swap needs tick `500000`, this is computed at runtime.

**Workarounds:**
1. Cache all dynamic field children at transaction time
2. Use synthetic/mocked transactions for testing
3. Pre-fetch all known indices for specific pools

### Dynamic Field ID Derivation

**Limitation:** Fully supported using the same algorithm as Sui.

**Details:**
- Uses `Blake2b256(0xf0 || parent || len(key_bytes) || key_bytes || bcs(key_type_tag))`
- The `derive_dynamic_field_id()` function produces correct IDs

**Impact:** None - works correctly.

---

## Package Upgrades

### Address Rewriting

**Limitation:** Package upgrades require address aliasing.

**Details:**
- On-chain, upgraded packages have new addresses but can reference types from original packages
- The sandbox maintains an alias map for address resolution
- Bytecode may contain the original (pre-upgrade) address while on-chain ID differs

**Impact:** Complex upgrade chains may require careful address mapping.

### Linkage Tables

**Limitation:** Linkage information must be provided explicitly.

**Details:**
- Package linkage (which original packages are used) comes from cached transaction data
- The `package_upgrades` field in `CachedTransaction` tracks these mappings

**Impact:** Upgraded package calls need proper linkage configuration.

---

## Receiving Objects

### Parent ID Discovery

**Limitation:** Receiving object parent IDs are not always available.

**Details:**
- Receiving objects are owned by another object (the parent)
- The `TransactionInput::Receiving` only provides object_id, version, digest
- Parent ID must be determined from on-chain object owner data

**Impact:** The `ObjectInput::Receiving` variant has an optional `parent_id` field.

### Authorization

**Limitation:** Receiving authorization checks are simplified.

**Details:**
- On-chain, the sender must prove ownership of the parent object
- The sandbox tracks receiving objects but may not enforce all authorization rules
- Use `permissive_ownership: false` for stricter checks

---

## Native Functions

### Supported Natives

Most Sui-specific native functions are implemented:
- `tx_context::*` - Transaction context (sender, epoch, fresh_id)
- `clock::*` - Clock timestamp
- `object::*` - Object operations (new, delete, borrow)
- `transfer::*` - Object transfers
- `dynamic_field::*` - Dynamic fields
- `event::emit` - Event emission
- `bcs::*` - BCS serialization
- `hash::*` - Cryptographic hashes
- `crypto::*` - Signature verification (mocked by default)

### Unsupported or Partial Natives

Some natives have limitations:
- **Consensus-dependent natives** - May not reflect real network state
- **Validator-specific natives** - Not applicable in local simulation
- **zkLogin natives** - Signature verification mocked

---

## Best Practices

### For Transaction Replay

```rust
let config = SimulationConfig::default()
    .with_sender_address(transaction_sender)
    .with_tx_hash(transaction_digest)
    .with_tx_timestamp(timestamp_ms)
    .with_epoch(epoch);
```

### For Testing with Strict Mode

```rust
let config = SimulationConfig::strict()
    .with_sender_address(test_address);
// Note: strict mode disables permissive_ownership and mock_crypto_pass
```

### For Understanding Differences

When local execution differs from on-chain:
1. Check gas budget and metering differences
2. Verify all input objects are cached
3. Ensure clock/timestamp is correctly set
4. Check if dynamic fields are being accessed
5. Verify sender address and ownership settings

---

## Reporting Issues

If you encounter behavior that differs from on-chain execution and isn't covered by these limitations, please report it with:

1. Transaction digest (if replaying)
2. Move code being executed
3. Expected vs actual behavior
4. SimulationConfig settings used

---

## See Also

- **[Examples](../../examples/README.md)** - Learn by running working examples
- [Transaction Replay Guide](../guides/TRANSACTION_REPLAY.md) - How replay works
- [Architecture](../ARCHITECTURE.md) - System internals
