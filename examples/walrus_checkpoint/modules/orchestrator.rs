//! Retry state machine and phase coordination.
//!
//! Provides explicit state machine for the 4-attempt escalation strategy:
//! 1. WalrusOnly - Attempt with only Walrus data
//! 2. RetryWithChildFetcher - Install child-fetcher for dynamic fields
//! 3. RetryWithPrefetch - Add GraphQL prefetch
//! 4. RetryWithMM2 - Use predictive prefetch from gRPC transaction
//!
//! Note: Some methods may appear unused until full migration is complete.

#![allow(dead_code)]

use super::validator::ReasonCode;
use std::time::{Duration, Instant};

// ============================================================================
// Attempt Classification
// ============================================================================

/// Classification of retry attempt strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptKind {
    /// First attempt: use only Walrus data, no child fetcher
    WalrusOnly,
    /// Second attempt: install versioned child-fetcher for dynamic field lookups
    RetryWithChildFetcher,
    /// Third attempt: add GraphQL prefetch for dynamic field children
    RetryWithPrefetch,
    /// Fourth attempt: use MM2 predictive prefetch from gRPC transaction
    RetryWithMM2,
}

impl AttemptKind {
    /// Get the attempt index (0-3).
    pub fn index(&self) -> usize {
        match self {
            AttemptKind::WalrusOnly => 0,
            AttemptKind::RetryWithChildFetcher => 1,
            AttemptKind::RetryWithPrefetch => 2,
            AttemptKind::RetryWithMM2 => 3,
        }
    }

    /// Get the next escalation level, if any.
    pub fn next(&self) -> Option<AttemptKind> {
        match self {
            AttemptKind::WalrusOnly => Some(AttemptKind::RetryWithChildFetcher),
            AttemptKind::RetryWithChildFetcher => Some(AttemptKind::RetryWithPrefetch),
            AttemptKind::RetryWithPrefetch => Some(AttemptKind::RetryWithMM2),
            AttemptKind::RetryWithMM2 => None,
        }
    }

    /// Get from index.
    pub fn from_index(idx: usize) -> Option<AttemptKind> {
        match idx {
            0 => Some(AttemptKind::WalrusOnly),
            1 => Some(AttemptKind::RetryWithChildFetcher),
            2 => Some(AttemptKind::RetryWithPrefetch),
            3 => Some(AttemptKind::RetryWithMM2),
            _ => None,
        }
    }

    /// Human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            AttemptKind::WalrusOnly => "Walrus-only (no child fetcher)",
            AttemptKind::RetryWithChildFetcher => "With child-fetcher installed",
            AttemptKind::RetryWithPrefetch => "With GraphQL prefetch",
            AttemptKind::RetryWithMM2 => "With MM2 predictive prefetch",
        }
    }
}

// ============================================================================
// Attempt Report
// ============================================================================

/// Report for a single execution attempt.
#[derive(Debug, Clone)]
pub struct AttemptReport {
    /// The strategy used for this attempt
    pub kind: AttemptKind,
    /// Whether execution completed without error
    pub success: bool,
    /// Whether local effects match on-chain effects
    pub parity: bool,
    /// Classification of the result
    pub reason: ReasonCode,
    /// Time taken for this attempt
    pub duration: Duration,
    /// Additional notes/messages
    pub notes: Vec<String>,
}

impl AttemptReport {
    /// Create a new attempt report.
    pub fn new(kind: AttemptKind, reason: ReasonCode, duration: Duration) -> Self {
        Self {
            kind,
            success: reason == ReasonCode::StrictMatch,
            parity: reason == ReasonCode::StrictMatch,
            reason,
            duration,
            notes: Vec::new(),
        }
    }

    /// Add a note to the report.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Add multiple notes.
    pub fn with_notes(mut self, notes: impl IntoIterator<Item = String>) -> Self {
        self.notes.extend(notes);
        self
    }
}

