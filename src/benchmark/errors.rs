//! Error codes and diagnostic messages for type inhabitation evaluation.
//!
//! # Error Taxonomy (v0.5.0)
//!
//! The type inhabitation pipeline uses a phase-based error taxonomy:
//!
//! | Phase | Purpose | Error Codes |
//! |-------|---------|-------------|
//! | Build | Compile Move code | E001-E006 |
//! | Resolution | Find modules/functions | E101-E103 |
//! | TypeCheck | Static type validation | E201-E205 |
//! | Synthesis | Build argument values | E301-E304 |
//! | Execution | VM execution | E401-E404 |
//! | Validation | Verify results | E501-E502 |
//!
//! Each error includes:
//! - Phase: Where in the pipeline it occurred
//! - Code: Specific error type
//! - Message: Human-readable description

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Phase-Based Error Taxonomy (v0.4.0)
// =============================================================================

/// Phase of the type inhabitation pipeline.
///
/// The pipeline processes in order: Build -> Resolution -> TypeCheck -> Synthesis -> Execution -> Validation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Phase 0: Build/compile the Move code
    Build,
    /// Phase 1: Module and function resolution
    Resolution,
    /// Phase 2: Static type checking (MM2)
    TypeCheck,
    /// Phase 3: Value synthesis and constructor chaining
    Synthesis,
    /// Phase 4: VM execution
    Execution,
    /// Phase 5: Result validation
    Validation,
}

impl Phase {
    /// Get the numeric prefix for this phase (0xx, 1xx, 2xx, etc.)
    pub fn code_prefix(&self) -> u16 {
        match self {
            Phase::Build => 0,
            Phase::Resolution => 100,
            Phase::TypeCheck => 200,
            Phase::Synthesis => 300,
            Phase::Execution => 400,
            Phase::Validation => 500,
        }
    }

    /// Get a short name for this phase
    pub fn short_name(&self) -> &'static str {
        match self {
            Phase::Build => "build",
            Phase::Resolution => "resolution",
            Phase::TypeCheck => "typecheck",
            Phase::Synthesis => "synthesis",
            Phase::Execution => "execution",
            Phase::Validation => "validation",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// Specific error codes within each phase.
///
/// Error codes are numbered by phase:
/// - 0xx: Build errors (pre-pipeline, Move compiler)
/// - 1xx: Resolution errors
/// - 2xx: Type check errors
/// - 3xx: Synthesis errors
/// - 4xx: Execution errors
/// - 5xx: Validation errors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    // =========================================================================
    // Build Errors (0xx) - Move compiler errors from `sui move build`
    // =========================================================================
    /// E001: Module address not defined in Move.toml
    #[serde(rename = "E001")]
    ModuleAddressUndefined,

    /// E002: Invalid Move.toml syntax
    #[serde(rename = "E002")]
    InvalidManifest,

    /// E003: Import resolution failed (use statement points to non-existent module/type)
    #[serde(rename = "E003")]
    ImportResolutionFailed,

    /// E004: Type syntax error (E03006 from Move compiler - invalid qualified path in field)
    #[serde(rename = "E004")]
    TypeSyntaxError,

    /// E005: Entry function signature invalid (Sui E02002 - wrong return type, etc.)
    #[serde(rename = "E005")]
    InvalidEntrySignature,

    /// E006: Ability constraint error at compile time (missing copy/drop/store/key)
    #[serde(rename = "E006")]
    CompileTimeAbilityError,

    // =========================================================================
    // Resolution Errors (1xx)
    // =========================================================================
    /// E101: Module not found in bytecode corpus
    #[serde(rename = "E101")]
    ModuleNotFound,

    /// E102: Function not found in module
    #[serde(rename = "E102")]
    FunctionNotFound,

    /// E103: Function exists but is not callable (private, not entry)
    #[serde(rename = "E103")]
    NotCallable,

    // =========================================================================
    // Type Check Errors (2xx)
    // =========================================================================
    /// E201: Type mismatch between argument and parameter
    #[serde(rename = "E201")]
    TypeMismatch,

    /// E202: Ability constraint violation (copy/drop/store/key)
    #[serde(rename = "E202")]
    AbilityViolation,

    /// E203: Generic type parameter bounds not satisfied
    #[serde(rename = "E203")]
    GenericBoundsViolation,

    /// E204: Recursive type detected (cannot compute layout)
    #[serde(rename = "E204")]
    RecursiveType,

    /// E205: Unknown type (struct not found in any loaded module)
    #[serde(rename = "E205")]
    UnknownType,

    // =========================================================================
    // Synthesis Errors (3xx)
    // =========================================================================
    /// E301: No constructor found for required type
    #[serde(rename = "E301")]
    NoConstructor,

    /// E302: Constructor chain exceeds maximum depth
    #[serde(rename = "E302")]
    ChainTooDeep,

    /// E303: Constructor has unsupported parameter type
    #[serde(rename = "E303")]
    UnsupportedConstructorParam,

    /// E304: BCS serialization failed for synthesized value
    #[serde(rename = "E304")]
    BCSSerializationFailed,

    // =========================================================================
    // Execution Errors (4xx)
    // =========================================================================
    /// E401: VM harness setup failed
    #[serde(rename = "E401")]
    VMSetupFailed,

    /// E402: Constructor execution aborted
    #[serde(rename = "E402")]
    ConstructorAborted,

    /// E403: Target function execution aborted
    #[serde(rename = "E403")]
    TargetAborted,

    /// E404: Unsupported native function called (crypto, random, etc.)
    #[serde(rename = "E404")]
    UnsupportedNative,

    // =========================================================================
    // Validation Errors (5xx)
    // =========================================================================
    /// E501: No target package modules were accessed during execution
    #[serde(rename = "E501")]
    NoTargetModulesAccessed,

    /// E502: Return type does not match expected type
    #[serde(rename = "E502")]
    ReturnTypeMismatch,
}

impl ErrorCode {
    /// Get the numeric code (e.g., 1, 101, 201, etc.)
    pub fn numeric_code(&self) -> u16 {
        match self {
            // Build (0xx)
            ErrorCode::ModuleAddressUndefined => 1,
            ErrorCode::InvalidManifest => 2,
            ErrorCode::ImportResolutionFailed => 3,
            ErrorCode::TypeSyntaxError => 4,
            ErrorCode::InvalidEntrySignature => 5,
            ErrorCode::CompileTimeAbilityError => 6,
            // Resolution (1xx)
            ErrorCode::ModuleNotFound => 101,
            ErrorCode::FunctionNotFound => 102,
            ErrorCode::NotCallable => 103,
            // TypeCheck (2xx)
            ErrorCode::TypeMismatch => 201,
            ErrorCode::AbilityViolation => 202,
            ErrorCode::GenericBoundsViolation => 203,
            ErrorCode::RecursiveType => 204,
            ErrorCode::UnknownType => 205,
            // Synthesis (3xx)
            ErrorCode::NoConstructor => 301,
            ErrorCode::ChainTooDeep => 302,
            ErrorCode::UnsupportedConstructorParam => 303,
            ErrorCode::BCSSerializationFailed => 304,
            // Execution (4xx)
            ErrorCode::VMSetupFailed => 401,
            ErrorCode::ConstructorAborted => 402,
            ErrorCode::TargetAborted => 403,
            ErrorCode::UnsupportedNative => 404,
            // Validation (5xx)
            ErrorCode::NoTargetModulesAccessed => 501,
            ErrorCode::ReturnTypeMismatch => 502,
        }
    }

    /// Get the phase this error belongs to
    pub fn phase(&self) -> Phase {
        match self.numeric_code() / 100 {
            0 => Phase::Build,
            1 => Phase::Resolution,
            2 => Phase::TypeCheck,
            3 => Phase::Synthesis,
            4 => Phase::Execution,
            // 5xx and any future codes default to Validation phase
            _ => Phase::Validation,
        }
    }

