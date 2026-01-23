//! Shared types for the sui-sandbox workspace.
//!
//! This crate provides foundational types used across multiple crates in the workspace,
//! breaking circular dependency chains.
//!
//! ## Transaction Types
//!
//! The [`transaction`] module contains core transaction types used for replay:
//! - [`CachedTransaction`](transaction::CachedTransaction) - Cached transaction with packages and objects
//! - [`FetchedTransaction`](transaction::FetchedTransaction) - Transaction fetched from network
//! - [`TransactionCache`](transaction::TransactionCache) - File-based transaction cache

pub mod transaction;

// Re-export commonly used transaction types at crate root
pub use transaction::{
    CachedDynamicField, CachedTransaction, DynamicFieldEntry, EffectsComparison, FetchedObject,
    FetchedTransaction, GasSummary, ObjectID, PtbArgument, PtbCommand, ReplayResult,
    TransactionCache, TransactionDigest, TransactionEffectsSummary, TransactionInput,
    TransactionStatus,
};

use std::time::Duration;

/// Configuration for retry behavior on network operations.
#[derive(Debug, Copy, Clone)]
pub struct RetryConfig {
    /// Number of retry attempts.
    pub retries: usize,
    /// Initial backoff duration between retries.
    pub initial_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
}

impl RetryConfig {
    /// Create a new RetryConfig with the specified parameters.
    pub fn new(retries: usize, initial_backoff_ms: u64, max_backoff_ms: u64) -> Self {
        Self {
            retries,
            initial_backoff: Duration::from_millis(initial_backoff_ms),
            max_backoff: Duration::from_millis(max_backoff_ms),
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            retries: 8,
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_millis(5000),
        }
    }
}
