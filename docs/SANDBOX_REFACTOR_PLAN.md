# Sandbox Exec Refactoring Plan

## Current State

`sandbox_exec.rs` is a 4,911 LOC file that handles all sandbox operations for LLM integration. It contains:

- **48 request types** (SandboxRequest enum variants)
- **48 handler functions** (execute_* functions)
- **15+ helper functions** (encoding, decoding, formatting)
- **10+ data structures** (request/response types)

## Problems

1. **God module**: Single file handles 10+ distinct responsibilities
2. **Hard to navigate**: 4,911 lines makes finding code difficult
3. **Hard to test**: Monolithic structure limits unit testing
4. **Tight coupling**: Types, handlers, and helpers all intertwined

## Proposed Architecture

```
src/benchmark/sandbox/
├── mod.rs              # Re-exports, execute_request dispatcher
├── types.rs            # SandboxRequest, SandboxResponse, PtbCommand, etc.
├── handlers/
│   ├── mod.rs          # Handler trait, common utilities
│   ├── module.rs       # load_module, list_modules, compile_move
│   ├── introspection.rs # list_functions, get_function_info, list_structs, etc.
│   ├── objects.rs      # create_object, inspect_object, list_objects
│   ├── execution.rs    # execute_ptb, validate_ptb, call_function
│   ├── coins.rs        # register_coin, get_coin_metadata, list_coins
│   ├── clock.rs        # get_clock, set_clock, get_lamport_clock
│   ├── encoding.rs     # encode_bcs, decode_bcs, encode_vector
│   ├── bytecode.rs     # disassemble_function, disassemble_module
│   ├── cache.rs        # load_cached_*, is_framework_cached
│   ├── events.rs       # list_events, get_events_by_type, clear_events
│   └── utils.rs        # generate_id, parse_address, compute_hash
└── cli.rs              # run_sandbox_exec, run_interactive_sandbox
```

## Migration Strategy

### Phase 1: Extract Types (Low Risk)

Create `sandbox/types.rs` with all type definitions:

```rust
// sandbox/types.rs
pub enum SandboxRequest { ... }
pub enum PtbCommand { ... }
pub enum PtbInput { ... }
pub enum PtbArg { ... }
pub struct SandboxResponse { ... }
pub struct TransactionEffectsResponse { ... }
// ... etc
```

Keep sandbox_exec.rs unchanged but add:

```rust
// Re-export for backwards compatibility
pub use sandbox::types::*;
```

**Validation**: `cargo check` passes, all tests pass.

### Phase 2: Extract Handlers Module Structure

Create handler module skeleton:

```rust
// sandbox/handlers/mod.rs
use super::types::*;
use crate::benchmark::simulation::SimulationEnvironment;

/// Trait for sandbox request handlers (optional, for future extensibility)
pub trait SandboxHandler {
    fn execute(&self, env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse;
}

// Re-export all handlers
pub mod module;
pub mod introspection;
// ... etc
```

### Phase 3: Migrate Handlers (One Category at a Time)

Start with the simplest, most isolated handlers:

1. **clock.rs** (4 handlers, ~100 LOC) - No external dependencies beyond SimulationEnvironment
2. **coins.rs** (3 handlers, ~80 LOC) - Simple CRUD operations
3. **utils.rs** (6 handlers, ~200 LOC) - Stateless utility functions
4. **events.rs** (4 handlers, ~100 LOC) - Event query operations

Then move to more complex handlers:

5. **encoding.rs** (4 handlers, ~150 LOC) - BCS encoding/decoding
6. **introspection.rs** (8 handlers, ~300 LOC) - Type/function queries
7. **objects.rs** (5 handlers, ~400 LOC) - Object lifecycle
8. **bytecode.rs** (4 handlers, ~200 LOC) - Disassembly/compilation
9. **cache.rs** (5 handlers, ~250 LOC) - Cached object loading
10. **module.rs** (3 handlers, ~200 LOC) - Module loading

Finally, the most complex:

11. **execution.rs** (3 handlers, ~1500 LOC) - PTB execution, validation

### Phase 4: Update Dispatcher

Update `execute_request` to delegate to handler modules:

```rust
// sandbox/mod.rs
pub fn execute_request(
    env: &mut SimulationEnvironment,
    request: &SandboxRequest,
    verbose: bool,
) -> SandboxResponse {
    match request {
        // Clock operations
        SandboxRequest::GetClock => handlers::clock::execute_get_clock(env, verbose),
        SandboxRequest::SetClock { timestamp_ms } =>
            handlers::clock::execute_set_clock(env, *timestamp_ms, verbose),

        // Coin operations
        SandboxRequest::RegisterCoin { .. } => handlers::coins::execute_register_coin(..),

        // ... etc
    }
}
```

### Phase 5: Extract CLI

Move `run_sandbox_exec` and `run_interactive_sandbox` to `sandbox/cli.rs`.

## File Size Targets

| File | Target LOC | Content |
|------|------------|---------|
| types.rs | 800-900 | All type definitions |
| handlers/mod.rs | 50-100 | Common utilities, re-exports |
| handlers/execution.rs | 1200-1500 | PTB execution (largest handler) |
| handlers/introspection.rs | 300-400 | Type queries |
| handlers/objects.rs | 400-500 | Object operations |
| handlers/encoding.rs | 150-200 | BCS operations |
| handlers/bytecode.rs | 200-300 | Disassembly |
| handlers/clock.rs | 100-150 | Time operations |
| handlers/coins.rs | 80-120 | Coin operations |
| handlers/events.rs | 100-150 | Event queries |
| handlers/cache.rs | 250-350 | Cached loading |
| handlers/utils.rs | 200-300 | Utilities |
| handlers/module.rs | 200-300 | Module loading |
| cli.rs | 200-300 | CLI entry points |
| mod.rs | 200-300 | Dispatcher, re-exports |

**Total**: ~4,200-4,900 LOC (similar to current, but well-organized)

## Testing Strategy

1. **Before each phase**: Run full test suite, record baseline
2. **After types extraction**: Verify all consumers still compile
3. **After each handler migration**: Run integration tests
4. **Final validation**: Full benchmark suite

## Backwards Compatibility

The public API must remain unchanged:

```rust
// These must continue to work
use crate::benchmark::sandbox_exec::{
    SandboxRequest,
    SandboxResponse,
    execute_request,
    run_sandbox_exec,
};
```

Achieved via re-exports in the new module structure.

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Breaking changes | Re-export everything from old location |
| Circular dependencies | Careful module organization |
| Performance regression | Profile before/after |
| Test failures | Run tests after each small change |

## Implementation Order

1. [ ] Create `sandbox/` directory structure
2. [ ] Extract types to `sandbox/types.rs` (with re-exports)
3. [ ] Create handler module skeleton
4. [ ] Migrate clock handlers (simplest)
5. [ ] Migrate coins handlers
6. [ ] Migrate utils handlers
7. [ ] Migrate events handlers
8. [ ] Migrate encoding handlers
9. [ ] Migrate introspection handlers
10. [ ] Migrate objects handlers
11. [ ] Migrate bytecode handlers
12. [ ] Migrate cache handlers
13. [ ] Migrate module handlers
14. [ ] Migrate execution handlers (most complex)
15. [ ] Extract CLI
16. [ ] Update dispatcher
17. [ ] Clean up old sandbox_exec.rs
18. [ ] Final testing and documentation

## Success Criteria

- [ ] All existing tests pass
- [ ] No public API changes (re-exports maintain compatibility)
- [ ] Each handler file < 500 LOC (except execution.rs)
- [ ] Clear separation of concerns
- [ ] Improved test coverage opportunity
