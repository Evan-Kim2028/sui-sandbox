# Benchmark Module Full Extraction Plan

**Date**: 2026-01-22
**Status**: PLANNED
**Goal**: Fully extract benchmark code to sui-sandbox-core with zero example regressions

## Executive Summary

Full extraction is **feasible** but requires 6 sequential phases with careful verification after each phase. The key insight is that the circular dependency can be broken by extracting in the right order and creating trait abstractions.

## Example Dependencies Analysis

All 4 replay examples (deepbook, cetus, kriya, scallop) share the same dependency pattern:

| Module | inspect_df | deepbook | cetus | kriya | scallop |
|--------|-----------|----------|-------|-------|---------|
| resolver | ✓ | ✓ | ✓ | ✓ | ✓ |
| vm | - | ✓ | ✓ | ✓ | ✓ |
| tx_replay | - | ✓ | ✓ | ✓ | ✓ |
| object_runtime | - | ✓ | ✓ | ✓ | ✓ |
| object_patcher | - | - | - | ✓ | ✓ |
| CachedTransaction | - | ✓ | ✓ | ✓ | ✓ |
| GrpcClient | - | ✓ | ✓ | ✓ | ✓ |
| GraphQLClient | ✓ | - | - | - | - |

## Breaking the Circular Dependency

The circular dependency chain is:

```text
benchmark/tx_replay.rs → CachedTransaction, FetchedTransaction
         ↑
    cache/mod.rs → re-exports CachedTransaction
         ↑
    DataFetcher → uses cache::CacheManager
         ↑
benchmark/fetcher.rs → wraps DataFetcher
```

**Solution**: Extract in this order:

1. First extract types that have no dependencies (CachedTransaction, FetchedTransaction)
2. Then extract modules that only depend on those types
3. Leave cache integration as a wrapper in the main crate

## Phased Extraction Plan

### Phase 0: Baseline Verification

**Goal**: Establish baseline before any changes

```bash
# Create fresh baseline
mkdir -p .baseline/phase0
cargo test --lib 2>&1 > .baseline/phase0/test_results.txt
cargo run --example deepbook_replay --release 2>&1 | tee .baseline/phase0/deepbook.txt
cargo run --example cetus_swap --release 2>&1 | tee .baseline/phase0/cetus.txt
cargo run --example kriya_swap --release 2>&1 | tee .baseline/phase0/kriya.txt
cargo run --example scallop_deposit --release 2>&1 | tee .baseline/phase0/scallop.txt
cargo run --example inspect_df --release 2>&1 | tee .baseline/phase0/inspect_df.txt

# Record test counts
echo "Test count: $(grep -c 'test .* ok' .baseline/phase0/test_results.txt)" > .baseline/phase0/summary.txt
```

**Verification Script**:

```bash
#!/bin/bash
verify_phase() {
  PHASE=$1
  # Check deepbook passes
  if ! grep -q "ALL TRANSACTIONS MATCH EXPECTED OUTCOMES" .baseline/$PHASE/deepbook.txt; then
    echo "FAIL: deepbook_replay changed behavior"
    return 1
  fi
  # Check cetus matches baseline (may succeed or fail, but consistently)
  CETUS_LOCAL=$(grep "local:" .baseline/$PHASE/cetus.txt | head -1)
  CETUS_BASE=$(grep "local:" .baseline/phase0/cetus.txt | head -1)
  if [ "$CETUS_LOCAL" != "$CETUS_BASE" ]; then
    echo "FAIL: cetus_swap changed behavior"
    return 1
  fi
  echo "PASS: Phase $PHASE verification complete"
  return 0
}
```

---

### Phase 1: Extract Transaction Types to sui-sandbox-types

**Goal**: Move CachedTransaction and FetchedTransaction to break the cycle
**Risk**: LOW
**Files to move**: ~500 lines

**Changes**:

