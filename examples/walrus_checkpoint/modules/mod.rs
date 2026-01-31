//! Modular components for Walrus checkpoint replay.
//!
//! This module provides a clean separation of concerns for the replay engine:
//! - `cache_layer`: Trait-based object and package caching with tiered storage
//! - `orchestrator`: Retry state machine and attempt coordination
//! - `validator`: Parity comparison and error classification
//! - `executor`: VM setup, fetcher installation, and PTB execution
//! - `data_fetcher`: Walrus/gRPC/GraphQL data fetching coordination
//! - `predictor`: Predictive dynamic field analysis
//!
//! Note: These modules define traits and types for a future modular architecture.
//! Re-exports are provided for external consumers, though some may not yet be
//! integrated into the main replay engine.

#![allow(unused_imports)]

pub mod cache_layer;
pub mod data_fetcher;
pub mod executor;
pub mod orchestrator;
pub mod predictor;
pub mod validator;

// Re-export commonly used types for external consumers
pub use cache_layer::{
    CacheLookupResult, CacheSource, FsObjectStoreAdapter, MemoryObjectStore, MemoryPackageStore,
    ObjectData, ObjectEntry, ObjectResolver, ObjectStore, PackageStore, TieredObjectStore,
    UnifiedObjectResolver, WalrusObjectData,
};
pub use data_fetcher::{
    BatchPrefetchResult, DataFetcherCoordinator, ObjectFetcher, PackageFetcher, TxVersionMap,
};
pub use executor::{ExecutionConfig, ExecutionResult, FetcherContext, PtbExecutor};
pub use orchestrator::{AttemptKind, AttemptReport, RetryConfig, RetryStateMachine, TxOutcome};
pub use predictor::{
    AccessPattern, CommandAnalyzer, DefaultDynamicFieldPredictor, DynamicFieldPredictor,
    PredictedAccess, PredictionConfidence, PredictionResult, PredictionStats, PredictiveConfig,
};
pub use validator::{ComparisonResult, ErrorClassifier, ObjectDiff, ParityValidator, ReasonCode};
