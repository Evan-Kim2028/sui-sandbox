# Type Inhabitation Benchmark: Improvement Opportunities

This document outlines potential improvements for the type inhabitation benchmark infrastructure, based on analysis of the current implementation and observed failure patterns.

## Current State (v0.4.0)

### What Works Well
- **Phase-based error taxonomy** (E101-E502) provides good categorization
- **MM2 integration** enables static type checking before execution
- **Rust stub generation** produces valid Move 2024 syntax
- **Legacy compatibility** maintained with A1-B2 mapping

### Known Gaps

## 1. Build Phase Errors (Pre-Pipeline)

Currently, build failures from `sui move build` are not categorized with our error codes. They're just reported as "build failed" with raw compiler output.

**Proposal: Add Phase 0 - Build Errors (E0xx)**

```rust
/// E001: Module address not defined
ModuleAddressUndefined,

/// E002: Invalid Move.toml syntax
InvalidManifest,

/// E003: Import resolution failed (use statement)
ImportResolutionFailed,

/// E004: Type syntax error (E03006 from Move compiler)
TypeSyntaxError,

/// E005: Entry function signature invalid (Sui E02002)
InvalidEntrySignature,

/// E006: Ability constraint error at compile time
CompileTimeAbilityError,
```

**Benefit**: Distinguishes "LLM wrote invalid syntax" from "code compiled but couldn't execute"

---

## 2. LLM vs Infrastructure Error Attribution

The `is_expected_limitation` flag is a start, but we could be more precise:

**Proposal: Add `error_source` field**

```rust
#[derive(Debug, Clone, Copy)]
pub enum ErrorSource {
    /// LLM generated incorrect code
    LlmError,
    /// Infrastructure limitation (sandbox can't simulate this)
    InfrastructureLimitation,
    /// Target package has no valid entry points
    TargetPackageLimitation,
    /// Unknown/ambiguous source
    Unknown,
}
```

**Examples**:
- `E404 UnsupportedNative` -> `InfrastructureLimitation`
- `E201 TypeMismatch` -> `LlmError`
- `E301 NoConstructor` for types with no public constructors -> `TargetPackageLimitation`
- `E301 NoConstructor` when constructor exists but LLM didn't use it -> `LlmError`

---

## 3. More Granular Grading

Current grading is binary: `ok: true/false`. We could provide more nuance:

**Proposal: Scoring Rubric**

```json
{
  "score": 0.75,
  "criteria": {
    "compiles": true,           // 0.25 points
    "imports_target": true,     // 0.25 points
    "creates_target_type": true,// 0.25 points
    "executes_cleanly": false   // 0.25 points (failed here)
  },
  "partial_credit": {
    "reason": "Created target type but execution aborted",
    "phase_reached": "execution"
  }
}
```

**Benefit**: Better signal for model comparison. A model that reaches execution is better than one that fails at resolution, even if both ultimately fail.

---

## 4. Constructor Discovery Feedback

When synthesis fails with E301 (NoConstructor), we could provide more actionable info:

**Proposal: Enhance Failure Context**

```rust
pub struct ConstructorSearchResult {
    /// Types that have accessible constructors
    pub constructible_types: Vec<String>,
    /// Types with no constructor (explain why)
    pub unconstructible_types: Vec<(String, UnconstructibleReason)>,
}

pub enum UnconstructibleReason {
    NoPublicConstructor,
    RequiresCapability(String),      // e.g., "AdminCap"
    RequiresExternalObject(String),  // e.g., "Clock"
    RequiresSignerAuth,
    CyclicDependency,
}
```

**Benefit**: LLM can use this feedback to adjust strategy on retry.

---

## 5. Type Inhabitation Success Metrics

Beyond binary success, track what types were actually inhabited:

**Proposal: Inhabitation Report**

