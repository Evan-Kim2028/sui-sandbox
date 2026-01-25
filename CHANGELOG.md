# Changelog

All notable changes to the Sui Move Interface Extractor project will be documented in this file.

## [0.9.0] - 2026-01-25

### Breaking Changes

#### Crate Architecture Refactor

- **`sui-data-fetcher` crate removed**: Split into specialized crates:
  - `sui-transport`: GraphQL/gRPC transport layer
  - `sui-resolver`: Address normalization, package linkage, upgrade resolution
  - `sui-prefetch`: Ground-truth prefetching and MM2 predictive prefetch
  - `sui-state-fetcher`: State provider abstraction with VM integration

#### PTB API Changes

- **`ObjectInput` enum now requires `version` field**: All variants (`ImmRef`, `MutRef`, `Owned`, `Shared`, `Receiving`) now have a `version: Option<u64>` field for version tracking
- **`TypedReturnValue` replaces `Vec<u8>`**: `ExecutionOutput.return_values` is now `Vec<TypedReturnValue>` with type information
- **`mutable_ref_outputs` tuple extended**: Now `Vec<(u8, Vec<u8>, Option<TypeTag>)>` (was 2-tuple)
- **`ReplayResult` has new fields**: `objects_tracked`, `lamport_timestamp`, `version_summary`

### Added

#### Accurate Gas Metering System

New gas metering module with Sui-compatible implementation:
- `AccurateGasMeter` with per-instruction costs
- Storage tracking for read/write/delete charges
- Native function gas costs
- Protocol-version-aware cost tables
- Computation bucketing matching Sui's gas model
- `GasSummary` builder for detailed cost breakdown
- Gas metering enabled by default (use `without_gas_metering()` to disable)

#### PTB Execution Improvements

All 9 edge cases from FEASIBILITY_PLAN.md have been fixed:
- **SplitCoins/MergeCoins Result argument handling**: Balance mutations now properly propagate to Result/NestedResult arguments
- **Version tracking system**: `TrackedObject` with version, `is_modified`, owner, digest fields
- **Structured abort info**: `StructuredAbortInfo` captures abort codes directly from VMError
- **Receiving objects support**: New `ObjectInput::Receiving` variant with `parent_id` tracking
- **DryRunResult**: Per-command validation and estimated gas
- **PTBTraceSummary**: Timing statistics (total/avg/max duration)

#### MM2 Bytecode Analysis (Predictive Prefetch)

Predictive prefetch system for dynamic field access:
- `bytecode_analyzer.rs` - Walks bytecode to find dynamic_field calls
- `field_access_predictor.rs` - Predicts dynamic field accesses with type resolution
- `call_graph.rs` - Build call graphs for inter-module analysis
- `key_synthesizer.rs` - Generate synthetic keys for prefetch
- `predictive_prefetch.rs` - Integration layer for prefetch predictions
- `eager_prefetch.rs` - Enhanced prefetching with MM2 analysis

#### CLI Bridge Enhancements

- **`bridge info` subcommand**: Comprehensive transition workflow guide
  - Shows deployment steps, environment checks, network options
  - `--verbose` flag adds protocol version and error handling tips
  - JSON output support for tooling integration

#### Test Infrastructure

- New `tests/common/` module with shared test infrastructure
- `ptb_tests.rs`: PTB-specific tests
- `bug_fix_verification_tests.rs`: Regression tests

#### Examples

- New `PrefetchStrategy` enum: GroundTruth, MM2Predictive, LegacyGraphQL
- New `deepbook_orders.rs`, `ptb_basics.rs`, `version_tracking_test.rs`
- Simplified `examples/common/mod.rs` to re-export from workspace crates

### Changed

- Examples now use workspace crate APIs directly rather than duplicating utility code
- Documentation consolidated and simplified (~4,300 lines removed, ~500 lines of focused docs added)
- Outdated documentation archived to `docs/archive/`

### Fixed

- Fixed `Identifier::new()` type conversion in tx_replay.rs
- Fixed clippy warnings (unused variables, field assignment patterns)
- Fixed test compilation errors for new API types

