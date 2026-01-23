//! Transaction Dependency Cache
//!
//! Records what data was actually needed to replay a transaction.
//! This is pure observation - no assumptions about protocols or patterns.
//!
//! # Purpose
//!
//! When replaying a transaction, we often discover dependencies during execution:
//! - Missing packages (discovered on linker error, retry)
//! - Dynamic field children (fetched on-demand during execution)
//! - Historical object versions (found via binary search)
//!
//! This cache records what was actually needed so that:
//! 1. Replaying the same transaction again is instant (no re-discovery)
//! 2. We can analyze dependency patterns across transactions
//! 3. Prefetching can be based on known requirements
//!
//! # Design Principles
//!
//! - **Pure observation**: Only records what happened, no predictions
//! - **No protocol knowledge**: Doesn't know what "Cetus" or "DeepBook" is
//! - **Append-friendly**: New discoveries are added, never removed
//! - **Separate from data cache**: Dependencies != cached data

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::normalize::normalize_address;

/// Complete dependency record for a transaction.
///
/// Records everything that was fetched/needed to successfully replay
/// a transaction. This is populated during replay and saved afterward.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionDependency {
    /// Transaction digest
    pub digest: String,

    /// Checkpoint at which this transaction was executed
    #[serde(default)]
    pub checkpoint: Option<u64>,

    /// Transaction sender
    #[serde(default)]
    pub sender: Option<String>,

    /// Packages that were loaded (in order of loading)
    pub packages: Vec<PackageDependency>,

    /// Objects that were fetched as transaction inputs
    pub input_objects: Vec<ObjectDependency>,

    /// Dynamic field children that were accessed during execution
    #[serde(default)]
    pub dynamic_fields: Vec<DynamicFieldDependency>,

    /// Package address aliases (on-chain address -> bytecode self-address)
    #[serde(default)]
    pub address_aliases: HashMap<String, String>,

    /// Fetch statistics for analysis
    #[serde(default)]
    pub fetch_stats: FetchStats,

    /// When this dependency record was created
    pub recorded_at: u64,

    /// Whether the replay was successful with these dependencies
    pub replay_successful: bool,

    /// Number of retries needed to discover all dependencies
    #[serde(default)]
    pub retries_needed: u32,
}

/// A package that was needed for the transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDependency {
    /// Package address (normalized)
    pub address: String,

    /// How this package was discovered
    pub discovery: DependencyDiscovery,

    /// Package version (if known)
    #[serde(default)]
    pub version: Option<u64>,

    /// If this is an upgraded package, the original address
    #[serde(default)]
    pub original_address: Option<String>,

    /// Module names in this package
    #[serde(default)]
    pub module_names: Vec<String>,
}

/// An object that was needed as a transaction input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectDependency {
    /// Object address (normalized)
    pub address: String,

    /// Version that was used
    pub version: u64,

    /// Type string (e.g., "0x2::coin::Coin<0x2::sui::SUI>")
    #[serde(default)]
    pub type_string: Option<String>,

    /// How this object was fetched
    pub fetch_method: FetchMethod,

    /// Whether this is a shared object
    #[serde(default)]
    pub is_shared: bool,

    /// Role in the transaction (if identifiable)
    #[serde(default)]
    pub role: Option<String>,
}

/// A dynamic field child that was accessed during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFieldDependency {
    /// Parent object ID
    pub parent_id: String,

    /// Child object ID (derived from parent + key)
    pub child_id: String,

    /// Key type (e.g., "u64", "vector<u8>")
    pub key_type: String,

    /// Key value (serialized as string for readability)
    #[serde(default)]
    pub key_value: Option<String>,

    /// Child type string
    #[serde(default)]
    pub child_type: Option<String>,

    /// Version used
    pub version: u64,

    /// How this was fetched
    pub fetch_method: FetchMethod,
}

/// How a dependency was discovered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyDiscovery {
    /// Listed in transaction's package references
    TransactionReference,
    /// Discovered during execution (linker error -> retry)
    ExecutionDiscovery,
    /// Transitive dependency of another package
    TransitiveDependency,
    /// Pre-known from previous replay
    Cached,
}

