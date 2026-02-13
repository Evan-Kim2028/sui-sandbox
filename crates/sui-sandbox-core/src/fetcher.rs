//! Data Fetcher Abstraction
//!
//! This module provides the `Fetcher` trait for abstracting data fetching operations.
//! This allows the simulation environment to work with different data sources:
//! - Real mainnet/testnet via gRPC
//! - Cached data for offline replay
//! - Mock data for testing
//!
//! The trait is intentionally minimal, containing only the methods needed by SimulationEnvironment.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use sui_sandbox_types::encoding::base64_decode;
use tokio::runtime::{Builder, Runtime};

use sui_transport::graphql::GraphQLClient;
use sui_transport::grpc::{historical_endpoint_and_api_key_from_env, GrpcClient, GrpcOwner};

use crate::simulation::FetcherConfig;

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

    /// Fetch all modules from a package at a specific checkpoint.
    ///
    /// This is critical for replay fidelity - packages may be upgraded over time,
    /// and we need the exact bytecode that was deployed at the time of the transaction.
    ///
    /// Default implementation falls back to `fetch_package_modules` (latest version).
    fn fetch_package_modules_at_checkpoint(
        &self,
        package_id: &str,
        _checkpoint: u64,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        // Default: fall back to latest (for backward compatibility)
        self.fetch_package_modules(package_id)
    }

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
        Err(anyhow!(
            "Fetching is disabled. Enable with with_mainnet_fetching() or with_fetcher_config()."
        ))
    }

    fn fetch_object(&self, _object_id: &str) -> Result<FetchedObjectData> {
        Err(anyhow!(
            "Fetching is disabled. Enable with with_mainnet_fetching() or with_fetcher_config()."
        ))
    }

    fn fetch_object_at_version(
        &self,
        _object_id: &str,
        _version: u64,
    ) -> Result<FetchedObjectData> {
        Err(anyhow!(
            "Fetching is disabled. Enable with with_mainnet_fetching() or with_fetcher_config()."
        ))
    }

    fn network_name(&self) -> &str {
        "none"
    }
}

/// Adapter that wraps gRPC client access to implement the Fetcher trait.
///
/// This provides backward compatibility with existing code while enabling
/// the new trait-based abstraction.
///
/// For checkpoint-based package queries (needed for replay fidelity), this uses
/// GraphQL as gRPC doesn't support historical package fetching.
pub struct GrpcFetcher {
    endpoint: String,
    api_key: Option<String>,
    network: String,
    runtime: Runtime,
    client: parking_lot::Mutex<Option<Arc<GrpcClient>>>,
    /// GraphQL client for checkpoint-based queries (lazy initialized)
    graphql_client: parking_lot::Mutex<Option<GraphQLClient>>,
}