```json
{
  "inhabited_types": [
    {
      "type": "0x123::module::MyStruct",
      "how": "direct_construction",
      "constructor_used": "0x123::module::new"
    },
    {
      "type": "0x123::module::Wrapper<T>",
      "how": "generic_instantiation",
      "type_arg": "u64"
    }
  ],
  "attempted_types": [
    {
      "type": "0x123::module::AdminCap",
      "failure": "requires OTW from init"
    }
  ],
  "coverage": {
    "entry_functions_tested": 3,
    "entry_functions_total": 5,
    "public_types_inhabited": 4,
    "public_types_total": 8
  }
}
```

---

## 6. MM2 Integration Enhancements

### 6.1 Pre-flight Type Analysis
Before sending to LLM, analyze which types are likely inhabitable:

```rust
pub struct TypeInhabitabilityAnalysis {
    /// Types that can definitely be constructed
    pub definitely_inhabitable: Vec<TypeInfo>,
    /// Types that might be constructible with the right approach
    pub possibly_inhabitable: Vec<TypeInfo>,
    /// Types that cannot be constructed in sandbox
    pub definitely_uninhabitable: Vec<(TypeInfo, String)>, // (type, reason)
}
```

**Use case**: Filter prompts to only include types that have a chance of success.

### 6.2 Static Call Graph Analysis
Use MM2 to trace what a function might call:

```rust
pub struct CallGraphAnalysis {
    /// Functions called directly
    pub direct_calls: Vec<FunctionId>,
    /// Native functions that might be reached
    pub native_calls: Vec<(String, NativeCategory)>,
    /// Potential abort points
    pub abort_points: Vec<AbortInfo>,
}
```

**Use case**: Skip functions that will definitely hit unsupported natives.

---

## 7. Prompt Feedback Loop

When builds fail, provide structured feedback:

**Proposal: Error-to-Guidance Mapping**

```python
ERROR_GUIDANCE = {
    "E03001": "Define the address in [addresses] section of Move.toml",
    "E03006": "Use 'use' imports for types, don't inline qualified paths in struct fields",
    "E02002": "Entry functions cannot return types without 'drop' ability",
    "E04007": "Type mismatch - check function signature against your arguments",
}
```

This is partially implemented in `_enhance_errors_with_sui_guidance` but could be more comprehensive.

---

## 8. Execution Trace Analysis

When execution fails, provide more context:

**Proposal: Trace-based Failure Analysis**

```rust
pub struct ExecutionTrace {
    /// Stack of function calls before failure
    pub call_stack: Vec<FunctionCall>,
    /// Last instruction before abort
    pub abort_location: Option<CodeLocation>,
    /// Values on stack at abort (if recoverable)
    pub stack_state: Option<Vec<String>>,
    /// Gas used before failure
    pub gas_consumed: u64,
}
```

---

## 9. Reproducibility Improvements

### 9.1 Deterministic Timestamps
Replace `time.time()` with a reproducible run ID based on seed.

### 9.2 Model Response Caching
Cache LLM responses by (prompt_hash, model, seed) for cheaper re-runs.

### 9.3 Checkpoint/Resume
For large benchmark runs, support resuming from last completed package.

---

## Implementation Priority

| Improvement | Impact | Effort | Priority |
|------------|--------|--------|----------|
| Build phase errors (E0xx) | High | Medium | P1 |
| Error source attribution | High | Low | P1 |
| Scoring rubric | High | Medium | P1 |
| Constructor feedback | Medium | Medium | P2 |
| Inhabitation metrics | Medium | Low | P2 |
| MM2 pre-flight analysis | Medium | High | P2 |
| Prompt feedback loop | Medium | Low | P2 |
| Execution traces | Low | High | P3 |
| Reproducibility | Medium | Medium | P3 |

---

## Next Steps

1. **Short-term**: Add E0xx build errors and error source attribution
2. **Medium-term**: Implement scoring rubric and constructor feedback
3. **Long-term**: MM2 pre-flight analysis and execution traces