/// How an object/package was fetched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FetchMethod {
    /// Direct fetch at known version
    Direct,
    /// Fetched from gRPC transaction data (already had it)
    GrpcTransactionData,
    /// Required binary search to find correct version
    BinarySearch {
        /// Number of iterations needed
        iterations: u32,
    },
    /// Fetched at historical version via archive
    HistoricalArchive,
    /// Fetched current version (fallback)
    CurrentFallback,
    /// Loaded from local cache
    Cache,
}

/// Statistics about fetching for this transaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetchStats {
    /// Total packages loaded
    pub packages_loaded: u32,
    /// Packages discovered on retry (not upfront)
    pub packages_from_retry: u32,
    /// Total objects fetched
    pub objects_fetched: u32,
    /// Objects that required binary search
    pub objects_binary_searched: u32,
    /// Total binary search iterations across all objects
    pub total_binary_search_iterations: u32,
    /// Dynamic fields accessed
    pub dynamic_fields_accessed: u32,
    /// Dynamic fields fetched via historical version
    pub dynamic_fields_historical: u32,
    /// Dynamic fields fetched via current fallback
    pub dynamic_fields_fallback: u32,
}

impl TransactionDependency {
    /// Create a new empty dependency record for a transaction.
    pub fn new(digest: &str) -> Self {
        Self {
            digest: digest.to_string(),
            checkpoint: None,
            sender: None,
            packages: Vec::new(),
            input_objects: Vec::new(),
            dynamic_fields: Vec::new(),
            address_aliases: HashMap::new(),
            fetch_stats: FetchStats::default(),
            recorded_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            replay_successful: false,
            retries_needed: 0,
        }
    }

    /// Add a package dependency.
    pub fn add_package(&mut self, address: &str, discovery: DependencyDiscovery) {
        let normalized = normalize_address(address);

        // Check if already recorded
        if self.packages.iter().any(|p| p.address == normalized) {
            return;
        }

        self.packages.push(PackageDependency {
            address: normalized,
            discovery: discovery.clone(),
            version: None,
            original_address: None,
            module_names: Vec::new(),
        });

        self.fetch_stats.packages_loaded += 1;
        if discovery == DependencyDiscovery::ExecutionDiscovery {
            self.fetch_stats.packages_from_retry += 1;
        }
    }

    /// Add a package with full details.
    pub fn add_package_full(
        &mut self,
        address: &str,
        discovery: DependencyDiscovery,
        version: Option<u64>,
        original_address: Option<&str>,
        module_names: Vec<String>,
    ) {
        let normalized = normalize_address(address);

        // Update if exists, otherwise add
        if let Some(pkg) = self.packages.iter_mut().find(|p| p.address == normalized) {
            pkg.version = version.or(pkg.version);
            pkg.original_address = original_address.map(|s| normalize_address(s)).or(pkg.original_address.clone());
            if !module_names.is_empty() {
                pkg.module_names = module_names;
            }
            return;
        }

        self.packages.push(PackageDependency {
            address: normalized,
            discovery: discovery.clone(),
            version,
            original_address: original_address.map(|s| normalize_address(s)),
            module_names,
        });

        self.fetch_stats.packages_loaded += 1;
        if discovery == DependencyDiscovery::ExecutionDiscovery {
            self.fetch_stats.packages_from_retry += 1;
        }
    }

    /// Add an object dependency.
    pub fn add_object(
        &mut self,
        address: &str,
        version: u64,
        type_string: Option<String>,
        fetch_method: FetchMethod,
        is_shared: bool,
    ) {
        let normalized = normalize_address(address);

        // Check if already recorded at this version
        if self
            .input_objects
            .iter()
            .any(|o| o.address == normalized && o.version == version)
        {
            return;
        }

        // Track binary search stats
        if let FetchMethod::BinarySearch { iterations } = &fetch_method {
            self.fetch_stats.objects_binary_searched += 1;
            self.fetch_stats.total_binary_search_iterations += *iterations;
        }

        self.input_objects.push(ObjectDependency {
            address: normalized,
            version,
            type_string,
            fetch_method,
            is_shared,
            role: None,
        });

        self.fetch_stats.objects_fetched += 1;
    }