    /// Get a short description of this error
    pub fn description(&self) -> &'static str {
        match self {
            // Build
            ErrorCode::ModuleAddressUndefined => "module address not defined in Move.toml",
            ErrorCode::InvalidManifest => "invalid Move.toml syntax",
            ErrorCode::ImportResolutionFailed => "import resolution failed (use statement)",
            ErrorCode::TypeSyntaxError => "type syntax error (qualified path in field)",
            ErrorCode::InvalidEntrySignature => "invalid entry function signature",
            ErrorCode::CompileTimeAbilityError => "ability constraint error at compile time",
            // Resolution
            ErrorCode::ModuleNotFound => "module not found in bytecode corpus",
            ErrorCode::FunctionNotFound => "function not found in module",
            ErrorCode::NotCallable => "function is not public or entry",
            // TypeCheck
            ErrorCode::TypeMismatch => "argument type does not match parameter type",
            ErrorCode::AbilityViolation => "type ability constraint violated",
            ErrorCode::GenericBoundsViolation => "generic type parameter bounds not satisfied",
            ErrorCode::RecursiveType => "recursive type detected",
            ErrorCode::UnknownType => "unknown type (struct not found)",
            // Synthesis
            ErrorCode::NoConstructor => "no constructor found for type",
            ErrorCode::ChainTooDeep => "constructor chain exceeds maximum depth",
            ErrorCode::UnsupportedConstructorParam => "constructor has unsupported parameter",
            ErrorCode::BCSSerializationFailed => "BCS serialization failed",
            // Execution
            ErrorCode::VMSetupFailed => "VM harness setup failed",
            ErrorCode::ConstructorAborted => "constructor execution aborted",
            ErrorCode::TargetAborted => "target function execution aborted",
            ErrorCode::UnsupportedNative => "unsupported native function called",
            // Validation
            ErrorCode::NoTargetModulesAccessed => "no target modules accessed",
            ErrorCode::ReturnTypeMismatch => "return type mismatch",
        }
    }

    /// Get the string code (e.g., "E001", "E101", "E201", etc.)
    pub fn code_string(&self) -> String {
        let code = self.numeric_code();
        if code < 100 {
            format!("E{:03}", code) // E001, E002, etc.
        } else {
            format!("E{}", code) // E101, E201, etc.
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code_string(), self.description())
    }
}

/// Complete failure information for the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    /// Which phase the failure occurred in
    pub phase: Phase,
    /// Specific error code
    pub code: ErrorCode,
    /// Human-readable error message with context
    pub message: String,
    /// Optional additional context (e.g., which type failed, which function)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<FailureContext>,
    /// Context stack showing the chain of operations that led to this failure.
    ///
    /// This provides a "breadcrumb trail" for debugging:
    /// - synthesizing argument 2 for function transfer
    /// - constructing type 0x2::coin::Coin<SUI>
    /// - calling constructor new_coin
    #[serde(skip_serializing_if = "context_stack_is_empty", default)]
    pub context_stack: ContextStack,
}

/// Additional context for a failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureContext {
    /// Module where failure occurred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Function where failure occurred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// Type involved in failure
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Parameter index (0-based)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param_index: Option<usize>,
}

// =============================================================================
// Error Context Stacking (P2.5 Enhancement)
// =============================================================================

/// A single frame in the error context stack.
///
/// Each frame represents one level of nesting in the operation that failed.
/// For example, when synthesizing an argument:
/// - Frame 1: "synthesizing argument 2 for function transfer"
/// - Frame 2: "constructing type 0x2::coin::Coin<SUI>"
/// - Frame 3: "calling constructor new_coin"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextFrame {
    /// What operation was being performed
    pub operation: String,
    /// Phase where this operation occurs
    pub phase: Phase,
    /// Optional additional details (type names, indices, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl ContextFrame {
    /// Create a new context frame
    pub fn new(phase: Phase, operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            phase,
            details: None,
        }
    }

    /// Add details to this frame
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Create a resolution phase frame
    pub fn resolution(operation: impl Into<String>) -> Self {
        Self::new(Phase::Resolution, operation)
    }

    /// Create a type check phase frame
    pub fn type_check(operation: impl Into<String>) -> Self {
        Self::new(Phase::TypeCheck, operation)
    }

    /// Create a synthesis phase frame
    pub fn synthesis(operation: impl Into<String>) -> Self {
        Self::new(Phase::Synthesis, operation)
    }

    /// Create an execution phase frame
    pub fn execution(operation: impl Into<String>) -> Self {
        Self::new(Phase::Execution, operation)
    }
}

impl fmt::Display for ContextFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref details) = self.details {
            write!(f, "{} ({})", self.operation, details)
        } else {
            write!(f, "{}", self.operation)
        }
    }
}

/// Stack of context frames for rich error reporting.
///
/// This provides a "breadcrumb trail" showing exactly how an error occurred,
/// similar to a call stack but for logical operations across phases.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextStack {
    /// Frames from outermost (first) to innermost (last)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub frames: Vec<ContextFrame>,
}

impl ContextStack {
    /// Create an empty context stack
    pub fn new() -> Self {
        Self { frames: Vec::new() }
    }

    /// Push a new frame onto the stack
    pub fn push(&mut self, frame: ContextFrame) {
        self.frames.push(frame);
    }

    /// Push a simple operation string
    pub fn push_operation(&mut self, phase: Phase, operation: impl Into<String>) {
        self.frames.push(ContextFrame::new(phase, operation));
    }

    /// Check if the stack is empty
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Get the number of frames
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Format as a multi-line trace
    pub fn format_trace(&self) -> String {
        if self.frames.is_empty() {
            return String::new();
        }

        let mut lines = Vec::with_capacity(self.frames.len());
        for (i, frame) in self.frames.iter().enumerate() {
            let indent = "  ".repeat(i);
            lines.push(format!("{}→ {}", indent, frame));
        }
        lines.join("\n")
    }
}

impl fmt::Display for ContextStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.frames.is_empty() {
            return Ok(());
        }

        // Single-line format: "while X → while Y → while Z"
        let parts: Vec<String> = self.frames.iter().map(|f| format!("{}", f)).collect();
        write!(f, "while {}", parts.join(" → "))
    }
}

impl Failure {
    /// Create a new failure with just the essentials
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            phase: code.phase(),
            code,
            message: message.into(),
            context: None,
            context_stack: ContextStack::default(),
        }
    }

    /// Create a failure with context
    pub fn with_context(
        code: ErrorCode,
        message: impl Into<String>,
        context: FailureContext,
    ) -> Self {
        Self {
            phase: code.phase(),
            code,
            message: message.into(),
            context: Some(context),
            context_stack: ContextStack::default(),
        }
    }

    /// Create a failure with a context stack
    pub fn with_stack(code: ErrorCode, message: impl Into<String>, stack: ContextStack) -> Self {
        Self {
            phase: code.phase(),
            code,
            message: message.into(),
            context: None,
            context_stack: stack,
        }
    }

    /// Add context to an existing failure
    pub fn add_context(mut self, context: FailureContext) -> Self {
        self.context = Some(context);
        self
    }

    /// Push a context frame onto the stack
    pub fn push_frame(mut self, frame: ContextFrame) -> Self {
        self.context_stack.push(frame);
        self
    }

    /// Push a context frame at the front (for wrapping errors from inner operations)
    pub fn prepend_frame(mut self, frame: ContextFrame) -> Self {
        self.context_stack.frames.insert(0, frame);
        self
    }

    /// Add context stack from another failure (for error chaining)
    pub fn chain_stack(mut self, other_stack: ContextStack) -> Self {
        for frame in other_stack.frames {
            self.context_stack.push(frame);
        }
        self
    }

    /// Format a detailed error message including context stack
    pub fn detailed_message(&self) -> String {
        let mut msg = format!(
            "[{}] {}: {}",
            self.phase,
            self.code.code_string(),
            self.message
        );

        if !self.context_stack.is_empty() {
            msg.push_str("\n\nContext trace:\n");
            msg.push_str(&self.context_stack.format_trace());
        }

        if let Some(ref ctx) = self.context {
            let mut parts = Vec::new();
            if let Some(ref module) = ctx.module {
                parts.push(format!("module: {}", module));
            }
            if let Some(ref func) = ctx.function {
                parts.push(format!("function: {}", func));
            }
            if let Some(ref type_name) = ctx.type_name {
                parts.push(format!("type: {}", type_name));
            }
            if let Some(idx) = ctx.param_index {
                parts.push(format!("param #{}", idx));
            }
            if !parts.is_empty() {
                msg.push_str(&format!("\n\nAt: {}", parts.join(", ")));
            }
        }

        msg
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.phase,
            self.code.code_string(),
            self.message
        )
    }
}

// =============================================================================
// Scoring Rubric (P1 Improvement)
// =============================================================================

