# DeepBook Margin State Historical Replay Example

This workflow demonstrates **historical DeepBook v3 margin state replay**.

The runnable Rust examples now live in the regular examples directory:
- `examples/deepbook_margin_state.rs`
- `examples/deepbook_timeseries.rs`

`deepbook_margin_state.rs` is intentionally thin and calls the first-class generic Rust helper:
`sui_sandbox_core::orchestrator::ReplayOrchestrator::execute_historical_view_from_versions(...)`.

That helper handles:
1. versions snapshot loading
2. historical object/package hydration
3. local `manager_state` execution
4. decoded margin output

## Overview

DeepBook v3 is Sui's native order book protocol with margin trading capabilities. This example calls the `manager_state` view function on a historical margin position to retrieve health and liquidation data.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     HISTORICAL STATE REPLAY WORKFLOW                        │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────────┐
  │   1. SNOWFLAKE   │  Query: "What were the object versions at checkpoint X?"
  │   (Pre-compute)  │
  └────────┬─────────┘
           │
           │  Object ID → (Version, Checkpoint Found)
           ▼
  ┌──────────────────┐
  │  2. JSON MANIFEST│  data/deepbook_versions_240733000.json
  │  (Version Map)   │  Maps each object to its version + checkpoint
  └────────┬─────────┘
           │
           │  For each object, fetch from its checkpoint
           ▼
  ┌──────────────────┐
  │  3. WALRUS HTTP  │  GET /v1/checkpoint/full?checkpoint=240732596
  │  (Fetch BCS)     │  → Returns full checkpoint with object BCS data
  └────────┬─────────┘
           │
           │  All required objects loaded
           ▼
  ┌──────────────────┐
  │  4. LOCAL VM     │  Execute PTB with manager_state() call
  │  (Execute PTB)   │  → Returns 14 values (risk ratio, balances, etc.)
  └──────────────────┘
```

## Why This Matters

Traditional historical queries require:
- Expensive archive RPC nodes
- Centralized infrastructure
- Trust in the RPC provider

This approach uses:
- **Snowflake** for efficient version lookup (your data warehouse)
- **Archive gRPC** for historical object/package hydration
- **Local execution** for trustless computation

## Files and Data

```
examples/
├── deepbook_margin_state.rs                         # Position A snapshot query
├── deepbook_timeseries.rs                           # Position B 8-day series query
└── data/deepbook_margin_state/
    ├── deepbook_versions_240732600.json             # Position A: earlier snapshot
    ├── deepbook_versions_240733000.json             # Position A: later snapshot
    └── position_b_daily_timeseries.json             # Position B: 8 daily snapshots
```

## Examples

### Position A: Single Snapshot Query (`deepbook_margin_state`)

Query a margin position at a specific checkpoint:

```bash
# Use pre-computed versions from Snowflake
DEEPBOOK_SCENARIO=position_a_snapshot \
  cargo run --example deepbook_margin_state

# If archive endpoint misses runtime objects, use a historical gRPC endpoint
SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443 \
DEEPBOOK_SCENARIO=position_a_snapshot \
  cargo run --example deepbook_margin_state
```

### Position B: 8-Day Time Series (`deepbook_timeseries`)

Track margin position evolution across 8 consecutive daily snapshots:

```bash
# Run default time series scenario
DEEPBOOK_SCENARIO=position_b_timeseries \
  cargo run --example deepbook_timeseries
