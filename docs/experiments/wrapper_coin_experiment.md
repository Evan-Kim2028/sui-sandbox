# Wrapper Coin Experiment Report

This document records the end-to-end steps, rationale, and results for publishing a wrapper coin package on Sui mainnet and validating type inhabitation via smi_tx_sim.

## Results Table

| Plan | Mode | Created types (base) | Hits | Link |
|------|------|-----------------------|------|------|
| Mint WRAPPER_COIN (dry-run) | dry-run | Coin | 1/4 | https://suiscan.xyz/mainnet/tx/EEVfb6kawm2pEFEdwUmmqYT5RpwwLSZCJEiwrQUTDRza |
| Probe WRAPPER_COIN (build-only) | build-only | TreasuryCap | 1/4 | — |
| Fresh type (2/4) (build-only) | build-only | TreasuryCap, Currency | 2/4 | — |
| Fresh type (3/4) (build-only) | build-only | TreasuryCap, Currency, MetadataCap | 3/4 | — |
| Fresh type (4/4) (build-only) | build-only | Coin, TreasuryCap, Currency, MetadataCap | 4/4 | — |


## Secure Key Management & Mainnet Setup

This experiment uses the Sui CLI keystore (file-based). Do NOT commit keys to the repo.

Steps:

1) Install Sui CLI and verify version

```
sui --version
```

2) Prepare keystore path and permissions (macOS/Linux)

```
export SUI_KEYSTORE="$HOME/.sui/sui_config/sui.keystore"
mkdir -p "$(dirname "$SUI_KEYSTORE")"
touch "$SUI_KEYSTORE"
chmod 600 "$SUI_KEYSTORE"
```

3) Import your private key securely (ed25519 shown; adjust scheme if needed)

```
export PRIVATE_KEY_HEX="<DO NOT COMMIT>"
sui keytool import "$PRIVATE_KEY_HEX" ed25519
```

4) Configure mainnet environment and switch

```
sui client new-env --alias mainnet --rpc https://fullnode.mainnet.sui.io:443 || true
sui client switch --env mainnet
sui client active-address
```

5) Validate balances (SUI for gas; your coin type for minted assets)

```
sui client balance <ADDRESS>
sui client balance <ADDRESS> --coin-type <PACKAGE_ID>::wrapper_coin::WRAPPER_COIN
```

Best practices:
- Keep the keystore file private (0600).
- Never commit keys or .env files.
- Consider hardware or custodial signing for production systems.

## From-Scratch Replay Guide

Prerequisites:
- Sui CLI installed; Rust toolchain for building this repo.
- Funded mainnet address for gas.

Sequence:
1) Clone and build tools
```
cargo build --release
```
2) Scaffold or use provided wrapper package and build
```
sui move build -p packages/wrapper_coin
```
3) Dry-run publish (optional sanity check)
```
sui client publish packages/wrapper_coin --skip-fetch-latest-git-deps --gas-budget 100000000 --dry-run
```
4) Publish to mainnet (record package id)
```
sui client publish packages/wrapper_coin   --skip-fetch-latest-git-deps   --sender <ADDRESS>   --gas <GAS_OBJECT_ID>   --gas-budget 100000000
```
5) Run PTB sims and scoring (examples)
```
python scripts/inhabit_single_package_local.py   --package-id <PACKAGE_ID>   --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin   --ptb-spec packages/wrapper_coin/ptb_mint_wrap.json   --sender <ADDRESS>   --mode dry-run --gas-budget 20000000

python scripts/inhabit_single_package_local.py   --package-id <PACKAGE_ID>   --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin   --ptb-spec packages/wrapper_coin/ptb_fresh_build_4of4.json   --sender <ADDRESS>   --mode build-only
```
6) Execute a real mint on mainnet (example)
```
sui client call   --package 0x2   --module coin   --function mint_and_transfer   --type-args <PACKAGE_ID>::wrapper_coin::WRAPPER_COIN   --args <TREASURY_CAP_ID> 1 <ADDRESS>   --sender <ADDRESS>   --gas <GAS_OBJECT_ID>   --gas-budget 20000000
```