/// Scoring criteria for partial credit evaluation.
///
/// Instead of binary pass/fail, this provides more granular scoring
/// to distinguish between models that fail early vs. late in the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScoringCriteria {
    /// Whether the code compiled successfully (0.25 points)
    pub compiles: bool,
    /// Whether the code imports/uses the target package (0.25 points)
    pub imports_target: bool,
    /// Whether target types were successfully created (0.25 points)
    pub creates_target_type: bool,
    /// Whether execution completed without errors (0.25 points)
    pub executes_cleanly: bool,
}

impl ScoringCriteria {
    /// Calculate the total score (0.0 to 1.0)
    pub fn score(&self) -> f64 {
        let mut score = 0.0;
        if self.compiles {
            score += 0.25;
        }
        if self.imports_target {
            score += 0.25;
        }
        if self.creates_target_type {
            score += 0.25;
        }
        if self.executes_cleanly {
            score += 0.25;
        }
        score
    }

    /// Get the furthest phase reached in the pipeline
    pub fn phase_reached(&self) -> Phase {
        if self.executes_cleanly {
            Phase::Validation
        } else if self.creates_target_type {
            Phase::Execution
        } else if self.imports_target {
            Phase::Synthesis
        } else if self.compiles {
            Phase::Resolution
        } else {
            Phase::Build
        }
    }

    /// Create criteria from a phase (for when execution stops at a given phase)
    pub fn from_phase(phase: Phase) -> Self {
        match phase {
            Phase::Build => Self::default(),
            Phase::Resolution => Self {
                compiles: true,
                ..Default::default()
            },
            Phase::TypeCheck | Phase::Synthesis => Self {
                compiles: true,
                imports_target: true,
                ..Default::default()
            },
            Phase::Execution => Self {
                compiles: true,
                imports_target: true,
                creates_target_type: true,
                ..Default::default()
            },
            Phase::Validation => Self {
                compiles: true,
                imports_target: true,
                creates_target_type: true,
                executes_cleanly: true,
            },
        }
    }
}

// =============================================================================
// Inhabitation Metrics
// =============================================================================

/// Metrics about type inhabitation success.
///
/// Tracks which types from the target package were successfully created/used.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InhabitationMetrics {
    /// Total number of types in the target package interface
    pub target_types_total: usize,
    /// Number of target types that were successfully inhabited
    pub target_types_inhabited: usize,
    /// List of type names that were successfully inhabited
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub inhabited_types: Vec<String>,
    /// List of type names that could not be inhabited (with reason)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub uninhabited_types: Vec<UninhabitedType>,
    /// Number of entry functions in target package
    pub target_entry_functions: usize,
    /// Number of entry functions successfully called
    pub entry_functions_called: usize,
    /// Types used from stdlib (for context, not counted as inhabitation)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stdlib_types_used: Vec<String>,
}

/// A type that could not be inhabited, with reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninhabitedType {
    /// Fully qualified type name
    pub type_name: String,
    /// Why it couldn't be inhabited
    pub reason: UninhabitedReason,
    /// Additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Reason why a type could not be inhabited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UninhabitedReason {
    /// No public constructor available
    NoConstructor,
    /// Constructor requires unsupported parameter type
    UnsupportedParam,
    /// Constructor chain too deep
    ChainTooDeep,
    /// Type is recursive
    RecursiveType,
    /// Ability constraints prevent construction
    AbilityConstraint,
    /// Type requires runtime values (Clock, Random, etc.)
    RequiresRuntimeValue,
    /// Unknown/other reason
    Unknown,
}

impl InhabitationMetrics {
    /// Calculate inhabitation rate (0.0 to 1.0)
    pub fn inhabitation_rate(&self) -> f64 {
        if self.target_types_total == 0 {
            0.0
        } else {
            self.target_types_inhabited as f64 / self.target_types_total as f64
        }
    }

    /// Calculate entry function coverage (0.0 to 1.0)
    pub fn entry_coverage(&self) -> f64 {
        if self.target_entry_functions == 0 {
            0.0
        } else {
            self.entry_functions_called as f64 / self.target_entry_functions as f64
        }
    }
}

// =============================================================================
// Execution Trace
// =============================================================================

/// Execution trace for debugging and analysis.
///
/// Records what happened during VM execution, including call stack at abort.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionTrace {
    /// Whether execution was attempted
    pub execution_attempted: bool,
    /// Modules that were loaded during execution
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub modules_loaded: Vec<String>,
    /// Functions that were called (in order)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub functions_called: Vec<FunctionCall>,
    /// Call stack at point of failure (if execution aborted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abort_info: Option<AbortInfo>,
    /// Gas used during execution (if tracked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_used: Option<u64>,
    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Information about a function call during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Module containing the function
    pub module: String,
    /// Function name
    pub function: String,
    /// Type arguments (if generic)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub type_args: Vec<String>,
    /// Whether the call succeeded
    pub succeeded: bool,
    /// Error message if call failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Detailed information about an abort during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortInfo {
    /// The abort code (if MoveAbort)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abort_code: Option<u64>,
    /// Module where abort occurred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abort_location: Option<String>,
    /// Call stack at time of abort (deepest first)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub call_stack: Vec<StackFrame>,
    /// Human-readable abort message
    pub message: String,
    /// Whether this was a known/expected abort (e.g., E_NOT_SUPPORTED)
    pub is_expected: bool,
    /// Category of the abort for analysis
    pub category: AbortCategory,
}

/// A frame in the call stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    /// Module address and name (e.g., "0x2::object")
    pub module: String,
    /// Function name
    pub function: String,
    /// Instruction offset within function (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction_offset: Option<u64>,
}

/// Category of abort for easier analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortCategory {
    /// Abort from unsupported native function (crypto, random, etc.)
    UnsupportedNative,
    /// Assertion failure in user code
    AssertionFailed,
    /// Object/ownership error
    ObjectError,
    /// Arithmetic error (overflow, divide by zero)
    ArithmeticError,
    /// Vector bounds error
    VectorBoundsError,
    /// Type/ability error at runtime
    TypeError,
    /// Out of gas
    OutOfGas,
    /// Unknown/uncategorized abort
    Unknown,
}

impl AbortInfo {
    /// Create abort info from a MoveAbort error
    pub fn from_move_abort(code: u64, location: Option<String>, message: String) -> Self {
        let is_expected = code == E_NOT_SUPPORTED;
        let category = Self::categorize_abort(code, &message);

        Self {
            abort_code: Some(code),
            abort_location: location,
            call_stack: Vec::new(),
            message,
            is_expected,
            category,
        }
    }

    /// Categorize an abort based on code and message
    fn categorize_abort(code: u64, message: &str) -> AbortCategory {
        // Check for known abort codes
        if code == E_NOT_SUPPORTED {
            return AbortCategory::UnsupportedNative;
        }

        // Check message for patterns
        let msg_lower = message.to_lowercase();
        if msg_lower.contains("assert") {
            AbortCategory::AssertionFailed
        } else if msg_lower.contains("object") || msg_lower.contains("ownership") {
            AbortCategory::ObjectError
        } else if msg_lower.contains("overflow")
            || msg_lower.contains("underflow")
            || msg_lower.contains("divide")
        {
            AbortCategory::ArithmeticError
        } else if msg_lower.contains("vector") || msg_lower.contains("index") {
            AbortCategory::VectorBoundsError
        } else if msg_lower.contains("type") || msg_lower.contains("ability") {
            AbortCategory::TypeError
        } else if msg_lower.contains("gas") {
            AbortCategory::OutOfGas
        } else {
            AbortCategory::Unknown
        }
    }

    /// Add a stack frame to the call stack
    pub fn push_frame(&mut self, module: String, function: String, offset: Option<u64>) {
        self.call_stack.push(StackFrame {
            module,
            function,
            instruction_offset: offset,
        });
    }
}

impl ExecutionTrace {
    /// Record a function call
    pub fn record_call(&mut self, module: String, function: String, type_args: Vec<String>) {
        self.functions_called.push(FunctionCall {
            module,
            function,
            type_args,
            succeeded: true,
            error: None,
        });
    }

    /// Mark the last call as failed
    pub fn mark_last_failed(&mut self, error: String) {
        if let Some(last) = self.functions_called.last_mut() {
            last.succeeded = false;
            last.error = Some(error);
        }
    }
}

/// Complete evaluation result with scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Whether the evaluation passed completely
    pub ok: bool,
    /// Numeric score from 0.0 to 1.0
    pub score: f64,
    /// Detailed scoring criteria
    pub criteria: ScoringCriteria,
    /// Failure information if not ok (None if passed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    /// Partial credit explanation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_credit_reason: Option<String>,
    /// Inhabitation metrics (what types were successfully inhabited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inhabitation_metrics: Option<InhabitationMetrics>,
    /// Execution trace (if execution was attempted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_trace: Option<ExecutionTrace>,
}

