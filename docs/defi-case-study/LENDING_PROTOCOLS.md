# Lending Protocol Case Study

This case study demonstrates **historical transaction replay** for lending protocols on Sui, specifically Scallop's deposit_collateral operation.

## Overview

| Capability | Status |
|------------|--------|
| **Scallop Deposit Collateral** | ✓ SUCCESS |
| **Historical Object Versions** | ✓ Working (gRPC) |
| **Dynamic Field Resolution** | ✓ Working (address aliasing) |
| **Package Upgrade Handling** | ✓ Working (linkage tables) |

**Test File**: `tests/execute_lending_protocols.rs`

---

## Test Results Summary

| Test | Transaction | Protocol | Result |
|------|-------------|----------|--------|
| `test_replay_scallop_deposit` | `JwCJUP4DEXRJna37UEXGJfLS1qMd1TUqdmvhpfyhNmU` | Scallop | ✓ SUCCESS |

---

## Scallop Deposit Collateral Replay

### Transaction Details

| Field | Value |
|-------|-------|
| Digest | `JwCJUP4DEXRJna37UEXGJfLS1qMd1TUqdmvhpfyhNmU` |
| Checkpoint | 235,921,652 |
| Operation | Deposit USDC as collateral |
| Commands | 3 (SplitCoins, MoveCall, MergeCoins) |

### Running the Test

```bash
# Run the replay test
cargo test test_replay_scallop_deposit --test execute_lending_protocols -- --nocapture
```text

### Key Technical Challenges Solved

#### 1. Historical Object Versions via gRPC

The gRPC API provides `unchanged_loaded_runtime_objects` which contains the exact versions of dynamic field children that were loaded during execution:

```rust
// Fetch transaction with historical object versions
let grpc_tx = grpc_client.get_transaction(digest).await?;

// unchanged_loaded_runtime_objects: objects READ but not modified
for (object_id, version) in &grpc_tx.unchanged_loaded_runtime_objects {
    let obj = graphql.fetch_object_at_version(object_id, *version)?;
    storage.preload_dynamic_field(obj);
}

// changed_objects: objects MODIFIED during execution (use INPUT version)
for (object_id, input_version) in &grpc_tx.changed_objects {
    let obj = graphql.fetch_object_at_version(object_id, input_version)?;
    storage.preload_dynamic_field(obj);
}
```text

#### 2. Address Aliasing for Package Upgrades

Scallop has been upgraded multiple times. The bytecode references the **original** package address, but dynamic field keys are stored with the **runtime** (current) package address:

| Address Type | Value | Usage |
|--------------|-------|-------|
| Bytecode (original) | `0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf` | Module self-address |
| Runtime (current) | `0xd384ded6b9e7f4d2c4c9007b0291ef88fbfed8e709bce83d2da69de2d79d013d` | Package ID on-chain |

The `hash_type_and_key` native must rewrite type tags to use runtime addresses:

```rust
// In hash_type_and_key native:
let key_tag = ctx.type_to_type_tag(&key_ty)?;

// Rewrite bytecode addresses → runtime addresses for hash computation
let key_tag = shared_runtime.rewrite_type_tag(key_tag);

// Now hash produces correct child ID matching on-chain storage
let child_id = derive_dynamic_field_id(parent, &key_tag, &key_bytes);
```text

Without this fix, the hash for `MinCollateralAmountKey` would be:

- **Wrong** (bytecode addr): `0xe448f03af51ff6aa...` (doesn't exist)
- **Correct** (runtime addr): `0xa7ae4c1c381a0b48...` (exists in Market)

#### 3. Dynamic Field Preloading

Lending protocols like Scallop use extensive dynamic fields for:

- Whitelist configuration (`AllowAllKey`, `RejectAllKey`)
- Collateral limits (`MinCollateralAmountKey`)
- Reserve pools (keyed by `TypeName` of the coin)
- User obligations

These are preloaded from historical versions before execution:

```rust
// Pre-load dynamic fields from gRPC effects
let mut dynamic_fields = Vec::new();
for (child_id, version) in &all_historical_objects {
    let obj = graphql.fetch_object_at_version(child_id, version)?;
    if let Some(parent) = obj.owner.as_parent() {
        dynamic_fields.push((parent, child_id, obj.type_tag, obj.bcs_bytes));
    }
}
harness.preload_dynamic_fields(dynamic_fields);
```text

---

## Scallop Protocol Architecture

### Package Versions

| Component | Address |
|-----------|---------|
| Current Package | `0xd384ded6b9e7f4d2c4c9007b0291ef88fbfed8e709bce83d2da69de2d79d013d` |
| Bytecode Address | `0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf` |
| Market Object | `0xa757975255146dc9686aa823b7838b507f315d704f428cbadad2f4ea061939d9` |

### Key Modules

| Module | Purpose |
|--------|---------|
| `deposit_collateral` | Entry point for collateral deposits |
| `market` | Core market state and operations |
| `market_dynamic_keys` | Dynamic field key types |
| `whitelist` | Access control configuration |
| `obligation` | User debt positions |
| `obligation_collaterals` | Collateral tracking per obligation |
| `collateral_stats` | Aggregate collateral statistics |

### Dynamic Field Structure

The Market object uses dynamic fields extensively:

```text
Market (0xa757...)
├── AllowAllKey → bool (whitelist config)
├── RejectAllKey → bool (whitelist config)
├── MinCollateralAmountKey<USDC> → u64 (minimum deposit)
├── SupplyLimitKey<USDC> → u64 (max supply)
├── BorrowFeeKey<USDC> → FixedPoint32 (fee rate)
├── Reserve<SUI> → Reserve object
├── Reserve<USDC> → Reserve object
└── ... (more per-coin configurations)
```text

---

## PTB Command Structure

### Deposit Collateral PTB (3 commands)

```text
Command 0: SplitCoins
  - Source: User's USDC coin
  - Amount: Deposit amount
  - Result: Split coin to deposit

Command 1: MoveCall
  - Package: 0xd384ded6b9e7f4d2c4...
  - Module: deposit_collateral
  - Function: deposit_collateral<USDC>
  - Arguments:
    - &Version (immutable)
    - &mut Obligation (user's debt position)
    - &mut Market (shared lending market)
    - Coin<USDC> (from split)
    - &mut TxContext

Command 2: MergeCoins
  - Merge remaining coins back to source
```text

---

## Data Flow During Replay

```text
┌─────────────────────────────────────────────────────────────────┐
│                     REPLAY INITIALIZATION                        │
├─────────────────────────────────────────────────────────────────┤
│ 1. Fetch transaction via JSON-RPC                                │
│ 2. Connect to gRPC for historical versions                       │
│ 3. Extract unchanged_loaded_runtime_objects (5 objects)          │
│ 4. Extract changed_objects with INPUT versions (7 objects)       │
│ 5. Fetch packages and resolve dependencies (10 packages)         │
│ 6. Build address aliases (bytecode → runtime)                    │
│ 7. Preload dynamic fields at historical versions                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     EXECUTION PHASE                              │
├─────────────────────────────────────────────────────────────────┤
│ 1. hash_type_and_key: Compute child IDs (with address rewriting)│
│ 2. borrow_child_object: Fetch from preloaded state               │
│ 3. Whitelist check: AllowAllKey lookup → PASS                    │
│ 4. MinCollateralAmountKey check → Found at 0xa7ae4c1c...         │
│ 5. Update collateral stats and obligation                        │
│ 6. Return SUCCESS                                                │
└─────────────────────────────────────────────────────────────────┘
```text

---

## Objects Loaded During Execution

### Unchanged Runtime Objects (from effects)

| Object ID | Version | Description |
|-----------|---------|-------------|
| `0x0a7ae4c1c381a0b4...` | 744401840 | MinCollateralAmountKey<USDC> |
| `0x184a292d5cf79204...` | 313334037 | Randomness state |
| `0x198b24db213bfeb8...` | 513809333 | Reserve TypeName key |
| `0x229852ada09eba63...` | 372121016 | Interest model config |
| `0xdb278b6aa54845ca...` | 8854892 | AllowAllKey whitelist |

### Changed Objects (modified during tx)

| Object ID | Input Version | Description |
|-----------|---------------|-------------|
| `0xa757975255146dc9...` | 756653129 | Market (shared) |
| `0xfbc999c187a4a763...` | 756653129 | Obligation (shared) |
| `0x84b345bf60333eaa...` | 756653128 | Collateral entry |
| `0x8f0d529ba179c5b3...` | 756653128 | CollateralStat |
| `0x2ae8a0351d96c69e...` | 756653128 | Reserve balance |

---

## Key Insights

### 1. Address Aliasing is Critical

Without rewriting type tags in `hash_type_and_key`, dynamic field lookups fail:

- Bytecode uses original package address in struct tags
- On-chain storage uses runtime package address
- Hash must match on-chain storage format

### 2. gRPC Provides Complete Historical Data

Standard Sui RPC doesn't expose `unchanged_loaded_runtime_objects`. The gRPC API provides:

- Exact versions of all loaded objects
- Both unchanged (read-only) and changed (modified) objects
- Enables perfect historical state reconstruction

### 3. Dynamic Fields Dominate State

Lending protocols use dynamic fields for nearly all state:

- Per-coin configurations (limits, fees, rates)
- Per-user obligations and collateral
- Global whitelist and access control

Preloading these from historical versions is essential for replay.

### 4. Package Upgrades Require Careful Handling

Scallop v17 has:

- Runtime address: `0xd384ded6...` (used for function calls)
- Bytecode address: `0xefe8b36d...` (used in module self-references)

The resolver must:

1. Load bytecode at runtime address for linking
2. Provide alias map for type tag rewriting
3. Handle transitive dependencies with their own upgrades

---

## Comparison with Other DeFi Protocols

| Aspect | Scallop | Cetus | DeepBook |
|--------|---------|-------|----------|
| Primary State | Dynamic fields | Pools + Positions | Order book |
| Package Upgrades | Yes (v17) | Yes | Yes |
| Shared Objects | Market, Obligations | Pools | Pools |
| Time Sensitivity | Interest accrual | Low | Order expiry |
| Address Aliasing | Required | Required | Required |

---

## Future Work

1. **Other Lending Operations**: Extend to borrow, repay, liquidate
2. **NAVI Protocol**: Similar architecture with different package structure
3. **Suilend Protocol**: Test with their obligation model
4. **Batch Replay**: Replay multiple lending transactions in sequence
5. **What-If Simulation**: Test alternative collateral amounts
