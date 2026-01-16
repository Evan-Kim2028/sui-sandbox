# Transaction Replay Guide

Replay real Sui mainnet transactions locally for testing and analysis.

## Overview

Transaction replay allows you to:

1. Download transaction data from mainnet
2. Replay it in the local sandbox
3. Compare results with actual on-chain effects

## Prerequisites

- Network access to Sui RPC
- Disk space for cached transactions

## Workflow

### Step 1: Download Transactions

```bash
# Download a specific transaction
sui-move-interface-extractor tx-replay \
  --digest <TRANSACTION_DIGEST> \
  --download-only

# Download recent transactions
sui-move-interface-extractor tx-replay \
  --recent 100 \
  --download-only
```

This creates `.tx-cache/` containing:

- Transaction commands and inputs
- Object states at time of transaction
- Package bytecode

### Step 2: Replay with PTB Eval

```bash
sui-move-interface-extractor ptb-eval \
  --cache-dir .tx-cache/ \
  --verbose
```

Options:

- `--max-retries N`: Retry with dependency fetching (default: 3)
- `--enable-fetching`: Allow fetching missing packages/objects from mainnet
- `--framework-only`: Only test Sui framework transactions
- `--third-party-only`: Only test third-party package transactions
- `--limit N`: Process only N transactions

### Step 3: Analyze Results

```
=== PTB Evaluation Summary ===
Total evaluated: 1000
Success: 847 (84.7%)
Failed: 153 (15.3%)

--- By Transaction Type ---
Framework-only: 312/320 (97.5%)
Third-party: 535/680 (78.7%)

--- Error Categories ---
  ContractAbort(3): 42
  MissingObject: 38
  ...
```

## Dependency Fetching

When `--enable-fetching` is set, the evaluator automatically fetches:

- **Missing packages**: If a transaction references a package not in cache
- **Missing objects**: If a transaction references an object not in cache

This is automatic in PTB eval mode (for regression testing), but **not** in interactive sandbox mode (LLM must explicitly fetch).

## Output Format

Results are written as JSONL:

```json
{
  "digest": "...",
  "is_framework_only": false,
  "command_count": 3,
  "input_count": 5,
  "status": "Success",
  "retry_count": 0,
  "healing_actions": [],
  "error": null
}
```

## Use Cases

### Regression Testing

Test sandbox accuracy against known-good mainnet transactions:

```bash
# Download diverse transaction set
sui-move-interface-extractor tx-replay --recent 1000 --download-only

# Run evaluation
sui-move-interface-extractor ptb-eval --cache-dir .tx-cache/ --output results.jsonl
```

### Debugging Specific Transactions

```bash
# Download the problematic transaction
sui-move-interface-extractor tx-replay --digest <DIGEST> --download-only

# Replay with verbose output
sui-move-interface-extractor ptb-eval --cache-dir .tx-cache/ --verbose
```

### Comparing Framework vs Third-Party

```bash
# Framework only
sui-move-interface-extractor ptb-eval --framework-only

# Third party only
sui-move-interface-extractor ptb-eval --third-party-only
```

## Limitations

- Historical object state may not be available for very old transactions
- Gas metering may differ from mainnet (use `sui_dryRunTransactionBlock` for exact gas)
- Randomness is deterministic (not VRF-based like mainnet)
- `ecvrf` operations are mocked (VRF verification not yet implemented)

## See Also

- [CLI Reference](../reference/CLI_REFERENCE.md) - Full command options
- [Error Codes](../reference/ERROR_CODES.md) - Understanding failures
