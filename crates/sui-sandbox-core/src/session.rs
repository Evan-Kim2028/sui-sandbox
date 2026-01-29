//! Simulation Session Management
//!
//! This module provides `SimulationSession` - a wrapper that separates runtime concerns
//! from persistent state. This enables cleaner save/load semantics where:
//!
//! - `SimulationEnvironment` holds the persistent state (objects, modules, coins, config)
//! - `SimulationSession` holds runtime components (fetcher, callbacks) plus the environment
//!
//! ## Usage
//!
//! ```ignore
//! // Create a session with mainnet fetching
//! let session = SimulationSession::new()?
//!     .with_mainnet_fetching();
//!
//! // Use the session for operations
//! session.deploy_package_from_mainnet("0x...")?;
//!
//! // Save just the state (runtime components are recreated on load)
//! session.save_state("./my-simulation.json")?;
//!
//! // Later: load the state into a new session
//! let session2 = SimulationSession::from_state_file("./my-simulation.json")?;
//! // The fetcher is automatically restored based on saved FetcherConfig
//! ```
//!
//! ## Architecture
//!
//! The session acts as a facade over `SimulationEnvironment`, forwarding most operations
//! while managing runtime-only components. This provides:
//!
//! - **Clean persistence**: State files only contain serializable data
//! - **Auto-reconnection**: Fetcher is automatically restored from config on load
//! - **Extensibility**: Easy to add other session-scoped components (metrics, logging)

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

use crate::fetcher::{Fetcher, GrpcFetcher};
use crate::object_runtime::ChildFetcherFn;
use crate::simulation::{FetcherConfig, PersistentState, SimulationEnvironment};

/// A session-aware wrapper around SimulationEnvironment.
///
/// This separates runtime concerns (fetcher, callbacks) from persistent state,
/// enabling cleaner save/load semantics.
pub struct SimulationSession {
    /// The underlying simulation environment (persistent state).
    env: SimulationEnvironment,
    /// The data fetcher for on-demand loading (runtime-only).
    fetcher: Option<Box<dyn Fetcher>>,
    /// Fetcher configuration (persisted with state).
    fetcher_config: FetcherConfig,
    /// Optional child fetcher callback (runtime-only).
    child_fetcher: Option<Arc<ChildFetcherFn>>,
}

impl SimulationSession {
    /// Create a new simulation session with default settings.
    pub fn new() -> Result<Self> {
        Ok(Self {
            env: SimulationEnvironment::new()?,
            fetcher: None,
            fetcher_config: FetcherConfig::default(),
            child_fetcher: None,
        })
    }

    /// Enable mainnet fetching for on-demand package/object loading.
    pub fn with_mainnet_fetching(mut self) -> Self {
        self.fetcher = Some(Box::new(GrpcFetcher::mainnet()));
        self.fetcher_config = FetcherConfig::mainnet();
        self
    }

    /// Enable mainnet fetching with archive support for historical data.
    pub fn with_mainnet_archive_fetching(mut self) -> Self {
        self.fetcher = Some(Box::new(GrpcFetcher::mainnet_with_archive()));
        self.fetcher_config = FetcherConfig::mainnet_with_archive();
        self
    }

    /// Enable fetching with a specific configuration.
    pub fn with_fetcher_config(mut self, config: FetcherConfig) -> Self {
        self.fetcher_config = config.clone();
        if let Some(fetcher) = GrpcFetcher::from_config(&config) {
            self.fetcher = Some(Box::new(fetcher));
        }
        self
    }

    /// Enable fetching with a custom fetcher implementation.
    pub fn with_fetcher(mut self, fetcher: Box<dyn Fetcher>, config: FetcherConfig) -> Self {
        self.fetcher = Some(fetcher);
        self.fetcher_config = config;
        self
    }

    /// Set a callback for on-demand child object fetching.
    pub fn with_child_fetcher(mut self, fetcher: ChildFetcherFn) -> Self {
        self.child_fetcher = Some(Arc::new(fetcher));
        self
    }

    /// Get the current fetcher configuration.
    pub fn fetcher_config(&self) -> &FetcherConfig {
        &self.fetcher_config
    }

    /// Check if fetching is enabled.
    pub fn is_fetching_enabled(&self) -> bool {
        self.fetcher.is_some() && self.fetcher_config.enabled
    }

    /// Get the network name of the current fetcher.
    pub fn fetcher_network(&self) -> &str {
        self.fetcher
            .as_ref()
            .map(|f| f.network_name())
            .unwrap_or("none")
    }

    // ========================================================================
    // State Persistence
    // ========================================================================

    /// Export the current state (without runtime components).
    pub fn export_state(&self) -> PersistentState {
        let mut state = self.env.export_state();
        // Ensure fetcher config is captured
        if self.fetcher_config.enabled {
            state.fetcher_config = Some(self.fetcher_config.clone());
        }
        state
    }

