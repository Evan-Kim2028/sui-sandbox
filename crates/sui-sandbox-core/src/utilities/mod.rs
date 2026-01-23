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

pub mod address;
pub mod generic_patcher;
pub mod type_utils;
pub mod version_utils;

// Re-export commonly used items
pub use address::{is_framework_package, normalize_address};
pub use generic_patcher::{FieldPatchRule, GenericObjectPatcher, PatchAction, PatchCondition};
pub use type_utils::{
    extract_dependencies_from_bytecode, extract_package_ids_from_type, parse_and_rewrite_type,
    parse_type_tag, rewrite_type_tag, split_type_params,
};
pub use version_utils::detect_version_constants;
