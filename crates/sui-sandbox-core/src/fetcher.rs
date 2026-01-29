//! Data Fetcher Abstraction
//!
//! This module provides the `Fetcher` trait for abstracting data fetching operations.
//! This allows the simulation environment to work with different data sources:
//! - Real mainnet/testnet via gRPC
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
/// (gRPC clients, etc.) should use lazy initialization to allow the
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

/// Adapter that wraps TransactionFetcher to implement the Fetcher trait.
///
/// This provides backward compatibility with existing code while enabling
/// the new trait-based abstraction.
pub struct GrpcFetcher {
    inner: crate::benchmark::tx_replay::TransactionFetcher,
    network: String,
}

impl GrpcFetcher {
    /// Create a new gRPC fetcher for mainnet.
    pub fn mainnet() -> Self {
        Self {
            inner: crate::benchmark::tx_replay::TransactionFetcher::mainnet(),
            network: "mainnet".to_string(),
        }
    }

    /// Create a new gRPC fetcher for mainnet with archive support.
    pub fn mainnet_with_archive() -> Self {
        Self {
            inner: crate::benchmark::tx_replay::TransactionFetcher::mainnet_with_archive(),
            network: "mainnet".to_string(),
        }
    }

    /// Create a new gRPC fetcher for testnet.
    pub fn testnet() -> Self {
        Self {
            inner: crate::benchmark::tx_replay::TransactionFetcher::new(
                "https://fullnode.testnet.sui.io:443",
            ),
            network: "testnet".to_string(),
        }
    }

    /// Create a new gRPC fetcher with a custom endpoint.
    pub fn custom(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        Self {
            inner: crate::benchmark::tx_replay::TransactionFetcher::new(&endpoint),
            network: format!("custom({})", endpoint),
        }
    }

    /// Create a gRPC fetcher from a FetcherConfig.
    pub fn from_config(config: &crate::benchmark::simulation::FetcherConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        Some(if let Some(endpoint) = &config.endpoint {
            Self::custom(endpoint.clone())
        } else if let Some(network) = &config.network {
            match network.as_str() {
                "testnet" => Self::testnet(),
                _ => {
                    if config.use_archive {
                        Self::mainnet_with_archive()
                    } else {
                        Self::mainnet()
                    }
                }
            }
        } else if config.use_archive {
            Self::mainnet_with_archive()
        } else {
            Self::mainnet()
        })
    }

    /// Get the underlying TransactionFetcher for advanced operations.
    pub fn inner(&self) -> &crate::benchmark::tx_replay::TransactionFetcher {
        &self.inner
    }
}

impl Fetcher for GrpcFetcher {
    fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.fetch_package_modules(package_id)
    }

    fn fetch_object(&self, object_id: &str) -> Result<FetchedObjectData> {
        let fetched = self.inner.fetch_object_full(object_id)?;
        Ok(FetchedObjectData {
            bcs_bytes: fetched.bcs_bytes,
            type_string: fetched.type_string,
            is_shared: fetched.is_shared,
            is_immutable: fetched.is_immutable,
            version: fetched.version,
        })
    }

    fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<FetchedObjectData> {
        let fetched = self
            .inner
            .fetch_object_at_version_full(object_id, version)?;
        Ok(FetchedObjectData {
            bcs_bytes: fetched.bcs_bytes,
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
    fn test_grpc_fetcher_from_config_disabled() {
        let config = crate::benchmark::simulation::FetcherConfig::default();
        assert!(GrpcFetcher::from_config(&config).is_none());
    }

    #[test]
    fn test_grpc_fetcher_from_config_mainnet() {
        let config = crate::benchmark::simulation::FetcherConfig::mainnet();
        let fetcher = GrpcFetcher::from_config(&config).unwrap();
        assert_eq!(fetcher.network_name(), "mainnet");
    }

    #[test]
    fn test_grpc_fetcher_from_config_testnet() {
        let config = crate::benchmark::simulation::FetcherConfig::testnet();
        let fetcher = GrpcFetcher::from_config(&config).unwrap();
        assert_eq!(fetcher.network_name(), "testnet");
    }
}
