//! Unified historical state fetching for Sui transaction replay.
//!
//! This crate provides [`HistoricalStateProvider`], a single entry point for
//! fetching all state needed to replay a Sui transaction locally.
//!
//! # Example
//!
//! ```ignore
//! use sui_state_fetcher::HistoricalStateProvider;
//!
//! let provider = HistoricalStateProvider::mainnet().await?;
//! let state = provider.fetch_replay_state("8JTTa...").await?;
//!
//! // state.transaction - PTB commands and inputs
//! // state.objects - objects at their input versions
//! // state.packages - packages with linkage resolved
//! ```

pub mod cache;
pub mod provider;
pub mod replay;
pub mod types;
pub mod vm_integration;

// Re-export main types
pub use cache::VersionedCache;
pub use provider::HistoricalStateProvider;
pub use replay::{
    build_address_aliases, get_historical_versions, to_raw_objects, to_replay_data, ReplayData,
};
pub use types::{FetchStats, ObjectID, PackageData, ReplayState, VersionedObject};
