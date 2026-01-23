# Error Handling Improvement Plan

## Overview

This plan outlines a three-phase approach to improving error handling in the sui-move-interface-extractor project. The goal is to create error messages that are **helpful but neutral** - providing enough context for users to understand what went wrong without prescribing exactly how to fix it.

### Design Principles

1. **Factual, not prescriptive**: Report what happened, not what to do
2. **Context-rich**: Include relevant state, versions, and relationships
3. **Neutral tone**: Describe the situation without judgment
4. **DeFi-aware**: Understand that users are debugging complex multi-step transactions
5. **Debuggable**: Provide enough information for users to investigate further

### Error Message Structure Pattern

```
[PHASE] ERROR_TYPE: What happened
  Location: Where in the code/transaction
  Context: Relevant objects, values, versions
  State: What was expected vs. what was found
  Historical: What may have changed since the transaction
```

---

## Phase 1: Critical Fixes (P0 - Prevent Panics/Data Loss)

**Goal**: Eliminate all panic-inducing code paths and silent data loss scenarios.

**Timeline**: Immediate priority

### 1.1 Remove Unsafe `.unwrap()` in Coin Byte Parsing

**Files**: `src/benchmark/ptb.rs`
**Lines**: 2046, 2059, 2146, 2154

**Current Code**:

```rust
// Line 2059
let original_value = u64::from_le_bytes(coin_bytes[32..40].try_into().unwrap());
```

**Problem**: Can panic if byte slice conversion fails, even after length check.

**Fix**:

```rust
let original_value = u64::from_le_bytes(
    coin_bytes.get(32..40)
        .ok_or_else(|| anyhow!(
            "Coin data too short: expected at least 40 bytes, got {}",
            coin_bytes.len()
        ))?
        .try_into()
        .map_err(|_| anyhow!(
            "Failed to parse coin balance from bytes at offset 32-40"
        ))?
);
```

**Locations to fix**:

- [ ] Line 2046: `bytes[..8].try_into().unwrap()` in balance extraction
- [ ] Line 2059: `coin_bytes[32..40].try_into().unwrap()` in SplitCoins
- [ ] Line 2146: `dest_bytes[32..40].try_into().unwrap()` in MergeCoins
- [ ] Line 2154: `source_bytes[32..40].try_into().unwrap()` in MergeCoins loop

---

### 1.2 Fix Lock Poisoning Silent Data Loss

**Files**: `src/benchmark/vm.rs`
**Lines**: 1560, 1582, 1087

**Current Code**:

```rust
// Line 1560 - PTBSession::finish()
pub fn finish(self) -> DynamicFieldSnapshot {
    let state = self.shared_state.lock().ok();  // Swallows poison error
    let children = state
        .map(|s| { /* ... */ })
        .unwrap_or_default();  // Returns empty on error
    DynamicFieldSnapshot { children }
}
```

**Problem**: If mutex is poisoned (panic in another thread), silently returns empty data. This loses all dynamic field changes (Tables, Bags) without any indication.

**Fix**:

```rust
pub fn finish(self) -> Result<DynamicFieldSnapshot> {
    let state = self.shared_state.lock()
        .map_err(|e| anyhow!(
            "PTBSession state lock poisoned - execution state may be corrupted: {}",
            e
        ))?;

    let children = state.dynamic_field_children
        .iter()
        .map(|(k, v)| /* ... */)
        .collect();

    Ok(DynamicFieldSnapshot { children })
}
```

**Locations to fix**:

- [ ] Line 1560: `PTBSession::finish()` - return `Result<DynamicFieldSnapshot>`
- [ ] Line 1582: `PTBSession::finish_with_bytes()` - return `Result<(DynamicFieldSnapshot, HashMap)>`
- [ ] Line 1087: `get_trace()` - return `Result<ExecutionTrace>` or log warning

**Migration note**: This changes the public API. Callers will need to handle the Result.

---

### 1.3 Create Static Well-Known Types

**Files**: `src/benchmark/ptb.rs`
**Lines**: 2078-2084, 2529-2530, 2580-2582