```

This example iterates through all 8 daily checkpoints and outputs a summary table showing:
- Execution status for each day
- Gas usage per query
- Success rate across the time series

### Daily Time Series Data

The `position_b_daily_timeseries.json` file contains **8 consecutive daily snapshots** for position `0xbcb8ee...` (SUI/USDC):

| Day | Checkpoint | Margin Manager Version | Description |
|-----|------------|------------------------|-------------|
| 1 | 235510810 | v755845885 | Position creation |
| 2 | 235859237 | v756400242 | First activity day |
| 3 | 236134228 | v757472086 | Continued trading |
| 4 | 236289445 | v757848631 | Position growth |
| 5 | 236527001 | v758456911 | Mid-week |
| 6 | 236926043 | v759855738 | Active trading |
| 7 | 237137954 | v760412772 | Approaching week end |
| 8 | 237335780 | v760921405 | Week 1 complete |

This demonstrates the power of historical tracking - you can reconstruct portfolio state at any point in time to analyze:
- How margin positions evolved day-over-day
- Historical risk ratios and collateral values
- Oracle price movements affecting margin health
- P&L trajectory over the position's lifetime

### JSON Manifest Format

Each JSON file contains pre-computed object versions from Snowflake:

```json
{
  "checkpoint": 240733000,
  "description": "Object versions at checkpoint 240733000 for DeepBook margin state",
  "objects": {
    "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407": {
      "name": "DeepBook_Pool",
      "version": 771532076,
      "checkpoint_found": 240733000
    },
    "0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80": {
      "name": "Margin_Manager",
      "version": 771531876,
      "checkpoint_found": 240732967
    }
    // ... more objects
  }
}
```

**Key fields:**
- `checkpoint` - The target checkpoint for the query
- `version` - The object's version at that checkpoint
- `checkpoint_found` - The specific checkpoint where this version exists

## Usage

### Mode 1: Snowflake + Archive gRPC (Recommended)

```bash
# Use pre-computed versions, fetch from gRPC
DEEPBOOK_SCENARIO=position_a_snapshot \
  cargo run --example deepbook_margin_state
```

### Mode 2: Custom historical gRPC endpoint

```bash
SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443 \
DEEPBOOK_SCENARIO=position_a_snapshot \
  cargo run --example deepbook_margin_state
```

## How It Works

### Step 1: Snowflake Version Lookup

The Snowflake query efficiently finds object versions at a target checkpoint:

```sql
-- For each object, find its version at or before the target checkpoint
SELECT 'DeepBook_Pool' as name, object_id, version, checkpoint as checkpoint_found
FROM (
    SELECT object_id, version, checkpoint
    FROM ANALYTICS_DB_V2.CHAINDATA_MAINNET.OBJECT
    WHERE object_id = '0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407'
      AND checkpoint <= 240733000
    ORDER BY checkpoint DESC LIMIT 1
)
UNION ALL
SELECT 'Margin_Manager' as name, object_id, version, checkpoint as checkpoint_found
FROM (
    SELECT object_id, version, checkpoint
    FROM ANALYTICS_DB_V2.CHAINDATA_MAINNET.OBJECT
    WHERE object_id = '0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80'
      AND checkpoint <= 240733000
    ORDER BY checkpoint DESC LIMIT 1
)
-- ... repeat for all objects
```

### Step 2: Historical Hydration via Archive gRPC

The versions manifest pins object versions per checkpoint. The helper then:
- fetches required object bytes at those versions,
- fetches package bytecode closure (root packages + dependencies),
- executes the Move view call locally against that historical state.

### Step 3: Local PTB Execution

Build and execute a Programmable Transaction Block locally:

```rust
// Build PTB calling manager_state<SUI, USDC>
let commands = vec![Command::MoveCall {
    package: MARGIN_PACKAGE,
    module: "manager_state",
    function: "manager_state",
    type_arguments: vec![SUI, USDC],
    arguments: vec![
        clock,           // &Clock
        margin_manager,  // &MarginManager<B,Q>
        deepbook_pool,   // &Pool<B,Q>
        base_margin,     // &MarginPool<B>
        quote_margin,    // &MarginPool<Q>
        base_oracle,     // &PriceInfoObject
        quote_oracle,    // &PriceInfoObject
        margin_registry, // &Registry
    ],
}];
```

### Step 4: Parse Return Values

The `manager_state<B, Q>` function returns 14 values:

| Index | Name | Type | Description |
|-------|------|------|-------------|
| 0 | manager_id | address | Margin manager object ID |
| 1 | deepbook_pool_id | address | Associated DeepBook pool |
| 2 | risk_ratio | u64 | Health factor (assets/debt), scaled by 1e9 |
| 3 | base_asset | u64 | Base asset balance with locked (MIST, 1e9 = 1 SUI) |
| 4 | quote_asset | u64 | Quote asset balance (scaled by 1e6) |
| 5 | base_debt | u64 | Borrowed base amount (MIST) |
| 6 | quote_debt | u64 | Borrowed quote amount (scaled by 1e6) |
| 7 | base_pyth_price | u64 | Pyth oracle price for base asset |
| 8 | base_pyth_decimals | u64 | Base price decimals |
| 9 | quote_pyth_price | u64 | Pyth oracle price for quote asset |
| 10 | quote_pyth_decimals | u64 | Quote price decimals |
| 11 | current_price | u64 | Calculated base/quote price (scaled by 1e9) |
| 12 | lowest_trigger_above | u64 | TP/SL trigger for longs (u64::MAX if not set) |
| 13 | highest_trigger_below | u64 | TP/SL trigger for shorts (0 if not set) |

**Note:** The risk_ratio of 100000% (1e12 scaled) indicates no debt / fully collateralized position.

## Important Notes

### Historical Endpoint Requirements

Historical replays require an archival-capable gRPC endpoint. By default, examples target:
- `https://archive.mainnet.sui.io:443`