// ============================================================================
// Transaction Outcome
// ============================================================================

/// Final outcome for a transaction after all attempts.
#[derive(Debug, Clone)]
pub struct TxOutcome {
    /// Transaction digest
    pub digest: String,
    /// Checkpoint number
    pub checkpoint: u64,
    /// All attempts made
    pub attempts: Vec<AttemptReport>,
    /// Whether final result achieved parity
    pub final_parity: bool,
    /// Final reason code
    pub final_reason: ReasonCode,
}

impl TxOutcome {
    /// Get total time across all attempts.
    pub fn total_duration(&self) -> Duration {
        self.attempts.iter().map(|a| a.duration).sum()
    }

    /// Get number of attempts made.
    pub fn num_attempts(&self) -> usize {
        self.attempts.len()
    }

    /// Check if transaction achieved strict match.
    pub fn is_strict_match(&self) -> bool {
        self.final_reason == ReasonCode::StrictMatch
    }
}

// ============================================================================
// Retry Configuration
// ============================================================================

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of attempts (1-4)
    pub max_attempts: usize,
    /// Enable child-fetcher on retry
    pub enable_child_fetcher_on_retry: bool,
    /// Enable GraphQL prefetch on retry
    pub enable_prefetch_on_retry: bool,
    /// Enable MM2 predictive prefetch on retry
    pub enable_mm2_on_retry: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            enable_child_fetcher_on_retry: true,
            enable_prefetch_on_retry: true,
            enable_mm2_on_retry: true,
        }
    }
}

impl RetryConfig {
    /// Create config for single attempt (no retries).
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            enable_child_fetcher_on_retry: false,
            enable_prefetch_on_retry: false,
            enable_mm2_on_retry: false,
        }
    }

    /// Create config with custom max attempts.
    pub fn with_max_attempts(max: usize) -> Self {
        Self {
            max_attempts: max.min(4),
            ..Default::default()
        }
    }
}

// ============================================================================
// Retry State Machine
// ============================================================================

/// State machine for retry orchestration.
///
/// Manages the 4-attempt escalation strategy with explicit state transitions.
#[derive(Debug, Clone)]
pub struct RetryStateMachine {
    config: RetryConfig,
    current_attempt: usize,
    attempts: Vec<AttemptReport>,
    start_time: Option<Instant>,
}

impl RetryStateMachine {
    /// Create a new state machine with the given configuration.
    pub fn new(config: RetryConfig) -> Self {
        Self {
            config,
            current_attempt: 0,
            attempts: Vec::new(),
            start_time: None,
        }
    }

    /// Create with default configuration and max attempts.
    pub fn with_max_attempts(max: usize) -> Self {
        Self::new(RetryConfig::with_max_attempts(max))
    }

    /// Get the next attempt kind based on current state.
    ///
    /// Returns `None` if max attempts reached or if retries are disabled
    /// for the next escalation level.
    pub fn next_attempt_kind(&self) -> Option<AttemptKind> {
        if self.current_attempt >= self.config.max_attempts {
            return None;
        }

        let kind = AttemptKind::from_index(self.current_attempt)?;

        // Check if this escalation level is enabled
        match kind {
            AttemptKind::WalrusOnly => Some(kind),
            AttemptKind::RetryWithChildFetcher => {
                if self.config.enable_child_fetcher_on_retry {
                    Some(kind)
                } else {
                    None
                }
            }
            AttemptKind::RetryWithPrefetch => {
                if self.config.enable_prefetch_on_retry {
                    Some(kind)
                } else {
                    None
                }
            }
            AttemptKind::RetryWithMM2 => {
                if self.config.enable_mm2_on_retry {
                    Some(kind)
                } else {
                    None
                }
            }
        }
    }

    /// Start timing an attempt.
    pub fn start_attempt(&mut self) {
        self.start_time = Some(Instant::now());
    }

