# Documentation Index

**Complete guide to all documentation for Sui Move Interface Extractor Benchmark.**

---

## 🚀 Quick Navigation

### New Users (Start Here)
1. [**QUICK_START_GUIDE.md**](QUICK_START_GUIDE.md) - ⚡ 30-second quick start
   - Fastest way to test with Docker + HTTP API
   - Real-world test script
   - Model comparison testing
   - Docker fluid usage

2. [**README.md**](README.md) - Project overview and setup

3. [**benchmark/GETTING_STARTED.md**](benchmark/GETTING_STARTED.md) - Environment setup guide

### Production Users
1. [**PRODUCTION_DEPLOYMENT.md**](PRODUCTION_DEPLOYMENT.md) - Complete production deployment
2. [**QUICK_START_GUIDE.md**](QUICK_START_GUIDE.md) - Models, Docker, fast runs

### Local Benchmark (No-Chain) - NEW
1. [**docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md**](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) - Technical specification for offline validation
2. [**docs/CLI_REFERENCE.md**](docs/CLI_REFERENCE.md) - `benchmark-local` command documentation
3. [**benchmark/scripts/e2e_one_package.py**](benchmark/scripts/e2e_one_package.py) - E2E one-package evaluation script

### Datasets & Case Studies
1. [**benchmark/DATASETS.md**](benchmark/DATASETS.md) - Dataset creation and usage guide
2. [**specs/LIQUID_STAKING_CASE_STUDY.md**](specs/LIQUID_STAKING_CASE_STUDY.md) - E2E pipeline case study with complex DeFi package

### Testing & Development
1. [**benchmark/TESTING_QUICKSTART.md**](benchmark/TESTING_QUICKSTART.md) - Testing tools reference
2. [**docs/BENCHMARK_GUIDE.md**](docs/BENCHMARK_GUIDE.md) - Comprehensive benchmark guide
3. [**benchmark/docs/TESTING.md**](benchmark/docs/TESTING.md) - Testing methodology (canonical)
4. [**docs/TEST_FIXTURES.md**](docs/TEST_FIXTURES.md) - Test fixture organization and failure cases
5. [**docs/TROUBLESHOOTING.md**](docs/TROUBLESHOOTING.md) - Common issues and fixes

### Architecture & Technical
1. [**benchmark/docs/ARCHITECTURE.md**](benchmark/docs/ARCHITECTURE.md) - System architecture (canonical)
2. [**IMPLEMENTATION_SUMMARY.md**](IMPLEMENTATION_SUMMARY.md) - Implementation decisions and code structure
3. [**docs/A2A_PROTOCOL.md**](docs/A2A_PROTOCOL.md) - HTTP API specification
4. [**docs/SCHEMA.md**](docs/SCHEMA.md) - Result schemas

### Reference & CLI
1. [**docs/CLI_REFERENCE.md**](docs/CLI_REFERENCE.md) - Complete CLI reference
2. [**docs/PTB_SCHEMA.md**](docs/PTB_SCHEMA.md) - PTB JSON schema

---

## 🆕 What's New? (Latest Updates)

### 1. Local Benchmark (`benchmark-local`) - No-Chain Validation ⚡
**Command:** `sui_move_interface_extractor benchmark-local`

**New Capabilities:**
- Validate type inhabitation **without network access**
- Tier A (preflight) and Tier B (VM execution) validation stages
- Deterministic, reproducible results
- Works in air-gapped CI/CD environments

**Quick Start:**
```bash
./target/release/sui_move_interface_extractor benchmark-local \
  --target-corpus /path/to/bytecode_modules \
  --output results.jsonl \
  --restricted-state
```

**Documentation:** [NO_CHAIN_TYPE_INHABITATION_SPEC.md](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md)

---

### 2. E2E One-Package Script ⚡
**File:** `benchmark/scripts/e2e_one_package.py`

