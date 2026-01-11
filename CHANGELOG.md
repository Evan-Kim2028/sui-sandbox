# Changelog

All notable changes to the Sui Move Interface Extractor project will be documented in this file.

## [0.3.0] - 2026-01-11

### 🎯 Headline Features: Comprehensive VM Execution Support

**Full Dynamic Field Support via VM Extensions**
- Implemented complete dynamic field support via `ObjectRuntime` VM extension
- All 7 dynamic_field operations now work: `add_child_object`, `borrow_child_object`, `borrow_child_object_mut`, `remove_child_object`, `has_child_object`, `has_child_object_with_ty`, `hash_type_and_key`
- Objects stored with proper reference semantics via `GlobalValue`
- Functions using dynamic fields for state management can now execute successfully

**Constructor Chaining & Complex Type Synthesis**
- Automatic discovery of multi-step constructor paths for nested struct instantiation
- Generic type instantiation with default type arguments (u64)
- Return value capture and synthesis for constructor chaining
- OTW (One-Time Witness) pattern detection and automatic handling

**Enhanced Module Loading & Execution Tracing**
- Sui framework bytecode (move-stdlib, sui-framework, sui-system) bundled at compile time
- **Dual-level execution tracing**: Module-level (via ExecutionTrace) and function-level (via static bytecode analysis)
- Target package verification: proof that target code was actually executed, not just framework code
- Static bytecode analysis for function call tracing without runtime instrumentation
- Support for calling functions across package boundaries
- Module export tracking with detailed execution reports in `target_modules_accessed` field

**Structured Failure Stage Categorization & Progressive Error Disclosure**
- Comprehensive documentation for all failure stages with descriptions
- **Tier A (Argument Synthesis)**: A1 (target validation), A2 (layout resolution), A3 (value synthesis), A4 (reserved), A5 (type parameter bounds)
- **Tier B (Execution)**: B1 (VM setup/constructor), B2 (execution abort)
- Self-describing error messages with stage context and explanations
- Progressive disclosure: error codes (e.g., 1000) automatically expand to human-readable explanations
- Unsupported native detection with automatic listing of blocked operations (crypto, randomness, zklogin)
- Clear distinction between compilation vs execution failures for better debugging

### Added

**Infrastructure & Native Functions**
- **Framework Bytecode Bundled**: 192KB of Sui framework bytecode (move-stdlib, sui-framework, sui-system, bridge, deepbook) included at compile time via `include_bytes!`
- **Native Function Implementations**: 1000+ lines of native implementations across categories A (real), B (safe mocks), and C (abort stubs)
- **Synthetic System Parameters**: Automatic generation of TxContext, Clock, and other Sui system types
- **BCS Roundtrip Validation**: Ensures all synthesized arguments can be serialized and deserialized correctly
- **Target Package Validation**: Pre-execution validation that functions exist, are public, and parameters are synthesizable
- **Centralized Error Module**: New `errors.rs` with error codes, native support info, and self-describing error messages

**Error Handling & Documentation**
- **Documented Failure Stages**: Comprehensive docs for A1-A5 (argument synthesis) and B1-B2 (execution) failure stages with descriptions
- **Unsupported Native Detection**: Clear error messages when functions use unsupported natives (crypto, randomness, zklogin)
- **Self-Describing Errors**: Error messages include context and don't require external documentation

**Python Benchmark Enhancements**
- **E2E Pipeline Documentation**: Added 1120+ lines of improvements to `e2e_one_package.py` with comprehensive inline docs
- **Enhanced Diagnostics**: `doctor.py` now has 492+ lines of diagnostic and repair capabilities
- **Shared Test Infrastructure**: New `conftest.py` with pytest fixtures for Docker port management
- **Security Regression Tests**: New test suite for security validation

### Changed

**Documentation Consolidation**
- Removed 1575 lines of outdated/duplicate documentation
- Added 784 lines of focused, well-organized documentation
- Moved content to `benchmark/` directory for better organization
- Added `docs/CLI_REFERENCE.md` with comprehensive CLI documentation
- Major refactor of README.md and QUICK_START_GUIDE.md
- Added `docs/TYPE_INHABITATION_EVALUATION.md` describing the evaluation framework

**Test Infrastructure**
- 482 tests passing (99.0% pass rate)
- 5 environment-specific tests marked as xfail (Docker/subprocess issues)
- Enhanced test coverage across all components

### Technical Details

**Supported Natives (Category A & B)**
- Real implementations: vector::*, bcs::*, hash::*, string::*, type_name::*, debug::*, signer::*
- Safe mocks: tx_context::*, object::*, transfer::*, event::*, types::*
- Full support via VM extension: dynamic_field::* (all operations)

**Unsupported Natives (Category C)**
- Crypto verification: bls12381::*, ecdsa_*::*, ed25519::*, groth16::*
- Randomness/ZK: random::*, zklogin::*, poseidon::*
- Config: config::*, nitro_attestation::*

### Fixed
- Resolved module-level documentation duplication across multiple files
- Fixed code formatting and clippy warnings
- Corrected outdated documentation references to dynamic fields

### Breaking Changes
None - this release is fully backward compatible with 0.2.x

---

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
