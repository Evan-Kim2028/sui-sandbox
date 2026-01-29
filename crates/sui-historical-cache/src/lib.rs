//! Filesystem-backed historical object/version cache for Walrus checkpoint replay.
//!
//! This crate provides:
//! - `FsObjectStore`: Sharded filesystem storage for `(object_id, version)` -> BCS bytes
//! - `FsPackageStore`: Filesystem storage for package modules (gRPC miss-fill)
//! - `ProgressTracker`: Resume-safe checkpoint/blob ingestion tracking

pub mod metrics;
pub mod objects;
pub mod packages;
pub mod paths;
pub mod progress;

pub use metrics::CacheMetrics;
pub use objects::{FsObjectStore, ObjectMeta, ObjectVersionStore};
pub use packages::{CachedPackage, FsPackageStore, LinkageEntry, PackageStore};
pub use progress::ProgressTracker;
