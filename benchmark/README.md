# Benchmark Harness (`benchmark/`)

This directory contains the automated benchmarking harness for Sui Move packages.

## Phase Overview

- **Phase I (Key-Struct Discovery):** Predict which structs in a package have the `key` ability based on field shapes.
- **Phase II (Type Inhabitation):** Plan valid transaction sequences (Programmable Transaction Blocks) to create target Move objects.

## 🚀 Getting Started

> **[GETTING_STARTED.md](./GETTING_STARTED.md)** — Start here for installation, API setup, and running your first benchmark.

## Benchmark Features

- **Bytecode-First:** All ground truth is derived from compiled Move bytecode, ensuring accuracy even for private constructors.
- **A2A Compliant:** Implements Google's Agent2Agent (A2A) protocol for seamless integration with the AgentBeats platform.
- **Planning-Focused:** Automatically corrects common JSON formatting errors to measure true planning and reasoning capability.
- **Multi-Model Support:** Built-in scripts for parallel evaluation of multiple models via OpenRouter.

## Key Resources

- **[Methodology](../docs/METHODOLOGY.md)** - Detailed scoring rules and extraction logic.
- **[A2A Compliance](docs/A2A_COMPLIANCE.md)** - Protocol implementation and testing strategy.
- **[A2A Examples](docs/A2A_EXAMPLES.md)** - Concrete JSON-RPC request/response examples.
- **[Architecture](docs/ARCHITECTURE.md)** - Internal design of the benchmark harness.

## Quick Command Reference

```bash
# Run local A2A scenario
uv run smi-agentbeats-scenario scenario_smi --launch-mode current

# Run smoke test
uv run smi-a2a-smoke --corpus-root ../sui-packages/packages/mainnet_most_used --samples 1

# View results
python scripts/phase2_status.py results/my_run.json
```
