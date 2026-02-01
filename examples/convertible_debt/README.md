# Convertible Debt Demo (ETH-Upside, Downside-Protected)

This example implements a minimal, production-style convertible debt flow on Sui:

- Lender deposits USD stablecoins and receives a convertible note.
- Borrower locks ETH collateral and receives the USD principal.
- If ETH goes up, lender can convert into ETH at the original strike price.
- If ETH goes down, lender can redeem USD principal + yield after repayment.

The Move package includes:

- `tokens.move`: demo USD/ETH coins + mint helper.
- `oracle.move`: shared oracle for ETH/USD price (strike at offer time).
- `convertible_debt.move`: offer, note, repay, redeem, convert.

## Quick CLI demo (MCP tool mode)

This is the easiest way to exercise the flow with object IDs.

```bash
./examples/cli_mcp/12_convertible_debt.sh
```

The script will:

1. Publish the package.
2. Create demo tokens + shared oracle.
3. Mint USD to a lender and ETH to a borrower.
4. Borrower creates an offer (shared).
5. Lender takes the offer and receives a shared note.
6. Lender converts the note to ETH at the strike price.

## Redeem (downside-protected) path

To simulate the downside scenario, the borrower repays and the lender redeems:

```text
borrower: repay(note, usd_coin)
lender: redeem(note, now_ms)
```

The note tracks repayment in a shared balance and releases collateral on redeem.

## Parameters + Units

- USD has 6 decimals (1 USD = 1_000_000).
- ETH has 9 decimals (1 ETH = 1_000_000_000).
- `oracle::PRICE_SCALE = 1_000_000` (price in USD with 6 decimals).
- `yield_bps` is in basis points (500 = 5%).

## Simulator

To compare outcomes against ETH holding or stable yields, run:

```bash
cargo run --example convertible_simulator -- \
  --principal 1000 \
  --yield-bps 500 \
  --strike 2000 \
  --end 3000 \
  --apy 0.05 \
  --years 1
```

This prints the USD outcome for:

- Convertible note
- ETH HODL
- Stable deposit (Aave-like APY)