**Current Code**:

```rust
// Scattered throughout, repeated multiple times
address: AccountAddress::from_hex_literal("0x2").unwrap(),
module: Identifier::new("coin").unwrap(),
name: Identifier::new("Coin").unwrap(),
```

**Problem**: Unwrap on every use, even though these are constants that should never fail.

**Fix**: Create a new module `src/benchmark/well_known.rs`:

```rust
//! Well-known Sui framework types and addresses.
//!
//! These are validated once at compile time / initialization,
//! eliminating runtime panics from identifier parsing.

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use once_cell::sync::Lazy;

/// Sui framework address (0x2)
pub static SUI_FRAMEWORK: Lazy<AccountAddress> = Lazy::new(|| {
    AccountAddress::from_hex_literal("0x2")
        .expect("0x2 is a valid address - this is a bug if it fails")
});

/// Move stdlib address (0x1)
pub static MOVE_STDLIB: Lazy<AccountAddress> = Lazy::new(|| {
    AccountAddress::from_hex_literal("0x1")
        .expect("0x1 is a valid address")
});

/// Sui system address (0x3)
pub static SUI_SYSTEM: Lazy<AccountAddress> = Lazy::new(|| {
    AccountAddress::from_hex_literal("0x3")
        .expect("0x3 is a valid address")
});

/// Common identifiers
pub mod ident {
    use super::*;

    pub static COIN: Lazy<Identifier> = Lazy::new(||
        Identifier::new("coin").expect("'coin' is valid")
    );
    pub static SUI: Lazy<Identifier> = Lazy::new(||
        Identifier::new("sui").expect("'sui' is valid")
    );
    pub static BALANCE: Lazy<Identifier> = Lazy::new(||
        Identifier::new("balance").expect("'balance' is valid")
    );
    // ... more as needed
}

/// Well-known type tags
pub mod types {
    use super::*;

    /// 0x2::sui::SUI
    pub static SUI_COIN_TYPE: Lazy<TypeTag> = Lazy::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *SUI_FRAMEWORK,
            module: ident::SUI.clone(),
            name: ident::SUI.clone(),
            type_params: vec![],
        }))
    });

    /// 0x2::coin::Coin<T>
    pub fn coin_type(inner: TypeTag) -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: *SUI_FRAMEWORK,
            module: ident::COIN.clone(),
            name: Identifier::new("Coin").expect("Coin is valid"),
            type_params: vec![inner],
        }))
    }
}
```

**Usage after fix**:

```rust
use crate::benchmark::well_known::{SUI_FRAMEWORK, types};

// Instead of unwrap everywhere:
let coin_type = types::coin_type(types::SUI_COIN_TYPE.clone());
```

---

## Phase 2: Error Context Enhancement

**Goal**: Ensure every error message includes enough context to understand what went wrong.

**Timeline**: After Phase 1

### 2.1 Enhance SimulationError with Execution Context

**File**: `src/benchmark/simulation.rs`

**Current**:

```rust
pub enum SimulationError {
    MissingPackage {
        address: String,
        module: Option<String>,
    },
    // ...
}
```

**Enhanced**:

