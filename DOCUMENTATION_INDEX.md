# Documentation Index

**Complete guide to all documentation for Sui Move Interface Extractor Benchmark.**

---

## 🚀 Quick Navigation

### New Users (Start Here)
1. [**QUICK_START_GUIDE.md**](QUICK_START_GUIDE.md) - ⚡ 30-second quick start (NEW)
   - Fastest way to test with Docker + HTTP API
   - Real-world test script
   - Model comparison testing
   - Docker fluid usage

2. [**README.md**](README.md) - Project overview and setup

3. [**benchmark/GETTING_STARTED.md**](benchmark/GETTING_STARTED.md) - Environment setup guide

### Production Users
1. [**PRODUCTION_DEPLOYMENT.md**](PRODUCTION_DEPLOYMENT.md) - Complete production deployment
2. [**REAL_WORLD_TEST_GUIDE.md**](REAL_WORLD_TEST_GUIDE.md) - Real-world testing with mainnet packages
3. [**QUICK_START_GUIDE.md**](QUICK_START_GUIDE.md) - New features (models, Docker, fast runs)

### Testing & Development
1. [**benchmark/TESTING_QUICKSTART.md**](benchmark/TESTING_QUICKSTART.md) - Testing tools reference
2. [**docs/BENCHMARK_GUIDE.md**](docs/BENCHMARK_GUIDE.md) - Comprehensive benchmark guide
3. [**docs/TESTING.md**](docs/TESTING.md) - Testing methodology
4. [**docs/TROUBLESHOOTING.md**](docs/TROUBLESHOOTING.md) - Common issues and fixes

### Architecture & Technical
1. [**docs/ARCHITECTURE.md**](docs/ARCHITECTURE.md) - System architecture
2. [**docs/STATIC_ANALYSIS.md**](docs/STATIC_ANALYSIS.md) - High-fidelity static engine (NEW)
3. [**docs/A2A_PROTOCOL.md**](docs/A2A_PROTOCOL.md) - HTTP API specification
4. [**docs/SCHEMA.md**](docs/SCHEMA.md) - Result schemas

### Reference & CLI
1. [**docs/CLI_REFERENCE.md**](docs/CLI_REFERENCE.md) - Complete CLI reference
2. [**docs/PTB_SCHEMA.md**](docs/PTB_SCHEMA.md) - PTB JSON schema

---

## 🆕 What's New? (Latest Updates)

### 1. Real-World Test Script ⚡
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

### 2. Docker Fluid Usage 🐳

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

### 3. Multi-Model Testing 📊

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

### 4. Fast Iteration Loop 🔄

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

### 5. Detailed Analytics 📈
...
**Documentation:** [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md#--analytics--results)

---

### 6. High-Fidelity Static Engine 🧠
**File:** `src/bin/smi_tx_sim.rs`

**New Capabilities:**
- Full call-graph traversal (depth 10)
- Generic type substitution (e.g., `Coin<T>` resolution)
- Infinite loop prevention
- Offline benchmarking without mainnet gas

**Documentation:** [docs/STATIC_ANALYSIS.md](docs/STATIC_ANALYSIS.md)

---

## 📚 Documentation Hierarchy

```
DOCUMENTATION_INDEX.md (this file)
│
├── QUICK_START_GUIDE.md (NEW - Start here)
│   ├── Docker fluid usage
│   ├── Real-world test script
│   ├── Model comparison
│   ├── Fast iteration (dry-run)
│   └── Analytics & results
│
├── README.md
│   ├── Project overview
│   ├── Getting started
│   └── Top-25 dataset
│
├── PRODUCTION_DEPLOYMENT.md
│   ├── Corpus setup
│   ├── Docker mounting
│   ├── Service startup
│   └── Running tests
│
├── REAL_WORLD_TEST_GUIDE.md
│   ├── Traditional API testing
│   ├── Direct API calls
│   └── Example requests
│
├── benchmark/
│   ├── GETTING_STARTED.md
│   ├── TESTING_QUICKSTART.md
│   └── README.md
│
└── docs/
    ├── ARCHITECTURE.md
    ├── A2A_PROTOCOL.md
    ├── BENCHMARK_GUIDE.md
    ├── CLI_REFERENCE.md
    ├── SCHEMA.md
    ├── TESTING.md
    └── TROUBLESHOOTING.md
```

---

## 🎯 Finding What You Need

### I Want To...

**...get started in 30 seconds:**
→ Read [QUICK_START_GUIDE.md](QUICK_START_GUIDE.md)

**...test multiple models:**
→ Read [QUICK_START_GUIDE.md - Model Comparison](QUICK_START_GUIDE.md#--model-comparison-testing)

**...use Docker easily:**
→ Read [QUICK_START_GUIDE.md - Docker Fluid Usage](QUICK_START_GUIDE.md#--docker-fluid-usage)

**...do fast iterations:**
→ Read [QUICK_START_GUIDE.md - Dry-Run vs Live Mode](QUICK_START_GUIDE.md#--dry-run-vs-live-mode)

**...deploy to production:**
→ Read [PRODUCTION_DEPLOYMENT.md](PRODUCTION_DEPLOYMENT.md)

**...run real-world tests:**
→ Read [REAL_WORLD_TEST_GUIDE.md](REAL_WORLD_TEST_GUIDE.md)

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
