//! Sui Prefetch
//!
//! Strategic data prefetching for Sui transaction replay.
//!
//! This crate provides:
//! - [`eager_prefetch`]: Two prefetching strategies for transaction replay
//!   - Ground-truth-first (recommended): Uses `unchanged_loaded_runtime_objects`
//!   - GraphQL discovery (legacy): Discovers dynamic fields via GraphQL
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

// Re-export main conversion function
pub use conversion::grpc_to_fetched_transaction;

// Re-export ground-truth-first prefetch types (recommended)
pub use eager_prefetch::{
    ground_truth_prefetch_for_transaction, FetchedObject, FetchedPackage,
    GroundTruthPrefetchConfig, GroundTruthPrefetchResult, GroundTruthPrefetchStats,
};

// Re-export eager prefetch types (legacy GraphQL-first)
pub use eager_prefetch::{
    analyze_transaction_access_patterns, eager_prefetch_for_transaction, EagerPrefetchConfig,
    EagerPrefetchResult, PrefetchStats, TransactionAccessAnalysis,
};

// Re-export utilities
pub use utilities::{
    collect_historical_versions, compute_dynamic_field_id, prefetch_dynamic_fields,
    prefetch_dynamic_fields_default, prefetch_dynamic_fields_with_version_bound,
    prefetch_epoch_keyed_fields, type_string_to_bcs, DynamicFieldKey, PrefetchedChild,
    PrefetchedDynamicFields,
};
