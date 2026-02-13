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
pub mod fetch_utils;
pub mod provider;
pub mod replay;
pub mod replay_builder;
pub mod replay_provider;
pub mod sparse_replay;
pub mod types;
pub mod vm_integration;
pub mod walrus_replay;

// Re-export main types
pub use cache::VersionedCache;
pub use fetch_utils::{build_aliases, fetch_child_object, fetch_object_via_grpc, PackageAliases};
pub use provider::{package_data_from_move_package, HistoricalStateProvider};
pub use replay::{
    build_address_aliases, get_historical_versions, to_raw_objects, to_replay_data, ReplayData,
};
pub use replay_builder::{ReplayStateBuilder, ReplayStateConfig};
pub use replay_provider::ReplayStateProvider;
pub use sparse_replay::{
    DynamicFieldFailure, ObjectFetchOutcome, ObjectFetchRecord, OnDemandFetchSummary,
    PackageFetchOutcome, PackageFetchRecord, SparseReplayOutcome, SparseReplayPolicy,
    SparseReplayReport, SparseReplaySummary,
};
pub use types::{FetchStats, ObjectID, PackageData, ReplayState, VersionedObject};
pub use walrus_replay::{checkpoint_to_replay_state, find_tx_in_checkpoint};
