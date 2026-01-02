# Documentation Audit and Categorization

This document categorizes all markdown documentation files by audience and identifies planned items that should be executed.

## Documentation Categorization

### User-Facing Documentation

**Primary audience:** End users, integrators, researchers using the tool

#### Root Level
- `README.md` - Main project entry point with **Documentation Map** ✅
- `AGENTS.md` - Agent development guidelines ✅

#### `docs/` (Extraction & Reference)
- `docs/METHODOLOGY.md` - Unified extraction and benchmark methodology ✅
- `docs/SCHEMA.md` - JSON schema reference ✅
- `docs/RUNBOOK.md` - Reproducible commands guide ✅
- `docs/TROUBLESHOOTING.md` - Common issues and fixes ✅
- `docs/AGENTBEATS.md` - AgentBeats integration guide ✅
- `docs/DATASET_SNAPSHOTS.md` - Dataset versioning guide ✅

#### `benchmark/` (Phase II Benchmark)
- `benchmark/README.md` - Benchmark entry point ✅
- `benchmark/GETTING_STARTED.md` - Unified Phase II quickstart guide ✅
- `benchmark/docs/A2A_EXAMPLES.md` - Concrete A2A usage examples ✅
- `benchmark/docs/A2A_COMPLIANCE.md` - Unified protocol compliance and testing reference ✅

#### Directory READMEs
- `benchmark/manifests/README.md` - Manifest file guide ✅
- `results/README.md` - Results directory policy ✅

### Internal/Developer Documentation

**Primary audience:** Maintainers, contributors

- `benchmark/docs/ARCHITECTURE.md` - Code architecture reference ✅
- `benchmark/docs/TESTING.md` - Documentation testing standards ✅
- `benchmark/tests/README.md` - Test suite documentation ✅

---

## Consolidation Summary

The following consolidations have been performed to reduce "floating documents" and improve discoverability:

1.  **A2A Strategy Consolidation**: Merged `A2A_TESTING_STRATEGY.md` into `A2A_COMPLIANCE.md`.
2.  **Methodology Consolidation**: Merged `benchmark/docs/METHODOLOGY.md` into `docs/METHODOLOGY.md`.
3.  **Quickstart Consolidation**: Merged `QUICKSTART.md`, `A2A_GETTING_STARTED.md`, and `OPENROUTER_QUICKSTART.md` into a single `benchmark/GETTING_STARTED.md`.
4.  **Examples Consolidation**: Merged `docs/A2A_EXCHANGES.md` into `benchmark/docs/A2A_EXAMPLES.md`.
5.  **Main Entry Point**: Added a **Documentation Map** to the root `README.md` for high-level navigation.

---

## Action Items

### High Priority
- None (All high-priority items implemented)

### Medium Priority
1. **Add performance tests** - Validate streaming latency and cancellation responsiveness
2. **Enhance streaming event tests** - Ensure comprehensive coverage

### Low Priority / Future
1. **Fuzzing** - JSON-RPC request fuzzing
2. **Compatibility tests** - Multi-version A2A testing
3. **Contract testing** - Consumer-driven contracts (Pact)