If your environment still points at `https://fullnode.mainnet.sui.io:443`, the helpers auto-switch to the archive endpoint for historical mode.

### Required Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `DEEPBOOK_SCENARIO` | Scenario selector (`position_a_snapshot`, `position_b_timeseries`, `position_a_json_bcs`) | Required |
| `SUI_GRPC_ENDPOINT` | gRPC endpoint URL override | Optional (historical mode auto-selects archival default when unset) |
| `SUI_GRPC_API_KEY` | gRPC API key | Recommended |

If `SUI_GRPC_ENDPOINT` points to `https://fullnode.mainnet.sui.io:443` during historical runs, the examples auto-switch to `https://archive.mainnet.sui.io:443`.
If replay aborts due missing unchanged runtime objects, override with `SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443`.
When this happens, the example now prints an explicit hint at runtime including the active endpoint and the override command.

### Target Margin Position

The included manifests track this specific margin position:

- **Margin Manager**: `0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80`
- **Pool**: SUI/USDC on DeepBook v3
- **Created at**: Checkpoint 240732410
- **Tracked snapshots**: 240732600 and 240733000

## Architecture Deep Dive

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         COMPLETE SYSTEM ARCHITECTURE                        │
└─────────────────────────────────────────────────────────────────────────────┘

                    ┌─────────────────────────────────────┐
                    │           YOUR WORKFLOW             │
                    └─────────────────────────────────────┘
                                     │
         ┌───────────────────────────┼───────────────────────────┐
         │                           │                           │
         ▼                           ▼                           ▼
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│   SNOWFLAKE     │       │   WALRUS HTTP   │       │   LOCAL MOVE    │
│   (Indexer)     │       │   (Archival)    │       │   (Execution)   │
├─────────────────┤       ├─────────────────┤       ├─────────────────┤
│ 27B+ OBJECT     │       │ Decentralized   │       │ Move VM with    │
│ rows indexed    │       │ checkpoint      │       │ full bytecode   │
│                 │       │ storage         │       │                 │
│ Fast version    │       │                 │       │ Execute PTBs    │
│ lookups via     │       │ Verifiable via  │       │ locally with    │
│ checkpoint      │       │ merkle proofs   │       │ loaded state    │
├─────────────────┤       ├─────────────────┤       ├─────────────────┤
│ Output:         │       │ Output:         │       │ Output:         │
│ object_id →     │       │ Full checkpoint │       │ Function return │
│ (version,       │       │ BCS data        │       │ values (14)     │
│  checkpoint)    │       │                 │       │                 │
└────────┬────────┘       └────────┬────────┘       └────────┬────────┘
         │                         │                         │
         │    JSON Manifest        │    BCS Objects          │    Results
         └─────────────────────────┴─────────────────────────┘
                                   │
                                   ▼
                    ┌─────────────────────────────────────┐
                    │     HISTORICAL STATE REPLAY         │
                    │   (Trustless, Verifiable, Local)    │
                    └─────────────────────────────────────┘
```

## Generating New Manifests

To create a manifest for a different margin position or checkpoint:

1. **Find the margin position** in Snowflake:
```sql
SELECT object_id, checkpoint
FROM ANALYTICS_DB_V2.CHAINDATA_MAINNET.OBJECT
WHERE object_type LIKE '%MarginManager%'
ORDER BY checkpoint DESC
LIMIT 10;
```

2. **Identify all required objects** (manager, pools, oracles, registry, clock)

3. **Query versions at target checkpoint** using the UNION ALL pattern

4. **Save to JSON** in the format shown above

## Related Resources

- [DeepBook v3 SDK](https://github.com/MystenLabs/deepbook-v3)
- [Sui RPC / gRPC Documentation](https://docs.sui.io/references/sui-api)
- [sui-sandbox Repository](https://github.com/your-org/sui-sandbox)
