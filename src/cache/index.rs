//! In-memory index structures for cached data.
//!
//! These structures track metadata about cached packages and objects without
//! loading the actual bytecode into memory. The bytecode is loaded on-demand
//! from the source transaction file.

use std::path::PathBuf;

/// Cached package metadata entry.
///
/// Stores information about a cached package without the actual bytecode.
/// The bytecode is loaded on-demand from the source file.
#[derive(Debug, Clone)]
pub struct CachedPackageEntry {
    /// Normalized package address (0x + 64 hex chars)
    pub address: String,
    /// Package version (for upgraded packages)
    pub version: u64,
    /// Module names in this package
    pub module_names: Vec<String>,
    /// Path to the transaction file containing this package
    pub source_file: PathBuf,
}

/// Cached object metadata entry.
///
/// Stores information about a cached object without the actual BCS bytes.
/// The bytes are loaded on-demand from the source file.
#[derive(Debug, Clone)]
pub struct CachedObjectEntry {
    /// Normalized object address (0x + 64 hex chars)
    pub address: String,
    /// Object version
    pub version: u64,
    /// Type tag string (e.g., "0x2::coin::Coin<0x2::sui::SUI>")
    pub type_tag: Option<String>,
    /// Whether this object is shared
    pub is_shared: bool,
    /// Whether this object is immutable
    pub is_immutable: bool,
    /// Path to the transaction file containing this object
    pub source_file: PathBuf,
}

impl CachedPackageEntry {
    /// Create a new package entry.
    pub fn new(
        address: String,
        version: u64,
        module_names: Vec<String>,
        source_file: PathBuf,
    ) -> Self {
        Self {
            address,
            version,
            module_names,
            source_file,
        }
    }
}

impl CachedObjectEntry {
    /// Create a new object entry.
    pub fn new(
        address: String,
        version: u64,
        type_tag: Option<String>,
        source_file: PathBuf,
    ) -> Self {
        Self {
            address,
            version,
            type_tag,
            is_shared: false,
            is_immutable: false,
            source_file,
        }
    }

    /// Set whether this object is shared.
    pub fn with_shared(mut self, is_shared: bool) -> Self {
        self.is_shared = is_shared;
        self
    }

    /// Set whether this object is immutable.
    pub fn with_immutable(mut self, is_immutable: bool) -> Self {
        self.is_immutable = is_immutable;
        self
    }
}
