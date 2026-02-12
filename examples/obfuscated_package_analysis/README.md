# Obfuscated Package Analysis

Reverse-engineer an obfuscated Sui Move package using bytecode inspection and transaction replay.

## What This Demonstrates

Some on-chain packages have **obfuscated function and field names** with no published source code, making them opaque to standard block explorers. This example shows how `sui-sandbox` can still extract meaningful structure from such packages by combining two capabilities:

1. **Static bytecode analysis** — Extract typed function signatures, struct layouts, module dependencies, and friend relationships directly from on-chain bytecode, even when names are randomized hex strings.

2. **PTB replay** — Re-execute historical transactions locally and verify effects match on-chain. This maps concrete objects to specific function parameters, confirming what each argument slot actually receives.

Together, these reveal the architecture and behavior of packages that would otherwise be black boxes.

## Case Study: Stonker (Sui Market-Making Bot)

[Stonker](https://github.com/RandyPen/Stonker) is a multi-DEX market-making bot on Sui with:

- **14 modules** with obfuscated names (e.g., `aaf6cc9d45ba0185f`, `a5ada7d72000d7a1d`)
- **No source code** in the repository
- **Integration with 6 DEX protocols** (Deepbook, Cetus, Turbos, Bluefin, MMT, NAVI)
- **Complex PTBs** with up to 22 arguments in a single MoveCall

Package ID: `0xe3b9bd64ba2fb3256293c3fc0119994ec6fc7c96541680959de4d7052be65973`

Full analysis results: [Stonker PR #1](https://github.com/RandyPen/Stonker/pull/1)

## Workflow

### Step 1: Fetch Package Bytecode

```bash
sui-sandbox fetch package \
  0xe3b9bd64ba2fb3256293c3fc0119994ec6fc7c96541680959de4d7052be65973 \
  --with-deps
```

This downloads the package bytecode and all transitive dependencies (15 packages total including Deepbook, Cetus, Turbos, etc.) into the local store.

### Step 2: Inspect Module Interfaces

```bash
# List all modules
sui-sandbox view modules \
  0xe3b9bd64ba2fb3256293c3fc0119994ec6fc7c96541680959de4d7052be65973

# View a specific module's full typed signatures
sui-sandbox view module \
  0xe3b9bd64ba2fb3256293c3fc0119994ec6fc7c96541680959de4d7052be65973::stonker --json
```

Despite obfuscated names, this reveals:
- **Function parameter types** — e.g., a function taking `&mut BalanceManager`, `&Pool<SUI, USDC>`, `&Clock` is clearly a Deepbook operation
- **Struct field types** — config objects with `address`, `bool`, `u64` fields reveal bot parameters
- **Friend declarations** — which modules can call which, exposing the internal architecture
- **Dependency linkage** — maps original package IDs to upgraded storage addresses

### Step 3: Discover Transactions

Find recent transactions for the package using a block explorer (e.g., SuiScan) filtered by the package address. Look for transactions with varying numbers of inputs — these exercise different code paths.

### Step 4: Replay Transactions

```bash
# Simple operation (5 inputs — order cancellation)
sui-sandbox replay 7kMBy9LW6NshWEGi6TvAjeMUu1mc9TugY93RmRRbwf2r --compare --verbose

# Complex operation (21 inputs — multi-DEX rebalance across 6 protocols)
sui-sandbox replay HXAygNqYf7AP1JgtTXT4SmJAJ8Q6vG5TWjb6aBgkfgGY --compare --verbose

# Config update (2 inputs)
sui-sandbox replay DsgegauRVGMvVxEMeySoRceJTSGZa15F63pFxHMA2Hzr --compare --verbose
```

A successful replay with `--compare` confirms:
- **Status match** — local execution succeeded/failed the same way
- **Created/Mutated/Deleted match** — identical object effects

### Step 5: Map Arguments to Parameters

The replay output combined with `view module --json` signatures lets you map each transaction input to its function parameter. For example, the 22-argument rebalance function:

| Arg | Type | Object |
|-----|------|--------|
| arg0 | `&mut stonker::Config` | Stonker config state |
| arg1 | `&mut deepbook::BalanceManager` | Deepbook balance manager |
| arg2 | `&mut Coin<SUI>` | Gas coin (passed as mutable ref) |
| arg3 | `&deepbook::Pool<SUI,USDC>` | Deepbook SUI/USDC pool |
| arg4-7 | Cetus pool, position, config, rewarder | Cetus CLMM objects |
| arg8-9 | Turbos pool, versioned | Turbos finance objects |
| arg10-11 | Bluefin pool, config | Bluefin exchange objects |
| ... | ... | ... |

This parameter mapping is not available from any block explorer — explorers show the input objects but not which function parameter slot each one maps to.

## What Replay Reveals That Explorers Can't

1. **Parameter-to-object mapping** — For a 22-argument obfuscated function, replay confirms exactly which object is `arg0`, `arg1`, etc. Explorers show the input list but not the mapping to function parameters.

2. **Decompiled signature validation** — Static analysis produces candidate function signatures. Replay proves they're correct: if the VM successfully deserializes all arguments according to those types and produces matching effects, the signatures are verified.

3. **GasCoin-as-argument detection** — Sui allows passing the gas coin directly as a `&mut Coin<SUI>` parameter. This is invisible in explorer transaction views but critical for understanding fund flows. (This case study also uncovered a [replay engine bug](https://github.com/Evan-Kim2028/sui-sandbox/commit/f0a8908) in GasCoin handling for MoveCall arguments.)

4. **Cross-protocol interaction patterns** — Replay shows which shared objects are accessed together, revealing the bot's multi-DEX arbitrage/rebalancing strategy across Deepbook, Cetus, Turbos, Bluefin, MMT, and NAVI in a single atomic transaction.

## Running the Script

```bash
# Analyze the Stonker package (default)
./examples/obfuscated_package_analysis/cli_obfuscated_analysis.sh

# Analyze any other obfuscated package (static analysis only)
./examples/obfuscated_package_analysis/cli_obfuscated_analysis.sh <PACKAGE_ID>
```

**Prerequisites**: gRPC endpoint with historical data configured in `.env` (transaction replay requires archival coverage for pruned transactions). Static analysis (fetch + view) works without gRPC.

## Prerequisites

```bash
cp .env.example .env
# Edit .env with your gRPC endpoint and API key
```

Example `.env`:

```
SUI_GRPC_ENDPOINT=https://grpc.surflux.dev:443
SUI_GRPC_API_KEY=<your-api-key>
SUI_GRAPHQL_ENDPOINT=https://graphql.mainnet.sui.io/graphql
```

Public Sui fullnodes prune historical data after ~2 epochs. For replaying older transactions, use a gRPC provider with archival coverage (e.g., [Surflux](https://surflux.dev)).