```rust
pub enum SimulationError {
    /// A required package/module is not available.
    MissingPackage {
        /// The address of the missing package
        address: String,
        /// The specific module within the package (if known)
        module: Option<String>,
        /// Packages that depend on this one (helps trace the dependency)
        referenced_by: Vec<String>,
        /// Whether this package has known upgrades
        upgrade_info: Option<PackageUpgradeInfo>,
    },

    /// A required object is not available.
    MissingObject {
        /// Object ID that was not found
        id: String,
        /// Expected type of the object
        expected_type: Option<String>,
        /// The command that tried to access this object
        accessing_command: Option<usize>,
        /// Version requested (if historical)
        requested_version: Option<u64>,
    },

    /// Type mismatch between expected and provided.
    TypeMismatch {
        expected: String,
        got: String,
        /// Where the mismatch occurred (e.g., "input 0", "command 3 argument 1")
        location: String,
        /// The command index where this occurred
        command_index: Option<usize>,
    },

    /// Contract assertion failed (abort).
    ContractAbort {
        /// Full module path (e.g., "0x1eabed72::pool")
        module: String,
        /// Function name
        function: String,
        /// Abort code from the contract
        abort_code: u64,
        /// Human-readable message if available
        message: Option<String>,
        /// The command index that aborted
        command_index: Option<usize>,
        /// What objects were involved in this call
        involved_objects: Vec<String>,
    },

    // ... other variants enhanced similarly
}

/// Information about package upgrades
#[derive(Debug, Clone)]
pub struct PackageUpgradeInfo {
    /// Original package ID (what transactions reference)
    pub original_id: String,
    /// Current storage ID (where bytecode lives)
    pub storage_id: String,
    /// Version number
    pub version: u64,
}
```

---

### 2.2 Add Error Context Trait

**New file**: `src/benchmark/error_context.rs`

```rust
//! Error context enrichment for debugging.
//!
//! Provides traits and utilities for adding execution context to errors
//! without prescribing fixes - just facts for debugging.

use std::collections::HashMap;

/// Context that can be attached to any error for debugging.
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// Which PTB command was executing (0-indexed)
    pub command_index: Option<usize>,

    /// Description of the command (e.g., "MoveCall 0x2::coin::split")
    pub command_description: Option<String>,

    /// Objects that were inputs to this operation
    pub input_objects: Vec<ObjectContext>,

    /// Packages that were loaded for this execution
    pub loaded_packages: Vec<String>,

    /// The transaction checkpoint/timestamp if replaying historical
    pub historical_checkpoint: Option<u64>,

    /// Any version mismatches detected
    pub version_notes: Vec<String>,
}

/// Context about a specific object involved in an error.
#[derive(Debug, Clone)]
pub struct ObjectContext {
    pub id: String,
    pub type_tag: Option<String>,
    pub version: Option<u64>,
    pub owner: Option<String>,
    /// Size of BCS data (helps identify truncation issues)
    pub data_size: Option<usize>,
}

/// Trait for errors that can have execution context attached.
pub trait WithContext {
    /// Add execution context to this error.
    fn with_context(self, ctx: ExecutionContext) -> Self;

    /// Add a single context note.
    fn with_note(self, note: impl Into<String>) -> Self;
}

impl ExecutionContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn at_command(mut self, index: usize, description: impl Into<String>) -> Self {
        self.command_index = Some(index);
        self.command_description = Some(description.into());
        self
    }

    pub fn with_object(mut self, obj: ObjectContext) -> Self {
        self.input_objects.push(obj);
        self
    }

    pub fn with_checkpoint(mut self, checkpoint: u64) -> Self {
        self.historical_checkpoint = Some(checkpoint);
        self
    }

    pub fn with_version_note(mut self, note: impl Into<String>) -> Self {
        self.version_notes.push(note.into());
        self
    }
}
```

---

### 2.3 Improve Error Messages in Key Locations

**File**: `src/benchmark/resolver.rs`

**Current**:

```rust
.map_err(|e| anyhow!("failed to deserialize framework module: {:?}", e))?
```

**Improved**:

```rust
.map_err(|e| anyhow!(
    "Failed to deserialize module '{}' from framework package at {}: {}",
    module_name,
    path.display(),
    e
))?
```

---

**File**: `src/benchmark/ptb.rs`

**Current**:

```rust
Err(anyhow!("command returned no values"))
```

**Improved**:

```rust
Err(anyhow!(
    "Command {} ({}) returned no values, but result was expected by command {}",
    cmd_idx,
    self.describe_command(cmd_idx),
    dependent_cmd_idx
))
```

---

**File**: `src/benchmark/tx_replay.rs`

**Current**:

```rust
.map_err(|e| anyhow!("Failed to BCS-serialize type tag: {}", e))?
```

**Improved**:

```rust
.map_err(|e| anyhow!(
    "Failed to BCS-serialize type tag '{}' for dynamic field lookup: {}",
    format_type_tag(key_type_tag),
    e
))?
```

---

### 2.4 DeFi-Specific Error Context

For DeFi transaction replay, errors should include relevant financial context without prescribing fixes.

**Add to ContractAbort display**:

```rust
impl fmt::Display for SimulationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimulationError::ContractAbort {
                module, function, abort_code, message, command_index, involved_objects
            } => {
                writeln!(f, "ContractAbort at {module}::{function}")?;
                writeln!(f, "  Abort code: {abort_code}")?;

                if let Some(msg) = message {
                    writeln!(f, "  Message: {msg}")?;
                }

                if let Some(idx) = command_index {
                    writeln!(f, "  Command index: {idx}")?;
                }

                if !involved_objects.is_empty() {
                    writeln!(f, "  Objects involved:")?;
                    for obj in involved_objects {
                        writeln!(f, "    - {obj}")?;
                    }
                }

                // Add abort code context (factual, not prescriptive)
                if let Some(context) = get_abort_code_context(*abort_code, module) {
                    writeln!(f, "  Context: {context}")?;
                }

                Ok(())
            }
            // ... other variants
        }
    }
}

/// Get factual context about common abort codes.
/// Does NOT prescribe fixes - just explains what the code typically means.
fn get_abort_code_context(code: u64, module: &str) -> Option<String> {
    // Common Sui framework abort codes
    match code {
        0 => Some("Assertion failed (generic)".into()),
        1 => Some("Arithmetic overflow or underflow".into()),
        513 => Some("Version check failed - package version mismatch".into()),
        // DeFi-specific codes (by module pattern)
        _ if module.contains("pool") => match code {
            3 => Some("Pool liquidity or tick range check".into()),
            4 => Some("Slippage tolerance exceeded".into()),
            _ => None,
        },
        _ if module.contains("balance") => match code {
            0 => Some("Insufficient balance for operation".into()),
            _ => None,
        },
        _ => None,
    }
}
```

---

## Phase 3: Custom Error Types

**Goal**: Replace `anyhow::Error` with structured error types in public APIs for better programmatic handling.

**Timeline**: After Phase 2

### 3.1 Define Module-Specific Error Enums

**New file**: `src/benchmark/errors/mod.rs` (expand existing)

```rust
use thiserror::Error;

/// Errors from the VM execution layer.
#[derive(Debug, Error)]
pub enum VMError {
    #[error("Failed to load module {module} from package {package}: {reason}")]
    ModuleLoadError {
        package: String,
        module: String,
        reason: String,
    },

    #[error("Function {function} not found in module {module}")]
    FunctionNotFound {
        module: String,
        function: String,
    },

    #[error("Execution aborted in {module}::{function} with code {code}")]
    ExecutionAbort {
        module: String,
        function: String,
        code: u64,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Type resolution failed for {type_str}: {reason}")]
    TypeResolutionError {
        type_str: String,
        reason: String,
    },

    #[error("Object {object_id} not found (expected type: {expected_type:?})")]
    ObjectNotFound {
        object_id: String,
        expected_type: Option<String>,
    },

    #[error("Lock poisoned in {component}: {reason}")]
    LockPoisoned {
        component: String,
        reason: String,
    },
}

/// Errors from PTB construction and execution.
#[derive(Debug, Error)]
pub enum PTBError {
    #[error("Invalid argument at index {index}: {reason}")]
    InvalidArgument {
        index: usize,
        reason: String,
    },

    #[error("Command {command_index} failed: {reason}")]
    CommandFailed {
        command_index: usize,
        command_type: String,
        reason: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Result {result_index} from command {command_index} is not available")]
    ResultNotAvailable {
        command_index: usize,
        result_index: usize,
    },

    #[error("Coin operation failed: {reason}")]
    CoinOperationFailed {
        operation: String,
        reason: String,
        coin_type: Option<String>,
    },
}

/// Errors from data fetching (gRPC, GraphQL).
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("Connection to {endpoint} failed: {reason}")]
    ConnectionFailed {
        endpoint: String,
        reason: String,
    },

    #[error("Object {object_id} not found at version {version:?}")]
    ObjectNotFound {
        object_id: String,
        version: Option<u64>,
    },

    #[error("Package {package_id} not found")]
    PackageNotFound {
        package_id: String,
    },

    #[error("Transaction {digest} not found")]
    TransactionNotFound {
        digest: String,
    },

    #[error("Rate limited by {endpoint}: retry after {retry_after_secs:?}s")]
    RateLimited {
        endpoint: String,
        retry_after_secs: Option<u64>,
    },
}

/// Errors from transaction replay.
#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("Missing historical state for object {object_id} at checkpoint {checkpoint}")]
    MissingHistoricalState {
        object_id: String,
        checkpoint: u64,
    },

    #[error("Package {package_id} has been upgraded: original={original_version}, current={current_version}")]
    PackageVersionMismatch {
        package_id: String,
        original_version: u64,
        current_version: u64,
    },

    #[error("Dynamic field not found: parent={parent_id}, key_type={key_type}")]
    DynamicFieldNotFound {
        parent_id: String,
        key_type: String,
    },

    #[error("Sender mismatch: transaction was sent by {original_sender}, but replay uses {replay_sender}")]
    SenderMismatch {
        original_sender: String,
        replay_sender: String,
    },
}
```