impl EvaluationResult {
    /// Create a successful result
    pub fn success() -> Self {
        Self {
            ok: true,
            score: 1.0,
            criteria: ScoringCriteria {
                compiles: true,
                imports_target: true,
                creates_target_type: true,
                executes_cleanly: true,
            },
            failure: None,
            partial_credit_reason: None,
            inhabitation_metrics: None,
            execution_trace: None,
        }
    }

    /// Create a successful result with metrics and trace
    pub fn success_with_details(metrics: InhabitationMetrics, trace: ExecutionTrace) -> Self {
        Self {
            ok: true,
            score: 1.0,
            criteria: ScoringCriteria {
                compiles: true,
                imports_target: true,
                creates_target_type: true,
                executes_cleanly: true,
            },
            failure: None,
            partial_credit_reason: None,
            inhabitation_metrics: Some(metrics),
            execution_trace: Some(trace),
        }
    }

    /// Create a failed result with partial credit
    pub fn failed(failure: Failure) -> Self {
        let criteria = ScoringCriteria::from_phase(failure.phase);
        let score = criteria.score();
        let phase_reached = criteria.phase_reached();

        let partial_credit_reason = if score > 0.0 {
            Some(format!("Reached {} phase before failure", phase_reached))
        } else {
            None
        };

        Self {
            ok: false,
            score,
            criteria,
            failure: Some(failure),
            partial_credit_reason,
            inhabitation_metrics: None,
            execution_trace: None,
        }
    }

    /// Create a failed result with custom criteria
    pub fn failed_with_criteria(failure: Failure, criteria: ScoringCriteria) -> Self {
        let score = criteria.score();
        let phase_reached = criteria.phase_reached();

        let partial_credit_reason = if score > 0.0 {
            Some(format!("Reached {} phase before failure", phase_reached))
        } else {
            None
        };

        Self {
            ok: false,
            score,
            criteria,
            failure: Some(failure),
            partial_credit_reason,
            inhabitation_metrics: None,
            execution_trace: None,
        }
    }

    /// Create a failed result with full details
    pub fn failed_with_details(
        failure: Failure,
        criteria: ScoringCriteria,
        metrics: Option<InhabitationMetrics>,
        trace: Option<ExecutionTrace>,
    ) -> Self {
        let score = criteria.score();
        let phase_reached = criteria.phase_reached();

        let partial_credit_reason = if score > 0.0 {
            Some(format!("Reached {} phase before failure", phase_reached))
        } else {
            None
        };

        Self {
            ok: false,
            score,
            criteria,
            failure: Some(failure),
            partial_credit_reason,
            inhabitation_metrics: metrics,
            execution_trace: trace,
        }
    }

    /// Add inhabitation metrics to an existing result
    pub fn with_metrics(mut self, metrics: InhabitationMetrics) -> Self {
        self.inhabitation_metrics = Some(metrics);
        self
    }

    /// Add execution trace to an existing result
    pub fn with_trace(mut self, trace: ExecutionTrace) -> Self {
        self.execution_trace = Some(trace);
        self
    }
}

// =============================================================================
// Legacy Compatibility: FailureStage (v0.3.x)
// =============================================================================

/// Legacy failure stage enum for backwards compatibility.
///
/// **Deprecated**: Use `ErrorCode` instead for new code.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailureStage {
    /// A1: Target function doesn't exist, isn't public, or module not found
    A1,
    /// A2: Cannot resolve type layout for parameter (unknown struct, recursive type)
    A2,
    /// A3: Cannot synthesize value for parameter (no constructor, no default generator)
    A3,
    /// A4: Reserved for future use
    A4,
    /// A5: Generic type parameter index out of bounds
    A5,
    /// B1: VM harness creation failed or constructor chaining failed
    B1,
    /// B2: Function execution aborted (assertion, unsupported native, runtime error)
    B2,
}

impl FailureStage {
    /// Get a human-readable description of this failure stage.
    pub fn description(&self) -> &'static str {
        match self {
            FailureStage::A1 => "target validation failed (function not found or not callable)",
            FailureStage::A2 => "type layout resolution failed (unknown or recursive type)",
            FailureStage::A3 => "value synthesis failed (no constructor or default available)",
            FailureStage::A4 => "reserved stage (unused)",
            FailureStage::A5 => "type parameter out of bounds",
            FailureStage::B1 => "VM setup or constructor execution failed",
            FailureStage::B2 => "function execution aborted",
        }
    }

    /// Get the tier (A or B) for this stage.
    pub fn tier(&self) -> &'static str {
        match self {
            FailureStage::A1
            | FailureStage::A2
            | FailureStage::A3
            | FailureStage::A4
            | FailureStage::A5 => "A (argument synthesis)",
            FailureStage::B1 | FailureStage::B2 => "B (execution)",
        }
    }

    /// Convert legacy FailureStage to new ErrorCode.
    ///
    /// Note: This is a lossy conversion since the new taxonomy is more precise.
    /// Use the most common mapping for each stage.
    pub fn to_error_code(&self) -> ErrorCode {
        match self {
            FailureStage::A1 => ErrorCode::FunctionNotFound,
            FailureStage::A2 => ErrorCode::UnknownType,
            FailureStage::A3 => ErrorCode::NoConstructor,
            FailureStage::A4 => ErrorCode::NoConstructor, // Unused, map to something
            FailureStage::A5 => ErrorCode::GenericBoundsViolation,
            FailureStage::B1 => ErrorCode::ConstructorAborted,
            FailureStage::B2 => ErrorCode::TargetAborted,
        }
    }
}

impl From<FailureStage> for ErrorCode {
    fn from(stage: FailureStage) -> Self {
        stage.to_error_code()
    }
}

impl From<ErrorCode> for FailureStage {
    fn from(code: ErrorCode) -> Self {
        match code {
            // Build errors -> A1 (closest match: code didn't even compile)
            ErrorCode::ModuleAddressUndefined
            | ErrorCode::InvalidManifest
            | ErrorCode::ImportResolutionFailed
            | ErrorCode::TypeSyntaxError
            | ErrorCode::InvalidEntrySignature
            | ErrorCode::CompileTimeAbilityError => FailureStage::A1,
            // Resolution -> A1
            ErrorCode::ModuleNotFound | ErrorCode::FunctionNotFound | ErrorCode::NotCallable => {
                FailureStage::A1
            }
            // TypeCheck -> A2 or A5
            ErrorCode::TypeMismatch
            | ErrorCode::AbilityViolation
            | ErrorCode::RecursiveType
            | ErrorCode::UnknownType => FailureStage::A2,
            ErrorCode::GenericBoundsViolation => FailureStage::A5,
            // Synthesis -> A3
            ErrorCode::NoConstructor
            | ErrorCode::ChainTooDeep
            | ErrorCode::UnsupportedConstructorParam
            | ErrorCode::BCSSerializationFailed => FailureStage::A3,
            // Execution -> B1 or B2
            ErrorCode::VMSetupFailed | ErrorCode::ConstructorAborted => FailureStage::B1,
            ErrorCode::TargetAborted | ErrorCode::UnsupportedNative => FailureStage::B2,
            // Validation -> B2 (closest match)
            ErrorCode::NoTargetModulesAccessed | ErrorCode::ReturnTypeMismatch => FailureStage::B2,
        }
    }
}

// =============================================================================
// Native Function Support (unchanged from v0.3.x)
// =============================================================================

/// Error code used by native functions that cannot be simulated.
///
/// When a function calls an unsupported native (crypto verification, randomness, zklogin),
/// the native aborts with this error code. The runner detects this and provides a
/// user-friendly error message.
pub const E_NOT_SUPPORTED: u64 = 1000;

/// Categories of native functions and their support status.
///
/// This is the source of truth for what natives are supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCategory {
    /// Real implementations from move-stdlib-natives
    RealImpl,
    /// Safe mocks that return placeholder values
    SafeMock,
    /// Full support via ObjectRuntime VM extension
    VmExtension,
    /// Permissive mocks that return success values (crypto verification returns true, etc.)
    PermissiveMock,
    /// Aborts with E_NOT_SUPPORTED - cannot be simulated
    Unsupported,
}

/// Information about a native function module.
pub struct NativeModuleInfo {
    pub module: &'static str,
    pub category: NativeCategory,
    pub description: &'static str,
}