    /// Add a dynamic field dependency.
    pub fn add_dynamic_field(
        &mut self,
        parent_id: &str,
        child_id: &str,
        key_type: &str,
        key_value: Option<String>,
        child_type: Option<String>,
        version: u64,
        fetch_method: FetchMethod,
    ) {
        let parent_normalized = normalize_address(parent_id);
        let child_normalized = normalize_address(child_id);

        // Check if already recorded
        if self
            .dynamic_fields
            .iter()
            .any(|df| df.child_id == child_normalized)
        {
            return;
        }

        // Track stats
        match &fetch_method {
            FetchMethod::HistoricalArchive | FetchMethod::GrpcTransactionData => {
                self.fetch_stats.dynamic_fields_historical += 1;
            }
            FetchMethod::CurrentFallback => {
                self.fetch_stats.dynamic_fields_fallback += 1;
            }
            _ => {}
        }

        self.dynamic_fields.push(DynamicFieldDependency {
            parent_id: parent_normalized,
            child_id: child_normalized,
            key_type: key_type.to_string(),
            key_value,
            child_type,
            version,
            fetch_method,
        });

        self.fetch_stats.dynamic_fields_accessed += 1;
    }

    /// Record an address alias (on-chain -> bytecode self-address).
    pub fn add_address_alias(&mut self, on_chain: &str, bytecode: &str) {
        let on_chain_norm = normalize_address(on_chain);
        let bytecode_norm = normalize_address(bytecode);

        if on_chain_norm != bytecode_norm {
            self.address_aliases.insert(on_chain_norm, bytecode_norm);
        }
    }

    /// Mark the replay as successful.
    pub fn mark_successful(&mut self) {
        self.replay_successful = true;
    }

    /// Increment retry count.
    pub fn increment_retries(&mut self) {
        self.retries_needed += 1;
    }

    /// Get all package addresses that were needed.
    pub fn package_addresses(&self) -> Vec<&str> {
        self.packages.iter().map(|p| p.address.as_str()).collect()
    }

    /// Get all object addresses that were needed.
    pub fn object_addresses(&self) -> Vec<(&str, u64)> {
        self.input_objects
            .iter()
            .map(|o| (o.address.as_str(), o.version))
            .collect()
    }

    /// Get packages that were discovered on retry (not upfront).
    pub fn packages_from_retry(&self) -> Vec<&PackageDependency> {
        self.packages
            .iter()
            .filter(|p| p.discovery == DependencyDiscovery::ExecutionDiscovery)
            .collect()
    }

    /// Get objects that required binary search.
    pub fn objects_needing_binary_search(&self) -> Vec<&ObjectDependency> {
        self.input_objects
            .iter()
            .filter(|o| matches!(o.fetch_method, FetchMethod::BinarySearch { .. }))
            .collect()
    }

    /// Check if this transaction had any "expensive" fetches.
    pub fn had_expensive_fetches(&self) -> bool {
        self.fetch_stats.packages_from_retry > 0
            || self.fetch_stats.objects_binary_searched > 0
            || self.fetch_stats.dynamic_fields_fallback > 0
    }
}

/// Cache manager for transaction dependencies.
///
/// Stores dependency records in a separate directory from the main data cache.
/// This allows dependency tracking without modifying the existing cache format.
pub struct DependencyCache {
    /// Cache directory path
    cache_dir: PathBuf,
}

impl DependencyCache {
    /// Create a new dependency cache.
    pub fn new<P: AsRef<Path>>(cache_dir: P) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Get the file path for a transaction's dependencies.
    fn dep_path(&self, digest: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.deps.json", digest))
    }

    /// Check if dependencies are cached for a transaction.
    pub fn has(&self, digest: &str) -> bool {
        self.dep_path(digest).exists()
    }

    /// Load cached dependencies for a transaction.
    pub fn load(&self, digest: &str) -> Result<TransactionDependency> {
        let path = self.dep_path(digest);
        let content = fs::read_to_string(&path)?;
        let deps: TransactionDependency = serde_json::from_str(&content)?;
        Ok(deps)
    }