---

### 3.2 Create Unified Error Type

```rust
/// Top-level error type for the benchmark/simulation system.
#[derive(Debug, Error)]
pub enum BenchmarkError {
    #[error(transparent)]
    VM(#[from] VMError),

    #[error(transparent)]
    PTB(#[from] PTBError),

    #[error(transparent)]
    Fetch(#[from] FetchError),

    #[error(transparent)]
    Replay(#[from] ReplayError),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl BenchmarkError {
    /// Get the error category for classification/reporting.
    pub fn category(&self) -> &'static str {
        match self {
            BenchmarkError::VM(_) => "vm",
            BenchmarkError::PTB(_) => "ptb",
            BenchmarkError::Fetch(_) => "fetch",
            BenchmarkError::Replay(_) => "replay",
            BenchmarkError::Internal(_) => "internal",
        }
    }

    /// Check if this error is recoverable (e.g., retry might help).
    pub fn is_recoverable(&self) -> bool {
        matches!(self,
            BenchmarkError::Fetch(FetchError::RateLimited { .. }) |
            BenchmarkError::Fetch(FetchError::ConnectionFailed { .. })
        )
    }
}
```

---

### 3.3 Migration Strategy

**Step 1**: Add new error types alongside existing code
**Step 2**: Gradually convert internal functions to use new types
**Step 3**: Update public APIs (breaking change, document in CHANGELOG)
**Step 4**: Provide migration guide for users

Example migration in `simulation.rs`:

```rust
// Before (anyhow everywhere)
pub fn execute_ptb(&mut self, inputs: Vec<InputValue>, commands: Vec<Command>) -> ExecutionResult

// After (structured errors)
pub fn execute_ptb(&mut self, inputs: Vec<InputValue>, commands: Vec<Command>) -> Result<TransactionEffects, PTBError>

// ExecutionResult can still be used for compatibility, but now has structured error:
pub struct ExecutionResult {
    pub success: bool,
    pub effects: Option<TransactionEffects>,
    pub error: Option<BenchmarkError>,  // Changed from SimulationError
    // ...
}
```

---

## Testing Strategy

### Phase 1 Tests

```rust
#[test]
fn test_coin_parsing_with_truncated_data() {
    let short_bytes = vec![0u8; 30]; // Less than required 40 bytes
    let result = parse_coin_balance(&short_bytes);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("expected at least 40 bytes"));
}

#[test]
fn test_lock_poison_returns_error() {
    // Create poisoned mutex scenario
    let session = create_session_with_poisoned_lock();
    let result = session.finish();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("poisoned"));
}
```

### Phase 2 Tests

