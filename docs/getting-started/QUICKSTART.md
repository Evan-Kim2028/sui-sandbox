# Quickstart

Get up and running with the sui-move-interface-extractor in minutes.

## Prerequisites

- Rust toolchain (1.70+)
- ~2GB disk space for Sui framework bytecode

## Installation

```bash
# Clone and build
git clone <repository-url>
cd sui-move-interface-extractor
cargo build --release

# Add to PATH (optional)
export PATH="$PATH:$(pwd)/target/release"
```

## Choose Your Path

### I want to... run the interactive sandbox (for LLM integration)

```bash
# Start interactive mode (reads JSON from stdin, writes to stdout)
sui-move-interface-extractor sandbox-exec --interactive

# Discover available tools
echo '{"action": "list_available_tools"}' | sui-move-interface-extractor sandbox-exec -i - -o -
```

See: [LLM Integration Guide](../guides/LLM_INTEGRATION.md)

### I want to... run benchmarks on Move packages

```bash
# Quick test with a single package
sui-move-interface-extractor benchmark-local \
  --target-corpus ./path/to/bytecode \
  --output results.jsonl

# View results
cat results.jsonl | jq '.status'
```

See: [Running Benchmarks](../guides/RUNNING_BENCHMARKS.md)

### I want to... replay mainnet transactions

```bash
# Download a transaction and its dependencies
sui-move-interface-extractor tx-replay \
  --digest <TRANSACTION_DIGEST> \
  --download-only

# Replay it locally
sui-move-interface-extractor ptb-eval \
  --cache-dir .tx-cache/
```

See: [Transaction Replay](../guides/TRANSACTION_REPLAY.md)

## 30-Second Demo

```bash
# 1. Start the sandbox
sui-move-interface-extractor sandbox-exec --interactive << 'EOF'
{"action": "get_clock"}
{"action": "list_modules"}
{"action": "list_functions", "module_path": "0x2::coin"}
EOF
```

This will:
1. Show the current simulated clock time
2. List all loaded modules (Sui framework by default)
3. List functions in the `0x2::coin` module

## Directory Structure

After running, you'll see:

```
.tx-cache/           # Cached transactions (if using tx-replay)
.sui-llm-logs/       # Execution logs and artifacts
results.jsonl        # Benchmark results (if running benchmarks)
```

## Common Options

| Flag | Description |
|------|-------------|
| `--verbose` / `-v` | Show detailed output |
| `--enable-fetching` | Allow fetching packages/objects from mainnet |
| `--state-file FILE` | Persist sandbox state between runs |
| `--help` | Show all options |

## Next Steps

- [Understand the architecture](../../ARCHITECTURE.md)
- [Integrate with an LLM](../guides/LLM_INTEGRATION.md)
- [CLI Reference](../reference/CLI_REFERENCE.md)

## Troubleshooting

### "Module not found" errors

The sandbox loads the Sui framework (0x1, 0x2, 0x3) by default. For other packages:

```bash
# Fetch from mainnet
echo '{"action": "deploy_package_from_mainnet", "address": "0x<PACKAGE_ADDRESS>"}' \
  | sui-move-interface-extractor sandbox-exec -i - -o -
```

### Build errors

```bash
# Clean rebuild
cargo clean
cargo build --release
```

### Need more help?

See [Troubleshooting Guide](../TROUBLESHOOTING.md) or open an issue.