impl GrpcFetcher {
    fn api_key_from_env() -> Option<String> {
        std::env::var("SUI_GRPC_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    }

    fn endpoint_from_env(var: &str, default: &str) -> String {
        std::env::var(var).unwrap_or_else(|_| default.to_string())
    }

    fn build_runtime() -> Runtime {
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for GrpcFetcher")
    }

    fn new(endpoint: String, network: String, api_key: Option<String>) -> Self {
        Self {
            endpoint,
            api_key,
            network,
            runtime: Self::build_runtime(),
            client: parking_lot::Mutex::new(None),
            graphql_client: parking_lot::Mutex::new(None),
        }
    }

    /// Create a new gRPC fetcher for mainnet.
    pub fn mainnet() -> Self {
        let endpoint =
            Self::endpoint_from_env("SUI_GRPC_ENDPOINT", "https://fullnode.mainnet.sui.io:443");
        Self::new(endpoint, "mainnet".to_string(), Self::api_key_from_env())
    }

    /// Create a new gRPC fetcher for mainnet with archive support.
    pub fn mainnet_with_archive() -> Self {
        let (endpoint, api_key) = historical_endpoint_and_api_key_from_env();
        Self::new(
            endpoint,
            "mainnet-archive".to_string(),
            api_key.or_else(Self::api_key_from_env),
        )
    }

    /// Create a new gRPC fetcher for testnet.
    pub fn testnet() -> Self {
        let endpoint = Self::endpoint_from_env(
            "SUI_GRPC_TESTNET_ENDPOINT",
            "https://fullnode.testnet.sui.io:443",
        );
        Self::new(endpoint, "testnet".to_string(), Self::api_key_from_env())
    }

    /// Create a new gRPC fetcher with a custom endpoint.
    pub fn custom(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        let network = format!("custom({})", endpoint);
        Self::new(endpoint, network, Self::api_key_from_env())
    }

    /// Create a gRPC fetcher from a FetcherConfig.
    pub fn from_config(config: &FetcherConfig) -> Option<Self> {
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

    /// Get the underlying gRPC client (initializes lazily).
    pub fn inner(&self) -> Result<Arc<GrpcClient>> {
        self.client()
    }

    fn client(&self) -> Result<Arc<GrpcClient>> {
        if let Some(client) = self.client.lock().as_ref() {
            return Ok(client.clone());
        }

        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let client =
            self.block_on(async move { GrpcClient::with_api_key(&endpoint, api_key).await })?;
        let client = Arc::new(client);
        *self.client.lock() = Some(client.clone());
        Ok(client)
    }

    fn block_on<F, T>(&self, fut: F) -> Result<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            let handle = self.runtime.handle().clone();
            let join = std::thread::spawn(move || handle.block_on(fut));
            join.join()
                .map_err(|_| anyhow!("gRPC runtime thread panicked"))?
        } else {
            self.runtime.block_on(fut)
        }
    }

    /// Get GraphQL client for checkpoint-based queries (lazy initialization).
    fn graphql_client(&self) -> GraphQLClient {
        if let Some(client) = self.graphql_client.lock().as_ref() {
            return client.clone();
        }

        // Determine GraphQL endpoint based on network
        let client = if self.network.contains("testnet") {
            GraphQLClient::testnet()
        } else {
            GraphQLClient::mainnet()
        };

        *self.graphql_client.lock() = Some(client.clone());
        client
    }

    fn to_fetched(
        object_id: &str,
        obj: sui_transport::grpc::GrpcObject,
    ) -> Result<FetchedObjectData> {
        let bcs_bytes = obj
            .bcs
            .ok_or_else(|| anyhow!("gRPC object {} missing BCS bytes", object_id))?;
        let is_shared = matches!(obj.owner, GrpcOwner::Shared { .. });
        let is_immutable = matches!(obj.owner, GrpcOwner::Immutable);
        Ok(FetchedObjectData {
            bcs_bytes,
            type_string: obj.type_string,
            is_shared,
            is_immutable,
            version: obj.version,
        })
    }
}

impl Fetcher for GrpcFetcher {
    fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let client = self.client()?;
        let package_id = package_id.to_string();
        let fetch_id = package_id.clone();
        let object = self.block_on(async move { client.get_object(&fetch_id).await })?;
        let object = object.ok_or_else(|| anyhow!("package not found: {}", package_id))?;
        object
            .package_modules
            .ok_or_else(|| anyhow!("object is not a package: {}", package_id))
    }

    fn fetch_package_modules_at_checkpoint(
        &self,
        package_id: &str,
        checkpoint: u64,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        // Use GraphQL for checkpoint-based package fetching since gRPC doesn't support it
        let graphql = self.graphql_client();
        let pkg = graphql.fetch_package_at_checkpoint(package_id, checkpoint)?;

        // Convert GraphQL modules to (name, bytes) pairs
        let mut modules = Vec::with_capacity(pkg.modules.len());
        for module in pkg.modules {
            let bytes = module
                .bytecode_base64
                .as_ref()
                .ok_or_else(|| anyhow!("module {} missing bytecode", module.name))
                .and_then(|b64| base64_decode(b64, &format!("module {} bytecode", module.name)))?;
            modules.push((module.name, bytes));
        }
        Ok(modules)
    }

