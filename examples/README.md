# PTB Replay Examples

These examples demonstrate local replay of real Sui DeFi transactions using the Move VM sandbox.
Each example has **1:1 parity with corresponding tests** in `tests/`.

## Quick Start

```bash
# Run a single example
cargo run --example cetus_swap

# Run with release optimizations (faster)
cargo run --release --example cetus_swap

# List all available examples
cargo run --example
```

## Available Examples

| Example | Protocol | Description |
|---------|----------|-------------|
| `cetus_swap` | Cetus AMM | Historical swap with dynamic field child fetching |
| `scallop_deposit` | Scallop Lending | Lending deposit with gRPC historical data |
| `deepbook_replay` | DeepBook CLOB | Flash loans - success and failure cases |
| `kriya_swap` | Kriya DEX | Multi-hop swap with automatic version-lock patching |
| `inspect_df` | Sui Framework | Dynamic field inspector (no API key needed) |

## Example Status

### Cetus Swap ✓ Full Success

- Replays historical Cetus CLMM swap transaction
- Fetches pool state at exact transaction version
- Uses on-demand skip_list node fetching
- **Result: Matches on-chain success**

### Scallop Deposit ~ Partial Success (with Object Patching)

- Demonstrates full sandbox infrastructure working:
  - gRPC connection to Surflux for `unchanged_loaded_runtime_objects`
  - Historical dynamic field preloading at exact versions
  - Address aliasing for upgraded packages
  - **Object patching** fixes version-lock checks
- ObjectPatcher successfully patches `::version::Version` object
- Remaining issue: argument deserialization (struct layout changes)
- **Result: Version-lock bypassed, blocked by BCS compatibility**

### DeepBook Replay ✓ Validation Pass

- Replays 2 DeepBook flash loan transactions:
  - Flash Loan Swap - succeeds locally (matches on-chain)
  - Flash Loan Arb - correctly reproduces on-chain failure
- Uses automated gRPC historical state reconstruction
- **Result: All transactions match expected outcomes**

### Kriya Swap ✓ Full Success (with Object Patching)

- Replays a complex multi-hop swap routing through Kriya, Bluefin, and Cetus pools
- **Automatic version-lock fix**: `ObjectPatcher` patches Cetus GlobalConfig to match bytecode
- Uses flash loans, multi-protocol routing, and CLMM pools
- **Result: Matches on-chain success**

## What These Examples Demonstrate

### 1. Transaction Data Loading

- Auto-fetch from mainnet GraphQL with local caching
- Transaction commands, packages, and input objects

### 2. Package Resolution

- Loading upgraded packages at original addresses
- Handling package linkage tables for version checks

### 3. Historical State

- Fetching objects at their transaction-time versions
- Constructing Clock object with correct timestamp
- Using gRPC for `unchanged_loaded_runtime_objects`

### 4. Dynamic Fields

- On-demand fetching of child objects (skip_list nodes, balance managers)
- Pre-loading known dynamic field children

### 5. PTB Execution

- Full Move VM execution with real cryptography
- Comparing local results with on-chain effects

## Example Output

### Cetus Swap

```text
=== REPLAY RESULT ===
Local success: true

✓ HISTORICAL TRANSACTION REPLAYED SUCCESSFULLY!

On-chain status: Success
Status match: true
```

### Scallop Deposit

```text
Step 4: Fetching objects at historical versions via gRPC...
   ✓ Fetched 14 objects (0 failed)
   Object patches applied:
      ::version::Version -> 1 patches

  Local execution: FAILURE
  Error: FAILED_TO_DESERIALIZE_ARGUMENT

[PARTIAL SUCCESS]
ObjectPatcher successfully bypassed version-lock checks by patching
the Version object. The remaining error is due to BCS format changes
in the protocol's struct layouts between versions.
```

### DeepBook Replay

```text
╔══════════════════════════════════════════════════════════════════════╗
║                         VALIDATION SUMMARY                           ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ Flash Loan Swap             | local: SUCCESS | expected: SUCCESS ║
║ ✓ Flash Loan Arb              | local: FAILURE | expected: FAILURE ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ ALL TRANSACTIONS MATCH EXPECTED OUTCOMES                          ║
╚══════════════════════════════════════════════════════════════════════╝
```

### Kriya Swap

```text
╔══════════════════════════════════════════════════════════════════════╗
║                         VALIDATION SUMMARY                           ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ Kriya Multi-Hop Swap      | local: SUCCESS | expected: SUCCESS ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ TRANSACTION REPLAYED SUCCESSFULLY                                 ║
║                                                                      ║
║ ObjectPatcher automatically fixed version-locked GlobalConfig        ║
║ by patching package_version to match bytecode's CURRENT_VERSION.    ║
╚══════════════════════════════════════════════════════════════════════╝
```

Note: The `ObjectPatcher` automatically detects and patches Cetus GlobalConfig's
`package_version` field to match the bytecode's `CURRENT_VERSION` constant,
enabling successful historical replay of transactions through version-locked protocols.

## Requirements

- Data is automatically fetched from Sui mainnet GraphQL
- No API keys required for basic usage
- **For Scallop/DeepBook**: Set `SURFLUX_API_KEY` in `.env` for historical object versions

## Caching

Transaction data is cached in `.tx-cache/` for faster subsequent runs:

```bash
# Clear cache to force fresh fetch
rm -rf .tx-cache

# Run example (will fetch everything fresh)
cargo run --example cetus_swap
```

## Related Documentation

- [DeFi Case Studies](../docs/defi-case-study/README.md) - Detailed technical documentation
- [Data Fetching Guide](../docs/guides/DATA_FETCHING.md) - GraphQL and gRPC usage
- [Local Sandbox Guide](../docs/guides/LOCAL_BYTECODE_SANDBOX.md) - VM configuration