    /// Save the current state to a file.
    pub fn save_state(&self, path: impl AsRef<Path>) -> Result<()> {
        let state = self.export_state();
        let json = serde_json::to_string_pretty(&state)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Save the current state with custom metadata.
    pub fn save_state_with_metadata(
        &self,
        path: impl AsRef<Path>,
        description: Option<String>,
        tags: Vec<String>,
    ) -> Result<()> {
        let mut state = self.export_state();
        if let Some(ref mut metadata) = state.metadata {
            metadata.description = description;
            metadata.tags = tags;
        }
        let json = serde_json::to_string_pretty(&state)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load state from a file, restoring runtime components.
    pub fn load_state(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.env.load_state(path.as_ref())?;

        // Restore fetcher from the loaded state's config
        if let Some(config) = self.env.export_state().fetcher_config {
            if config.enabled {
                self.fetcher_config = config.clone();
                if let Some(fetcher) = GrpcFetcher::from_config(&config) {
                    self.fetcher = Some(Box::new(fetcher));
                }
            }
        }

        Ok(())
    }

    /// Create a new session from a saved state file.
    pub fn from_state_file(path: impl AsRef<Path>) -> Result<Self> {
        let mut session = Self::new()?;
        session.load_state(path)?;
        Ok(session)
    }

    // ========================================================================
    // Environment Access
    // ========================================================================

    /// Get immutable access to the underlying environment.
    pub fn env(&self) -> &SimulationEnvironment {
        &self.env
    }

    /// Get mutable access to the underlying environment.
    pub fn env_mut(&mut self) -> &mut SimulationEnvironment {
        &mut self.env
    }

    /// Get the fetcher (if enabled).
    pub fn fetcher(&self) -> Option<&dyn Fetcher> {
        self.fetcher.as_ref().map(|f| f.as_ref())
    }

    // ========================================================================
    // Delegated Operations
    // ========================================================================

    /// Deploy a package from mainnet using the session's fetcher.
    pub fn deploy_package_from_mainnet(
        &mut self,
        package_id: &str,
    ) -> Result<move_core_types::account_address::AccountAddress> {
        let fetcher = self
            .fetcher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Fetching not enabled"))?;

        let modules = fetcher.fetch_package_modules(package_id)?;
        let (count, _) = self.env.resolver_mut().add_package_modules(modules)?;

        if count == 0 {
            return Err(anyhow::anyhow!(
                "No modules loaded from package {}",
                package_id
            ));
        }

        move_core_types::account_address::AccountAddress::from_hex_literal(package_id)
            .map_err(|e| anyhow::anyhow!("Invalid package address: {}", e))
    }

    /// Fetch an object from mainnet using the session's fetcher.
    pub fn fetch_object_from_mainnet(
        &mut self,
        object_id: &str,
    ) -> Result<move_core_types::account_address::AccountAddress> {
        let fetcher = self
            .fetcher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Fetching not enabled"))?;

        let fetched = fetcher.fetch_object(object_id)?;
        self.env.load_object_from_data(
            object_id,
            fetched.bcs_bytes,
            fetched.type_string.as_deref(),
            fetched.is_shared,
            fetched.is_immutable,
            fetched.version,
        )
    }

    /// Reset the session, clearing all state but preserving configuration.
    pub fn reset(&mut self) -> Result<()> {
        self.env.reset()?;
        // Keep fetcher and config intact
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = SimulationSession::new().expect("create session");
        assert!(!session.is_fetching_enabled());
        assert_eq!(session.fetcher_network(), "none");
    }

    #[test]
    fn test_session_with_mainnet() {
        let session = SimulationSession::new()
            .expect("create session")
            .with_mainnet_fetching();
        assert!(session.is_fetching_enabled());
        assert_eq!(session.fetcher_network(), "mainnet");
    }

    #[test]
    fn test_session_export_includes_fetcher_config() {
        let session = SimulationSession::new()
            .expect("create session")
            .with_mainnet_fetching();

        let state = session.export_state();
        assert!(state.fetcher_config.is_some());
        let fc = state.fetcher_config.unwrap();
        assert!(fc.enabled);
        assert_eq!(fc.network, Some("mainnet".to_string()));
    }

    #[test]
    fn test_session_round_trip() {
        let dir = tempfile::TempDir::new().expect("create temp dir");
        let path = dir.path().join("session-state.json");

        // Create session with fetching and save
        {
            let session = SimulationSession::new()
                .expect("create session")
                .with_mainnet_fetching();
            session.save_state(&path).expect("save");
        }

        // Load into new session
        let session2 = SimulationSession::from_state_file(&path).expect("load");
        assert!(session2.is_fetching_enabled());
        assert_eq!(session2.fetcher_network(), "mainnet");
    }
}