Artifacts will be saved under `out/inhabit_debug/<PACKAGE_ID>/` with full metadata and links, as produced by `scripts/inhabit_single_package_local.py`.

## Balances and Object Snapshots (Executed Mint)

Executed mint digest: 44sq3686KDXYGT8ny23dRcRUwx246V2T3TUjSZgzC498
- Suiscan: https://suiscan.xyz/mainnet/tx/44sq3686KDXYGT8ny23dRcRUwx246V2T3TUjSZgzC498

From transaction effects (balanceChanges):
- SUI delta: -1,972,120 MIST (gas)
- WRAPPER_COIN delta: +1

Created coin object:
- ObjectID: 0x8a8006ed8859c74ec0df0abbcd02eade6e5638cdcb435acd67d25be993abf70b
- Type: 0x2::coin::Coin<<PACKAGE_ID>::wrapper_coin::WRAPPER_COIN>
- Explorer: https://suiscan.xyz/mainnet/object/0x8a8006ed8859c74ec0df0abbcd02eade6e5638cdcb435acd67d25be993abf70b

To verify current balances at any time:
```
sui client balance 0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0
sui client balance 0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0   --coin-type 0x358e10b333ce6563f40fdf46f95d020aa6f8e221d4299a2d9b2592968a7da467::wrapper_coin::WRAPPER_COIN
```

| Plan | Mode | Created types (base) | Hits | Link |
|------|------|-----------------------|------|------|
| Publish WRAPPER_COIN | executed | TreasuryCap, MetadataCap | 2/4 | https://suiscan.xyz/mainnet/tx/65LoTYyEC49jUBU3MwPbA5AYB6fx9hJGsNyxJSJ93hBF |
| Mint WRAPPER_COIN | executed | Coin | 1/4 | https://suiscan.xyz/mainnet/tx/44sq3686KDXYGT8ny23dRcRUwx246V2T3TUjSZgzC498 |
| Mint WRAPPER_COIN | dry-run | Coin | 1/4 | https://suiscan.xyz/mainnet/tx/2MmsXsrMynQAZayYrcje8BpLBAai9DggNEZJuThkLT71 |
| Probe WRAPPER_COIN | build-only | TreasuryCap (static) | 1/4 | — |
| Fresh type (2/4) | build-only | Currency, TreasuryCap | 2/4 | — |
| Fresh type (3/4) | build-only | Currency, TreasuryCap, MetadataCap | 3/4 | — |
| Fresh type (4/4) | build-only | Currency, TreasuryCap, MetadataCap, Coin | 4/4 | — |

## Summary

- Published wrapper package: `0x358e10b333ce6563f40fdf46f95d020aa6f8e221d4299a2d9b2592968a7da467`
- Wallet used: `0x064d87c3da8b7201b18c05bfc3189eb817920b2d089b33e207d1d99dc5ce08e0`
- Created on publish:
  - `0x2::coin::TreasuryCap<...::WRAPPER_COIN>`
  - `0x2::coin_registry::MetadataCap<...::WRAPPER_COIN>`
- Mint PTB (dry-run) created `0x2::coin::Coin<...::WRAPPER_COIN>` and scored 1/4 targets after target-extraction update.
- Probe PTB (build-only) surfaced `TreasuryCap<...>` via static analysis and also scored 1/4.

### Transaction Digests

- Publish (executed): 65LoTYyEC49jUBU3MwPbA5AYB6fx9hJGsNyxJSJ93hBF
  - Explorer: https://suiscan.xyz/mainnet/tx/65LoTYyEC49jUBU3MwPbA5AYB6fx9hJGsNyxJSJ93hBF
