//! Shared types for the sui-sandbox workspace.
//!
//! This crate provides foundational types used across multiple crates in the workspace,
//! breaking circular dependency chains.
//!
//! ## Fetched Data Types (Unified)
//!
//! The [`fetched`] module contains canonical definitions for data fetched from the network:
//! - [`FetchedObject`](fetched::FetchedObject) - Object data with BCS bytes and metadata
//! - [`FetchedPackage`](fetched::FetchedPackage) - Package with modules and linkage
//! - [`ObjectID`](fetched::ObjectID) - 32-byte object/address identifier
//!
//! These types are the single source of truth, replacing duplicates in other crates.
//!
//! ## Transaction Types
//!
//! The [`transaction`] module contains core transaction types used for replay:
//! - [`CachedTransaction`](transaction::CachedTransaction) - Cached transaction with packages and objects
//! - [`FetchedTransaction`](transaction::FetchedTransaction) - Transaction fetched from network
//! - [`TransactionCache`](transaction::TransactionCache) - File-based transaction cache

pub mod encoding;
pub mod env_utils;
pub mod fetched;
pub mod framework;
pub mod transaction;
pub mod type_parsing;

// Re-export unified fetched types at crate root (CANONICAL definitions)
pub use fetched::{FetchedObject, FetchedPackage, ObjectID};

// Re-export type parsing utilities (canonical implementations)
pub use type_parsing::{parse_type_tag, split_type_params};

// Re-export encoding utilities (hex, base64, address normalization)
pub use encoding::{
    address_to_string, base64_decode, base64_encode, format_address_full, format_address_short,
    normalize_address, normalize_address_checked, normalize_address_short, parse_address,
    parse_hex_bytes, try_base64_decode, try_parse_address,
};

// Re-export framework constants
pub use framework::{
    is_framework_address, is_system_object, synthesize_clock_bytes, synthesize_random_bytes,
    CLOCK_OBJECT_ID, CLOCK_OBJECT_ID_STR, CLOCK_TYPE_STR, DEEPBOOK, DEFAULT_CLOCK_BASE_MS,
    DENY_LIST_OBJECT_ID, DENY_LIST_OBJECT_ID_STR, FRAMEWORK_ADDRESSES, MOVE_STDLIB,
    RANDOM_OBJECT_ID, RANDOM_OBJECT_ID_STR, RANDOM_TYPE_STR, SUI_BRIDGE, SUI_FRAMEWORK, SUI_SYSTEM,
    SYSTEM_STATE_OBJECT_ID,
};

// Re-export environment utilities
pub use env_utils::{env_bool, env_bool_or, env_list, env_string_or, env_var, env_var_or};

// Re-export commonly used transaction types at crate root
pub use transaction::{
    CachedDynamicField, CachedTransaction, DynamicFieldEntry, EffectsComparison,
    FetchedTransaction, GasSummary, LocalVersionInfo, PtbArgument, PtbCommand, ReplayResult,
    TransactionCache, TransactionDigest, TransactionEffectsSummary, TransactionInput,
    TransactionStatus, VersionMismatch, VersionMismatchType, VersionSummary,
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
