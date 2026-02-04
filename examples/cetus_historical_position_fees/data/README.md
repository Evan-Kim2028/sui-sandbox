# Cetus DLMM Position Data Files

Cached Snowflake data for the `cetus_dlmm_position` example. These files enable running the example without live Snowflake queries.

## Data Files

### `dlmm_snowflake_objects.json`
Pool and Position OBJECT_JSON exported from Snowflake.

```sql
SELECT OBJECT_ID, VERSION, TYPE, OWNER_TYPE, INITIAL_SHARED_VERSION, OBJECT_JSON
FROM ANALYTICS_DB_V2.CHAINDATA_MAINNET.OBJECT
WHERE OBJECT_ID IN (
    '0x64e590b0e4d4f7dfc7ae9fae8e9983cd80ad83b658d8499bf550a9d4f6667076',  -- Pool
    '0x33f5514521220478d3b3e141c7a67f766fd6b4150e25148a13171b4b68089417'   -- Position
)
```

### `dlmm_extended_data.json`
Bin group reserves and PositionInfo for calculating exact token amounts.

Contains:
- `bin_groups`: Bin reserves (amount_a, amount_b, liquidity_share) from dynamic field objects
- `position_stats`: Position's per-bin liquidity shares from PositionInfo

```sql
-- Bin groups (owner = bin_manager.bins.id)
SELECT OBJECT_JSON FROM OBJECT
WHERE OWNER_ADDRESS = '0x0a0eeca470c2ed9dafe4cc189a1077f4f3d8af239808ad5182583a6a043e4ecc'
  AND OBJECT_JSON:value:value:group:idx::INT IN (27810, 27811)

-- PositionInfo (owner = position_manager.positions.id)
SELECT OBJECT_JSON FROM OBJECT
WHERE OBJECT_ID = '0x95b1af050f139b62907db98752a3eb79b157aec3a67101e2a9de0e4ad631e444'
```

### `dlmm_historical_snapshots.json`
Daily bin snapshots for 7-day historical position tracking.

Contains `daily_snapshots` array with bin_groups for each date, enabling historical token amount calculations.

## Position Details

- **Position ID**: `0x33f5514521220478d3b3e141c7a67f766fd6b4150e25148a13171b4b68089417`
- **Pool ID**: `0x64e590b0e4d4f7dfc7ae9fae8e9983cd80ad83b658d8499bf550a9d4f6667076`
- **Pool Type**: Cetus DLMM USDC/SUI
- **Bin Range**: 1325-1349 (25 bins, bin_step=50)
- **Created**: 2026-01-30

## Calculation Formula

Position token amounts are calculated from Snowflake bin data:

```
position_amount = (position_liquidity_share / bin_total_liquidity_share) * bin_reserve
```

Sum across all bins in the position's range to get total USDC and SUI amounts.
