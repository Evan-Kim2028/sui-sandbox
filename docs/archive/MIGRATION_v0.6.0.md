# Migration Guide: v0.5.0 → v0.6.0

This document describes all deprecated APIs in v0.5.0 that will be removed in v0.6.0, along with migration paths and code examples.

## Overview

v0.6.0 focuses on API consolidation:
- **Single LLM API**: `SandboxRequest` replaces `ToolCall`/`LlmToolkit`
- **Unified execution**: PTB mode becomes the only execution path
- **Simplified CLI**: Legacy compatibility flags removed
- **Modern type analysis**: MM2-based analysis becomes the only option

### Removal Summary

| Deprecated Item | Location | Replacement |
|-----------------|----------|-------------|
| `LlmToolkit` struct | `benchmark::llm_tools` | `SandboxRequest` |
| `ToolCall` enum | `benchmark::llm_tools` | `SandboxRequest` |
| `tool_schema()` | `LlmToolkit` | `{"action": "list_available_tools"}` |
| `--no-mm2` flag | CLI | Remove (MM2 is default) |
| `--no-phase-errors` flag | CLI | Remove (phase errors are default) |
| `--no-ptb` flag | CLI | Remove (PTB is default) |
| `execute_constructor_chain_as_ptb()` | `benchmark::runner` | `execute_constructor_chain_as_ptb_via_sim()` |
| `execute_ptb_with_locking()` | `SimulationEnvironment` | `execute_ptb()` |
| `mark_expected()` | `BenchmarkError` | `set_source(ErrorSource::InfrastructureLimitation)` |

---

## 1. LLM API Migration

### Overview

The legacy `ToolCall` enum and `LlmToolkit` struct are replaced by `SandboxRequest`, which provides:
- Single entry point via `execute_request()`
- JSON-based request/response format
- Stateful execution through `SimulationEnvironment`
- Self-documenting via `list_available_tools`

### Before (Deprecated)

```rust
use sui_move_interface_extractor::benchmark::llm_tools::{LlmToolkit, ToolCall};

// Create toolkit
let mut toolkit = LlmToolkit::new(bytecode_path)?;

// List modules
let result = toolkit.execute(ToolCall::ListModules)?;

// Get struct info
let result = toolkit.execute(ToolCall::GetStructInfo {
    module_path: "0x2::coin".to_string(),
    struct_name: "Coin".to_string(),
})?;

// Create object
let result = toolkit.execute(ToolCall::CreateObject {
    type_path: "0x2::coin::Coin<0x2::sui::SUI>".to_string(),
    fields: serde_json::json!({"balance": {"value": 1000}}),
    is_shared: false,
})?;

// Get tool schema for LLM
let schema = LlmToolkit::tool_schema();
```

### After (v0.6.0)

```rust
use sui_move_interface_extractor::benchmark::sandbox_exec::{SandboxRequest, execute_request};
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;

// Create environment
let mut env = SimulationEnvironment::new_with_framework()?;

// List modules (after loading)
let request: SandboxRequest = serde_json::from_value(serde_json::json!({
    "action": "list_modules"
}))?;
let result = execute_request(&mut env, request)?;

// Get struct info
let request: SandboxRequest = serde_json::from_value(serde_json::json!({
    "action": "get_struct_info",
    "module_path": "0x2::coin",
    "struct_name": "Coin"
}))?;
let result = execute_request(&mut env, request)?;

// Create object
let request: SandboxRequest = serde_json::from_value(serde_json::json!({
    "action": "create_object",
    "object_type": "0x2::coin::Coin<0x2::sui::SUI>",
    "fields": {"balance": {"value": 1000}}
}))?;
let result = execute_request(&mut env, request)?;

// Get tool schema for LLM
let request: SandboxRequest = serde_json::from_value(serde_json::json!({
    "action": "list_available_tools"
}))?;
let schema = execute_request(&mut env, request)?;
```

### JSON API (CLI / External Integration)

The `sandbox-exec --interactive` command accepts JSON on stdin:

```bash
# List modules
echo '{"action": "list_modules"}' | sandbox-exec --interactive

# Get struct info
echo '{"action": "get_struct_info", "module_path": "0x2::coin", "struct_name": "Coin"}' | sandbox-exec --interactive

# Create object
echo '{"action": "create_object", "object_type": "0x2::coin::Coin<0x2::sui::SUI>", "fields": {"balance": {"value": 1000}}}' | sandbox-exec --interactive
```

### Complete ToolCall → SandboxRequest Mapping

| ToolCall Variant | SandboxRequest Action | Notes |
|------------------|----------------------|-------|
| `ListModules` | `list_modules` | |
| `ListStructs { module_path }` | `list_structs` | |
| `GetStructInfo { module_path, struct_name }` | `get_struct_info` | |
| `ListFunctions { module_path }` | `list_functions` | |
| `GetFunctionInfo { module_path, function_name }` | `get_function_info` | |
| `ModuleSummary { module_path }` | `module_summary` | |
| `DisassembleFunction { module_path, function_name }` | `disassemble_function` | |
| `DisassembleModule { module_path }` | `disassemble_module` | |
| `CreateObject { type_path, fields, is_shared }` | `create_object` | `type_path` → `object_type` |
| `ParseError { error }` | `parse_error` | |
| `CompileSource { package_name, module_name, source }` | `compile_move` | |
| `IsFrameworkCached` | `is_framework_cached` | |
| `EnsureFrameworkCached` | `ensure_framework_cached` | |
| `GenerateFreshId` | `generate_id` | |
| `ValidateType { type_str }` | `validate_type` | |
| `EncodeBcs { type_str, value }` | `encode_bcs` | |
| `DecodeBcs { type_str, bytes_hex }` | `decode_bcs` | |
| `ParseAddress { address }` | `parse_address` | |
| `FormatAddress { address, format }` | `format_address` | |

### Additional SandboxRequest Actions (Not in ToolCall)

These actions are only available in the new API:

| Action | Description |
|--------|-------------|
| `load_module` | Load bytecode into environment |
| `execute_ptb` | Execute a Programmable Transaction Block |
| `call_function` | Direct function call |
| `inspect_object` | Get object details by ID |
| `list_objects` | List all objects in environment |
| `list_shared_objects` | List shared objects |
| `get_clock` | Get current clock timestamp |
| `set_clock` | Set clock for testing |
| `find_constructors` | Find constructor functions for a type |
| `search_types` | Search types by pattern |
| `search_functions` | Search functions by pattern |
| `register_coin` | Register a coin type |
| `get_coin_metadata` | Get coin metadata |
| `list_coins` | List registered coins |
| `list_available_tools` | Get full API schema |

---

## 2. CLI Flag Migration

### `--no-mm2` (Legacy Bytecode Analyzer)

**Status**: Deprecated in v0.5.0, removed in v0.6.0

The legacy bytecode analyzer used direct bytecode parsing. MM2 (Move Model v2) provides more accurate type analysis.

**Before**:
```bash
# Use legacy analyzer
sui-move-interface-extractor benchmark --no-mm2 corpus/

# Force MM2 (redundant, it's default)
sui-move-interface-extractor benchmark --use-mm2 corpus/
```

**After (v0.6.0)**:
```bash
# MM2 is the only option, no flag needed
sui-move-interface-extractor benchmark corpus/
```

