//! Data Fetcher Abstraction
//!
//! This module provides the `Fetcher` trait for abstracting data fetching operations.
//! This allows the simulation environment to work with different data sources:
//! - Real mainnet/testnet via GraphQL
//! - Cached data for offline replay
//! - Mock data for testing
//!
//! The trait is intentionally minimal, containing only the methods needed by SimulationEnvironment.

use anyhow::Result;

/// Result of fetching an object from the network.
#[derive(Debug, Clone)]
pub struct FetchedObjectData {
    /// BCS-serialized object bytes.
    pub bcs_bytes: Vec<u8>,
    /// Type string (e.g., "0x2::coin::Coin<0x2::sui::SUI>").
    pub type_string: Option<String>,
    /// Whether this is a shared object.
    pub is_shared: bool,
    /// Whether this is an immutable object.
    pub is_immutable: bool,
    /// Object version.
    pub version: u64,
}

/// Trait for fetching data from Sui networks or other sources.
///
/// This abstraction allows the simulation environment to be decoupled from
/// the specific data source (mainnet, testnet, cached files, mocks, etc.).
///
/// ## Implementation Notes
///
/// Implementations should be stateless where possible. Connection state
/// (GraphQL clients, etc.) should use lazy initialization to allow the
/// fetcher to be cloned or serialized if needed.
pub trait Fetcher: Send + Sync {
    /// Fetch all modules from a deployed package.
    ///
    /// Returns a list of (module_name, module_bytes) pairs.
    fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>>;

    /// Fetch full object data including type, ownership, and BCS bytes.
    fn fetch_object(&self, object_id: &str) -> Result<FetchedObjectData>;

    /// Fetch object data at a specific version (for historical replay).
    fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<FetchedObjectData>;

    /// Get the network name this fetcher connects to (for logging/debugging).
    fn network_name(&self) -> &str;
}

/// A no-op fetcher that always returns errors.
/// Used when fetching is disabled.
pub struct NoopFetcher;

impl Fetcher for NoopFetcher {
    fn fetch_package_modules(&self, _package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        Err(anyhow::anyhow!(
            "Fetching is disabled. Enable with with_mainnet_fetching() or with_fetcher_config()."
        ))
    }

    fn fetch_object(&self, _object_id: &str) -> Result<FetchedObjectData> {
        Err(anyhow::anyhow!(
            "Fetching is disabled. Enable with with_mainnet_fetching() or with_fetcher_config()."
        ))
    }

    fn fetch_object_at_version(
        &self,
        _object_id: &str,
        _version: u64,
    ) -> Result<FetchedObjectData> {
        Err(anyhow::anyhow!(
            "Fetching is disabled. Enable with with_mainnet_fetching() or with_fetcher_config()."
        ))
    }

    fn network_name(&self) -> &str {
        "none"
    }
}

/// A mock fetcher for testing that returns pre-configured responses.
///
/// Use this in tests to avoid network dependencies and to test specific scenarios
/// like missing packages, corrupted data, or specific object states.
///
/// # Example
/// ```
/// use sui_sandbox_core::fetcher::{MockFetcher, FetchedObjectData, Fetcher};
///
/// let mut fetcher = MockFetcher::new("test");
///
/// // Add a mock package
/// fetcher.add_package("0x2", vec![
///     ("coin".to_string(), vec![0x01, 0x02, 0x03]),
/// ]);
///
/// // Add a mock object
/// fetcher.add_object("0x123", FetchedObjectData {
///     bcs_bytes: vec![0x01, 0x02],
///     type_string: Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()),
///     is_shared: false,
///     is_immutable: false,
///     version: 1,
/// });
///
/// // Use in tests
/// assert!(fetcher.fetch_package_modules("0x2").is_ok());
/// assert!(fetcher.fetch_package_modules("0x999").is_err()); // Not found
/// ```
#[derive(Debug, Clone, Default)]
pub struct MockFetcher {
    /// Network name for identification
    network: String,
    /// Pre-loaded package modules: package_id -> [(module_name, module_bytes)]
    packages: std::collections::HashMap<String, Vec<(String, Vec<u8>)>>,
    /// Pre-loaded objects: object_id -> FetchedObjectData
    objects: std::collections::HashMap<String, FetchedObjectData>,
    /// Pre-loaded versioned objects: (object_id, version) -> FetchedObjectData
    versioned_objects: std::collections::HashMap<(String, u64), FetchedObjectData>,
    /// If set, all fetch calls will return this error
    force_error: Option<String>,
}