    /// Save dependencies for a transaction.
    pub fn save(&self, deps: &TransactionDependency) -> Result<()> {
        let path = self.dep_path(&deps.digest);
        let content = serde_json::to_string_pretty(deps)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// List all cached transaction digests.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut digests = Vec::new();
        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".deps.json") {
                    let digest = name.trim_end_matches(".deps.json");
                    digests.push(digest.to_string());
                }
            }
        }
        Ok(digests)
    }

    /// Get the number of cached dependency records.
    pub fn count(&self) -> usize {
        self.list().map(|l| l.len()).unwrap_or(0)
    }

    /// Get aggregate statistics across all cached dependencies.
    pub fn aggregate_stats(&self) -> Result<AggregateStats> {
        let mut stats = AggregateStats::default();

        for digest in self.list()? {
            if let Ok(deps) = self.load(&digest) {
                stats.total_transactions += 1;
                if deps.replay_successful {
                    stats.successful_replays += 1;
                }
                stats.total_packages += deps.packages.len();
                stats.total_objects += deps.input_objects.len();
                stats.total_dynamic_fields += deps.dynamic_fields.len();
                stats.total_retries += deps.retries_needed as usize;
                stats.total_binary_search_iterations +=
                    deps.fetch_stats.total_binary_search_iterations as usize;

                // Track packages discovered on retry
                stats.packages_from_retry += deps
                    .packages
                    .iter()
                    .filter(|p| p.discovery == DependencyDiscovery::ExecutionDiscovery)
                    .count();
            }
        }

        Ok(stats)
    }

    /// Find transactions that used a specific package.
    pub fn find_by_package(&self, package_address: &str) -> Result<Vec<String>> {
        let normalized = normalize_address(package_address);
        let mut results = Vec::new();

        for digest in self.list()? {
            if let Ok(deps) = self.load(&digest) {
                if deps.packages.iter().any(|p| p.address == normalized) {
                    results.push(digest);
                }
            }
        }

        Ok(results)
    }

    /// Find transactions that had expensive fetches (retries, binary search, etc).
    pub fn find_expensive(&self) -> Result<Vec<String>> {
        let mut results = Vec::new();

        for digest in self.list()? {
            if let Ok(deps) = self.load(&digest) {
                if deps.had_expensive_fetches() {
                    results.push(digest);
                }
            }
        }

        Ok(results)
    }
}

/// Aggregate statistics across all cached dependencies.
#[derive(Debug, Clone, Default)]
pub struct AggregateStats {
    pub total_transactions: usize,
    pub successful_replays: usize,
    pub total_packages: usize,
    pub total_objects: usize,
    pub total_dynamic_fields: usize,
    pub total_retries: usize,
    pub total_binary_search_iterations: usize,
    pub packages_from_retry: usize,
}

impl std::fmt::Display for AggregateStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Dependency Cache Statistics:")?;
        writeln!(f, "  Transactions: {}", self.total_transactions)?;
        writeln!(
            f,
            "  Successful:   {} ({:.1}%)",
            self.successful_replays,
            if self.total_transactions > 0 {
                100.0 * self.successful_replays as f64 / self.total_transactions as f64
            } else {
                0.0
            }
        )?;
        writeln!(f, "  Packages:     {}", self.total_packages)?;
        writeln!(f, "  Objects:      {}", self.total_objects)?;
        writeln!(f, "  Dyn Fields:   {}", self.total_dynamic_fields)?;
        writeln!(f, "  Total Retries: {}", self.total_retries)?;
        writeln!(
            f,
            "  Binary Search Iterations: {}",
            self.total_binary_search_iterations
        )?;
        writeln!(
            f,
            "  Packages from Retry: {}",
            self.packages_from_retry
        )
    }
}

/// Live dependency recorder for use during transaction replay.
///
/// This wraps a `TransactionDependency` and provides a convenient API
/// for recording dependencies as they are discovered during execution.
///
/// # Usage
///
/// ```
/// use sui_move_interface_extractor::cache::dependency::{DependencyRecorder, DependencyDiscovery, FetchMethod};
///
/// let mut recorder = DependencyRecorder::new("tx_digest_123");
/// recorder.set_checkpoint(234219761);
/// recorder.set_sender("0x1234");
///
/// // During package loading
/// recorder.record_package("0x2", DependencyDiscovery::TransactionReference);
///
/// // During object fetching
/// recorder.record_object("0x6", 100, Some("Clock"), FetchMethod::Direct, true);
///
/// // After execution
/// recorder.mark_successful();
///
/// // Get the recorded dependencies
/// let deps = recorder.finish();
/// assert_eq!(deps.digest, "tx_digest_123");
/// ```
pub struct DependencyRecorder {
    inner: TransactionDependency,
}

impl DependencyRecorder {
    /// Create a new dependency recorder for a transaction.
    pub fn new(digest: &str) -> Self {
        Self {
            inner: TransactionDependency::new(digest),
        }
    }

    /// Set the checkpoint number.
    pub fn set_checkpoint(&mut self, checkpoint: u64) {
        self.inner.checkpoint = Some(checkpoint);
    }

