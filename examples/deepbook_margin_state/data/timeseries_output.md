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

| Day | Checkpoint | SUI Price | SUI Balance | SUI Debt | USDC Balance | USDC Debt | Risk % | Status |
|-----|------------|-----------|-------------|----------|--------------|-----------|--------|--------|
| 1 | 235510810 | $1.8021 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 2 | 235859237 | $1.7728 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 3 | 236134228 | $1.5712 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 4 | 236289445 | $1.5783 | 0.0914 | 2.0002 | 0.96 | 0.00 | 34.87% | ✅ |
| 5 | 236527001 | $1.5097 | 0.0914 | 6.5336 | 0.96 | 0.00 | 11.10% | ✅ |
| 6 | 236926043 | $1.4778 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |
| 7 | 237137954 | $1.5308 | 0.0914 | 3.0667 | 0.96 | 0.00 | 23.36% | ✅ |
| 8 | 237335780 | $1.4928 | 0.0914 | 0.0000 | 0.96 | 0.00 | NO DEBT | ✅ |

**Success Rate: 8/8 (100%)**

**Trends:** SUI Price ↓ $-0.31 (from $1.80 to $1.49)

## Interpretation

### Risk Ratio
- **NO DEBT** = Position has no borrowed assets (risk_ratio = 100000%)
- **Lower %** = More leveraged position (e.g., 11.10% on Day 5 = highest leverage)

### Position Activity Timeline
1. **Days 1-3**: Position created with ~0.09 SUI and ~0.96 USDC, no borrowing
2. **Day 4**: First borrow of ~2 SUI, risk ratio drops to 34.87%
3. **Day 5**: Increased borrowing to ~6.5 SUI, risk ratio at 11.10% (most leveraged)
4. **Day 6**: Position fully repaid, back to NO DEBT
5. **Day 7**: New borrow of ~3 SUI, risk ratio at 23.36%
6. **Day 8**: Position fully repaid again

### Price Movement
- SUI/USDC price dropped from $1.80 to $1.49 over the 8-day period (-17%)
- Oracle prices confirm: SUI Pyth price ~$1.49, USDC Pyth price ~$1.00

### Why No USDC Debt?
This position only borrowed SUI (base asset), not USDC (quote asset). The trader was likely:
- Going long SUI by borrowing and selling
- Or using SUI for margin trading on the order book

## Sample Day Output

```
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
