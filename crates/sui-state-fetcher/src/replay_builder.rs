//! Replay state builder for CLI replay hydration.
//!
//! This provides a small abstraction layer to keep replay config and
//! hydration logic consistent across callers.

use anyhow::Result;

use crate::replay_provider::ReplayStateProvider;
use crate::types::ReplayState;

/// Configuration for building a replay state.
#[derive(Debug, Clone)]
pub struct ReplayStateConfig {
    /// Whether to prefetch dynamic field children.
    pub prefetch_dynamic_fields: bool,
    /// Maximum depth for dynamic field discovery.
    pub df_depth: usize,
    /// Maximum number of children to prefetch per parent.
    pub df_limit: usize,
    /// Whether to auto-inject Clock/Random when missing.
    pub auto_system_objects: bool,
}

impl Default for ReplayStateConfig {
    fn default() -> Self {
        Self {
            prefetch_dynamic_fields: true,
            df_depth: 3,
            df_limit: 200,
            auto_system_objects: true,
        }
    }
}

/// Builder for replay state hydration.
pub struct ReplayStateBuilder<'a, P: ReplayStateProvider + ?Sized> {
    provider: &'a P,
    config: ReplayStateConfig,
}

impl<'a, P: ReplayStateProvider + ?Sized> ReplayStateBuilder<'a, P> {
    pub fn new(provider: &'a P) -> Self {
        Self {
            provider,
            config: ReplayStateConfig::default(),
        }
    }

    pub fn with_config(mut self, config: ReplayStateConfig) -> Self {
        self.config = config;
        self
    }

    pub fn prefetch_dynamic_fields(mut self, enabled: bool) -> Self {
        self.config.prefetch_dynamic_fields = enabled;
        self
    }

    pub fn dynamic_field_depth(mut self, depth: usize) -> Self {
        self.config.df_depth = depth;
        self
    }

    pub fn dynamic_field_limit(mut self, limit: usize) -> Self {
        self.config.df_limit = limit;
        self
    }

    pub fn auto_system_objects(mut self, enabled: bool) -> Self {
        self.config.auto_system_objects = enabled;
        self
    }

    pub async fn build(self, digest: &str) -> Result<ReplayState> {
        self.provider
            .fetch_replay_state_with_config(digest, &self.config)
            .await
    }
}
