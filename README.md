# `sui-move-interface-extractor`

**Quantifying AI's understanding of Sui Move smart contracts through bytecode extraction and autonomous transaction planning.**

This project provides a standalone Rust CLI for parsing Sui Move `.mv` modules into deterministic JSON and a Python-based benchmark harness to evaluate LLM performance on complex "Type Inhabitation" (autonomous transaction planning) challenges.

```mermaid
graph LR
    A[Sui Bytecode] --> B[Rust Extractor]
    B --> C[JSON Interface]
    C --> D[LLM Agent]
    D --> E[PTB Generation]
    E --> F[Transaction Simulation]
    F --> G[Score / Metrics]
```

## Quick Links

| Audience | Start Here |
|----------|------------|
| **New Users** | [Quick Start Guide](QUICK_START_GUIDE.md) |
| **Researchers** | [Methodology](docs/METHODOLOGY.md) |
| **Operators** | [Production Deployment](PRODUCTION_DEPLOYMENT.md) |
| **Contributors** | [Architecture](benchmark/docs/ARCHITECTURE.md) |

---

## 🔥 Primary Features

1. **Bytecode Interface Extractor (Rust)**: High-performance CLI to extract canonical JSON interfaces from compiled `.mv` modules.
2. **No-Chain Validator (`benchmark-local`)**: Portable, deterministic validation of Type Inhabitation using a local Move VM—no RPC or gas required.
3. **LLM Benchmark Harness (Python)**: Comprehensive E2E pipeline to evaluate AI understanding of Move code via autonomous transaction planning.
4. **Dockerized API (A2A)**: Production-ready Agent-to-Agent interface for running large-scale evaluations with detailed analytics.

---

## 🚀 Quick Start: Top 25 Dataset

The **Top 25 Dataset** is the recommended starting point for running meaningful benchmarks. It contains 25 curated mainnet packages that represent diverse Move patterns.

### Run the Top 25 Benchmark

```bash
# 1. Build the Rust binary
cargo build --release

# 2. Clone the corpus (one-time)
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages

# 3. Run with the Top 25 dataset
cd benchmark
uv run smi-inhabit \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --dataset type_inhabitation_top25 \
  --samples 25 \
  --agent real-openai-compatible \
  --out results/top25_run.json
```

### Docker Quick Start (Recommended)

```bash
# Start the Docker API
docker compose up -d smi-bench

# Run Top 25 via HTTP API
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 25 \
    --models google/gemini-3-flash-preview \
    --simulation-mode dry-run
```

See [benchmark/DATASETS.md](benchmark/DATASETS.md) for all available datasets.

---

## 🧪 Case Study: Liquid Staking Package

The **Liquid Staking package** (`0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700`) is a complex Move package that serves as an excellent benchmark for LLM capabilities. It has no trivial entry points—requiring the agent to understand and chain multiple constructors.

### Why This Package?

- **Complex Type Dependencies**: Creating an LST token requires calling `liquid_staking::create_lst` with specific capability objects
- **No Simple Paths**: Unlike `coin::mint`, there's no single entry function that creates the target type
- **Real-World DeFi Logic**: Represents actual production staking infrastructure

### Run the E2E Pipeline

```bash
cd benchmark

# Test the LLM's ability to generate helper packages
export SMI_E2E_REAL_LLM=1
export OPENROUTER_API_KEY=sk-or-v1-...

uv run python scripts/e2e_one_package.py \
    --corpus-root ../sui-packages/packages/mainnet_most_used \
    --package-id 0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700 \
    --model google/gemini-3-flash-preview \
    --out-dir results/lst_case_study
```

### What the Pipeline Does

1. **Extracts Interface**: Parses bytecode to JSON interface
2. **LLM Generates Helper**: Agent creates a Move helper package that imports the target
3. **Compiles Helper**: `sui move build` in Docker with vendored dependencies
4. **Simulates TX**: Executes in local VM to verify type inhabitation

See [specs/LIQUID_STAKING_CASE_STUDY.md](specs/LIQUID_STAKING_CASE_STUDY.md) for detailed analysis.

---

## ⚡ Local Type Inhabitation Benchmark (No-Chain)

The `benchmark-local` subcommand validates type inhabitation **without any network access**. It uses a local Move VM with synthetic state.

### Usage

