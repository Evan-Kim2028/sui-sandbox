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
| `kriya_swap` | Kriya DEX | Multi-hop swap demonstrating version-lock challenge |
| `inspect_df` | Sui Framework | Dynamic field inspector (no API key needed) |

## Example Status

### Cetus Swap ✓ Full Success

- Replays historical Cetus CLMM swap transaction
- Fetches pool state at exact transaction version
- Uses on-demand skip_list node fetching
- **Result: Matches on-chain success**

### Scallop Deposit ~ Partial Success

- Demonstrates full sandbox infrastructure working:
  - gRPC connection to Surflux for `unchanged_loaded_runtime_objects`
  - Historical dynamic field preloading at exact versions
  - Address aliasing for upgraded packages
- Error 513 = application-level version check (not VM issue)
- **Result: Reaches Move execution, blocked by protocol version guard**

### DeepBook Replay ✓ Validation Pass

- Replays 2 DeepBook flash loan transactions:
  - Flash Loan Swap - succeeds locally (matches on-chain)
  - Flash Loan Arb - correctly reproduces on-chain failure
- Uses automated gRPC historical state reconstruction
- **Result: All transactions match expected outcomes**

### Kriya Swap ~ Version-Lock Demo

- Replays a complex multi-hop swap routing through Kriya, Bluefin, and Cetus pools
- Demonstrates the version-lock challenge: Cetus's `checked_package_version` prevents historical replay
- **Result: Expected failure - shows infrastructure works but protocol guards block execution**
- **Educational value**: Illustrates why some DeFi transactions can't be fully replayed

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
=== RESULT ===
Success: false
Error: ... version::assert_current_version ... error 513

[PACKAGE VERSION MISMATCH]
This is a PARTIAL SUCCESS - we got past the linker stage and into
Move execution. The version check is an application-level guard.
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

### Kriya Swap (Version-Lock Demo)

```text
╔══════════════════════════════════════════════════════════════════════╗
║                         VALIDATION SUMMARY                           ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ Kriya Multi-Hop Swap      | local: FAILURE | expected: FAILURE ║
╠══════════════════════════════════════════════════════════════════════╣
║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME                              ║
║                                                                      ║
║ This demonstrates the version-lock challenge: transactions that     ║
║ route through Cetus pools fail locally due to config version checks.║
╚══════════════════════════════════════════════════════════════════════╝
```

Note: The Kriya transaction succeeded on-chain but fails locally due to Cetus's
internal version check (`config::checked_package_version`). This is expected
behavior and demonstrates the challenge of replaying version-locked protocols.

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