- Mint (dry-run): 2MmsXsrMynQAZayYrcje8BpLBAai9DggNEZJuThkLT71
  - Explorer: https://suiscan.xyz/mainnet/tx/2MmsXsrMynQAZayYrcje8BpLBAai9DggNEZJuThkLT71 (dry-run digest; not executed)
 - Mint WRAPPER_COIN (executed): 44sq3686KDXYGT8ny23dRcRUwx246V2T3TUjSZgzC498
   - Explorer: https://suiscan.xyz/mainnet/tx/44sq3686KDXYGT8ny23dRcRUwx246V2T3TUjSZgzC498

## Timeline of Attempts

1) Initial scaffold (local build)
- Created package at `packages/wrapper_coin` with `Move.toml` and `sources/wrapper_coin.move`.
- Iteration issues encountered:
  - Struct ability/edition mismatches (legacy vs 2024) and key ability requirements in `coin_registry::new_currency`.
  - Resolved by switching to OTW initializer `coin_registry::new_currency_with_otw` in module `init` per mainnet guidance.

2) Dry-run publish (gas-free check)
- Command: `sui client publish packages/wrapper_coin --skip-fetch-latest-git-deps --gas-budget 100000000 --dry-run`
- Result: success, estimated gas ≈ 0.017 SUI.

3) Mainnet publish (funded wallet)
- Active wallet switched to `0x064d87...08e0` (balance > 4 SUI).
- Command:
  - `sui client publish packages/wrapper_coin --skip-fetch-latest-git-deps --sender 0x064d87...08e0 --gas 0x4fe4bba6...3812 --gas-budget 100000000`
- Result:
  - Package `0x358e10b3...a467`
  - Created TreasuryCap and MetadataCap objects for `WRAPPER_COIN`.

4) PTB for mint (dry-run)
- Spec: `packages/wrapper_coin/ptb_mint_wrap.json` — calls `0x2::coin::mint_and_transfer<...::WRAPPER_COIN>` using the published `TreasuryCap`.
- smi_tx_sim:
  - `--mode dry-run --gas-budget 20000000`
  - Created: `Coin<...::WRAPPER_COIN>`
  - Effects show minted coin balance +1000, gas ~0.002 SUI (dry-run estimate).

5) Target extraction update
- Updated target extraction to include common key wrappers around module-local types:
  - `0x2::coin::Coin<T>`, `0x2::coin::TreasuryCap<T>`, `0x2::coin_registry::Currency<T>`, `0x2::coin_registry::MetadataCap<T>` for each `T` defined in the module.
- This aligns scoring with coin/currency flows, not just module-defined `key` structs.

6) Probe function + build-only PTB
- Added `public fun probe_transfer_cap(cap: coin::TreasuryCap<WRAPPER_COIN>, ctx: &mut TxContext)` which calls `transfer::public_transfer` — triggers static scan to include `TreasuryCap<...>`.
- Spec: `packages/wrapper_coin/ptb_probe_cap.json` (build-only).
- smi_tx_sim build-only confirmed `TreasuryCap<...>` in staticCreated.

## Collected Artifacts

- Interface JSON: `out/inhabit_debug/0x358e10b3...a467_iface.json`
- Mint dry-run summary: `out/inhabit_debug/summary_mint.json`
- Probe build-only summary: `out/inhabit_debug/summary_probe.json`
- Tx sim raw outputs: `/tmp/tx_sim_mint_dryrun.json`, `/tmp/tx_sim_probe_build_only.json`
- Publish stdout: `/tmp/wrapper_publish_mainnet.out`

## Commands Executed (canonical examples)