impl MockFetcher {
    /// Create a new mock fetcher with a given network name.
    pub fn new(network: &str) -> Self {
        Self {
            network: network.to_string(),
            packages: std::collections::HashMap::new(),
            objects: std::collections::HashMap::new(),
            versioned_objects: std::collections::HashMap::new(),
            force_error: None,
        }
    }

    /// Add a package with its modules to the mock.
    pub fn add_package(&mut self, package_id: &str, modules: Vec<(String, Vec<u8>)>) -> &mut Self {
        self.packages
            .insert(Self::normalize_id(package_id), modules);
        self
    }

    /// Add an object to the mock.
    pub fn add_object(&mut self, object_id: &str, data: FetchedObjectData) -> &mut Self {
        self.objects.insert(Self::normalize_id(object_id), data);
        self
    }

    /// Add an object at a specific version to the mock.
    pub fn add_object_at_version(
        &mut self,
        object_id: &str,
        version: u64,
        data: FetchedObjectData,
    ) -> &mut Self {
        self.versioned_objects
            .insert((Self::normalize_id(object_id), version), data);
        self
    }

    /// Force all subsequent fetch calls to return the given error.
    /// Useful for testing error handling.
    pub fn set_error(&mut self, error: &str) -> &mut Self {
        self.force_error = Some(error.to_string());
        self
    }

    /// Clear the forced error, allowing normal mock behavior.
    pub fn clear_error(&mut self) -> &mut Self {
        self.force_error = None;
        self
    }

    /// Normalize object/package IDs to handle 0x prefix variations.
    fn normalize_id(id: &str) -> String {
        let id = id.trim();
        if id.starts_with("0x") || id.starts_with("0X") {
            id.to_lowercase()
        } else {
            format!("0x{}", id.to_lowercase())
        }
    }
}

impl Fetcher for MockFetcher {
    fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        if let Some(ref error) = self.force_error {
            return Err(anyhow::anyhow!("{}", error));
        }