    /// Set the sender address.
    pub fn set_sender(&mut self, sender: &str) {
        self.inner.sender = Some(normalize_address(sender));
    }

    /// Record a package dependency.
    pub fn record_package(&mut self, address: &str, discovery: DependencyDiscovery) {
        self.inner.add_package(address, discovery);
    }

    /// Record a package with full details.
    pub fn record_package_full(
        &mut self,
        address: &str,
        discovery: DependencyDiscovery,
        version: Option<u64>,
        original_address: Option<&str>,
        module_names: Vec<String>,
    ) {
        self.inner
            .add_package_full(address, discovery, version, original_address, module_names);
    }

    /// Record an object dependency.
    pub fn record_object(
        &mut self,
        address: &str,
        version: u64,
        type_string: Option<&str>,
        fetch_method: FetchMethod,
        is_shared: bool,
    ) {
        self.inner.add_object(
            address,
            version,
            type_string.map(|s| s.to_string()),
            fetch_method,
            is_shared,
        );
    }

    /// Record a dynamic field dependency.
    pub fn record_dynamic_field(
        &mut self,
        parent_id: &str,
        child_id: &str,
        key_type: &str,
        key_value: Option<&str>,
        child_type: Option<&str>,
        version: u64,
        fetch_method: FetchMethod,
    ) {
        self.inner.add_dynamic_field(
            parent_id,
            child_id,
            key_type,
            key_value.map(|s| s.to_string()),
            child_type.map(|s| s.to_string()),
            version,
            fetch_method,
        );
    }

    /// Record an address alias.
    pub fn record_alias(&mut self, on_chain: &str, bytecode: &str) {
        self.inner.add_address_alias(on_chain, bytecode);
    }

    /// Increment retry count (call when a retry happens).
    pub fn record_retry(&mut self) {
        self.inner.increment_retries();
    }

    /// Mark the replay as successful.
    pub fn mark_successful(&mut self) {
        self.inner.mark_successful();
    }

    /// Get a reference to the inner dependency record.
    pub fn inner(&self) -> &TransactionDependency {
        &self.inner
    }

    /// Get a mutable reference to the inner dependency record.
    pub fn inner_mut(&mut self) -> &mut TransactionDependency {
        &mut self.inner
    }

    /// Consume the recorder and return the dependency record.
    pub fn finish(self) -> TransactionDependency {
        self.inner
    }

    /// Get current fetch statistics.
    pub fn stats(&self) -> &FetchStats {
        &self.inner.fetch_stats
    }

    /// Check if any expensive fetches have been recorded.
    pub fn had_expensive_fetches(&self) -> bool {
        self.inner.had_expensive_fetches()
    }
}

/// Builder for creating a prefetch plan from cached dependencies.
///
/// When replaying a transaction that we've seen before, we can use
/// the cached dependency record to prefetch everything upfront.
pub struct PrefetchPlan {
    /// Packages to prefetch
    pub packages: Vec<String>,
    /// Objects to prefetch (address, version)
    pub objects: Vec<(String, u64)>,
    /// Dynamic fields to prefetch (parent, child, version)
    pub dynamic_fields: Vec<(String, String, u64)>,
    /// Address aliases to apply
    pub aliases: HashMap<String, String>,
}

impl PrefetchPlan {
    /// Create a prefetch plan from a cached dependency record.
    pub fn from_dependency(deps: &TransactionDependency) -> Self {
        Self {
            packages: deps.packages.iter().map(|p| p.address.clone()).collect(),
            objects: deps
                .input_objects
                .iter()
                .map(|o| (o.address.clone(), o.version))
                .collect(),
            dynamic_fields: deps
                .dynamic_fields
                .iter()
                .map(|df| (df.parent_id.clone(), df.child_id.clone(), df.version))
                .collect(),
            aliases: deps.address_aliases.clone(),
        }
    }