```bash
# Tier A only (fast, bytecode validation)
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode_modules \
  --output results.jsonl \
  --tier-a-only

# Full validation with VM execution (Tier A + B)
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode_modules \
  --output results.jsonl \
  --restricted-state
```

### Validation Tiers

| Tier | Name | What It Validates |
|------|------|-------------------|
| **A** | Preflight | Bytecode resolution, BCS serialization, type layouts |
| **B** | VM Execution | Local Move VM execution with synthetic state |

### Why Use `benchmark-local`?

- **Deterministic**: Same bytecode + same input = same result, every time
- **Fast**: Validates 100+ modules in seconds
- **Portable**: Works in offline/air-gapped CI/CD pipelines
- **Zero Cost**: No Sui tokens or gas budget required

See [docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) for the full technical specification.

---

## 📦 Installation

### Prerequisites

- **Rust**: 1.75+ (`rustup update stable`)
- **Python**: 3.11+ with `uv` (`curl -LsSf https://astral.sh/uv/install.sh | sh`)
- **Docker**: For containerized benchmarks (optional but recommended)

### Build from Source

```bash
# Clone repository
git clone https://github.com/your-org/sui-move-interface-extractor.git
cd sui-move-interface-extractor

# Build Rust CLI
cargo build --release

# Install Python dependencies
cd benchmark
uv sync --group dev --frozen
```

### Verify Installation

```bash
# Test Rust CLI
./target/release/sui_move_interface_extractor --help

# Test Python harness
cd benchmark
uv run smi-inhabit --help
```

---

## 🐳 Docker Deployment

```bash
# Start the benchmark API
docker compose up -d smi-bench

# Verify health
curl -s http://localhost:9999/health | jq '.status'
# Expected: "ok"

# Run a benchmark
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 5 \
    --models gpt-4o-mini \
    --simulation-mode dry-run
```

See [PRODUCTION_DEPLOYMENT.md](PRODUCTION_DEPLOYMENT.md) for production setup including offline mode.

---

## 📚 Documentation

### Core Guides

| Document | Description |
|----------|-------------|
| [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md) | 30-second quick start with all features |
| [benchmark/GETTING_STARTED.md](benchmark/GETTING_STARTED.md) | Complete benchmark setup and usage |
| [PRODUCTION_DEPLOYMENT.md](PRODUCTION_DEPLOYMENT.md) | Production deployment with Docker |

### Technical References

| Document | Description |
|----------|-------------|
| [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md) | All CLI commands and flags |
| [docs/METHODOLOGY.md](docs/METHODOLOGY.md) | Scoring rules and research methodology |
| [docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) | Tier A/B validation specification |
| [benchmark/DATASETS.md](benchmark/DATASETS.md) | Dataset creation and usage |

### Architecture & Internals

| Document | Description |
|----------|-------------|
| [benchmark/docs/ARCHITECTURE.md](benchmark/docs/ARCHITECTURE.md) | Python harness architecture |
| [IMPLEMENTATION_SUMMARY.md](IMPLEMENTATION_SUMMARY.md) | Implementation decisions |
| [docs/A2A_PROTOCOL.md](docs/A2A_PROTOCOL.md) | A2A protocol integration |

---

## 🧪 Running Tests

```bash
# Rust tests
cargo test

# Python tests
cd benchmark
uv run pytest tests/ -v

# Full CI checks
cargo fmt && cargo clippy && cargo test
cd benchmark && uv run pytest
```

---

## 📊 Output Example

### `benchmark-local` Output (JSONL)

```json
{
  "target_package": "0x059f94b85c07eb74...",
  "target_module": "liquid_staking",
  "target_function": "create_lst",
  "status": "tier_b_hit",
  "tier_a_details": {"validation_time_ms": 12, "bcs_roundtrip_verified": true},
  "tier_b_details": {"execution_success": true, "gas_used": 1000}
}
```

### Phase II Benchmark Output

```json
{
  "schema_version": 2,
  "aggregate": {
    "avg_hit_rate": 0.42,
    "packages_total": 25,
    "errors": 0
  },
  "packages": [...]
}
```

---

## 🤝 Contributing

See [AGENTS.md](AGENTS.md) for development guidelines and coding conventions.

```bash
# Development workflow
cargo fmt && cargo clippy
cd benchmark && uv run pytest tests/ -v
```

---

## 📄 License

MIT License - see [LICENSE](LICENSE) for details.
