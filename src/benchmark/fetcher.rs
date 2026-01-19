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

/// Network fetcher that uses GraphQL for all queries.
///
/// This replaces the old TransactionFetcher-based implementation with
/// the unified DataFetcher which uses GraphQL as the primary backend.
pub struct NetworkFetcher {
    inner: crate::data_fetcher::DataFetcher,
    network: String,
}

/// Type alias for backwards compatibility.
#[deprecated(since = "0.6.0", note = "Renamed to NetworkFetcher")]
pub type GrpcFetcher = NetworkFetcher;

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
    pub fn from_config(config: &crate::benchmark::simulation::FetcherConfig) -> Option<Self> {
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
    fn test_network_fetcher_from_config_disabled() {
        let config = crate::benchmark::simulation::FetcherConfig::default();
        assert!(NetworkFetcher::from_config(&config).is_none());
    }

    #[test]
    fn test_network_fetcher_from_config_mainnet() {
        let config = crate::benchmark::simulation::FetcherConfig::mainnet();
        let fetcher = NetworkFetcher::from_config(&config).unwrap();
        assert_eq!(fetcher.network_name(), "mainnet");
    }

    #[test]
    fn test_network_fetcher_from_config_testnet() {
        let config = crate::benchmark::simulation::FetcherConfig::testnet();
        let fetcher = NetworkFetcher::from_config(&config).unwrap();
        assert_eq!(fetcher.network_name(), "testnet");
    }
}
