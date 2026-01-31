//! Filesystem-backed historical object/version cache for Walrus checkpoint replay.
//!
//! This crate provides:
//! - `FsObjectStore`: Sharded filesystem storage for `(object_id, version)` -> BCS bytes
//! - `FsPackageStore`: Filesystem storage for package modules (gRPC miss-fill)
//! - `ProgressTracker`: Resume-safe checkpoint/blob ingestion tracking

pub mod metrics;
pub mod index;
pub mod tx_index;
pub mod dynamic_fields;
pub mod package_index;
pub mod objects;
pub mod packages;
pub mod paths;
pub mod progress;

pub use metrics::CacheMetrics;
pub use index::{FsObjectIndex, ObjectIndexEntry};
pub use tx_index::{FsTxDigestIndex, TxDigestIndexEntry};
pub use dynamic_fields::{DynamicFieldEntry, FsDynamicFieldCache};
pub use package_index::{FsPackageIndex, PackageIndexEntry};
pub use objects::{FsObjectStore, ObjectMeta, ObjectVersionStore};
pub use packages::{CachedPackage, FsPackageStore, LinkageEntry, PackageStore};
pub use progress::ProgressTracker;