### Removed

- `coin_transfer.rs` example (superseded by `ptb_basics.rs`)
- Outdated documentation: LLM integration guide, local bytecode sandbox guide, migration guides
- DeFi case studies moved to `docs/archive/`

## [0.8.0] - 2026-01-23

### Breaking Changes

- **Removed `pyo3-bindings` crate**: Experimental Python bindings were never released publicly and have been removed to simplify the codebase
- **Removed `object_patcher.rs`**: Functionality consolidated into `sui_sandbox_core::utilities::generic_patcher`
- **Moved type utilities from examples to core**: `parse_type_tag_simple`, `extract_package_ids_from_type`, and `extract_dependencies_from_bytecode` moved from `examples/common` to `sui_sandbox_core::utilities`
- **Removed deprecated tests**: Removed obsolete integration tests (execute_*, benchmark_*, debug_*, state_persistence_*)
- **Removed deprecated examples**: `inspect_df.rs` (superseded by fork_state), `kriya_swap.rs` (outdated)

### Added

#### New Utilities Architecture

**`sui_sandbox_core::utilities`** - Infrastructure workaround utilities:

- `address.rs`: `normalize_address()`, `is_framework_package()` for address handling
- `generic_patcher.rs`: `GenericObjectPatcher` for BCS object patching with version-lock workarounds
- `type_utils.rs`: `parse_type_tag()`, `extract_package_ids_from_type()`, `extract_dependencies_from_bytecode()` for type/bytecode analysis
- `version_utils.rs`: `detect_version_constants()` for bytecode version detection

**`sui_prefetch::utilities`** - gRPC data helper utilities:

- `create_grpc_client()`: Initialize gRPC client with API key
- `collect_historical_versions()`: Aggregate object versions from gRPC transaction response

#### CLI Tooling

- **`sui-sandbox` CLI binary** with comprehensive subcommands:
  - `fetch` - Fetch transaction data from gRPC endpoints
  - `replay` - Replay historical transactions in local sandbox
  - `run` - Execute Move functions against forked state
  - `publish` - Deploy Move packages to sandbox environment
  - `view` - Inspect sandbox state, objects, and dynamic fields
  - `state` - Manage persistent sandbox state (save/load/clear)

#### New Examples

- `fork_state.rs` - Demonstrates forking on-chain state for local testing
- `coin_transfer.rs` - Simple SUI coin transfer example
- `multi_swap_flash_loan.rs` - Complex multi-hop DeFi flash loan example
- `cli_workflow.sh` - Shell script demonstrating CLI usage patterns

### Changed

- **`examples/common/mod.rs`** now contains only application glue code between crates
  - Re-exports utilities from `sui_sandbox_core::utilities` and `sui_prefetch::utilities`
  - Provides bridge functions: `build_generic_patcher()`, `build_resolver_from_packages()`, `create_child_fetcher()`, `create_vm_harness()`, `register_input_objects()`
- **Clear architectural separation**:
  - `sui_sandbox_core::utilities` - Infrastructure workarounds (patching, normalization, bytecode analysis)
  - `sui_prefetch::utilities` - Data helpers (gRPC client setup, version aggregation)

### Migration Guide

If you were importing utilities from `examples/common`, update your imports:

```rust
// Before
use common::{parse_type_tag_simple, extract_package_ids_from_type, normalize_address};

// After
use sui_sandbox_core::utilities::{parse_type_tag, extract_package_ids_from_type, normalize_address};
```

For backwards compatibility, `examples/common` still re-exports these with `parse_type_tag_simple` as an alias for `parse_type_tag`.

## [0.7.1] - 2026-01-22

### Added