1. In `crates/sui-types/src/lib.rs`:
   - Add `mod transaction_types;`
   - Define `CachedTransaction`, `CachedDynamicField`, `FetchedTransaction`
   - These types use only serde, std::collections, base64 - no internal deps

2. In `src/benchmark/tx_replay.rs`:
   - Remove `CachedTransaction`, `CachedDynamicField` definitions
   - Add `pub use sui_sandbox_types::{CachedTransaction, CachedDynamicField};`

3. In `src/cache/mod.rs`:
   - Change `pub use crate::benchmark::tx_replay::CachedTransaction;`
   - To `pub use sui_sandbox_types::CachedTransaction;`

**Verification**:

```bash
cargo test --lib
cargo run --example deepbook_replay --release
# Must match baseline
```

---

### Phase 2: Extract grpc_to_fetched_transaction conversion

**Goal**: Move transaction conversion function to sui-data-fetcher
**Risk**: LOW
**Files to move**: ~200 lines

**Changes**:

1. In `crates/sui-data-fetcher/src/lib.rs`:
   - Add `mod conversion;`
   - Move `grpc_to_fetched_transaction` function
   - Depends on: grpc types (already in crate), FetchedTransaction (now in sui-types)

2. In `src/benchmark/tx_replay.rs`:
   - Remove `grpc_to_fetched_transaction`
   - Add `pub use sui_data_fetcher::conversion::grpc_to_fetched_transaction;`

3. Update examples:
   - Change import to `sui_data_fetcher::grpc_to_fetched_transaction`

**Verification**:

```bash
cargo test --lib
cargo run --example deepbook_replay --release
cargo run --example cetus_swap --release
# Both must match baseline
```

---

### Phase 3: Extract Core VM Components

**Goal**: Move vm.rs, natives.rs, object_runtime.rs to sui-sandbox-core
**Risk**: MEDIUM
**Files to move**: ~6,900 lines

**These modules have NO dependency on DataFetcher or cache.**

**Changes**:

1. Copy to `crates/sui-sandbox-core/src/`:
   - `vm.rs` (1,951 lines)
   - `natives.rs` (3,210 lines)
   - `object_runtime.rs` (1,747 lines)
   - `well_known.rs` (441 lines)
   - `errors.rs` (2,342 lines) - shared error types

2. Update Cargo.toml with Move VM dependencies (already prepared)

3. Update include_bytes! paths for framework bytecode:

   ```rust
   static MOVE_STDLIB: &[u8] = include_bytes!("../../../../framework_bytecode/move-stdlib");
   ```

4. Main crate re-exports:

   ```rust
   pub use sui_sandbox_core::vm;
   pub use sui_sandbox_core::natives;
   pub use sui_sandbox_core::object_runtime;
   ```

**Verification**:

```bash
cargo test --lib
cargo run --example deepbook_replay --release
cargo run --example inspect_df --release
# All must match baseline
```

---

### Phase 4: Extract Resolver Module

**Goal**: Move resolver.rs (module loading) to sui-sandbox-core
**Risk**: MEDIUM
**Files to move**: ~1,218 lines

**Dependencies**: vm.rs, well_known.rs (already extracted in Phase 3)

**Changes**:

1. Copy `resolver.rs` to `crates/sui-sandbox-core/src/`
2. Fix include_bytes! paths
3. Update internal imports to use `crate::` within sui-sandbox-core
4. Main crate re-exports: `pub use sui_sandbox_core::resolver;`

**Verification**:

```bash
cargo test --lib
cargo run --example inspect_df --release
cargo run --example deepbook_replay --release
# All must match baseline
```

---

### Phase 5: Extract PTB and Simulation

**Goal**: Move ptb.rs, simulation.rs to sui-sandbox-core
**Risk**: HIGH
**Files to move**: ~8,810 lines

**These are the most complex modules with extensive internal dependencies.**

**Changes**:

1. Copy to `crates/sui-sandbox-core/src/`:
   - `ptb.rs` (4,325 lines)
   - `simulation.rs` (4,485 lines)
   - Supporting modules they need

2. Update all `crate::benchmark::*` imports to `crate::*` within sui-sandbox-core

3. Main crate re-exports

**Verification**:

```bash
cargo test --lib
# Run ALL examples
for ex in deepbook_replay cetus_swap kriya_swap scallop_deposit inspect_df; do
  cargo run --example $ex --release 2>&1 | tee .baseline/phase5/$ex.txt
done
# Compare all outputs to baseline
```

---

### Phase 6: Extract Remaining Modules

**Goal**: Move remaining benchmark modules
**Risk**: HIGH
**Files to move**: ~15,000+ lines

**Modules**:

- `tx_replay.rs` (minus types already moved)
- `object_patcher.rs`
- `fetcher.rs` (wrapper, NOT DataFetcher)
- `sandbox/*` (LLM integration)
- `mm2/*` (type validation)
- `runner.rs`, `validator.rs`, etc.

**Note on fetcher.rs**:
The `NetworkFetcher` wrapper in `benchmark/fetcher.rs` wraps `DataFetcher`.
Two options:

1. Keep NetworkFetcher in main crate as thin wrapper
2. Move NetworkFetcher but use trait abstraction for DataFetcher

**Verification**:

```bash
# Full verification suite
cargo test --release
cargo run --example deepbook_replay --release
cargo run --example cetus_swap --release
cargo run --example kriya_swap --release
cargo run --example scallop_deposit --release
cargo run --example inspect_df --release

# Compare VALIDATION SUMMARY blocks
for ex in deepbook cetus kriya scallop; do
  diff <(grep -A 10 "VALIDATION SUMMARY" .baseline/phase0/${ex}*.txt) \
       <(grep -A 10 "VALIDATION SUMMARY" .baseline/phase6/${ex}*.txt)
done
```

---

## Rollback Strategy

Each phase creates a git branch:

```bash
# Before phase N
git checkout -b extraction/phase-N
git add -A && git commit -m "Phase N: <description>"

# If phase fails verification
git checkout main
git branch -D extraction/phase-N

# If phase passes
git checkout main
git merge extraction/phase-N
git tag extraction-phase-N-complete
```

---

## Risk Assessment

| Phase | Risk | Reason | Mitigation |
|-------|------|--------|------------|
| 0 | None | Baseline only | - |
| 1 | Low | Types are data-only | Extensive tests on CachedTransaction |
| 2 | Low | Single function | Direct comparison of conversion output |
| 3 | Medium | VM is complex | Framework loading tests critical |
| 4 | Medium | Path dependencies | include_bytes! verification |
| 5 | High | Core simulation | Every test + every example must pass |
| 6 | High | Many modules | Incremental sub-phases recommended |

---

## Success Criteria

After full extraction:

1. `cargo test --lib` passes 314+ tests
2. `cargo test -p sui-sandbox-core` runs extracted tests
3. All 5 examples produce identical VALIDATION SUMMARY output
4. `sui-sandbox-core` can be used standalone (with grpc/graphql from data-fetcher)
5. Main crate size reduced by ~30,000 lines
6. No circular dependencies between crates

---

## Time Estimate

- Phase 0: 10 minutes (baseline)
- Phase 1: 30 minutes (types extraction)
- Phase 2: 20 minutes (conversion function)
- Phase 3: 2 hours (VM core)
- Phase 4: 1 hour (resolver)
- Phase 5: 3 hours (ptb/simulation)
- Phase 6: 4+ hours (remaining modules)

**Total**: ~12 hours of focused work

---

## Alternative: Incremental Facade

If full extraction proves too risky, an incremental approach:

1. Extract only types to sui-sandbox-types (Phase 1)
2. Keep everything else as facade
3. Gradually extract modules over multiple PRs
4. Each PR is small, reviewable, testable

This is safer but takes longer to complete.