**New Capabilities:**
- Complete LLM-driven evaluation pipeline
- Target Package → LLM Helper Generation → Move Build → TX Simulation
- Offline mode (no API key needed for testing)
- Automatic repair loop for build failures

**Quick Start:**
```bash
# Offline test
cd benchmark
uv run python scripts/e2e_one_package.py \
    --corpus-root tests/fake_corpus \
    --package-id 0x1 \
    --out-dir results/my_test

# Real LLM test
export SMI_E2E_REAL_LLM=1
uv run python scripts/e2e_one_package.py \
    --corpus-root ../sui-packages/packages/mainnet_most_used \
    --dataset type_inhabitation_top25 \
    --samples 5 \
    --model google/gemini-3-flash-preview \
    --out-dir results/e2e_run
```

**Documentation:** See inline docstrings in `benchmark/scripts/e2e_one_package.py`

---

### 3. Real-World Test Script ⚡
**File:** `benchmark/run_real_world_test.py`

**New Features:**
- Submit benchmark tasks via HTTP API
- Test multiple models in single run
- Mix fast and slow models
- Get detailed analytics (duration, tokens, errors, hit rate)
- Save results to JSON file

**Quick Start:**
```bash
uv run python3 benchmark/run_real_world_test.py \
    --samples 1 \
    --models gpt-4o,google/gemini-3-flash-preview,google/gemini-2.5-flash-preview \
    --simulation-mode dry-run
```

**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md)

---

### 4. Docker Fluid Usage 🐳

**New Capabilities:**
- Easy start/stop/restart with `docker compose`
- Logs and results persist across restarts
- Real-time log monitoring with `docker logs -f`
- Resource monitoring with `docker stats`

**Quick Commands:**
```bash
# Start container
docker compose up -d smi-bench

# Restart (preserves state)
docker compose restart smi-bench

# Monitor logs
docker logs -f smi-bench-dev

# Check resources
docker stats smi-bench-dev
```

**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md#--docker-fluid-usage)

---

### 5. Multi-Model Testing 📊

**New Capabilities:**
- Test any combination of models
- Compare model performance
- Mix fast/cheap and slow/expensive models
- Automatic analytics aggregation

**Available Models:**
| Model | Speed | Cost | Best For |
|--------|-------|--------|-----------|
| `gpt-4o` | Medium | $$ | Top quality |
| `gpt-4o-mini` | Fast | $ | Quick iteration |
| `google/gemini-3-flash-preview` | Fast | $ | Latest features |
| `google/gemini-2.5-flash-preview` | Very Fast | $ | Quickest results |
| `claude-sonnet-4-5-20250929` | Medium | $$$ | Highest quality |

**Example:**
```bash
uv run python3 benchmark/run_real_world_test.py \
    --samples 2 \
    --models gpt-4o-mini,google/gemini-3-flash-preview,gpt-4o,claude-sonnet-4-5-20250929
```

**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md#--model-comparison-testing)

---

### 6. Fast Iteration Loop 🔄

**New Mode: Dry-Run**
- **Characteristics:** Mock agent, no LLM API calls, ~2 seconds per package
- **Benefits:** Free, fast iteration, debugging
- **When to use:** Initial setup, corpus testing, debugging

**vs Live Mode:**
- **Characteristics:** Real LLM API calls, ~30-120 seconds per package
- **Benefits:** Real results, model comparison
- **When to use:** Production runs, model evaluation

**Quick Switch:**
```bash
# Dry-run (fast, free)
--simulation-mode dry-run

# Live (real, paid)
--simulation-mode live
```

**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md#--dry-run-vs-live-mode)

---

### 7. Detailed Analytics 📈

**New Metrics Collected:**
- ✅ Execution duration (total and per-package)
- ✅ Token usage (prompt + completion)
- ✅ Hit rate (success rate)
- ✅ Error tracking
- ✅ Cost estimation

**Viewing Results:**
```bash
# Summary
cat real_world_test_results_*.json | jq '.summary'

# Detailed metrics
cat benchmark/results/a2a/<run_id>.json | jq '.metrics'

# Event logs
cat benchmark/logs/<run_id>/events.jsonl | jq .
```

