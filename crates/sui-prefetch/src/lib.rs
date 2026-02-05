//! Sui Prefetch
//!
//! Strategic data prefetching for Sui transaction replay.
//!
//! This crate provides:
//! - [`eager_prefetch`]: Ground-truth-first prefetching for transaction replay
//! - [`conversion`]: Convert gRPC transactions to internal FetchedTransaction format
//! - [`utilities`]: Dynamic field helpers and data aggregation
//!
//! # Example
//!
//! ```ignore
//! use sui_prefetch::{ground_truth_prefetch_for_transaction, GroundTruthPrefetchConfig};
//! use sui_transport::grpc::GrpcClient;
//!
//! let grpc = GrpcClient::mainnet().await?;
//! let tx = grpc.get_transaction("...").await?;
//!
//! let config = GroundTruthPrefetchConfig::default();
//! let result = ground_truth_prefetch_for_transaction(&grpc, None, &rt, &tx, &config);
//! ```

pub mod conversion;
pub mod eager_prefetch;
pub mod utilities;

// =============================================================================
// Primary API: Ground-Truth Prefetch (recommended)
// =============================================================================
// Uses unchanged_loaded_runtime_objects from transaction effects for 100% accuracy.
pub use eager_prefetch::{
    ground_truth_prefetch_for_transaction, GroundTruthPrefetchConfig, GroundTruthPrefetchResult,
    GroundTruthPrefetchStats,
};

// =============================================================================
// Conversion & Utilities
// =============================================================================
pub use conversion::grpc_to_fetched_transaction;
pub use sui_sandbox_types::{FetchedObject, FetchedPackage};
pub use utilities::{
    collect_historical_versions, compute_dynamic_field_id, prefetch_dynamic_fields,
    prefetch_dynamic_fields_at_checkpoint, prefetch_dynamic_fields_default,
    prefetch_dynamic_fields_with_version_bound, prefetch_epoch_keyed_fields, type_string_to_bcs,
    DynamicFieldKey, PrefetchedChild, PrefetchedDynamicFields,
};
