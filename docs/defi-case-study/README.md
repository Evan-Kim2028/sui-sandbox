# DeFi Case Studies

These case studies demonstrate **successful PTB (Programmable Transaction Block) replay** of real Sui DeFi transactions.

## The Goal: Faithful PTB Replay

The overarching goal is executing historical Sui transactions locally with identical results. Each case study shows:

1. **What the PTB does** - The actual commands (MoveCall, SplitCoins, TransferObjects)
2. **How replay works** - Fetching packages, state, and executing commands
3. **Why it succeeds** - The pieces that enable faithful execution

## Case Studies

| File | Protocol | PTB Type | Status |
|------|----------|----------|--------|
| [CETUS.md](CETUS.md) | Cetus AMM | Swap (5-7 commands) | ✓ Full Success |
| [DEEPBOOK.md](DEEPBOOK.md) | DeepBook CLOB | Orders, Flash Loans, What-If Simulation | ✓ Full Success |
| [BLUEFIN_PERPETUALS.md](BLUEFIN_PERPETUALS.md) | Bluefin Perpetuals | Swaps, Positions | ✓ Swap Success, Perps Pending |
| [LENDING_PROTOCOLS.md](LENDING_PROTOCOLS.md) | Flash Loans + Lending | Flash Loan, Deposit, Withdraw | ✓ Flash Loan Success |
| [BATCH_REPLAY.md](BATCH_REPLAY.md) | Various | Automated sampling | Analysis |

## PTB Replay Requirements

Every successful PTB replay needs:

| Requirement | Why |
|-------------|-----|
| **Correct package bytecode** | PTB calls functions by address; must load upgraded code at original address |
| **Historical object state** | Objects change; must use tx-time versions, not current |
| **Dynamic field support** | Pools use skip_lists/big_vectors; fetch children on-demand |
| **System object construction** | Clock must have original transaction timestamp |
| **Package linkage resolution** | Upgraded packages need linkage table to find storage addresses |

## Key Technique: Linkage Table Resolution

When Sui packages are upgraded, they create new storage objects while keeping the same runtime ID. To load the correct bytecode:

```rust
// 1. Fetch a dependent package to get its linkage table
let jk_pkg = fetcher.fetch_package_full(jk_aggregator)?;

// 2. Linkage maps: original_id → (storage_id, version)
// Bluefin: 0x3492c874... → 0xd075338d... (v17)
let (storage_id, version) = jk_pkg.linkage.get(&original_id)?;

// 3. Fetch bytecode from storage, load at original address
let modules = fetcher.fetch_package_modules(storage_id)?;
resolver.add_package_modules_at(modules, Some(original_id))?;
```

This solves version verification checks (e.g., Bluefin's `verify_version`).

## gRPC Archive Note

For accurate replay, a gRPC endpoint that provides `unchanged_loaded_runtime_objects` is required.

| Feature | Standard Archive | gRPC with Effects |
|---------|------------------|-------------------|
| `unchanged_loaded_runtime_objects` | Always empty | Populated |
| Read-only object versions | Must guess | Exact |

See [GitHub Issue #10](https://github.com/Evan-Kim2028/sui-move-interface-extractor/issues/10) for details.

## Test Files

| Test | Case Study |
|------|------------|
| `tests/execute_cetus_swap.rs` | Cetus AMM swaps |
| `tests/execute_deepbook_swap.rs` | DeepBook CLOB + Bluefin flash loans |
| `tests/execute_bluefin_perpetuals.rs` | Bluefin perpetual futures |
| `tests/execute_lending_protocols.rs` | NAVI + Suilend lending |
| `tests/complex_tx_replay.rs` | Batch replay infrastructure |

## Running Tests

```bash
# Cetus (requires SURFLUX_API_KEY)
SURFLUX_API_KEY=key cargo test --test execute_cetus_swap -- --nocapture

# DeepBook package loading (no key needed)
cargo test --test execute_deepbook_swap test_deepbook_package_loading -- --nocapture

# DeepBook flash loan with Bluefin (requires SURFLUX_API_KEY)
SURFLUX_API_KEY=key DEEPBOOK_TX_INDEX=5 cargo test --test execute_deepbook_swap test_deepbook_two_phase_replay -- --ignored --nocapture

# Bluefin package loading (validates version solution)
cargo test --test execute_bluefin_perpetuals test_bluefin_package_loading -- --nocapture

# Lending protocol tests
SURFLUX_API_KEY=key cargo test --test execute_lending_protocols -- --nocapture
```
