//! Error codes and diagnostic messages for type inhabitation evaluation.
//!
//! # Error Taxonomy (v0.4.0)
//!
//! The type inhabitation pipeline uses a phase-based error taxonomy:
//!
//! | Phase | Purpose | Error Codes |
//! |-------|---------|-------------|
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
//! - is_expected_limitation: Whether this is a sandbox limitation vs LLM failure

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Phase-Based Error Taxonomy (v0.4.0)
// =============================================================================

/// Phase of the type inhabitation pipeline.
///
/// The pipeline processes in order: Resolution -> TypeCheck -> Synthesis -> Execution -> Validation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
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
    /// Get the numeric prefix for this phase (1xx, 2xx, etc.)
    pub fn code_prefix(&self) -> u16 {
        match self {
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
/// - 1xx: Resolution errors
/// - 2xx: Type check errors
/// - 3xx: Synthesis errors
/// - 4xx: Execution errors
/// - 5xx: Validation errors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
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
    /// Get the numeric code (e.g., 101, 201, etc.)
    pub fn numeric_code(&self) -> u16 {
        match self {
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
            1 => Phase::Resolution,
            2 => Phase::TypeCheck,
            3 => Phase::Synthesis,
            4 => Phase::Execution,
            5 => Phase::Validation,
            _ => unreachable!("Invalid error code"),
        }
    }

    /// Get a short description of this error
    pub fn description(&self) -> &'static str {
        match self {
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

    /// Check if this error represents an expected sandbox limitation
    /// (as opposed to an LLM failure)
    pub fn is_expected_limitation(&self) -> bool {
        matches!(
            self,
            ErrorCode::UnsupportedNative
                | ErrorCode::ChainTooDeep
                | ErrorCode::UnsupportedConstructorParam
        )
    }

    /// Get the string code (e.g., "E101", "E201", etc.)
    pub fn code_string(&self) -> String {
        format!("E{}", self.numeric_code())
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
    /// Whether this is an expected sandbox limitation (not an LLM failure)
    pub is_expected_limitation: bool,
    /// Optional additional context (e.g., which type failed, which function)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<FailureContext>,
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

impl Failure {
    /// Create a new failure with just the essentials
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            phase: code.phase(),
            code,
            message: message.into(),
            is_expected_limitation: code.is_expected_limitation(),
            context: None,
        }
    }

    /// Create a failure with context
    pub fn with_context(code: ErrorCode, message: impl Into<String>, context: FailureContext) -> Self {
        Self {
            phase: code.phase(),
            code,
            message: message.into(),
            is_expected_limitation: code.is_expected_limitation(),
            context: Some(context),
        }
    }

    /// Add context to an existing failure
    pub fn add_context(mut self, context: FailureContext) -> Self {
        self.context = Some(context);
        self
    }

    /// Mark this failure as an expected limitation
    pub fn mark_expected(mut self) -> Self {
        self.is_expected_limitation = true;
        self
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
            // Resolution -> A1
            ErrorCode::ModuleNotFound
            | ErrorCode::FunctionNotFound
            | ErrorCode::NotCallable => FailureStage::A1,
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
        // Category C: Unsupported (abort with E_NOT_SUPPORTED)
        NativeModuleInfo {
            module: "0x2::bls12381",
            category: NativeCategory::Unsupported,
            description: "BLS12-381 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::ecdsa_k1",
            category: NativeCategory::Unsupported,
            description: "ECDSA secp256k1 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::ecdsa_r1",
            category: NativeCategory::Unsupported,
            description: "ECDSA secp256r1 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::ed25519",
            category: NativeCategory::Unsupported,
            description: "Ed25519 signature verification",
        },
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
            category: NativeCategory::Unsupported,
            description: "On-chain randomness",
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
        assert_eq!(ErrorCode::NoTargetModulesAccessed.phase(), Phase::Validation);
    }

    #[test]
    fn test_error_code_string() {
        assert_eq!(ErrorCode::ModuleNotFound.code_string(), "E101");
        assert_eq!(ErrorCode::UnsupportedNative.code_string(), "E404");
    }

    #[test]
    fn test_expected_limitations() {
        assert!(ErrorCode::UnsupportedNative.is_expected_limitation());
        assert!(ErrorCode::ChainTooDeep.is_expected_limitation());
        assert!(!ErrorCode::TypeMismatch.is_expected_limitation());
        assert!(!ErrorCode::TargetAborted.is_expected_limitation());
    }

    #[test]
    fn test_failure_creation() {
        let failure = Failure::new(ErrorCode::ModuleNotFound, "module foo not found");
        assert_eq!(failure.phase, Phase::Resolution);
        assert_eq!(failure.code, ErrorCode::ModuleNotFound);
        assert!(!failure.is_expected_limitation);
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
        assert_eq!(FailureStage::A1.to_error_code(), ErrorCode::FunctionNotFound);
        assert_eq!(FailureStage::A2.to_error_code(), ErrorCode::UnknownType);
        assert_eq!(FailureStage::A3.to_error_code(), ErrorCode::NoConstructor);
        assert_eq!(FailureStage::B2.to_error_code(), ErrorCode::TargetAborted);
    }

    #[test]
    fn test_legacy_conversion_from_error_code() {
        assert_eq!(FailureStage::from(ErrorCode::ModuleNotFound), FailureStage::A1);
        assert_eq!(FailureStage::from(ErrorCode::RecursiveType), FailureStage::A2);
        assert_eq!(FailureStage::from(ErrorCode::NoConstructor), FailureStage::A3);
        assert_eq!(FailureStage::from(ErrorCode::UnsupportedNative), FailureStage::B2);
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
}
