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

// Re-export core types from sui-sandbox-core
pub use sui_sandbox_core::fetcher::{FetchedObjectData, Fetcher, MockFetcher, NoopFetcher};

// Import simulation types for FetcherConfig
use crate::benchmark::simulation::FetcherConfig;

/// Network fetcher that uses GraphQL for all queries.
///
/// This replaces the old TransactionFetcher-based implementation with
/// the unified DataFetcher which uses GraphQL as the primary backend.
pub struct NetworkFetcher {
    inner: crate::data_fetcher::DataFetcher,
    network: String,
}

impl NetworkFetcher {
    /// Create a new fetcher for mainnet.
    pub fn mainnet() -> Self {
        Self {
            inner: crate::data_fetcher::DataFetcher::mainnet(),
            network: "mainnet".to_string(),
        }
    }

    /// Create a new fetcher for mainnet with archive support.
    /// Note: Archive support is now handled automatically by GraphQL.
    pub fn mainnet_with_archive() -> Self {
        // GraphQL handles historical queries automatically
        Self::mainnet()
    }

    /// Create a new fetcher for testnet.
    pub fn testnet() -> Self {
        Self {
            inner: crate::data_fetcher::DataFetcher::testnet(),
            network: "testnet".to_string(),
        }
    }

    /// Create a new fetcher with a custom GraphQL endpoint.
    pub fn custom(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        Self {
            inner: crate::data_fetcher::DataFetcher::new(&endpoint),
            network: format!("custom({})", endpoint),
        }
    }

    /// Create a fetcher from a FetcherConfig.
    pub fn from_config(config: &FetcherConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        Some(if let Some(endpoint) = &config.endpoint {
            Self::custom(endpoint.clone())
        } else if let Some(network) = &config.network {
            match network.as_str() {
                "testnet" => Self::testnet(),
                _ => Self::mainnet(),
            }
        } else {
            Self::mainnet()
        })
    }

    /// Get the underlying DataFetcher for advanced operations.
    pub fn inner(&self) -> &crate::data_fetcher::DataFetcher {
        &self.inner
    }
}

impl Fetcher for NetworkFetcher {
    fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let pkg = self.inner.fetch_package(package_id)?;
        Ok(pkg
            .modules
            .into_iter()
            .map(|m| (m.name, m.bytecode))
            .collect())
    }

    fn fetch_object(&self, object_id: &str) -> Result<FetchedObjectData> {
        let fetched = self.inner.fetch_object(object_id)?;
        Ok(FetchedObjectData {
            bcs_bytes: fetched.bcs_bytes.unwrap_or_default(),
            type_string: fetched.type_string,
            is_shared: fetched.is_shared,
            is_immutable: fetched.is_immutable,
            version: fetched.version,
        })
    }

    fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<FetchedObjectData> {
        let fetched = self.inner.fetch_object_at_version(object_id, version)?;
        Ok(FetchedObjectData {
            bcs_bytes: fetched.bcs_bytes.unwrap_or_default(),
            type_string: fetched.type_string,
            is_shared: fetched.is_shared,
            is_immutable: fetched.is_immutable,
            version: fetched.version,
        })
    }

    fn network_name(&self) -> &str {
        &self.network
    }
}

/// Extension trait for SimulationEnvironment to enable network fetching.
///
/// This trait provides the `with_mainnet_fetching()` and similar methods that
/// were previously on SimulationEnvironment directly, but now require NetworkFetcher
/// which is only available in the main crate.
pub trait SimulationEnvironmentExt {
    /// Enable mainnet fetching for on-demand package/object loading.
    fn with_mainnet_fetching(self) -> Self;

    /// Enable mainnet fetching with archive support for historical data.
    fn with_mainnet_archive_fetching(self) -> Self;

    /// Enable fetching with a specific configuration and create the appropriate fetcher.
    fn with_network_fetcher_config(self, config: FetcherConfig) -> Self;
}

impl SimulationEnvironmentExt for crate::benchmark::simulation::SimulationEnvironment {
    fn with_mainnet_fetching(mut self) -> Self {
        self.set_fetcher(Box::new(NetworkFetcher::mainnet()));
        self.with_fetcher_config(FetcherConfig::mainnet())
    }

    fn with_mainnet_archive_fetching(mut self) -> Self {
        self.set_fetcher(Box::new(NetworkFetcher::mainnet_with_archive()));
        self.with_fetcher_config(FetcherConfig::mainnet_with_archive())
    }

    fn with_network_fetcher_config(mut self, config: FetcherConfig) -> Self {
        if let Some(fetcher) = NetworkFetcher::from_config(&config) {
            self.set_fetcher(Box::new(fetcher));
        }
        self.with_fetcher_config(config)
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
    fn test_network_fetcher_from_config() {
        // Disabled config returns None
        let disabled_config = FetcherConfig::default();
        assert!(NetworkFetcher::from_config(&disabled_config).is_none());

        // Mainnet config
        let mainnet_config = FetcherConfig::mainnet();
        let mainnet_fetcher = NetworkFetcher::from_config(&mainnet_config).unwrap();
        assert_eq!(mainnet_fetcher.network_name(), "mainnet");

        // Testnet config
        let testnet_config = FetcherConfig::testnet();
        let testnet_fetcher = NetworkFetcher::from_config(&testnet_config).unwrap();
        assert_eq!(testnet_fetcher.network_name(), "testnet");
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
    fn test_mock_fetcher_network_name() {
        let fetcher = MockFetcher::new("mock-mainnet");
        assert_eq!(fetcher.network_name(), "mock-mainnet");

        let fetcher = MockFetcher::default();
        assert_eq!(fetcher.network_name(), "");
    }
}