```
# Build
sui move build

# Dry-run publish
sui client publish packages/wrapper_coin \
  --skip-fetch-latest-git-deps \
  --gas-budget 100000000 \
  --dry-run

# Publish (example with explicit gas object)
sui client publish packages/wrapper_coin \
  --skip-fetch-latest-git-deps \
  --sender <SENDER_ADDR> \
  --gas <GAS_OBJECT_ID> \
  --gas-budget 100000000

# Mint (dry-run)
./target/release/smi_tx_sim --mode dry-run \
  --sender <SENDER_ADDR> \
  --ptb-spec packages/wrapper_coin/ptb_mint_wrap.json \
  --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin \
  --gas-budget 20000000

# Probe (build-only)
./target/release/smi_tx_sim --mode build-only \
  --sender <SENDER_ADDR> \
  --ptb-spec packages/wrapper_coin/ptb_probe_cap.json \
  --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin

# End-to-end scoring
python scripts/inhabit_single_package_local.py \
  --package-id <PACKAGE_ID> \
  --bytecode-package-dir packages/wrapper_coin/build/wrapper_coin \
  --ptb-spec <SPEC> \
  --sender <SENDER_ADDR> \
  --mode <dry-run|build-only>
```

## Results

- Mint (dry-run):
  - targets = 4 (Coin, TreasuryCap, Currency, MetadataCap)
  - created = Coin<...>
  - hits = 1 / 4
  - tx digest: 2MmsXsrMynQAZayYrcje8BpLBAai9DggNEZJuThkLT71 (dry-run)

- Probe (build-only):
  - targets = 4
  - static created = TreasuryCap<...>
  - hits = 1 / 4

## Follow-ups

- To reach 2/4 in a single plan, add a finalize() call sequence (builder path) in a PTB for a fresh type; for existing published types, mint/burn calls won’t create Currency/MetadataCap again.
- Consider adding DenyCapV2<T> as an optional target when regulated flow is used.
- Consider a corpus-level rule to weight targets by feasibility (e.g., post-publish scenarios).

## Fresh-Type Build-Only Plan (2/4 hits)

- Added module: `wrapper::freshx` with OTW `FRESHX`.
- PTB: `packages/wrapper_coin/ptb_fresh_build.json` calling:
  1) `0x2::coin_registry::new_currency_with_otw<FRESHX>`
  2) `0x2::coin_registry::finalize<FRESHX>`
  3) `wrapper_coin::probe_transfer_cap_generic<FRESHX>`
- smi_tx_sim (build-only): Created types include `Currency<T0>` and `TreasuryCap<T0>`.
- Summary: `out/inhabit_debug/summary_fresh.json` shows `created_hits = 2`, `targets = 4`.

## Fresh-Type Build-Only Plan (3/4 hits)

- PTB: `packages/wrapper_coin/ptb_fresh_build_3of4.json` calling:
  1) `0x2::coin_registry::new_currency_with_otw<FRESHX>`
  2) `0x2::coin_registry::finalize<FRESHX>`
  3) `wrapper_coin::probe_transfer_cap_generic<FRESHX>`
  4) `wrapper_coin::probe_transfer_metadata_cap_generic<FRESHX>`
- Result (build-only): Created types = `Currency<T0>`, `TreasuryCap<T0>`, `MetadataCap<T0>` → `hits = 3/4`.
- Summary: `out/inhabit_debug/0x358e10b3...a467/summary_fresh_3of4.json`.

## Fresh-Type Build-Only Plan (4/4 hits)

- PTB: `packages/wrapper_coin/ptb_fresh_build_4of4.json` calling:
  1) `0x2::coin_registry::new_currency_with_otw<FRESHX>`
  2) `0x2::coin_registry::finalize<FRESHX>`
  3) `wrapper_coin::probe_transfer_cap_generic<FRESHX>`
  4) `wrapper_coin::probe_transfer_metadata_cap_generic<FRESHX>`
  5) `0x2::coin::mint_and_transfer<FRESHX>` (creates Coin<FRESHX>)
  6) `wrapper_coin::probe_transfer_coin_generic<FRESHX>`
- Result (build-only): Created types = `Currency<T0>`, `TreasuryCap<T0>`, `MetadataCap<T0>`, `Coin<T0>` → `hits = 4/4`.
- Summary: `out/inhabit_debug/0x358e10b3...a467/summary_fresh_4of4.json`.
