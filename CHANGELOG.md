# Changelog

All notable changes to the Sui Move Interface Extractor project will be documented in this file.

## [0.4.0] - 2026-01-12

### 🎯 Headline Features: MM2 Type Validation & Oracle-Based Evaluation

**Move Model 2 (MM2) Integration for Static Type Validation**
- New `mm2/` module with TypeModel, TypeValidator, and ConstructorGraph components
- Static type checking before VM execution catches type errors earlier
- Phase-based error taxonomy (Resolution → TypeCheck → Synthesis → Execution → Validation)
- Comprehensive error codes: E101-E502 covering all failure modes

**Producer Chains for Return Value Chaining**
- Support for multi-return functions (functions returning multiple values)
- Automatic discovery of producer functions via return type analysis
- Return value chaining: use results from one function as inputs to another
- `ProducerChain` synthesis variant in the benchmark runner

**Type Synthesizer Enhancements**
- SuiSystemState synthesis with 10 validators (avoids division-by-zero errors)
- ValidatorSet and StakingPool synthesis for staking-related functions
- StakedSui synthesis for LST (Liquid Staking Token) packages
- Realistic default values: Coins now synthesized with 1 SUI instead of 0

**Oracle & Evaluation System for Benchmark Scoring**
- New `inhabit/oracle.py`: Computes theoretical maximum scores (ceiling) for packages
- New `inhabit/evaluator.py`: Parses MM2 results and scores LLM output
- Difficulty ranking: Functions ranked by parameter complexity and execution success
- Normalized scores (0-100%) comparable across packages and LLM runs

**Prompt Template System**
- `--prompt-file` flag for externalized, customizable prompts
- Template variables: `{{PACKAGE_ID}}`, `{{INTERFACE_SUMMARY}}`, `{{MAX_ATTEMPTS}}`, etc.
- Three templates: `type_inhabitation.txt`, `type_inhabitation_detailed.txt`, `repair_build_error.txt`

### Added

**Rust MM2 Components (`src/benchmark/mm2/`)**
- `model.rs`: TypeModel wrapper around MM2's Model for bytecode analysis
- `type_validator.rs`: Static type checking for function calls and constructor chains
- `constructor_graph.rs`: Graph-based constructor discovery with BFS traversal
- `type_synthesizer.rs`: BCS byte generation for 40+ framework types

**Python Evaluation Modules (`benchmark/src/smi_bench/inhabit/`)**
- `oracle.py`: PackageOracle for ceiling computation, FunctionDifficulty ranking
- `evaluator.py`: EvaluationResult generation from benchmark artifacts
- `evaluation.py`: Type definitions for Phase, ErrorCode, ScoringCriteria

**Testing**
- `tests/mm2_integration_test.rs`: 7 integration tests for MM2 components
- `tests/test_inhabit_oracle.py`: 22 tests for oracle functionality
- `tests/test_inhabit_evaluator.py`: 26 tests for evaluator logic
- `tests/test_prompt_templates.py`: 18 tests for template rendering

### Changed

**Error Handling Improvements**
- Added logging to 15+ silent exception handlers in Python files
- Compile-time assertion ensures DEFAULT_VALIDATOR_COUNT > 0
- Safe identifier creation with LazyLock fallbacks (no panic risk)

**Type Synthesis Defaults**
- `Coin<T>` now synthesized with 1 SUI (1_000_000_000 MIST) balance
- `TreasuryCap<T>` now synthesized with 1000 SUI total supply
- Prevents zero-balance failures in balance checks

### Technical Details

**New Error Taxonomy Phases**
- Resolution (1xx): Module/function existence validation
- TypeCheck (2xx): Static type compatibility, ability constraints
- Synthesis (3xx): Constructor chain discovery, BCS serialization
- Execution (4xx): VM execution, constructor/target aborts
- Validation (5xx): Target access verification, return type matching

**Oracle Metrics**
- `execution_ceiling`: Maximum tier_b_hit rate achievable for package
- `synthesis_ceiling`: Maximum tier_a_hit rate achievable
- `difficulty_distribution`: Function counts by difficulty level (easy/medium/hard/impossible)

### Fixed
- ARITHMETIC_ERROR in ValidatorSet with 0 validators (now always 10)
- Silent exception handling that masked debugging information
- Potential panic in Identifier::new() fallback chains

---

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
