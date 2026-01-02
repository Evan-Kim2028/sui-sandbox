# Getting Started with Phase II Benchmark

This guide covers everything you need to run the Phase II (Type Inhabitation) benchmark, from initial setup to running multi-model evaluations.

---

## 1. One-time Setup

### Environment Dependencies
We use `uv` for Python dependency management and `cargo` for Rust components.

```bash
cd benchmark
uv sync --group dev --frozen

# Build Rust binaries (extractor and transaction simulator)
cd .. && cargo build --release --locked && cd benchmark

# Clone the package corpus (required for benchmarking)
git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages
```

### Credentials Configuration
Copy `.env.example` to `.env` and set your model credentials.

```bash
cp .env.example .env
```

**Recommended (OpenRouter):** A single API key for 150+ models.
```env
OPENROUTER_API_KEY=sk-or-v1-...
SMI_API_BASE_URL=https://openrouter.ai/api/v1
SMI_MODEL=anthropic/claude-sonnet-4.5
SMI_SENDER=0xYOUR_FUNDED_MAINNET_ADDRESS
```

---

## 2. Benchmark Modes

The benchmark can be run in two primary modes:

### A) Direct Mode (For Developers)
Runs the benchmark harness directly against the Rust extractor. Best for rapid iteration on prompts or debugging specific packages.

```bash
# Quick test (3 packages)
uv run smi-inhabit \
  --corpus-root ../sui-packages/packages/mainnet_most_used \
  --dataset packages_with_keys \
  --agent real-openai-compatible \
  --samples 3
```

### B) A2A Mode (For Integrators)
Follows the **Agent-to-Agent (A2A)** pattern used by the AgentBeats platform.

1.  **Start the Scenario** (launches local Green and Purple agents):
    ```bash
    uv run smi-agentbeats-scenario scenario_smi --launch-mode current
    ```

2.  **Send a Smoke Request**:
    ```bash
    uv run smi-a2a-smoke --scenario scenario_smi --corpus-root ../sui-packages/packages/mainnet_most_used --samples 1
    ```

---

## 3. Multi-Model Benchmarking

Use the provided scripts to evaluate multiple models in parallel via OpenRouter.

```bash
# Test 10 models in parallel (10 packages each)
./scripts/run_multi_model.sh 10 3
```

**Supported Models:** GLM-4.7, GPT-4o, Claude Sonnet 4.5, DeepSeek V3, Gemini 3 Flash, and more.

---

## 4. Understanding Results

### Scoring Metrics
- **`avg_hit_rate`**: Overall success rate (created objects / target objects).
- **`planning_only_hit_rate`**: Success rate excluding pure JSON formatting failures. **(Recommended for model comparison)**
- **`causality_success_rate`**: Percentage of plans with valid PTB dependency chaining.

### Analysis Tools
```bash
# View run status
python scripts/phase2_status.py results/my_run.json

# Compare multiple runs (Leaderboard)
python scripts/phase2_leaderboard.py results/run_a.json results/run_b.json
```

---

## 5. Troubleshooting

- **Rate Limits**: If using OpenRouter, reduce parallelism in `run_multi_model.sh`.
- **Empty Metrics**: Ensure `SMI_SENDER` is set to a funded address for dry-runs.
- **Port Conflicts**: If agents fail to start, check if ports 9999 (Green) or 9998 (Purple) are in use.

---

## Related Documentation

- [METHODOLOGY.md](../docs/METHODOLOGY.md) - Scoring rules and extraction logic.
- [A2A_COMPLIANCE.md](docs/A2A_COMPLIANCE.md) - Protocol implementation details.
- [A2A_EXAMPLES.md](docs/A2A_EXAMPLES.md) - Concrete JSON-RPC request/response examples.