        self.packages
            .get(&Self::normalize_id(package_id))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MockFetcher: package not found: {}", package_id))
    }

    fn fetch_object(&self, object_id: &str) -> Result<FetchedObjectData> {
        if let Some(ref error) = self.force_error {
            return Err(anyhow::anyhow!("{}", error));
        }

        self.objects
            .get(&Self::normalize_id(object_id))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MockFetcher: object not found: {}", object_id))
    }

    fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<FetchedObjectData> {
        if let Some(ref error) = self.force_error {
            return Err(anyhow::anyhow!("{}", error));
        }

        // First try versioned objects
        let key = (Self::normalize_id(object_id), version);
        if let Some(data) = self.versioned_objects.get(&key) {
            return Ok(data.clone());
        }

        // Fall back to regular objects if version matches
        if let Some(data) = self.objects.get(&Self::normalize_id(object_id)) {
            if data.version == version {
                return Ok(data.clone());
            }
        }

        Err(anyhow::anyhow!(
            "MockFetcher: object not found at version: {} @ v{}",
            object_id,
            version
        ))
    }

    fn network_name(&self) -> &str {
        &self.network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_fetcher_returns_errors() {
        let fetcher = NoopFetcher;
        assert!(fetcher.fetch_package_modules("0x1").is_err());
        assert!(fetcher.fetch_object("0x6").is_err());
        assert!(fetcher.fetch_object_at_version("0x6", 1).is_err());
        assert_eq!(fetcher.network_name(), "none");
    }

    #[test]
    fn test_mock_fetcher_packages() {
        let mut fetcher = MockFetcher::new("test");

        // Not found initially
        assert!(fetcher.fetch_package_modules("0x2").is_err());

        // Add a package
        fetcher.add_package(
            "0x2",
            vec![
                ("coin".to_string(), vec![0x01, 0x02, 0x03]),
                ("balance".to_string(), vec![0x04, 0x05]),
            ],
        );

        // Now it should be found
        let modules = fetcher.fetch_package_modules("0x2").unwrap();
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].0, "coin");
        assert_eq!(modules[0].1, vec![0x01, 0x02, 0x03]);

        // Test case normalization (0X vs 0x)
        assert!(fetcher.fetch_package_modules("0X2").is_ok());
    }

    #[test]
    fn test_mock_fetcher_objects() {
        let mut fetcher = MockFetcher::new("test");

        let obj_data = FetchedObjectData {
            bcs_bytes: vec![0x01, 0x02, 0x03],
            type_string: Some("0x2::coin::Coin<0x2::sui::SUI>".to_string()),
            is_shared: false,
            is_immutable: false,
            version: 5,
        };

        // Not found initially
        assert!(fetcher.fetch_object("0x123").is_err());

        // Add the object
        fetcher.add_object("0x123", obj_data.clone());

        // Now it should be found
        let fetched = fetcher.fetch_object("0x123").unwrap();
        assert_eq!(fetched.bcs_bytes, vec![0x01, 0x02, 0x03]);
        assert_eq!(fetched.version, 5);
        assert!(!fetched.is_shared);

        // Version mismatch
        assert!(fetcher.fetch_object_at_version("0x123", 10).is_err());

        // Correct version
        let fetched = fetcher.fetch_object_at_version("0x123", 5).unwrap();
        assert_eq!(fetched.version, 5);
    }

    #[test]
    fn test_mock_fetcher_versioned_objects() {
        let mut fetcher = MockFetcher::new("test");

        // Add same object at different versions
        fetcher.add_object_at_version(
            "0x123",
            1,
            FetchedObjectData {
                bcs_bytes: vec![0x01],
                type_string: None,
                is_shared: false,
                is_immutable: false,
                version: 1,
            },
        );
        fetcher.add_object_at_version(
            "0x123",
            5,
            FetchedObjectData {
                bcs_bytes: vec![0x05],
                type_string: None,
                is_shared: true,
                is_immutable: false,
                version: 5,
            },
        );

        // Fetch different versions
        let v1 = fetcher.fetch_object_at_version("0x123", 1).unwrap();
        assert_eq!(v1.bcs_bytes, vec![0x01]);
        assert!(!v1.is_shared);

        let v5 = fetcher.fetch_object_at_version("0x123", 5).unwrap();
        assert_eq!(v5.bcs_bytes, vec![0x05]);
        assert!(v5.is_shared);

        // Non-existent version
        assert!(fetcher.fetch_object_at_version("0x123", 3).is_err());
    }

    #[test]
    fn test_mock_fetcher_forced_errors() {
        let mut fetcher = MockFetcher::new("test");

        // Add some data
        fetcher.add_package("0x2", vec![("coin".to_string(), vec![0x01])]);
        fetcher.add_object(
            "0x123",
            FetchedObjectData {
                bcs_bytes: vec![0x01],
                type_string: None,
                is_shared: false,
                is_immutable: false,
                version: 1,
            },
        );

        // Data is accessible
        assert!(fetcher.fetch_package_modules("0x2").is_ok());
        assert!(fetcher.fetch_object("0x123").is_ok());

        // Force an error
        fetcher.set_error("simulated network failure");

        // All fetches now fail
        let err = fetcher.fetch_package_modules("0x2").unwrap_err();
        assert!(err.to_string().contains("simulated network failure"));

        let err = fetcher.fetch_object("0x123").unwrap_err();
        assert!(err.to_string().contains("simulated network failure"));

        // Clear the error
        fetcher.clear_error();

        // Data is accessible again
        assert!(fetcher.fetch_package_modules("0x2").is_ok());
        assert!(fetcher.fetch_object("0x123").is_ok());
    }

    #[test]
    fn test_mock_fetcher_network_name() {
        let fetcher = MockFetcher::new("mock-mainnet");
        assert_eq!(fetcher.network_name(), "mock-mainnet");

        let fetcher = MockFetcher::default();
        assert_eq!(fetcher.network_name(), "");
    }
}