**Migration**: Simply remove `--no-mm2` from your commands. If you have scripts using `--use-mm2`, that flag will also be removed (it's a no-op since MM2 is default).

---

### `--no-phase-errors` (Legacy Error Taxonomy)

**Status**: Deprecated in v0.5.0, removed in v0.6.0

The legacy error taxonomy used stages A1-A5 and B1-B2. The new phase-based taxonomy uses semantic phases: Resolution, TypeCheck, Synthesis, Execution, Validation.

**Before**:
```bash
# Use legacy A1-A5/B1-B2 error codes
sui-move-interface-extractor benchmark --no-phase-errors corpus/
```

**After (v0.6.0)**:
```bash
# Phase-based errors are the only option
sui-move-interface-extractor benchmark corpus/
```

**Error Code Mapping**:

| Legacy Stage | Phase | Error Codes |
|--------------|-------|-------------|
| A1 (Module Load) | Resolution | E101-E199 |
| A2 (Type Parse) | Resolution | E101-E199 |
| A3 (Type Check) | TypeCheck | E201-E299 |
| A4 (Synthesis) | Synthesis | E301-E399 |
| A5 (Execution) | Execution | E401-E499 |
| B1 (Return Check) | Validation | E501-E599 |
| B2 (State Check) | Validation | E501-E599 |

---

### `--no-ptb` (Legacy VMHarness Execution)

**Status**: Deprecated in v0.5.0, removed in v0.6.0

The legacy VMHarness path provided execution tracing but had inconsistent semantics. PTB (Programmable Transaction Block) execution through `SimulationEnvironment` is now the only path.

**Before**:
```bash
# Use legacy VMHarness for execution tracing
sui-move-interface-extractor benchmark --no-ptb corpus/
```

**After (v0.6.0)**:
```bash
# PTB execution is the only option
sui-move-interface-extractor benchmark corpus/
```

**Migration Notes**:
- If you were using `--no-ptb` for execution tracing, this capability will be added to `SimulationEnvironment` in a future release
- For most use cases, PTB execution provides identical results with better consistency

---

## 3. Internal API Migration

### `execute_constructor_chain_as_ptb()` → `execute_constructor_chain_as_ptb_via_sim()`

**Location**: `src/benchmark/runner.rs`

**Before**:
```rust
use crate::benchmark::runner::execute_constructor_chain_as_ptb;

let results = execute_constructor_chain_as_ptb(
    &mut harness,
    &constructor_chain,
    &default_values,
    &validator,
)?;
```

**After**:
```rust
use crate::benchmark::runner::execute_constructor_chain_as_ptb_via_sim;

let results = execute_constructor_chain_as_ptb_via_sim(
    &mut sim_env,
    &constructor_chain,
    &default_values,
    &validator,
)?;
```

**Key Difference**: Uses `SimulationEnvironment` instead of `VMHarness` for consistent execution semantics.

---

### `execute_ptb_with_locking()` → `execute_ptb()`

**Location**: `src/benchmark/simulation.rs`

**Before**:
```rust
let result = sim_env.execute_ptb_with_locking(inputs, commands);
```

**After**:
```rust
// Locking is now automatic
let result = sim_env.execute_ptb(inputs, commands);
```

**Note**: Object locking is now handled automatically by `execute_ptb()`. The `_with_locking` variant is redundant.

---

### `BenchmarkError::mark_expected()` → `set_source()`

**Location**: `src/benchmark/errors.rs`

**Before**:
```rust
let error = BenchmarkError::new(...)
    .mark_expected();
```

**After**:
```rust
use crate::benchmark::errors::ErrorSource;

let error = BenchmarkError::new(...)
    .set_source(ErrorSource::InfrastructureLimitation);
```

**Note**: `set_source()` provides more granular error categorization:
- `ErrorSource::InfrastructureLimitation` - Known sandbox limitations
- `ErrorSource::ModuleCode` - Error in the Move code being analyzed
- `ErrorSource::UserInput` - Invalid user input
- `ErrorSource::Internal` - Internal tool errors

---

## 4. Test Migration

If you have tests using deprecated APIs, here's a migration example:

### Before

```rust
#[test]
fn test_llm_toolkit() {
    let mut toolkit = LlmToolkit::new("fixtures/bytecode").unwrap();

    // List modules
    let result = toolkit.execute(ToolCall::ListModules).unwrap();
    assert!(result.contains("test_module"));

    // Get struct info
    let result = toolkit.execute(ToolCall::GetStructInfo {
        module_path: "0x1::test_module".to_string(),
        struct_name: "TestStruct".to_string(),
    }).unwrap();
    assert!(result.contains("fields"));
}
```

### After

```rust
#[test]
fn test_sandbox_api() {
    let mut env = SimulationEnvironment::new_with_framework().unwrap();

    // Load modules first
    let load_req: SandboxRequest = serde_json::from_value(json!({
        "action": "load_module",
        "bytecode_path": "fixtures/bytecode"
    })).unwrap();
    execute_request(&mut env, load_req).unwrap();

    // List modules
    let req: SandboxRequest = serde_json::from_value(json!({
        "action": "list_modules"
    })).unwrap();
    let result = execute_request(&mut env, req).unwrap();
    assert!(result.to_string().contains("test_module"));

    // Get struct info
    let req: SandboxRequest = serde_json::from_value(json!({
        "action": "get_struct_info",
        "module_path": "0x1::test_module",
        "struct_name": "TestStruct"
    })).unwrap();
    let result = execute_request(&mut env, req).unwrap();
    assert!(result.to_string().contains("fields"));
}
```

---

## 5. Checklist for v0.6.0 Upgrade

Use this checklist when upgrading:

### Code Changes

- [ ] Replace `LlmToolkit::new()` with `SimulationEnvironment::new_with_framework()`
- [ ] Replace `toolkit.execute(ToolCall::X)` with `execute_request(&mut env, SandboxRequest)`
- [ ] Replace `LlmToolkit::tool_schema()` with `{"action": "list_available_tools"}`
- [ ] Replace `execute_constructor_chain_as_ptb()` with `execute_constructor_chain_as_ptb_via_sim()`
- [ ] Replace `execute_ptb_with_locking()` with `execute_ptb()`
- [ ] Replace `mark_expected()` with `set_source(ErrorSource::InfrastructureLimitation)`

### CLI/Script Changes

- [ ] Remove `--no-mm2` flags
- [ ] Remove `--use-mm2` flags (redundant)
- [ ] Remove `--no-phase-errors` flags
- [ ] Remove `--use-phase-errors` flags (redundant)
- [ ] Remove `--no-ptb` flags
- [ ] Remove `--use-ptb` flags (redundant)

### Import Changes

```rust
// Remove these imports
use sui_move_interface_extractor::benchmark::llm_tools::{LlmToolkit, ToolCall};

// Add these imports
use sui_move_interface_extractor::benchmark::sandbox_exec::{SandboxRequest, execute_request};
use sui_move_interface_extractor::benchmark::simulation::SimulationEnvironment;
```

---

## 6. Timeline

| Version | Status | Notes |
|---------|--------|-------|
| v0.5.0 | Current | Deprecation warnings added |
| v0.6.0 | Planned | Deprecated items removed |

---

## 7. Getting Help

If you encounter issues during migration:

1. **Check the API docs**: `cargo doc --open` for full documentation
2. **Use `list_available_tools`**: Get complete schema of available actions
3. **File an issue**: https://github.com/your-org/sui-move-interface-extractor/issues

---

## Appendix: Files to Update in v0.6.0

When preparing the v0.6.0 release, these files need changes:

### Files to Delete
- `src/benchmark/llm_tools.rs` (2,686 lines)

### Files to Modify

**`src/args.rs`**:
- Remove `no_mm2` field and doc comments
- Remove `no_phase_errors` field and doc comments
- Remove `no_ptb` field and doc comments
- Remove `effective_use_mm2()` method
- Remove `effective_phase_errors()` method
- Remove `effective_use_ptb()` method
- Remove `use_mm2` field (redundant when no alternative)
- Remove `use_phase_errors` field (redundant when no alternative)
- Consider removing `use_ptb` field (redundant when no alternative)

**`src/benchmark/mod.rs`**:
- Remove `pub mod llm_tools;`
- Update module documentation to remove deprecated modules table

**`src/benchmark/runner.rs`**:
- Remove `execute_constructor_chain_as_ptb()` function

**`src/benchmark/simulation.rs`**:
- Remove `execute_ptb_with_locking()` method

**`src/benchmark/errors.rs`**:
- Remove `mark_expected()` method
- Remove `is_expected_limitation` field (if no longer needed)

**`tests/tx_replay_test.rs`**:
- Migrate all `LlmToolkit`/`ToolCall` tests to `SandboxRequest`
- Remove backwards compatibility tests

### Estimated Removal
- ~2,700 lines of deprecated code
- ~233 deprecation warnings eliminated
- Cleaner, unified API surface