    /// Record attempt result and determine if should continue.
    ///
    /// Returns `true` if another attempt should be made.
    pub fn record_attempt(&mut self, mut report: AttemptReport) -> bool {
        // Fill in duration if start_time was set
        if let Some(start) = self.start_time.take() {
            report.duration = start.elapsed();
        }

        let should_continue = !report.parity
            && self.current_attempt + 1 < self.config.max_attempts
            && is_retryable(report.reason);

        self.attempts.push(report);
        self.current_attempt += 1;

        should_continue
    }

    /// Record a successful attempt (convenience method).
    pub fn record_success(&mut self, kind: AttemptKind, duration: Duration) -> bool {
        self.record_attempt(AttemptReport {
            kind,
            success: true,
            parity: true,
            reason: ReasonCode::StrictMatch,
            duration,
            notes: Vec::new(),
        })
    }

    /// Record a failed attempt (convenience method).
    pub fn record_failure(
        &mut self,
        kind: AttemptKind,
        reason: ReasonCode,
        duration: Duration,
    ) -> bool {
        self.record_attempt(AttemptReport {
            kind,
            success: false,
            parity: false,
            reason,
            duration,
            notes: Vec::new(),
        })
    }

    /// Build final outcome.
    pub fn build_outcome(self, digest: String, checkpoint: u64) -> TxOutcome {
        let final_report = self.attempts.last().cloned();
        TxOutcome {
            digest,
            checkpoint,
            final_parity: final_report.as_ref().map(|r| r.parity).unwrap_or(false),
            final_reason: final_report
                .map(|r| r.reason)
                .unwrap_or(ReasonCode::Unknown),
            attempts: self.attempts,
        }
    }

    /// Get current attempt index (0-based).
    pub fn current_attempt(&self) -> usize {
        self.current_attempt
    }

    /// Get all recorded attempts.
    pub fn attempts(&self) -> &[AttemptReport] {
        &self.attempts
    }