    fn fetch_object(&self, object_id: &str) -> Result<FetchedObjectData> {
        let client = self.client()?;
        let object_id = object_id.to_string();
        let fetch_id = object_id.clone();
        let object = self.block_on(async move { client.get_object(&fetch_id).await })?;
        let object = object.ok_or_else(|| anyhow!("object not found: {}", object_id))?;
        Self::to_fetched(&object_id, object)
    }

    fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<FetchedObjectData> {
        let client = self.client()?;
        let object_id = object_id.to_string();
        let fetch_id = object_id.clone();
        let object =
            self.block_on(
                async move { client.get_object_at_version(&fetch_id, Some(version)).await },
            )?;
        let object = object
            .ok_or_else(|| anyhow!("object not found at version {}: {}", version, object_id))?;
        Self::to_fetched(&object_id, object)
    }

    fn network_name(&self) -> &str {
        &self.network
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
    packages: HashMap<String, Vec<(String, Vec<u8>)>>,
    /// Pre-loaded objects: object_id -> FetchedObjectData
    objects: HashMap<String, FetchedObjectData>,
    /// Pre-loaded versioned objects: (object_id, version) -> FetchedObjectData
    versioned_objects: HashMap<(String, u64), FetchedObjectData>,
    /// If set, all fetch calls will return this error
    force_error: Option<String>,
}

impl MockFetcher {
    /// Create a new mock fetcher with a given network name.
    pub fn new(network: &str) -> Self {
        Self {
            network: network.to_string(),
            packages: HashMap::new(),
            objects: HashMap::new(),
            versioned_objects: HashMap::new(),
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
    /// Delegates to the canonical implementation in sui-resolver for consistency.
    fn normalize_id(id: &str) -> String {
        sui_resolver::normalize_id(id)
    }
}

impl Fetcher for MockFetcher {
    fn fetch_package_modules(&self, package_id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        if let Some(ref error) = self.force_error {
            return Err(anyhow!("{}", error));
        }

        self.packages
            .get(&Self::normalize_id(package_id))
            .cloned()
            .ok_or_else(|| anyhow!("MockFetcher: package not found: {}", package_id))
    }

    fn fetch_object(&self, object_id: &str) -> Result<FetchedObjectData> {
        if let Some(ref error) = self.force_error {
            return Err(anyhow!("{}", error));
        }

        self.objects
            .get(&Self::normalize_id(object_id))
            .cloned()
            .ok_or_else(|| anyhow!("MockFetcher: object not found: {}", object_id))
    }

    fn fetch_object_at_version(&self, object_id: &str, version: u64) -> Result<FetchedObjectData> {
        if let Some(ref error) = self.force_error {
            return Err(anyhow!("{}", error));
        }

        let key = (Self::normalize_id(object_id), version);
        self.versioned_objects
            .get(&key)
            .cloned()
            .or_else(|| self.objects.get(&Self::normalize_id(object_id)).cloned())
            .ok_or_else(|| {
                anyhow!(
                    "MockFetcher: object not found: {} (version {})",
                    object_id,
                    version
                )
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
        let config = FetcherConfig::default();
        assert!(GrpcFetcher::from_config(&config).is_none());
    }

    #[test]
    fn test_grpc_fetcher_from_config_mainnet() {
        let config = FetcherConfig::mainnet();
        let fetcher = GrpcFetcher::from_config(&config).unwrap();
        assert_eq!(fetcher.network_name(), "mainnet");
    }

    #[test]
    fn test_grpc_fetcher_from_config_archive() {
        let config = FetcherConfig::mainnet_with_archive();
        let fetcher = GrpcFetcher::from_config(&config).unwrap();
        assert_eq!(fetcher.network_name(), "mainnet-archive");
    }
}