- **Shared Example Helpers Module**: Extracted common helper functions used across examples into `examples/common/mod.rs`, reducing code duplication by ~800 lines
  - `parse_type_tag_simple()` - Parse Move type strings into TypeTag
  - `split_type_params()` - Split generic type parameters
  - `extract_package_ids_from_type()` - Extract package addresses from type strings
  - `normalize_address()` - Normalize hex addresses with proper padding
  - `extract_dependencies_from_bytecode()` - Extract module dependencies from compiled bytecode
  - `is_framework_package()` - Check if a package is a Sui framework package (0x1, 0x2, 0x3)

- **Scallop Deposit Example**: Added `scallop_deposit.rs` example demonstrating lending protocol transaction replay with ObjectPatcher integration

### Fixed

- **Package Version Collision**: Fixed issue where upgraded package bytecode could be overwritten by original package when both have the same bytecode self_id
  - Packages referenced in MoveCall commands now load first
  - Bytecode addresses are tracked to prevent duplicate loading

### Known Issues

- **Struct Layout Compatibility**: Some historical transactions may fail with `FAILED_TO_DESERIALIZE_ARGUMENT` if the protocol changed struct layouts between package upgrades. This is a fundamental limitation of BCS deserialization where historical object data must match the bytecode's expected struct layout.

## [Unreleased]

### Breaking Changes

**JSON-RPC Removed from DataFetcher**

- `DataFetcher` now uses GraphQL exclusively (JSON-RPC backend removed)
- Removed `with_fallback()` and `with_prefer_graphql()` methods (no longer needed)
- `DataFetcher::new()` now takes only a GraphQL endpoint (previously took JSON-RPC + GraphQL)
- Removed `DataSource::JsonRpc` enum variant
- Removed `json_rpc()` accessor method

**GrpcFetcher Renamed to NetworkFetcher**

- `GrpcFetcher` is now `NetworkFetcher` (type alias kept for backwards compatibility)
- `NetworkFetcher` uses `DataFetcher` internally (GraphQL-based)
- Removed `inner()` method that returned `TransactionFetcher`

### Added

**New DataFetcher Helper Methods**

- `DataFetcher::extract_package_ids(tx)` - Extract all package IDs from a transaction's MoveCall commands
- `DataFetcher::fetch_transaction_inputs(tx)` - Fetch all input objects for a transaction
- `DataFetcher::fetch_transaction_packages(tx)` - Fetch all packages referenced in a transaction

### Deprecated

- `TransactionFetcher` is now deprecated. Use `DataFetcher` with GraphQL instead.
  - Sui is deprecating JSON-RPC in April 2026
  - GraphQL provides equivalent functionality with better data
  - `TransactionFetcher` will be removed in v0.7.0

- `GrpcFetcher` type alias is deprecated. Use `NetworkFetcher` instead.

### Migration Guide

To migrate from `TransactionFetcher` to `DataFetcher`:

```rust
// Before (JSON-RPC)
let fetcher = TransactionFetcher::mainnet();
let tx = fetcher.fetch_transaction_sync(digest)?;
let modules = fetcher.fetch_package_modules(pkg_id)?;

// After (GraphQL)
let fetcher = DataFetcher::mainnet();
let tx = fetcher.fetch_transaction(digest)?;
let pkg = fetcher.fetch_package(pkg_id)?;
let modules: Vec<(String, Vec<u8>)> = pkg.modules.into_iter()
    .map(|m| (m.name, m.bytecode)).collect();
```

### Removed

**Docker and A2A Protocol Support**

- Removed Docker infrastructure (Dockerfile, docker-compose.yml, docker-compose.ci.yml)
- Removed A2A (Agent-to-Agent) protocol modules and tests
- Removed `agentbeats` dependency
- Removed Docker-related scripts (`docker_quickstart.sh`, `run_docker_benchmark.sh`, `test_docker_a2a.sh`)

**Migration from JSON-RPC to gRPC**

- Deprecated JSON-RPC for transaction replay in favor of gRPC
- `tx_replay.rs` now uses `GrpcClient` exclusively for fetching transactions and object versions
- Removed `sui-sdk`, `sui-types`, `sui-json-rpc-types` dependencies

### Changed

- Updated documentation to remove Docker/A2A references
- Simplified benchmark runner to focus on local execution

---

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
