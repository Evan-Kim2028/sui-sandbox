# Changelog

All notable changes to the Sui Move Interface Extractor project will be documented in this file.

## [0.2.3] - 2026-01-10

### Added
- **Move Model 2 Integration**: Upgraded static analysis to use Move Model 2 for high-fidelity transaction simulation without network dependencies.
- **Robust Call Graph Traversal**: Implemented recursive function walking in `smi_tx_sim` to identify created types deep within call stacks.
- **Generic Type Substitution**: Correctly predicts concrete types for generic functions (e.g., `0x2::coin::Coin<T>`) by tracking substitutions across call boundaries.
- **Loop & Recursion Protection**: Added depth-limited traversal (max depth 10) and visited-set tracking to ensure deterministic termination on circular Move logic.
- **Automated Verification Suite**: Introduced comprehensive Rust unit tests, CLI integration tests, and a Python-based E2E smoke test for the static engine.
- **STATIC_ANALYSIS.md**: New documentation detailing engine capabilities and execution modes.

### Changed
- **Enhanced `build-only` Mode**: Significant quality improvement for local benchmarking; `build-only` now accurately predicts most object creations that previously required a live `dry-run`.
- **walkthrough.md Update**: Updated the "Life of a Hit" guide to explain the choice between static analysis and on-chain simulation.

## [0.2.2] - 2026-01-03

### Added
- **Standardized Datasets**: Introduced curated package ID lists for reproducible benchmarking, including `type_inhabitation_top25.txt` for fast iteration and `standard_phase2_benchmark.txt` (292 viable packages).
- **Gemini 3 Flash Support**: Official integration and optimization for Google's `gemini-3-flash-preview` (Dec 2025 release) as the primary introductory model.
- **Python-Native Docker Runner**: A robust replacement for brittle shell scripts, providing guaranteed container cleanup and better lifecycle management.
- **Phase II Targeted Run**: New CLI entry point for running high-signal, signal-only package benchmarks with automatic filtering.
- **Centralized Checkpointing**: Unified `smi_bench.checkpoint` module with checksum validation to ensure data integrity during long benchmark runs.
- **DATASETS.md**: Comprehensive guide for researchers to create, test, and integrate new benchmark subsets.

### Changed
- **Enforced "Fail Fast"**: Refactored benchmark runners to crash immediately on harness bugs (e.g., configuration errors, syntax issues) rather than swallowing exceptions, improving observability.
- **Polished Quickstart**: Updated `scripts/test_local_quickstart.sh` with Gemini 3 Flash defaults and robust environment verification.
- **Code Consolidation**: Eliminated over 100 lines of redundant code by centralizing git utilities, Rust build logic, and type extraction helpers.
- **Enhanced Documentation**: Streamlined `GETTING_STARTED.md` and added detailed schema documentation for Phase II results.

### Fixed
- Resolved multiple issues where broad exception handling led to silent failures in package processing loops.
- Fixed inconsistent git root detection across different modules.
- Repaired truncated checkpoint writes that could lead to corrupted results.

## [0.2.1] - 2026-01-01 (Internal Release)

### Added
- **Recursive Constructor Discovery**: Implemented advanced static analysis to find multi-step paths for creating objects, increasing viable package coverage by 10x (from 27 to 292 packages).
- **Mock Inventory Support**: Added support for resolving `$smi_placeholder` against a simulated inventory in `smi_tx_sim`.
- **Baseline Search Agent**: A deterministic agent that uses heuristics to generate valid Programmable Transaction Blocks (PTBs).

### Changed
- Improved `smi_tx_sim` to support transaction chaining and complex PTB constructs.

## [0.2.0] - 2026-01-01

### Added
- **Initial Phase II Support**: First implementation of the Type Inhabitation benchmark.
- **Rust Transaction Simulator**: Introduced `smi_tx_sim` for dry-running PTBs on-chain.
- **Basic A2A Integration**: Initial support for the Agent-to-Agent protocol for benchmark execution.
