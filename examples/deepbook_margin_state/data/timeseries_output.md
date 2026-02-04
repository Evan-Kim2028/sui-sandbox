# DeepBook Margin State Time Series Output

Position B (0xbcb8ee0447179ea67787dfca1d4d0c54ff82ffe6) - 8 Daily Snapshots

## Return Value Schema (from `manager_state` function)

Based on `main.rs` documentation, the `manager_state<B, Q>` function returns 14 values:

| Index | Field | Type | Description |
|-------|-------|------|-------------|
| 0 | manager_id | address | Margin manager object ID |
| 1 | deepbook_pool_id | address | Associated DeepBook pool |
| 2 | risk_ratio | u64 | Health factor (assets/debt), scaled by 1e9 |
| 3 | base_asset | u64 | Base asset balance with locked (MIST, 1e9 = 1 SUI) |
| 4 | quote_asset | u64 | Quote asset balance (scaled by 1e6) |
| 5 | base_debt | u64 | Borrowed base amount (MIST) |
| 6 | quote_debt | u64 | Borrowed quote amount (scaled by 1e6) |
| 7 | base_pyth_price | u64 | Pyth oracle price for base asset (scaled by 1e8) |
| 8 | base_pyth_decimals | u64 | Base price decimals |
| 9 | quote_pyth_price | u64 | Pyth oracle price for quote asset (scaled by 1e8) |
| 10 | quote_pyth_decimals | u64 | Quote price decimals |
| 11 | current_price | u64 | Calculated base/quote price (scaled by 1e6) |
| 12 | lowest_trigger_above | u64 | TP/SL trigger for longs (u64::MAX if not set) |
| 13 | highest_trigger_below | u64 | TP/SL trigger for shorts (0 if not set) |

## Time Series Summary

| Day | Date (UTC) | SUI Price | SUI Balance | SUI Debt | USDC Balance | USDC Debt | Risk % | Status |
|-----|------------|-----------|-------------|----------|--------------|-----------|--------|--------|
| 1 | 2026-01-17 12:54:18 | $1.8021 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 2 | 2026-01-18 13:55:20 | $1.7728 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 3 | 2026-01-19 09:51:21 | $1.5712 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 4 | 2026-01-19 21:09:27 | $1.5783 | 0.0914 | 2.0002 | 0.96 | 0.00 | 34.87% | ✅ |
| 5 | 2026-01-20 14:23:39 | $1.5097 | 0.0914 | 6.5336 | 0.96 | 0.00 | 11.10% | ✅ |
| 6 | 2026-01-21 18:48:40 | $1.4778 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 7 | 2026-01-22 09:36:30 | $1.5308 | 0.0914 | 3.0667 | 0.96 | 0.00 | 23.36% | ✅ |
| 8 | 2026-01-22 23:28:11 | $1.4928 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |

**Success Rate: 8/8 (100%)**

**Time Range:** Jan 17-22, 2026 (~6 days)

**Trends:** SUI Price ↓ $-0.31 (from $1.80 to $1.49)

## Interpretation

### Risk Ratio
- **NO DEBT** = Position has no borrowed assets (risk_ratio = 100000%)
- **Lower %** = More leveraged position (e.g., 11.10% on Day 5 = highest leverage)

### Position Activity Timeline
1. **Jan 17 (Day 1)**: Position created with ~0.09 SUI and ~0.96 USDC, no borrowing
2. **Jan 18-19 AM (Days 2-3)**: Still no active borrowing
3. **Jan 19 PM (Day 4)**: First borrow of ~2 SUI, risk ratio drops to 34.87%
4. **Jan 20 (Day 5)**: Increased borrowing to ~6.5 SUI, risk ratio at 11.10% (most leveraged)
5. **Jan 21 (Day 6)**: Position fully repaid, back to NO DEBT
6. **Jan 22 AM (Day 7)**: New borrow of ~3 SUI, risk ratio at 23.36%
7. **Jan 22 PM (Day 8)**: Position fully repaid again

### Price Movement
- SUI/USDC price dropped from $1.80 to $1.49 over the 6-day period (-17%)
- Oracle prices confirm: SUI Pyth price ~$1.49, USDC Pyth price ~$1.00

### Why No USDC Debt?
This position only borrowed SUI (base asset), not USDC (quote asset). The trader was likely:
- Going long SUI by borrowing and selling
- Or using SUI for margin trading on the order book

## Sample Day Output

```
┌───────────────────────────────────────────────────────────────────────────────────┐
│  Day 5: 2026-01-20T14:23:39Z | Day 5 (Checkpoint 236527001)
└───────────────────────────────────────────────────────────────────────────────────┘
  ┌───────────────────────────────────────────────────────────────────────┐
  │  📊 MARGIN STATE - Day 5                                              │
  ├───────────────────────────────────────────────────────────────────────┤
  │  Risk Ratio:                11.10%  🟠 HIGH RISK                       │
  ├───────────────────────────────────────────────────────────────────────┤
  │  SUI/USDC Price:       $    1.5097                                     │
  │  SUI Oracle (USD):     $    1.5093                                     │
  │  USDC Oracle (USD):    $    0.9997                                     │
  ├───────────────────────────────────────────────────────────────────────┤
  │  SUI Balance:                   0.0914 SUI                           │
  │  SUI Debt:                      6.5336 SUI                           │
  │  USDC Balance:                    0.96 USDC                          │
  │  USDC Debt:                       0.00 USDC                          │
  └───────────────────────────────────────────────────────────────────────┘
```

## Generated

This output was generated by running:
```bash
cargo run --example deepbook_timeseries
```

The time series data is stored in `position_b_daily_timeseries.json`.