**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md#--analytics--results)

---

## 📚 Documentation Hierarchy

```
Root (5 essential files)
├── README.md                    # Project overview, Top 25 dataset, LST case study
├── QUICK_START_GUIDE.md         # All user workflows (start here)
├── PRODUCTION_DEPLOYMENT.md     # Production + offline deployment
├── IMPLEMENTATION_SUMMARY.md    # Architecture decisions
└── AGENTS.md                    # Development guidelines

benchmark/
├── GETTING_STARTED.md           # Benchmark setup (canonical entrypoint)
├── DATASETS.md                  # Dataset creation and usage
├── TESTING_QUICKSTART.md        # Quick testing reference
├── scripts/e2e_one_package.py   # E2E LLM evaluation pipeline
└── docs/
    ├── ARCHITECTURE.md          # Python harness architecture (canonical)
    ├── TESTING.md               # Testing methodology (canonical)
    └── A2A_*.md                 # A2A protocol details

docs/
├── CLI_REFERENCE.md             # All CLI commands and flags
├── METHODOLOGY.md               # Research methodology and scoring
├── NO_CHAIN_TYPE_INHABITATION_SPEC.md  # Tier A/B validation spec
├── TEST_FIXTURES.md             # Test fixture documentation
├── A2A_PROTOCOL.md              # A2A protocol summary
├── SCHEMA.md                    # Output schemas
└── TROUBLESHOOTING.md           # Common issues

specs/
└── LIQUID_STAKING_CASE_STUDY.md # Complex DeFi package case study
```

**Note:** `docs/TESTING.md`, `docs/DATASETS.md`, and `docs/ARCHITECTURE.md` redirect to canonical versions in `benchmark/`.

---

## 🎯 Finding What You Need

### I Want To...

**...get started in 30 seconds:**
→ Read [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md)

