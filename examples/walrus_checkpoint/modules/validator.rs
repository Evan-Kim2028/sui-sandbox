//! Parity comparison, diffing, and error classification.
//!
//! Provides validation of local execution results against on-chain effects,
//! including:
//! - Status comparison
//! - Lamport version validation
//! - Per-object change tracking
//! - Error classification for retry decisions
//!
//! Note: Some types may appear unused until full migration is complete.

#![allow(dead_code)]

use anyhow::Result;
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;
use sui_sandbox_core::ptb::{TransactionEffects, VersionChangeType};

// ============================================================================
// Reason Codes
// ============================================================================

/// Classification of execution/validation outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReasonCode {
    /// Local effects match on-chain exactly
    StrictMatch,
    /// Failed to parse PTB from Walrus JSON
    ParseError,
    /// PTB contains unsupported command type (Publish, Upgrade)
    UnsupportedCommand,
    /// Required package bytecode not available
    MissingPackage,
    /// Required object not found in cache or network
    MissingObject,
    /// Dynamic field lookup failed
    DynamicFieldMiss,
    /// Execution timed out
    Timeout,
    /// Execution failed with VM error
    ExecutionFailure,
    /// Local status (success/failure) doesn't match on-chain
    StatusMismatch,
    /// Lamport version doesn't match on-chain
    LamportMismatch,
    /// Object bytes or version mismatch
    ObjectMismatch,
    /// Only gas object differs (may be acceptable)
    GasMismatch,
    /// Walrus data is internally inconsistent
    WalrusInconsistent,
    /// Feature not modeled in local execution (e.g., missing version tracking)
    NotModeled,
    /// Unknown error
    Unknown,
}

impl ReasonCode {
    /// Check if this reason indicates successful parity.
    pub fn is_success(&self) -> bool {
        matches!(self, ReasonCode::StrictMatch)
    }

    /// Check if this reason allows for retry.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ReasonCode::MissingPackage
                | ReasonCode::MissingObject
                | ReasonCode::DynamicFieldMiss
                | ReasonCode::Timeout
                | ReasonCode::ExecutionFailure
        )
    }

    /// Human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            ReasonCode::StrictMatch => "Strict match with on-chain effects",
            ReasonCode::ParseError => "Failed to parse PTB from JSON",
            ReasonCode::UnsupportedCommand => "Unsupported command type",
            ReasonCode::MissingPackage => "Required package not found",
            ReasonCode::MissingObject => "Required object not found",
            ReasonCode::DynamicFieldMiss => "Dynamic field lookup failed",
            ReasonCode::Timeout => "Execution timed out",
            ReasonCode::ExecutionFailure => "VM execution error",
            ReasonCode::StatusMismatch => "Status mismatch (success vs failure)",
            ReasonCode::LamportMismatch => "Lamport version mismatch",
            ReasonCode::ObjectMismatch => "Object bytes/version mismatch",
            ReasonCode::GasMismatch => "Gas object mismatch only",
            ReasonCode::WalrusInconsistent => "Walrus data inconsistent",
            ReasonCode::NotModeled => "Feature not modeled locally",
            ReasonCode::Unknown => "Unknown error",
        }
    }
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ============================================================================
// Comparison Results
// ============================================================================

/// Detailed comparison result between local and on-chain effects.
#[derive(Debug, Clone)]
pub struct ComparisonResult {
    /// Whether local effects match on-chain
    pub parity: bool,
    /// Primary reason code
    pub reason: ReasonCode,
    /// Detailed error/info messages
    pub details: Vec<String>,
    /// Per-object differences (if any)
    pub object_diffs: Vec<ObjectDiff>,
}

impl ComparisonResult {
    /// Create a successful match result.
    pub fn strict_match() -> Self {
        Self {
            parity: true,
            reason: ReasonCode::StrictMatch,
            details: Vec::new(),
            object_diffs: Vec::new(),
        }
    }

    /// Create a failure result.
    pub fn failure(reason: ReasonCode, detail: impl Into<String>) -> Self {
        Self {
            parity: false,
            reason,
            details: vec![detail.into()],
            object_diffs: Vec::new(),
        }
    }

    /// Add an object diff.
    pub fn with_diff(mut self, diff: ObjectDiff) -> Self {
        self.object_diffs.push(diff);
        self
    }

    /// Add a detail message.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }
}

/// Difference in a single object between local and on-chain.
#[derive(Debug, Clone)]
pub struct ObjectDiff {
    /// Object ID
    pub id: AccountAddress,
    /// Expected version from on-chain
    pub expected_version: Option<u64>,
    /// Actual version from local execution
    pub actual_version: Option<u64>,
    /// Expected change type
    pub expected_change: Option<VersionChangeType>,
    /// Actual change type
    pub actual_change: Option<VersionChangeType>,
    /// Whether bytes match (for mutations)
    pub bytes_match: bool,
    /// Additional notes
    pub notes: Vec<String>,
}