/// Get information about native function support.
///
/// Returns a list of all native modules and their support status.
pub fn native_support_info() -> Vec<NativeModuleInfo> {
    vec![
        // Category A: Real implementations
        NativeModuleInfo {
            module: "0x1::vector",
            category: NativeCategory::RealImpl,
            description: "Vector operations (empty, length, borrow, push, pop, etc.)",
        },
        NativeModuleInfo {
            module: "0x1::bcs",
            category: NativeCategory::RealImpl,
            description: "BCS serialization",
        },
        NativeModuleInfo {
            module: "0x1::hash",
            category: NativeCategory::RealImpl,
            description: "SHA2-256, SHA3-256 hashing",
        },
        NativeModuleInfo {
            module: "0x1::string",
            category: NativeCategory::RealImpl,
            description: "UTF-8 string operations",
        },
        NativeModuleInfo {
            module: "0x1::type_name",
            category: NativeCategory::RealImpl,
            description: "Type name reflection",
        },
        NativeModuleInfo {
            module: "0x1::debug",
            category: NativeCategory::RealImpl,
            description: "Debug printing (no-op in production)",
        },
        NativeModuleInfo {
            module: "0x1::signer",
            category: NativeCategory::RealImpl,
            description: "Signer address extraction",
        },
        // Category B: Safe mocks
        NativeModuleInfo {
            module: "0x2::tx_context",
            category: NativeCategory::SafeMock,
            description: "Transaction context (sender, epoch, etc.)",
        },
        NativeModuleInfo {
            module: "0x2::object",
            category: NativeCategory::SafeMock,
            description: "Object ID operations",
        },
        NativeModuleInfo {
            module: "0x2::transfer",
            category: NativeCategory::SafeMock,
            description: "Object transfers (no-op, ownership not tracked)",
        },
        NativeModuleInfo {
            module: "0x2::event",
            category: NativeCategory::SafeMock,
            description: "Event emission (no-op)",
        },
        NativeModuleInfo {
            module: "0x2::types",
            category: NativeCategory::SafeMock,
            description: "OTW type checking (real implementation)",
        },
        // Category: VM Extension (full support)
        NativeModuleInfo {
            module: "0x2::dynamic_field",
            category: NativeCategory::VmExtension,
            description: "Dynamic field operations (full support via ObjectRuntime)",
        },
        // Category C: Permissive crypto mocks (return success values)
        NativeModuleInfo {
            module: "0x2::bls12381",
            category: NativeCategory::PermissiveMock,
            description: "BLS12-381 signature verification (returns true)",
        },
        NativeModuleInfo {
            module: "0x2::ecdsa_k1",
            category: NativeCategory::PermissiveMock,
            description: "ECDSA secp256k1 signature verification (returns true)",
        },
        NativeModuleInfo {
            module: "0x2::ecdsa_r1",
            category: NativeCategory::PermissiveMock,
            description: "ECDSA secp256r1 signature verification (returns true)",
        },
        NativeModuleInfo {
            module: "0x2::ed25519",
            category: NativeCategory::PermissiveMock,
            description: "Ed25519 signature verification (returns true)",
        },
        // Category D: Unsupported (abort with E_NOT_SUPPORTED)
        NativeModuleInfo {
            module: "0x2::groth16",
            category: NativeCategory::Unsupported,
            description: "Groth16 ZK proof verification",
        },
        NativeModuleInfo {
            module: "0x2::poseidon",
            category: NativeCategory::Unsupported,
            description: "Poseidon hash for ZK circuits",
        },
        NativeModuleInfo {
            module: "0x2::zklogin",
            category: NativeCategory::Unsupported,
            description: "zkLogin verification",
        },
        NativeModuleInfo {
            module: "0x2::random",
            category: NativeCategory::PermissiveMock,
            description: "On-chain randomness (deterministic mock)",
        },
        NativeModuleInfo {
            module: "0x2::ecvrf",
            category: NativeCategory::PermissiveMock,
            description: "Verifiable random function (returns true)",
        },
        NativeModuleInfo {
            module: "0x2::config",
            category: NativeCategory::Unsupported,
            description: "System configuration",
        },
        NativeModuleInfo {
            module: "0x3::nitro_attestation",
            category: NativeCategory::Unsupported,
            description: "AWS Nitro attestation verification",
        },
    ]
}

/// Format an error message for unsupported native function.
///
/// This is used by the runner when it detects error code 1000.
pub fn unsupported_native_error_message() -> String {
    let unsupported: Vec<_> = native_support_info()
        .into_iter()
        .filter(|n| n.category == NativeCategory::Unsupported)
        .map(|n| n.module)
        .collect();

    format!(
        "execution failed: unsupported native function (error {}). \
         This function uses a native that cannot be simulated. \
         Unsupported modules: {}",
        E_NOT_SUPPORTED,
        unsupported.join(", ")
    )
}

/// Check if an error message indicates an unsupported native function.
pub fn is_unsupported_native_error(error: &str) -> bool {
    error.contains(&E_NOT_SUPPORTED.to_string()) && error.contains("MoveAbort")
}

// =============================================================================
// Helper functions for serde
// =============================================================================

/// Helper for skip_serializing_if for context_stack
fn context_stack_is_empty(stack: &ContextStack) -> bool {
    stack.is_empty()
}

// =============================================================================
// Error Extension Traits for Consistent Error Handling
// =============================================================================

/// Extension trait for adding phase context to `anyhow::Result`.
///
/// This trait provides a consistent way to add phase and context information
/// to errors throughout the codebase, ensuring all error messages follow
/// the same format: `[phase] operation: details`
///
/// # Example
///
/// ```ignore
/// use crate::benchmark::errors::{Phase, PhaseResultExt};
///
/// fn load_module(path: &str) -> anyhow::Result<Module> {
///     let bytes = std::fs::read(path)
///         .with_phase(Phase::Build, "reading module file")?;
///     // ...
/// }
/// ```
pub trait PhaseResultExt<T> {
    /// Add phase context to an error.
    ///
    /// Transforms the error message to: `[phase] context: original_error`
    fn with_phase(self, phase: Phase, context: &str) -> anyhow::Result<T>;

    /// Add phase context with formatted message.
    fn with_phase_context<F>(self, phase: Phase, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> String;
}

impl<T, E: Into<anyhow::Error>> PhaseResultExt<T> for Result<T, E> {
    fn with_phase(self, phase: Phase, context: &str) -> anyhow::Result<T> {
        self.map_err(|e| {
            let err = e.into();
            anyhow::anyhow!("[{}] {}: {}", phase.short_name(), context, err)
        })
    }

    fn with_phase_context<F>(self, phase: Phase, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| {
            let err = e.into();
            anyhow::anyhow!("[{}] {}: {}", phase.short_name(), f(), err)
        })
    }
}

/// Extension trait for Option types to add phase context.
pub trait PhaseOptionExt<T> {
    /// Convert None to an error with phase context.
    fn ok_or_phase(self, phase: Phase, context: &str) -> anyhow::Result<T>;

    /// Convert None to an error with formatted phase context.
    fn ok_or_phase_with<F>(self, phase: Phase, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> String;
}

impl<T> PhaseOptionExt<T> for Option<T> {
    fn ok_or_phase(self, phase: Phase, context: &str) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("[{}] {}", phase.short_name(), context))
    }

    fn ok_or_phase_with<F>(self, phase: Phase, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> String,
    {
        self.ok_or_else(|| anyhow::anyhow!("[{}] {}", phase.short_name(), f()))
    }
}

/// Create an error with phase context.
///
/// # Example
///
/// ```ignore
/// use crate::benchmark::errors::{Phase, phase_error};
///
/// return Err(phase_error(Phase::Execution, "VM crashed during execution"));
/// ```
#[inline]
pub fn phase_error(phase: Phase, message: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("[{}] {}", phase.short_name(), message)
}

/// Create an error with phase context and formatted message.
#[macro_export]
macro_rules! phase_err {
    ($phase:expr, $($arg:tt)*) => {
        $crate::benchmark::errors::phase_error($phase, format!($($arg)*))
    };
}

/// Bail with phase context.
#[macro_export]
macro_rules! phase_bail {
    ($phase:expr, $($arg:tt)*) => {
        return Err($crate::phase_err!($phase, $($arg)*))
    };
}