    /// Check if this plan is empty (no dependencies).
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.objects.is_empty() && self.dynamic_fields.is_empty()
    }

    /// Total number of items to prefetch.
    pub fn total_items(&self) -> usize {
        self.packages.len() + self.objects.len() + self.dynamic_fields.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_transaction_dependency_creation() {
        let mut deps = TransactionDependency::new("test_digest_123");
        assert_eq!(deps.digest, "test_digest_123");
        assert!(!deps.replay_successful);
        assert_eq!(deps.retries_needed, 0);

        // Add package
        deps.add_package("0x2", DependencyDiscovery::TransactionReference);
        assert_eq!(deps.packages.len(), 1);
        assert_eq!(deps.fetch_stats.packages_loaded, 1);
        assert_eq!(deps.fetch_stats.packages_from_retry, 0);

        // Add package discovered on retry
        deps.add_package("0xabc", DependencyDiscovery::ExecutionDiscovery);
        assert_eq!(deps.packages.len(), 2);
        assert_eq!(deps.fetch_stats.packages_from_retry, 1);

        // Add object
        deps.add_object(
            "0x123",
            100,
            Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()),
            FetchMethod::Direct,
            false,
        );
        assert_eq!(deps.input_objects.len(), 1);

        // Add object with binary search
        deps.add_object(
            "0x456",
            200,
            None,
            FetchMethod::BinarySearch { iterations: 47 },
            true,
        );
        assert_eq!(deps.fetch_stats.objects_binary_searched, 1);
        assert_eq!(deps.fetch_stats.total_binary_search_iterations, 47);

        // Add dynamic field
        deps.add_dynamic_field(
            "0xparent",
            "0xchild",
            "u64",
            Some("12345".to_string()),
            None,
            50,
            FetchMethod::HistoricalArchive,
        );
        assert_eq!(deps.dynamic_fields.len(), 1);
        assert_eq!(deps.fetch_stats.dynamic_fields_historical, 1);

        // Check expensive fetches
        assert!(deps.had_expensive_fetches());
    }

    #[test]
    fn test_dependency_cache_crud() -> Result<()> {
        let dir = tempdir()?;
        let cache = DependencyCache::new(dir.path())?;

        // Initially empty
        assert_eq!(cache.count(), 0);
        assert!(!cache.has("test_digest"));

        // Create and save
        let mut deps = TransactionDependency::new("test_digest");
        deps.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps.add_object("0x6", 1, Some("Clock".to_string()), FetchMethod::Direct, true);
        deps.mark_successful();
        cache.save(&deps)?;

        // Verify saved
        assert!(cache.has("test_digest"));
        assert_eq!(cache.count(), 1);

        // Load and verify
        let loaded = cache.load("test_digest")?;
        assert_eq!(loaded.digest, "test_digest");
        assert!(loaded.replay_successful);
        assert_eq!(loaded.packages.len(), 1);
        assert_eq!(loaded.input_objects.len(), 1);

        Ok(())
    }

    #[test]
    fn test_address_normalization() {
        let mut deps = TransactionDependency::new("test");

        // Add same package with different address formats
        deps.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps.add_package(
            "0x0000000000000000000000000000000000000000000000000000000000000002",
            DependencyDiscovery::TransactionReference,
        );

        // Should only have one entry (normalized)
        assert_eq!(deps.packages.len(), 1);
    }

    #[test]
    fn test_find_by_package() -> Result<()> {
        let dir = tempdir()?;
        let cache = DependencyCache::new(dir.path())?;

        // Create two transactions
        let mut deps1 = TransactionDependency::new("tx1");
        deps1.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps1.add_package("0xabc", DependencyDiscovery::TransactionReference);
        cache.save(&deps1)?;

        let mut deps2 = TransactionDependency::new("tx2");
        deps2.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps2.add_package("0xdef", DependencyDiscovery::TransactionReference);
        cache.save(&deps2)?;

        // Find by package
        let using_0x2 = cache.find_by_package("0x2")?;
        assert_eq!(using_0x2.len(), 2);

        let using_abc = cache.find_by_package("0xabc")?;
        assert_eq!(using_abc.len(), 1);
        assert_eq!(using_abc[0], "tx1");

        Ok(())
    }

    #[test]
    fn test_dependency_recorder() {
        let mut recorder = DependencyRecorder::new("test_tx");
        recorder.set_checkpoint(12345);
        recorder.set_sender("0xsender");

        // Record packages
        recorder.record_package("0x2", DependencyDiscovery::TransactionReference);
        recorder.record_package("0xabc", DependencyDiscovery::ExecutionDiscovery);

        // Record objects
        recorder.record_object("0x6", 100, Some("Clock"), FetchMethod::Direct, true);
        recorder.record_object(
            "0xhot",
            200,
            None,
            FetchMethod::BinarySearch { iterations: 25 },
            false,
        );

        // Record dynamic field
        recorder.record_dynamic_field(
            "0xparent",
            "0xchild",
            "u64",
            Some("999"),
            Some("SkipListNode"),
            50,
            FetchMethod::HistoricalArchive,
        );

        // Record alias
        recorder.record_alias("0xonchain", "0xbytecode");

        // Record retry
        recorder.record_retry();
        recorder.record_retry();

        // Mark successful
        recorder.mark_successful();

        // Check stats
        assert_eq!(recorder.stats().packages_loaded, 2);
        assert_eq!(recorder.stats().packages_from_retry, 1);
        assert_eq!(recorder.stats().objects_fetched, 2);
        assert_eq!(recorder.stats().objects_binary_searched, 1);
        assert_eq!(recorder.stats().total_binary_search_iterations, 25);
        assert_eq!(recorder.stats().dynamic_fields_accessed, 1);
        assert!(recorder.had_expensive_fetches());

        // Finish and verify
        let deps = recorder.finish();
        assert_eq!(deps.digest, "test_tx");
        assert_eq!(deps.checkpoint, Some(12345));
        assert!(deps.replay_successful);
        assert_eq!(deps.retries_needed, 2);
        assert_eq!(deps.packages.len(), 2);
        assert_eq!(deps.input_objects.len(), 2);
        assert_eq!(deps.dynamic_fields.len(), 1);
        assert_eq!(deps.address_aliases.len(), 1);
    }

    #[test]
    fn test_prefetch_plan() {
        // Create a dependency record
        let mut deps = TransactionDependency::new("test");
        deps.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps.add_package("0xabc", DependencyDiscovery::TransactionReference);
        deps.add_object("0x6", 100, None, FetchMethod::Direct, true);
        deps.add_object("0x123", 200, None, FetchMethod::Direct, false);
        deps.add_dynamic_field(
            "0xparent",
            "0xchild",
            "u64",
            None,
            None,
            50,
            FetchMethod::HistoricalArchive,
        );
        deps.add_address_alias("0xonchain", "0xbytecode");

        // Create prefetch plan
        let plan = PrefetchPlan::from_dependency(&deps);

        assert!(!plan.is_empty());
        assert_eq!(plan.total_items(), 5); // 2 packages + 2 objects + 1 df
        assert_eq!(plan.packages.len(), 2);
        assert_eq!(plan.objects.len(), 2);
        assert_eq!(plan.dynamic_fields.len(), 1);
        assert_eq!(plan.aliases.len(), 1);

        // Verify object versions are preserved
        let obj_versions: Vec<u64> = plan.objects.iter().map(|(_, v)| *v).collect();
        assert!(obj_versions.contains(&100));
        assert!(obj_versions.contains(&200));
    }

    #[test]
    fn test_aggregate_stats() -> Result<()> {
        let dir = tempdir()?;
        let cache = DependencyCache::new(dir.path())?;

        // Create transactions with varying characteristics
        let mut deps1 = TransactionDependency::new("tx1");
        deps1.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps1.add_object("0x6", 1, None, FetchMethod::Direct, true);
        deps1.mark_successful();
        cache.save(&deps1)?;

        let mut deps2 = TransactionDependency::new("tx2");
        deps2.add_package("0x2", DependencyDiscovery::TransactionReference);
        deps2.add_package("0xabc", DependencyDiscovery::ExecutionDiscovery);
        deps2.add_object(
            "0xhot",
            100,
            None,
            FetchMethod::BinarySearch { iterations: 30 },
            false,
        );
        deps2.increment_retries();
        deps2.mark_successful();
        cache.save(&deps2)?;

        let mut deps3 = TransactionDependency::new("tx3");
        deps3.add_package("0x2", DependencyDiscovery::TransactionReference);
        // Not successful
        cache.save(&deps3)?;

        // Check aggregate stats
        let stats = cache.aggregate_stats()?;
        assert_eq!(stats.total_transactions, 3);
        assert_eq!(stats.successful_replays, 2);
        assert_eq!(stats.total_packages, 4); // 1 + 2 + 1
        assert_eq!(stats.total_objects, 2);
        assert_eq!(stats.total_retries, 1);
        assert_eq!(stats.total_binary_search_iterations, 30);
        assert_eq!(stats.packages_from_retry, 1);

        Ok(())
    }
}