impl ObjectDiff {
    pub fn new(id: AccountAddress) -> Self {
        Self {
            id,
            expected_version: None,
            actual_version: None,
            expected_change: None,
            actual_change: None,
            bytes_match: true,
            notes: Vec::new(),
        }
    }

    pub fn with_versions(mut self, expected: Option<u64>, actual: Option<u64>) -> Self {
        self.expected_version = expected;
        self.actual_version = actual;
        self
    }

    pub fn with_changes(
        mut self,
        expected: Option<VersionChangeType>,
        actual: Option<VersionChangeType>,
    ) -> Self {
        self.expected_change = expected;
        self.actual_change = actual;
        self
    }

    pub fn with_bytes_match(mut self, matches: bool) -> Self {
        self.bytes_match = matches;
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

// ============================================================================
// Expected Changes
// ============================================================================

/// Expected change information from on-chain effects.
#[derive(Debug, Clone)]
pub struct ExpectedChange {
    /// Object ID
    pub id: AccountAddress,
    /// Input version (before transaction)
    pub input_version: Option<u64>,
    /// Output digest from effects
    pub output_digest: String,
    /// Type of change
    pub change_type: VersionChangeType,
}

// ============================================================================
// Parity Validator Trait
// ============================================================================

/// Trait for validating execution results against on-chain effects.
pub trait ParityValidator: Send + Sync {
    /// Perform strict comparison between local and on-chain effects.
    fn strict_compare(
        &self,
        tx_json: &serde_json::Value,
        local_effects: &TransactionEffects,
    ) -> ComparisonResult;

    /// Parse expected changes from transaction effects JSON.
    fn parse_expected_changes(
        &self,
        tx_json: &serde_json::Value,
    ) -> Result<HashMap<AccountAddress, ExpectedChange>>;
}

// ============================================================================
// Error Classifier Trait
// ============================================================================

/// Trait for classifying execution failures.
pub trait ErrorClassifier: Send + Sync {
    /// Classify an execution failure into a ReasonCode.
    fn classify_failure(
        &self,
        error: Option<&sui_sandbox_core::simulation::SimulationError>,
        raw_error: Option<&str>,
    ) -> ReasonCode;

    /// Check if a reason code is retryable.
    fn is_retryable(&self, reason: ReasonCode) -> bool {
        reason.is_retryable()
    }

    /// Parse parent-child conflict from error message.
    fn parse_conflict(&self, raw: &str) -> Option<(AccountAddress, AccountAddress)>;
}

// ============================================================================
// Default Implementations
// ============================================================================

/// Default error classifier implementation.
pub struct DefaultErrorClassifier;

impl ErrorClassifier for DefaultErrorClassifier {
    fn classify_failure(
        &self,
        error: Option<&sui_sandbox_core::simulation::SimulationError>,
        raw_error: Option<&str>,
    ) -> ReasonCode {
        // Check raw error string first
        if let Some(raw) = raw_error {
            let lower = raw.to_lowercase();
            if lower.contains("dynamic field") || lower.contains("dynamic_field") {
                return ReasonCode::DynamicFieldMiss;
            }
            if lower.contains("timeout") || lower.contains("timed out") {
                return ReasonCode::Timeout;
            }
        }

        // Check typed error
        if let Some(err) = error {
            use sui_sandbox_core::simulation::SimulationError;
            match err {
                SimulationError::MissingPackage { .. } => return ReasonCode::MissingPackage,
                SimulationError::MissingObject { .. } => return ReasonCode::MissingObject,
                _ => {}
            }
        }

        ReasonCode::ExecutionFailure
    }

    fn parse_conflict(&self, raw: &str) -> Option<(AccountAddress, AccountAddress)> {
        let parent_hex = extract_hex_after(raw, "parent ")?;
        let child_hex = extract_hex_after(raw, "child object ")?;
        let parent = AccountAddress::from_hex_literal(&parent_hex).ok()?;
        let child = AccountAddress::from_hex_literal(&child_hex).ok()?;
        Some((parent, child))
    }
}

impl Default for DefaultErrorClassifier {
    fn default() -> Self {
        Self
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract hex address after a marker string.
fn extract_hex_after(raw: &str, marker: &str) -> Option<String> {
    let start = raw.find(marker)? + marker.len();
    let rest = &raw[start..];
    let mut end = rest.len();
    for (idx, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            end = idx;
            break;
        }
    }
    let token = rest[..end]
        .trim_end_matches(|c: char| !c.is_ascii_hexdigit() && c != 'x')
        .trim_end_matches(['.', ',', ')']);
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Extract lamport version from transaction JSON.
pub fn on_chain_lamport_version(tx_json: &serde_json::Value) -> Option<u64> {
    tx_json
        .pointer("/effects/V2/lamport_version")
        .and_then(|v| v.as_u64())
}

/// Extract status from transaction JSON.
pub fn on_chain_status(tx_json: &serde_json::Value) -> Option<bool> {
    let status = tx_json.pointer("/effects/V2/status")?;
    Some(match status {
        serde_json::Value::String(s) => s == "Success",
        serde_json::Value::Object(o) => o.contains_key("Success"),
        _ => false,
    })
}

// ============================================================================
// Strict Diff Error (for backward compatibility)
// ============================================================================

/// Error type for strict comparison failures.
#[derive(Debug, Clone)]
pub struct StrictDiffError {
    pub code: ReasonCode,
    pub message: String,
}

impl StrictDiffError {
    pub fn new(code: ReasonCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StrictDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for StrictDiffError {}

impl From<StrictDiffError> for ComparisonResult {
    fn from(err: StrictDiffError) -> Self {
        ComparisonResult::failure(err.code, err.message)
    }
}

// ============================================================================
// Validation Utilities
// ============================================================================

/// Create a ComparisonResult from an Ok/Err pattern.
pub fn comparison_from_result<E: std::fmt::Display>(
    result: std::result::Result<(), E>,
    error_code: ReasonCode,
) -> ComparisonResult {
    match result {
        Ok(()) => ComparisonResult::strict_match(),
        Err(e) => ComparisonResult::failure(error_code, e.to_string()),
    }
}

/// Helper to check if strict_compare succeeded.
pub fn strict_compare_succeeded(result: &ComparisonResult) -> bool {
    result.parity && result.reason == ReasonCode::StrictMatch
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reason_code_is_retryable() {
        assert!(ReasonCode::MissingPackage.is_retryable());
        assert!(ReasonCode::MissingObject.is_retryable());
        assert!(ReasonCode::DynamicFieldMiss.is_retryable());
        assert!(ReasonCode::Timeout.is_retryable());
        assert!(ReasonCode::ExecutionFailure.is_retryable());

        assert!(!ReasonCode::StrictMatch.is_retryable());
        assert!(!ReasonCode::ParseError.is_retryable());
        assert!(!ReasonCode::StatusMismatch.is_retryable());
    }

    #[test]
    fn test_reason_code_is_success() {
        assert!(ReasonCode::StrictMatch.is_success());
        assert!(!ReasonCode::ParseError.is_success());
        assert!(!ReasonCode::MissingPackage.is_success());
    }

    #[test]
    fn test_comparison_result_strict_match() {
        let result = ComparisonResult::strict_match();
        assert!(result.parity);
        assert_eq!(result.reason, ReasonCode::StrictMatch);
        assert!(result.details.is_empty());
    }

    #[test]
    fn test_comparison_result_failure() {
        let result =
            ComparisonResult::failure(ReasonCode::StatusMismatch, "expected success, got failure");
        assert!(!result.parity);
        assert_eq!(result.reason, ReasonCode::StatusMismatch);
        assert_eq!(result.details.len(), 1);
    }

    #[test]
    fn test_extract_hex_after() {
        let raw = "new parent 0x123abc conflict with child object 0xdef456";
        assert_eq!(
            extract_hex_after(raw, "parent "),
            Some("0x123abc".to_string())
        );
        assert_eq!(
            extract_hex_after(raw, "child object "),
            Some("0xdef456".to_string())
        );
        assert_eq!(extract_hex_after(raw, "missing "), None);
    }

    #[test]
    fn test_default_error_classifier_dynamic_field() {
        let classifier = DefaultErrorClassifier;
        let reason = classifier.classify_failure(None, Some("dynamic field not found"));
        assert_eq!(reason, ReasonCode::DynamicFieldMiss);
    }

    #[test]
    fn test_default_error_classifier_timeout() {
        let classifier = DefaultErrorClassifier;
        let reason = classifier.classify_failure(None, Some("execution timed out"));
        assert_eq!(reason, ReasonCode::Timeout);
    }

    #[test]
    fn test_object_diff_builder() {
        let id = AccountAddress::from_hex_literal("0x1").unwrap();
        let diff = ObjectDiff::new(id)
            .with_versions(Some(5), Some(6))
            .with_changes(
                Some(VersionChangeType::Mutated),
                Some(VersionChangeType::Mutated),
            )
            .with_bytes_match(false)
            .with_note("bytes differ at offset 42");

        assert_eq!(diff.expected_version, Some(5));
        assert_eq!(diff.actual_version, Some(6));
        assert!(!diff.bytes_match);
        assert_eq!(diff.notes.len(), 1);
    }
}