/// Ensure a condition is true, or bail with phase context.
#[macro_export]
macro_rules! phase_ensure {
    ($cond:expr, $phase:expr, $($arg:tt)*) => {
        if !$cond {
            $crate::phase_bail!($phase, $($arg)*);
        }
    };
}

// Re-export macros at module level for convenience
pub use crate::phase_bail;
pub use crate::phase_ensure;
pub use crate::phase_err;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_numeric() {
        assert_eq!(ErrorCode::ModuleNotFound.numeric_code(), 101);
        assert_eq!(ErrorCode::TypeMismatch.numeric_code(), 201);
        assert_eq!(ErrorCode::NoConstructor.numeric_code(), 301);
        assert_eq!(ErrorCode::VMSetupFailed.numeric_code(), 401);
        assert_eq!(ErrorCode::NoTargetModulesAccessed.numeric_code(), 501);
    }

    #[test]
    fn test_error_code_phase() {
        assert_eq!(ErrorCode::ModuleNotFound.phase(), Phase::Resolution);
        assert_eq!(ErrorCode::TypeMismatch.phase(), Phase::TypeCheck);
        assert_eq!(ErrorCode::NoConstructor.phase(), Phase::Synthesis);
        assert_eq!(ErrorCode::TargetAborted.phase(), Phase::Execution);
        assert_eq!(
            ErrorCode::NoTargetModulesAccessed.phase(),
            Phase::Validation
        );
    }

    #[test]
    fn test_error_code_string() {
        assert_eq!(ErrorCode::ModuleNotFound.code_string(), "E101");
        assert_eq!(ErrorCode::UnsupportedNative.code_string(), "E404");
    }

    #[test]
    fn test_failure_creation() {
        let failure = Failure::new(ErrorCode::ModuleNotFound, "module foo not found");
        assert_eq!(failure.phase, Phase::Resolution);
        assert_eq!(failure.code, ErrorCode::ModuleNotFound);
    }

    #[test]
    fn test_failure_with_context() {
        let ctx = FailureContext {
            module: Some("0x1::test".to_string()),
            function: Some("do_thing".to_string()),
            type_name: None,
            param_index: Some(0),
        };
        let failure = Failure::with_context(ErrorCode::TypeMismatch, "expected u64, got bool", ctx);
        assert!(failure.context.is_some());
        assert_eq!(failure.context.unwrap().param_index, Some(0));
    }

    #[test]
    fn test_legacy_conversion_to_error_code() {
        assert_eq!(
            FailureStage::A1.to_error_code(),
            ErrorCode::FunctionNotFound
        );
        assert_eq!(FailureStage::A2.to_error_code(), ErrorCode::UnknownType);
        assert_eq!(FailureStage::A3.to_error_code(), ErrorCode::NoConstructor);
        assert_eq!(FailureStage::B2.to_error_code(), ErrorCode::TargetAborted);
    }

    #[test]
    fn test_legacy_conversion_from_error_code() {
        assert_eq!(
            FailureStage::from(ErrorCode::ModuleNotFound),
            FailureStage::A1
        );
        assert_eq!(
            FailureStage::from(ErrorCode::RecursiveType),
            FailureStage::A2
        );
        assert_eq!(
            FailureStage::from(ErrorCode::NoConstructor),
            FailureStage::A3
        );
        assert_eq!(
            FailureStage::from(ErrorCode::UnsupportedNative),
            FailureStage::B2
        );
    }

    #[test]
    fn test_native_support_info_has_all_categories() {
        let info = native_support_info();
        assert!(info.iter().any(|n| n.category == NativeCategory::RealImpl));
        assert!(info.iter().any(|n| n.category == NativeCategory::SafeMock));
        assert!(info
            .iter()
            .any(|n| n.category == NativeCategory::VmExtension));
        assert!(info
            .iter()
            .any(|n| n.category == NativeCategory::Unsupported));
    }

    #[test]
    fn test_is_unsupported_native_error() {
        assert!(is_unsupported_native_error("VMError: MoveAbort(1000)"));
        assert!(is_unsupported_native_error(
            "execution failed: MoveAbort with code 1000"
        ));
        assert!(!is_unsupported_native_error("VMError: MoveAbort(42)"));
        assert!(!is_unsupported_native_error("some other error"));
    }

    #[test]
    fn test_phase_display() {
        assert_eq!(format!("{}", Phase::Resolution), "resolution");
        assert_eq!(format!("{}", Phase::TypeCheck), "typecheck");
    }

    #[test]
    fn test_error_code_display() {
        let display = format!("{}", ErrorCode::ModuleNotFound);
        assert!(display.contains("E101"));
        assert!(display.contains("module not found"));
    }

    #[test]
    fn test_failure_display() {
        let failure = Failure::new(ErrorCode::TypeMismatch, "expected u64");
        let display = format!("{}", failure);
        assert!(display.contains("[typecheck]"));
        assert!(display.contains("E201"));
        assert!(display.contains("expected u64"));
    }

    #[test]
    fn test_failure_serialization() {
        let failure = Failure::new(ErrorCode::NoConstructor, "no ctor for Foo");
        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("\"phase\":\"synthesis\""));
        assert!(json.contains("\"code\":\"E301\""));
    }

    // =========================================================================
    // Build Phase Error Tests (v0.5.0)
    // =========================================================================

    #[test]
    fn test_build_error_codes() {
        assert_eq!(ErrorCode::ModuleAddressUndefined.numeric_code(), 1);
        assert_eq!(ErrorCode::InvalidManifest.numeric_code(), 2);
        assert_eq!(ErrorCode::ImportResolutionFailed.numeric_code(), 3);
        assert_eq!(ErrorCode::TypeSyntaxError.numeric_code(), 4);
        assert_eq!(ErrorCode::InvalidEntrySignature.numeric_code(), 5);
        assert_eq!(ErrorCode::CompileTimeAbilityError.numeric_code(), 6);
    }

    #[test]
    fn test_build_error_phase() {
        assert_eq!(ErrorCode::ModuleAddressUndefined.phase(), Phase::Build);
        assert_eq!(ErrorCode::InvalidManifest.phase(), Phase::Build);
        assert_eq!(ErrorCode::TypeSyntaxError.phase(), Phase::Build);
    }

    #[test]
    fn test_build_error_code_string() {
        assert_eq!(ErrorCode::ModuleAddressUndefined.code_string(), "E001");
        assert_eq!(ErrorCode::InvalidManifest.code_string(), "E002");
        assert_eq!(ErrorCode::CompileTimeAbilityError.code_string(), "E006");
    }

    #[test]
    fn test_build_phase_display() {
        assert_eq!(format!("{}", Phase::Build), "build");
    }

    // =========================================================================
    // Scoring Rubric Tests (v0.5.0)
    // =========================================================================

    #[test]
    fn test_scoring_criteria_score() {
        let empty = ScoringCriteria::default();
        assert_eq!(empty.score(), 0.0);

        let compiles_only = ScoringCriteria {
            compiles: true,
            ..Default::default()
        };
        assert_eq!(compiles_only.score(), 0.25);

        let full = ScoringCriteria {
            compiles: true,
            imports_target: true,
            creates_target_type: true,
            executes_cleanly: true,
        };
        assert_eq!(full.score(), 1.0);
    }

    #[test]
    fn test_scoring_criteria_phase_reached() {
        let empty = ScoringCriteria::default();
        assert_eq!(empty.phase_reached(), Phase::Build);

        let compiles = ScoringCriteria {
            compiles: true,
            ..Default::default()
        };
        assert_eq!(compiles.phase_reached(), Phase::Resolution);

        let imports = ScoringCriteria {
            compiles: true,
            imports_target: true,
            ..Default::default()
        };
        assert_eq!(imports.phase_reached(), Phase::Synthesis);
    }

    #[test]
    fn test_scoring_criteria_from_phase() {
        let build = ScoringCriteria::from_phase(Phase::Build);
        assert!(!build.compiles);
        assert_eq!(build.score(), 0.0);

        let resolution = ScoringCriteria::from_phase(Phase::Resolution);
        assert!(resolution.compiles);
        assert!(!resolution.imports_target);
        assert_eq!(resolution.score(), 0.25);

        let validation = ScoringCriteria::from_phase(Phase::Validation);
        assert!(validation.compiles);
        assert!(validation.imports_target);
        assert!(validation.creates_target_type);
        assert!(validation.executes_cleanly);
        assert_eq!(validation.score(), 1.0);
    }

    #[test]
    fn test_evaluation_result_success() {
        let result = EvaluationResult::success();
        assert!(result.ok);
        assert_eq!(result.score, 1.0);
        assert!(result.failure.is_none());
    }

    #[test]
    fn test_evaluation_result_failed() {
        let failure = Failure::new(ErrorCode::TypeSyntaxError, "bad syntax");
        let result = EvaluationResult::failed(failure);
        assert!(!result.ok);
        assert_eq!(result.score, 0.0); // Build phase = 0 points
        assert!(result.failure.is_some());
    }

    #[test]
    fn test_evaluation_result_partial_credit() {
        let failure = Failure::new(ErrorCode::TargetAborted, "abort");
        let result = EvaluationResult::failed(failure);
        assert!(!result.ok);
        // Execution phase reached = compiles + imports + creates = 0.75
        assert_eq!(result.score, 0.75);
        assert!(result.partial_credit_reason.is_some());
    }

    #[test]
    fn test_evaluation_result_serialization() {
        let result = EvaluationResult::success();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"score\":1"));
    }

    #[test]
    fn test_legacy_build_error_conversion() {
        // Build errors should map to A1 in legacy system
        assert_eq!(
            FailureStage::from(ErrorCode::TypeSyntaxError),
            FailureStage::A1
        );
        assert_eq!(
            FailureStage::from(ErrorCode::InvalidManifest),
            FailureStage::A1
        );
    }

    // =========================================================================
    // Inhabitation Metrics Tests
    // =========================================================================

    #[test]
    fn test_inhabitation_metrics_rate() {
        let metrics = InhabitationMetrics {
            target_types_total: 10,
            target_types_inhabited: 3,
            ..Default::default()
        };
        assert_eq!(metrics.inhabitation_rate(), 0.3);
    }

    #[test]
    fn test_inhabitation_metrics_zero_total() {
        let metrics = InhabitationMetrics::default();
        assert_eq!(metrics.inhabitation_rate(), 0.0);
    }

    #[test]
    fn test_inhabitation_metrics_entry_coverage() {
        let metrics = InhabitationMetrics {
            target_entry_functions: 5,
            entry_functions_called: 2,
            ..Default::default()
        };
        assert_eq!(metrics.entry_coverage(), 0.4);
    }

    #[test]
    fn test_uninhabited_type() {
        let uninhabited = UninhabitedType {
            type_name: "0x1::foo::Bar".to_string(),
            reason: UninhabitedReason::NoConstructor,
            details: Some("No public new() function".to_string()),
        };
        assert_eq!(uninhabited.reason, UninhabitedReason::NoConstructor);
    }

    // =========================================================================
    // Execution Trace Tests
    // =========================================================================

    #[test]
    fn test_execution_trace_record_call() {
        let mut trace = ExecutionTrace::default();
        trace.record_call(
            "0x2::coin".to_string(),
            "mint".to_string(),
            vec!["0x1::sui::SUI".to_string()],
        );
        assert_eq!(trace.functions_called.len(), 1);
        assert!(trace.functions_called[0].succeeded);
    }

    #[test]
    fn test_execution_trace_mark_failed() {
        let mut trace = ExecutionTrace::default();
        trace.record_call("0x2::coin".to_string(), "mint".to_string(), vec![]);
        trace.mark_last_failed("assertion failed".to_string());
        assert!(!trace.functions_called[0].succeeded);
        assert_eq!(
            trace.functions_called[0].error,
            Some("assertion failed".to_string())
        );
    }

    // =========================================================================
    // Abort Info Tests
    // =========================================================================

    #[test]
    fn test_abort_info_unsupported_native() {
        let abort = AbortInfo::from_move_abort(
            E_NOT_SUPPORTED,
            Some("0x2::random".to_string()),
            "random not supported".to_string(),
        );
        assert!(abort.is_expected);
        assert_eq!(abort.category, AbortCategory::UnsupportedNative);
    }

    #[test]
    fn test_abort_info_assertion() {
        let abort = AbortInfo::from_move_abort(
            42,
            Some("0x1::test".to_string()),
            "assertion failed in test".to_string(),
        );
        assert!(!abort.is_expected);
        assert_eq!(abort.category, AbortCategory::AssertionFailed);
    }

    #[test]
    fn test_abort_info_push_frame() {
        let mut abort = AbortInfo::from_move_abort(1, None, "error".to_string());
        abort.push_frame("0x2::coin".to_string(), "mint".to_string(), Some(42));
        abort.push_frame("0x1::test".to_string(), "main".to_string(), None);
        assert_eq!(abort.call_stack.len(), 2);
        assert_eq!(abort.call_stack[0].function, "mint");
        assert_eq!(abort.call_stack[1].instruction_offset, None);
    }

    #[test]
    fn test_abort_category_detection() {
        // Arithmetic
        let abort = AbortInfo::from_move_abort(100, None, "overflow detected".to_string());
        assert_eq!(abort.category, AbortCategory::ArithmeticError);

        // Vector
        let abort = AbortInfo::from_move_abort(100, None, "vector index out of bounds".to_string());
        assert_eq!(abort.category, AbortCategory::VectorBoundsError);

        // Object
        let abort = AbortInfo::from_move_abort(100, None, "object ownership error".to_string());
        assert_eq!(abort.category, AbortCategory::ObjectError);
    }

    // =========================================================================
    // EvaluationResult with Metrics/Trace Tests
    // =========================================================================

    #[test]
    fn test_evaluation_result_with_metrics() {
        let metrics = InhabitationMetrics {
            target_types_total: 5,
            target_types_inhabited: 3,
            inhabited_types: vec!["Foo".to_string(), "Bar".to_string(), "Baz".to_string()],
            ..Default::default()
        };
        let result = EvaluationResult::success().with_metrics(metrics);
        assert!(result.inhabitation_metrics.is_some());
        assert_eq!(
            result.inhabitation_metrics.unwrap().target_types_inhabited,
            3
        );
    }

    #[test]
    fn test_evaluation_result_with_trace() {
        let trace = ExecutionTrace {
            execution_attempted: true,
            duration_ms: Some(150),
            ..Default::default()
        };
        let result = EvaluationResult::success().with_trace(trace);
        assert!(result.execution_trace.is_some());
        assert_eq!(result.execution_trace.unwrap().duration_ms, Some(150));
    }

    #[test]
    fn test_evaluation_result_failed_with_details() {
        let failure = Failure::new(ErrorCode::TargetAborted, "abort");
        let criteria = ScoringCriteria::from_phase(Phase::Execution);
        let metrics = InhabitationMetrics {
            target_types_total: 10,
            target_types_inhabited: 5,
            ..Default::default()
        };
        let trace = ExecutionTrace {
            abort_info: Some(AbortInfo::from_move_abort(
                42,
                Some("0x1::test".to_string()),
                "assert failed".to_string(),
            )),
            ..Default::default()
        };

        let result =
            EvaluationResult::failed_with_details(failure, criteria, Some(metrics), Some(trace));

        assert!(!result.ok);
        assert!(result.inhabitation_metrics.is_some());
        assert!(result.execution_trace.is_some());
        assert!(result.execution_trace.unwrap().abort_info.is_some());
    }

    #[test]
    fn test_evaluation_result_serialization_with_metrics() {
        let metrics = InhabitationMetrics {
            target_types_total: 3,
            target_types_inhabited: 2,
            inhabited_types: vec!["Foo".to_string()],
            ..Default::default()
        };
        let result = EvaluationResult::success().with_metrics(metrics);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"inhabitation_metrics\""));
        assert!(json.contains("\"target_types_total\":3"));
    }

    #[test]
    fn test_evaluation_result_serialization_with_abort() {
        let failure = Failure::new(ErrorCode::TargetAborted, "abort");
        let trace = ExecutionTrace {
            abort_info: Some(AbortInfo::from_move_abort(
                E_NOT_SUPPORTED,
                None,
                "unsupported".to_string(),
            )),
            ..Default::default()
        };
        let result = EvaluationResult::failed(failure).with_trace(trace);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"abort_info\""));
        assert!(json.contains("\"category\":\"unsupported_native\""));
    }

    // =========================================================================
    // Context Stack Tests (P2.5)
    // =========================================================================

    #[test]
    fn test_context_frame_creation() {
        let frame = ContextFrame::synthesis("constructing Coin type");
        assert_eq!(frame.phase, Phase::Synthesis);
        assert_eq!(frame.operation, "constructing Coin type");
        assert!(frame.details.is_none());
    }

    #[test]
    fn test_context_frame_with_details() {
        let frame =
            ContextFrame::execution("calling function").with_details("0x2::coin::mint<SUI>");
        assert_eq!(frame.phase, Phase::Execution);
        assert_eq!(frame.details, Some("0x2::coin::mint<SUI>".to_string()));
    }

    #[test]
    fn test_context_frame_display() {
        let frame = ContextFrame::synthesis("constructing type");
        assert_eq!(format!("{}", frame), "constructing type");

        let frame_with_details =
            ContextFrame::synthesis("constructing type").with_details("Coin<SUI>");
        assert_eq!(
            format!("{}", frame_with_details),
            "constructing type (Coin<SUI>)"
        );
    }

    #[test]
    fn test_context_stack_push() {
        let mut stack = ContextStack::new();
        assert!(stack.is_empty());

        stack.push(ContextFrame::synthesis("synthesizing argument 1"));
        stack.push(ContextFrame::synthesis("constructing Coin type"));
        stack.push(ContextFrame::execution("calling constructor new"));

        assert_eq!(stack.len(), 3);
        assert!(!stack.is_empty());
    }

    #[test]
    fn test_context_stack_display() {
        let mut stack = ContextStack::new();
        stack.push(ContextFrame::synthesis("synthesizing argument 1"));
        stack.push(ContextFrame::synthesis("constructing Coin"));

        let display = format!("{}", stack);
        assert!(display.contains("while"));
        assert!(display.contains("synthesizing argument 1"));
        assert!(display.contains("→"));
        assert!(display.contains("constructing Coin"));
    }

    #[test]
    fn test_context_stack_format_trace() {
        let mut stack = ContextStack::new();
        stack.push(ContextFrame::synthesis("synthesizing argument"));
        stack.push(ContextFrame::synthesis("constructing type"));

        let trace = stack.format_trace();
        assert!(trace.contains("→ synthesizing argument"));
        assert!(trace.contains("  → constructing type"));
    }

    #[test]
    fn test_failure_with_stack() {
        let mut stack = ContextStack::new();
        stack.push(ContextFrame::synthesis("synthesizing argument 2"));
        stack.push(ContextFrame::synthesis("constructing 0x2::coin::Coin"));

        let failure = Failure::with_stack(ErrorCode::NoConstructor, "no constructor found", stack);
        assert!(!failure.context_stack.is_empty());
        assert_eq!(failure.context_stack.len(), 2);
    }

    #[test]
    fn test_failure_push_frame() {
        let failure = Failure::new(ErrorCode::NoConstructor, "no constructor found")
            .push_frame(ContextFrame::synthesis("synthesizing argument"))
            .push_frame(ContextFrame::synthesis("constructing type"));

        assert_eq!(failure.context_stack.len(), 2);
    }

    #[test]
    fn test_failure_prepend_frame() {
        let failure = Failure::new(ErrorCode::NoConstructor, "no constructor found")
            .push_frame(ContextFrame::synthesis("constructing type"))
            .prepend_frame(ContextFrame::synthesis("synthesizing argument"));

        // Prepended frame should be first
        assert_eq!(
            failure.context_stack.frames[0].operation,
            "synthesizing argument"
        );
        assert_eq!(
            failure.context_stack.frames[1].operation,
            "constructing type"
        );
    }

    #[test]
    fn test_failure_chain_stack() {
        let mut inner_stack = ContextStack::new();
        inner_stack.push(ContextFrame::execution("calling constructor"));

        let failure = Failure::new(ErrorCode::ConstructorAborted, "constructor aborted")
            .push_frame(ContextFrame::synthesis("synthesizing argument"))
            .chain_stack(inner_stack);

        assert_eq!(failure.context_stack.len(), 2);
    }

    #[test]
    fn test_failure_detailed_message() {
        let failure = Failure::new(ErrorCode::NoConstructor, "no constructor for Foo")
            .push_frame(ContextFrame::synthesis("synthesizing argument 2"))
            .push_frame(ContextFrame::synthesis("constructing type Foo"));

        let detailed = failure.detailed_message();
        assert!(detailed.contains("[synthesis] E301"));
        assert!(detailed.contains("no constructor for Foo"));
        assert!(detailed.contains("Context trace:"));
        assert!(detailed.contains("synthesizing argument 2"));
        assert!(detailed.contains("constructing type Foo"));
    }

    #[test]
    fn test_failure_detailed_message_with_context() {
        let ctx = FailureContext {
            module: Some("0x2::coin".to_string()),
            function: Some("mint".to_string()),
            type_name: None,
            param_index: Some(1),
        };
        let failure = Failure::with_context(ErrorCode::TypeMismatch, "expected u64, got bool", ctx)
            .push_frame(ContextFrame::type_check("validating argument types"));

        let detailed = failure.detailed_message();
        assert!(detailed.contains("module: 0x2::coin"));
        assert!(detailed.contains("function: mint"));
        assert!(detailed.contains("param #1"));
    }

    #[test]
    fn test_failure_serialization_with_stack() {
        let failure = Failure::new(ErrorCode::NoConstructor, "no constructor")
            .push_frame(ContextFrame::synthesis("synthesizing"));

        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("\"context_stack\""));
        assert!(json.contains("\"operation\":\"synthesizing\""));
        assert!(json.contains("\"phase\":\"synthesis\""));
    }

    #[test]
    fn test_failure_serialization_empty_stack_omitted() {
        let failure = Failure::new(ErrorCode::ModuleNotFound, "not found");

        let json = serde_json::to_string(&failure).unwrap();
        // Empty stack should be omitted from JSON
        assert!(!json.contains("\"context_stack\""));
    }

    #[test]
    fn test_context_frame_phase_helpers() {
        let resolution = ContextFrame::resolution("resolving module");
        assert_eq!(resolution.phase, Phase::Resolution);

        let type_check = ContextFrame::type_check("checking types");
        assert_eq!(type_check.phase, Phase::TypeCheck);

        let synthesis = ContextFrame::synthesis("synthesizing value");
        assert_eq!(synthesis.phase, Phase::Synthesis);

        let execution = ContextFrame::execution("executing function");
        assert_eq!(execution.phase, Phase::Execution);
    }

    // =========================================================================
    // Tests for Error Extension Traits
    // =========================================================================

    #[test]
    fn test_phase_result_ext_with_phase() {
        let result: Result<(), std::io::Error> = Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));

        let with_phase = result.with_phase(Phase::Build, "loading module");
        assert!(with_phase.is_err());

        let err_msg = with_phase.unwrap_err().to_string();
        assert!(err_msg.contains("[build]"));
        assert!(err_msg.contains("loading module"));
        assert!(err_msg.contains("file not found"));
    }

    #[test]
    fn test_phase_result_ext_with_phase_context() {
        let result: Result<(), std::io::Error> = Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "parsing failed",
        ));

        let with_phase = result.with_phase_context(Phase::TypeCheck, || {
            format!("parsing type {}", "0x2::coin::Coin")
        });
        assert!(with_phase.is_err());

        let err_msg = with_phase.unwrap_err().to_string();
        assert!(err_msg.contains("[typecheck]"));
        assert!(err_msg.contains("0x2::coin::Coin"));
    }

    #[test]
    fn test_phase_option_ext_ok_or_phase() {
        let none: Option<i32> = None;
        let result = none.ok_or_phase(Phase::Resolution, "module not found");

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("[resolution]"));
        assert!(err_msg.contains("module not found"));
    }

    #[test]
    fn test_phase_option_ext_ok_or_phase_with() {
        let none: Option<i32> = None;
        let module_name = "0x2::coin";
        let result = none.ok_or_phase_with(Phase::Resolution, || {
            format!("module {} not found", module_name)
        });

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("[resolution]"));
        assert!(err_msg.contains("0x2::coin"));
    }

    #[test]
    fn test_phase_option_ext_some_case() {
        let some: Option<i32> = Some(42);
        let result = some.ok_or_phase(Phase::Execution, "should not see this");

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_phase_error_function() {
        let err = phase_error(Phase::Synthesis, "constructor not found");
        let err_msg = err.to_string();

        assert!(err_msg.contains("[synthesis]"));
        assert!(err_msg.contains("constructor not found"));
    }

    #[test]
    fn test_phase_err_macro() {
        let type_name = "Coin<SUI>";
        let err = phase_err!(Phase::TypeCheck, "unknown type: {}", type_name);
        let err_msg = err.to_string();

        assert!(err_msg.contains("[typecheck]"));
        assert!(err_msg.contains("unknown type: Coin<SUI>"));
    }
}