    /// Check if any attempt achieved parity.
    pub fn achieved_parity(&self) -> bool {
        self.attempts.iter().any(|a| a.parity)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if a reason code indicates the failure is retryable.
pub fn is_retryable(reason: ReasonCode) -> bool {
    matches!(
        reason,
        ReasonCode::MissingPackage
            | ReasonCode::MissingObject
            | ReasonCode::DynamicFieldMiss
            | ReasonCode::Timeout
            | ReasonCode::ExecutionFailure
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attempt_kind_transitions() {
        assert_eq!(
            AttemptKind::WalrusOnly.next(),
            Some(AttemptKind::RetryWithChildFetcher)
        );
        assert_eq!(
            AttemptKind::RetryWithChildFetcher.next(),
            Some(AttemptKind::RetryWithPrefetch)
        );
        assert_eq!(
            AttemptKind::RetryWithPrefetch.next(),
            Some(AttemptKind::RetryWithMM2)
        );
        assert_eq!(AttemptKind::RetryWithMM2.next(), None);
    }

    #[test]
    fn test_attempt_kind_from_index() {
        assert_eq!(AttemptKind::from_index(0), Some(AttemptKind::WalrusOnly));
        assert_eq!(
            AttemptKind::from_index(1),
            Some(AttemptKind::RetryWithChildFetcher)
        );
        assert_eq!(
            AttemptKind::from_index(2),
            Some(AttemptKind::RetryWithPrefetch)
        );
        assert_eq!(AttemptKind::from_index(3), Some(AttemptKind::RetryWithMM2));
        assert_eq!(AttemptKind::from_index(4), None);
    }

    #[test]
    fn test_state_machine_next_attempt() {
        let mut sm = RetryStateMachine::with_max_attempts(4);

        assert_eq!(sm.next_attempt_kind(), Some(AttemptKind::WalrusOnly));
        sm.record_failure(
            AttemptKind::WalrusOnly,
            ReasonCode::DynamicFieldMiss,
            Duration::from_millis(100),
        );

        assert_eq!(
            sm.next_attempt_kind(),
            Some(AttemptKind::RetryWithChildFetcher)
        );
        sm.record_failure(
            AttemptKind::RetryWithChildFetcher,
            ReasonCode::MissingObject,
            Duration::from_millis(100),
        );

        assert_eq!(sm.next_attempt_kind(), Some(AttemptKind::RetryWithPrefetch));
    }

    #[test]
    fn test_state_machine_stops_on_success() {
        let mut sm = RetryStateMachine::with_max_attempts(4);

        assert_eq!(sm.next_attempt_kind(), Some(AttemptKind::WalrusOnly));
        let should_continue =
            sm.record_success(AttemptKind::WalrusOnly, Duration::from_millis(100));

        assert!(!should_continue);
        assert!(sm.achieved_parity());
    }

    #[test]
    fn test_state_machine_stops_on_non_retryable() {
        let mut sm = RetryStateMachine::with_max_attempts(4);

        assert_eq!(sm.next_attempt_kind(), Some(AttemptKind::WalrusOnly));
        let should_continue = sm.record_failure(
            AttemptKind::WalrusOnly,
            ReasonCode::ParseError, // Not retryable
            Duration::from_millis(100),
        );

        assert!(!should_continue);
        assert!(!sm.achieved_parity());
    }

    #[test]
    fn test_state_machine_max_attempts() {
        let mut sm = RetryStateMachine::with_max_attempts(2);

        sm.record_failure(
            AttemptKind::WalrusOnly,
            ReasonCode::MissingObject,
            Duration::from_millis(100),
        );
        sm.record_failure(
            AttemptKind::RetryWithChildFetcher,
            ReasonCode::MissingObject,
            Duration::from_millis(100),
        );

        // Should return None after max attempts
        assert_eq!(sm.next_attempt_kind(), None);
    }

    #[test]
    fn test_state_machine_build_outcome() {
        let mut sm = RetryStateMachine::with_max_attempts(4);
        sm.record_failure(
            AttemptKind::WalrusOnly,
            ReasonCode::DynamicFieldMiss,
            Duration::from_millis(50),
        );
        sm.record_success(
            AttemptKind::RetryWithChildFetcher,
            Duration::from_millis(100),
        );

        let outcome = sm.build_outcome("0xabc".to_string(), 12345);

        assert_eq!(outcome.digest, "0xabc");
        assert_eq!(outcome.checkpoint, 12345);
        assert!(outcome.final_parity);
        assert_eq!(outcome.final_reason, ReasonCode::StrictMatch);
        assert_eq!(outcome.num_attempts(), 2);
    }

    #[test]
    fn test_is_retryable() {
        assert!(is_retryable(ReasonCode::MissingPackage));
        assert!(is_retryable(ReasonCode::MissingObject));
        assert!(is_retryable(ReasonCode::DynamicFieldMiss));
        assert!(is_retryable(ReasonCode::Timeout));
        assert!(is_retryable(ReasonCode::ExecutionFailure));

        assert!(!is_retryable(ReasonCode::StrictMatch));
        assert!(!is_retryable(ReasonCode::ParseError));
        assert!(!is_retryable(ReasonCode::StatusMismatch));
        assert!(!is_retryable(ReasonCode::LamportMismatch));
    }

    #[test]
    fn test_no_retry_config() {
        let sm = RetryStateMachine::new(RetryConfig::no_retry());
        assert_eq!(sm.next_attempt_kind(), Some(AttemptKind::WalrusOnly));

        let mut sm = RetryStateMachine::new(RetryConfig::no_retry());
        sm.record_failure(
            AttemptKind::WalrusOnly,
            ReasonCode::MissingObject,
            Duration::from_millis(100),
        );
        assert_eq!(sm.next_attempt_kind(), None);
    }
}
