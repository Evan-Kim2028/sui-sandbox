//! Unified Cache Manager for Sui Data
//!
//! This module provides a unified caching layer for packages, objects, and transactions
//! with consistent address normalization, metadata tracking, and write-through capabilities.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        CacheManager                                  │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
//! │  │ PackageIndex │  │ ObjectIndex  │  │ TransactionCache         │  │
//! │  │ (in-memory)  │  │ (in-memory)  │  │ (disk-backed)            │  │
//! │  │              │  │              │  │                          │  │
//! │  │ - pkg_id     │  │ - obj_id     │  │ - Full CachedTransaction │  │
//! │  │ - version    │  │ - version    │  │ - All dependencies       │  │
//! │  │ - file_path  │  │ - type_tag   │  │ - Effects & events       │  │
//! │  │              │  │ - file_path  │  │                          │  │
//! │  └──────────────┘  └──────────────┘  └──────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Cache File Format
//!
//! The cache uses the same JSON format as `benchmark::tx_replay::CachedTransaction`:
//! ```json
//! {
//!   "transaction": { ... },
//!   "packages": { "0x...": [["module_name", "base64_bytecode"], ...] },
//!   "objects": { "0x...": "base64_bcs_bytes" },
//!   "object_types": { "0x...": "0x2::coin::Coin<0x2::sui::SUI>" },
//!   "cached_at": 1234567890
//! }
//! ```
//!
//! # Usage
//!
//! ```no_run
//! use sui_sandbox::cache::CacheManager;
//!
//! // Create cache manager
//! let mut cache = CacheManager::new(".tx-cache").unwrap();
//!
//! // Read-path: Check cache first
//! if let Some(pkg) = cache.get_package("0x2").unwrap() {
//!     println!("Found {} modules, version {}", pkg.modules.len(), pkg.version);
//! }
//!
//! // Write-path: Cache network fetches
//! let modules: Vec<(String, Vec<u8>)> = vec![];
//! let bcs_bytes: Vec<u8> = vec![];
//! cache.put_package("0x123", 1, modules).unwrap();
//! cache.put_object("0xabc", 5, Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()), bcs_bytes).unwrap();
//! ```

mod index;
mod manager;
pub use index::{CachedObjectEntry, CachedPackageEntry};
pub use manager::{CacheManager, CachedObject, CachedPackage};
pub use sui_sandbox_types::normalize_address;

// Re-export the canonical CachedTransaction from sui-sandbox-types
pub use sui_sandbox_types::{CachedTransaction, TransactionCache};

/// Statistics about the cache contents.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of unique packages indexed
    pub package_count: usize,
    /// Number of unique objects indexed
    pub object_count: usize,
    /// Number of cached transactions
    pub transaction_count: usize,
    /// Total size on disk in bytes
    pub disk_size_bytes: u64,
}