```rust
#[test]
fn test_error_includes_command_context() {
    let error = SimulationError::ContractAbort {
        module: "0x2::coin".into(),
        function: "split".into(),
        abort_code: 1,
        message: None,
        command_index: Some(3),
        involved_objects: vec!["0x123".into()],
    };

    let msg = error.to_string();
    assert!(msg.contains("Command index: 3"));
    assert!(msg.contains("0x123"));
}

#[test]
fn test_missing_package_shows_dependents() {
    let error = SimulationError::MissingPackage {
        address: "0xabc".into(),
        module: Some("utils".into()),
        referenced_by: vec!["0xdef::pool".into()],
        upgrade_info: None,
    };

    let msg = error.to_string();
    assert!(msg.contains("referenced by"));
    assert!(msg.contains("0xdef::pool"));
}
```

### Phase 3 Tests

```rust
#[test]
fn test_error_type_matching() {
    let error: BenchmarkError = VMError::FunctionNotFound {
        module: "coin".into(),
        function: "nonexistent".into(),
    }.into();

    assert_eq!(error.category(), "vm");
    assert!(!error.is_recoverable());

    match error {
        BenchmarkError::VM(VMError::FunctionNotFound { module, function }) => {
            assert_eq!(module, "coin");
            assert_eq!(function, "nonexistent");
        }
        _ => panic!("Wrong error variant"),
    }
}
```

---

## Implementation Order

### Week 1: Phase 1 (Critical Fixes)

- [ ] Create `src/benchmark/well_known.rs` with static types
- [ ] Fix all `.unwrap()` in `ptb.rs` coin parsing
- [ ] Fix lock poisoning in `vm.rs` PTBSession
- [ ] Add tests for all P0 fixes
- [ ] Run full test suite, fix any regressions

### Week 2-3: Phase 2 (Context Enhancement)

- [ ] Create `src/benchmark/error_context.rs`
- [ ] Enhance `SimulationError` with additional context fields
- [ ] Update error messages in `resolver.rs`, `ptb.rs`, `tx_replay.rs`
- [ ] Add abort code context mapping
- [ ] Add tests for enhanced error messages

### Week 4-5: Phase 3 (Custom Error Types)

- [ ] Create error type hierarchy in `src/benchmark/errors/`
- [ ] Migrate internal functions to new error types
- [ ] Update public APIs (document breaking changes)
- [ ] Create migration guide
- [ ] Full integration testing

---

## Success Criteria

1. **No panics in production paths**: All `.unwrap()` removed from non-test code or justified
2. **Context in every error**: Users can identify what failed and what state was involved
3. **Neutral messaging**: Errors describe facts, not solutions
4. **DeFi debuggability**: Transaction replay errors include checkpoint, version, and object context
5. **Programmatic handling**: Custom error types allow matching on specific error conditions
6. **Backwards compatibility**: Migration path provided for existing users

---

## Appendix: Common DeFi Error Patterns

### Package Version Mismatch

```
ContractAbort at 0xa757::version::assert_current_version
  Abort code: 513
  Command index: 2
  Context: Version check failed - package version mismatch

  The Market object was created with package version 1,
  but current bytecode expects version 17.

  This is expected for historical transaction replay when
  the protocol has been upgraded since the transaction.
```

### Insufficient Liquidity

```
ContractAbort at 0x1eabed72::pool::swap
  Abort code: 4
  Command index: 5
  Context: Slippage tolerance exceeded
  Objects involved:
    - Pool 0x8b7a1b6e... (current_tick: 2150)
    - Input coin: 1000000 MIST

  The swap could not complete within slippage bounds.
  Pool state may have changed since the original transaction.
```

### Dynamic Field Not Found

```
ContractAbort at 0xa757::market::borrow
  Abort code: 0
  Command index: 3
  Context: Dynamic field lookup failed

  Parent: Market (0xa757975255...)
  Expected child: CollateralConfig<USDC>

  This dynamic field may not have existed at the
  transaction's checkpoint, or was added in a later
  protocol upgrade.
```
