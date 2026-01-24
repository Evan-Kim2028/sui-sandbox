//! Infrastructure workaround utilities.
//!
//! This module contains utilities for working around **infrastructure limitations**
//! when replaying historical transactions. These are distinct from data helpers
//! (which live in `sui_data_fetcher::utilities`).
//!
//! ## Scope: Infrastructure Workarounds
//!
//! These utilities patch or transform data to handle limitations such as:
//!
//! - **Historical bytecode unavailability**: When replaying historical transactions,
//!   we often only have access to current bytecode, which may have different version
//!   constants than the historical objects being loaded.
//!
//! - **Address format inconsistency**: Sui addresses appear in both short (`0x2`) and
//!   full (`0x0000...0002`) formats, causing HashMap lookup issues.
//!
//! ## What Belongs Here
//!
//! - Object/data patching (modifying data before execution)
//! - Format normalization (address formats, etc.)
//! - Bytecode analysis for version detection
//! - Type string parsing and package extraction
//! - Historical state reconstruction
//!
//! ## What Does NOT Belong Here
//!
//! - Data aggregation from gRPC responses (use `sui_data_fetcher::utilities`)
//! - gRPC client setup and fetching (use `sui_data_fetcher::utilities`)
//!
//! ## Modules
//!
//! - [`address`]: Address normalization (`normalize_address`, `is_framework_package`)
//! - [`generic_patcher`]: Object patching for version-lock workarounds
//! - [`version_utils`]: Version constant detection from bytecode
//! - [`type_utils`]: Type string parsing and package extraction from types/bytecode
//! - [`historical_bytecode`]: Historical bytecode resolution using tx effects
//! - [`version_field_detector`]: Version field detection in objects
//! - [`offset_calculator`]: Byte offset calculation for BCS structs
//! - [`enhanced_patcher`]: Enhanced patching with fallback strategies
//! - [`historical_state`]: High-level facade for historical state reconstruction
//! - [`historical_package`]: Package resolution following linkage tables
//! - [`bcs_scanner`]: Extract embedded addresses from BCS object data

pub mod address;
pub mod bcs_scanner;
pub mod enhanced_patcher;
pub mod generic_patcher;
pub mod historical_bytecode;
pub mod historical_package;
pub mod historical_state;
pub mod offset_calculator;
pub mod type_utils;
pub mod version_field_detector;
pub mod version_utils;

// Re-export commonly used items
pub use address::{is_framework_package, normalize_address};
pub use generic_patcher::{FieldPatchRule, GenericObjectPatcher, PatchAction, PatchCondition};
pub use type_utils::{
    extract_dependencies_from_bytecode, extract_package_ids_from_type, parse_and_rewrite_type,
    parse_type_tag, rewrite_type_tag, split_type_params,
};
pub use version_utils::detect_version_constants;

// Re-export new historical state reconstruction utilities
pub use enhanced_patcher::{
    EnhancedObjectPatcher, FailureStrategy, ManualPatchOverride, PatchingStats,
};
pub use historical_bytecode::{
    extract_package_versions_from_effects, is_framework_id, normalize_id, LinkageEntry,
    ResolutionConfig, ResolvedPackage,
};
pub use historical_package::{
    grpc_object_to_package_data, CallbackPackageFetcher, CallbackResolver, FetchedPackage,
    FetchedPackageData, HistoricalPackageResolver, PackageFetcher, PackageLinkage,
    PackageResolutionConfig,
};
pub use historical_state::{
    HistoricalStateReconstructor, ReconstructedState, ReconstructionConfig, ReconstructorBuilder,
};
pub use offset_calculator::{OffsetCalculator, OffsetResult};
pub use version_field_detector::{
    calculate_offset, default_patterns, find_well_known_config, find_well_known_offset,
    DetectedVersionField, FieldPosition, FieldType, VersionFieldDetector, VersionPattern,
    WELL_KNOWN_VERSION_CONFIGS, WELL_KNOWN_VERSION_OFFSETS,
};