**...validate without network (air-gapped/CI):**
→ Read [QUICK_START_GUIDE.md - Local Benchmark](QUICK_START_GUIDE.md#-local-benchmark-no-chain-validation)

**...run the E2E LLM pipeline:**
→ Read [benchmark/GETTING_STARTED.md - E2E One-Package](benchmark/GETTING_STARTED.md#4-e2e-one-package-pipeline)

**...test multiple models:**
→ Read [QUICK_START_GUIDE.md - Model Comparison](QUICK_START_GUIDE.md#-model-comparison-testing)

**...use Docker easily:**
→ Read [QUICK_START_GUIDE.md - Docker Fluid Usage](QUICK_START_GUIDE.md#-docker-fluid-usage)

**...deploy to production (offline):**
→ Read [PRODUCTION_DEPLOYMENT.md - Offline Mode](PRODUCTION_DEPLOYMENT.md#-offline-mode--air-gapped-deployment)

**...understand Tier A/B validation:**
→ Read [docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md](docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md)

**...use test fixtures:**
→ Read [docs/TEST_FIXTURES.md](docs/TEST_FIXTURES.md)

**...run the Top 25 dataset:**
→ Read [README.md - Quick Start: Top 25 Dataset](README.md#-quick-start-top-25-dataset)

**...test a complex DeFi package (LST):**
→ Read [README.md - Case Study: Liquid Staking](README.md#-case-study-liquid-staking-package) or [specs/LIQUID_STAKING_CASE_STUDY.md](specs/LIQUID_STAKING_CASE_STUDY.md)

**...create custom datasets:**
→ Read [benchmark/DATASETS.md](benchmark/DATASETS.md)

**...understand the API:**
→ Read [docs/A2A_PROTOCOL.md](docs/A2A_PROTOCOL.md)

**...run benchmarks manually:**
→ Read [docs/BENCHMARK_GUIDE.md](docs/BENCHMARK_GUIDE.md)

**...use the CLI:**
→ Read [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)

**...fix issues:**
→ Read [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)

**...understand results:**
→ Read [benchmark/TESTING_QUICKSTART.md](benchmark/TESTING_QUICKSTART.md)

---

## 📖 Reading Order Recommendations

### For New Users
1. [README.md](README.md) - Understand the project
2. [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md) - Run your first test (30s)
3. [PRODUCTION_DEPLOYMENT.md](PRODUCTION_DEPLOYMENT.md) - Learn production setup
4. [REAL_WORLD_TEST_GUIDE.md](REAL_WORLD_TEST_GUIDE.md) - Run real-world tests

### For Testing
1. [benchmark/GETTING_STARTED.md](benchmark/GETTING_STARTED.md) - Setup environment
2. [benchmark/TESTING_QUICKSTART.md](benchmark/TESTING_QUICKSTART.md) - Learn testing tools
3. [docs/TESTING.md](docs/TESTING.md) - Understand testing methodology
4. [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) - Fix common issues

### For Development
1. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) - Understand system design
2. [docs/A2A_PROTOCOL.md](docs/A2A_PROTOCOL.md) - API specification
3. [docs/SCHEMA.md](docs/SCHEMA.md) - Result schemas
4. [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md) - CLI commands

---

## 🔍 Common Questions

### Q: What's the fastest way to test?
**A:** Use the new real-world test script:
```bash
uv run python3 benchmark/run_real_world_test.py \
    --samples 1 \
    --models gpt-4o-mini \
    --simulation-mode dry-run
```
[Details](QUICK_START_GUIDE.md)

---

### Q: How do I test multiple models?
**A:** Mix models in the real-world test script:
```bash
uv run python3 benchmark/run_real_world_test.py \
    --models gpt-4o,google/gemini-3-flash-preview,claude-sonnet-4-5-20250929
```
[Details](QUICK_START_GUIDE.md#--model-comparison-testing)

---

### Q: How do I do fast iterations without API costs?
**A:** Use dry-run mode:
```bash
--simulation-mode dry-run
```
[Details](QUICK_START_GUIDE.md#--dry-run-vs-live-mode)

---

### Q: How do I use Docker easily?
**A:** Use standard Docker compose commands:
```bash
docker compose up -d smi-bench       # Start
docker compose restart smi-bench          # Restart
docker logs -f smi-bench-dev          # Logs
```
[Details](QUICK_START_GUIDE.md#--docker-fluid-usage)

---

### Q: How do I view results and analytics?
**A:** Check the results files and event logs:
```bash
# Summary
cat real_world_test_results_*.json | jq '.summary'

# Detailed
cat benchmark/results/a2a/<run_id>.json | jq '.metrics'

# Events
cat benchmark/logs/<run_id>/events.jsonl | jq .
```
[Details](QUICK_START_GUIDE.md#--analytics--results)

---

## 📝 Summary

**New Features Highlighted:**
- ✅ Real-world test script (fast, flexible)
- ✅ Docker fluid usage (easy start/stop/restart)
- ✅ Multi-model testing (compare any models)
- ✅ Fast iteration (dry-run mode)
- ✅ Detailed analytics (duration, tokens, errors, hit rate)

**Documentation Updated:**
- 📖 QUICK_START_GUIDE.md - Main entry point (NEW)
- 📖 README.md - Links to quick start
- 📖 PRODUCTION_DEPLOYMENT.md - Updated with real-world test script
- 📖 REAL_WORLD_TEST_GUIDE.md - Added fast test section
- 📖 DOCUMENTATION_INDEX.md - Complete guide (this file)

**Quick Start:**
```bash
# 30-second quick start
cd /path/to/repo
docker compose up -d smi-bench
cd benchmark
uv run python3 run_real_world_test.py \
    --samples 1 \
    --models gpt-4o-mini \
    --simulation-mode dry-run
```

[**Get Started: QUICK_START_GUIDE.md**](QUICK_START_GUIDE.md) 🚀
