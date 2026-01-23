# Benchmark Extraction Phase 1: Transaction Types

**Date**: 2026-01-22
**Status**: COMPLETE
**Goal**: Extract CachedTransaction and FetchedTransaction to sui-sandbox-types

## Overview

This is Phase 1 of the benchmark module extraction. We're extracting the core transaction types to `sui-sandbox-types` to break the circular dependency chain.

## What's Being Extracted

From `src/benchmark/tx_replay.rs`:

- `CachedTransaction` struct (~90 lines)
- `CachedDynamicField` struct (~10 lines)
- `FetchedTransaction` struct and impl (~400 lines)
- Related types: `FetchedInput`, `FetchedCommand`, `FetchedArgument`, `FetchedEffects`, etc.

## Why This Breaks the Cycle

**Before**:

```text
benchmark/tx_replay.rs → defines CachedTransaction
         ↑
    cache/mod.rs → re-exports CachedTransaction from benchmark
         ↑
    DataFetcher → uses cache::CacheManager
         ↑
benchmark/fetcher.rs → wraps DataFetcher
```

**After**:

```text
sui-sandbox-types → defines CachedTransaction, FetchedTransaction
         ↑
    cache/mod.rs → re-exports from sui-sandbox-types
    benchmark/tx_replay.rs → re-exports from sui-sandbox-types
```

No more cycle - types come from a foundational crate.

## Changes

### 1. crates/sui-types/src/lib.rs

- Add `pub mod transaction;`
- Re-export transaction types

### 2. crates/sui-types/src/transaction.rs (NEW)

- Move `CachedTransaction`, `CachedDynamicField`
- Move `FetchedTransaction` and all related types
- Move helper methods (but NOT replay methods that depend on VM)

### 3. crates/sui-types/Cargo.toml

- Add `serde`, `base64` dependencies (for serialization)

### 4. src/benchmark/tx_replay.rs

- Remove type definitions
- Add `pub use sui_sandbox_types::transaction::*;`
- Keep replay logic that depends on VM

### 5. src/cache/mod.rs

- Change `pub use crate::benchmark::tx_replay::CachedTransaction;`
- To `pub use sui_sandbox_types::transaction::CachedTransaction;`

## Verification

```bash
# 1. All tests must pass
cargo test --lib

# 2. Examples must produce same output
cargo run --example deepbook_replay --release 2>&1 | grep -A 10 "VALIDATION SUMMARY"
# Expected: "ALL TRANSACTIONS MATCH EXPECTED OUTCOMES"

cargo run --example inspect_df --release
# Expected: Loads modules successfully
```

## Rollback

If this phase fails:

```bash
git revert HEAD  # Revert the phase 1 commit
```
