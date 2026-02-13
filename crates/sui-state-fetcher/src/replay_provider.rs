//! Replay state provider abstraction.
//!
//! This trait allows replay hydration to be sourced from different backends
//! (historical network sources, file-backed cache, in-memory fixtures) without
//! coupling callers to a specific provider implementation.

use anyhow::Result;

use crate::provider::HistoricalStateProvider;
use crate::replay_builder::ReplayStateConfig;
use crate::types::ReplayState;

/// Unified interface for replay state hydration backends.
#[async_trait::async_trait]
pub trait ReplayStateProvider: Send + Sync {
    /// Fetch replay state using provider defaults.
    async fn fetch_replay_state(&self, digest: &str) -> Result<ReplayState>;

    /// Fetch replay state using explicit hydration config.
    async fn fetch_replay_state_with_config(
        &self,
        digest: &str,
        config: &ReplayStateConfig,
    ) -> Result<ReplayState>;
}

#[async_trait::async_trait]
impl ReplayStateProvider for HistoricalStateProvider {
    async fn fetch_replay_state(&self, digest: &str) -> Result<ReplayState> {
        HistoricalStateProvider::fetch_replay_state(self, digest).await
    }

    async fn fetch_replay_state_with_config(
        &self,
        digest: &str,
        config: &ReplayStateConfig,
    ) -> Result<ReplayState> {
        HistoricalStateProvider::fetch_replay_state_with_config(
            self,
            digest,
            config.prefetch_dynamic_fields,
            config.df_depth,
            config.df_limit,
            config.auto_system_objects,
        )
        .await
    }
}
