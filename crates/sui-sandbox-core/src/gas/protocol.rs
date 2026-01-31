//! Protocol configuration loading for gas parameters.
//!
//! This module provides utilities to load and access gas-related parameters
//! from Sui's ProtocolConfig. The ProtocolConfig is the single source of truth
//! for all gas costs on the Sui network.

use std::sync::Arc;

use sui_protocol_config::{Chain, ProtocolConfig, ProtocolVersion};

/// Default protocol version to use (current mainnet).
/// This should be updated as mainnet progresses.
/// As of late 2025, mainnet is at protocol version 73.
pub const DEFAULT_PROTOCOL_VERSION: u64 = 73;

// =============================================================================
// Default Gas Constants
// =============================================================================
// These constants provide sensible defaults for gas-related parameters.
// For production use, actual values should be fetched from the network or
// derived from ProtocolConfig for the specific transaction context.

/// Default gas budget for transactions (50 SUI in MIST).
/// This is a reasonable default for most transactions on mainnet.
pub const DEFAULT_GAS_BUDGET: u64 = 50_000_000_000;

/// Default reference gas price (750 MIST per gas unit).
/// This matches the typical mainnet reference gas price as of protocol v68.
/// Note: Actual gas price should be fetched from the network for production use.
pub const DEFAULT_REFERENCE_GAS_PRICE: u64 = 750;

/// Default gas price for simulation (1000 MIST per gas unit).
/// This is slightly higher than reference to provide conservative estimates.
pub const DEFAULT_GAS_PRICE: u64 = 1000;

/// Default gas balance for simulated accounts (1 billion SUI in MIST).
/// This provides ample gas for any simulation scenario.
pub const DEFAULT_GAS_BALANCE: u64 = 1_000_000_000_000_000_000;

/// Load a ProtocolConfig for the specified version.
///
/// # Arguments
/// * `version` - The protocol version number (e.g., 68 for mainnet-v1.63.4)
///
/// # Returns
/// The ProtocolConfig for that version, configured for mainnet.
///
/// # Example
/// ```ignore
/// let config = load_protocol_config(68);
/// let gas_model_version = config.gas_model_version();
/// ```
pub fn load_protocol_config(version: u64) -> ProtocolConfig {
    let max_supported = ProtocolVersion::MAX.as_u64();
    let clamped_version = if version > max_supported {
        eprintln!(
            "[protocol] requested protocol_version={} exceeds max_supported={}, clamping",
            version, max_supported
        );
        max_supported
    } else {
        version
    };
    let protocol_version = ProtocolVersion::new(clamped_version);
    ProtocolConfig::get_for_version(protocol_version, Chain::Mainnet)
}

/// Load the ProtocolConfig for the current default version.
pub fn load_default_protocol_config() -> ProtocolConfig {
    load_protocol_config(DEFAULT_PROTOCOL_VERSION)
}

/// Load a ProtocolConfig wrapped in Arc for shared ownership.
pub fn load_protocol_config_arc(version: u64) -> Arc<ProtocolConfig> {
    Arc::new(load_protocol_config(version))
}

/// Comprehensive gas parameters extracted from ProtocolConfig.
///
/// This struct contains all gas-related parameters needed for accurate
/// gas metering. All values are extracted directly from the ProtocolConfig,
/// ensuring they match on-chain behavior.
#[derive(Debug, Clone)]
pub struct GasParameters {
    // ========== Transaction Base Costs ==========
    /// Base transaction cost (fixed overhead)
    pub base_tx_cost_fixed: u64,
    /// Cost per byte of transaction data
    pub base_tx_cost_per_byte: u64,

    // ========== Package Publish Costs ==========
    /// Fixed cost for publishing a package
    pub package_publish_cost_fixed: u64,
    /// Cost per byte for package publish data
    pub package_publish_cost_per_byte: u64,

    // ========== Object Access Costs ==========
    /// Cost per byte for reading an object
    pub obj_access_cost_read_per_byte: u64,
    /// Cost per byte for mutating an object
    pub obj_access_cost_mutate_per_byte: u64,
    /// Cost per byte for deleting an object
    pub obj_access_cost_delete_per_byte: u64,
    /// Cost per byte for type verification
    pub obj_access_cost_verify_per_byte: u64,

    // ========== Storage Costs ==========
    /// Refundable cost per byte of object data
    pub obj_data_cost_refundable: u64,
    /// Non-refundable cost per object (metadata)
    pub obj_metadata_cost_non_refundable: u64,
    /// Storage rebate rate (basis points, e.g., 9900 = 99%)
    pub storage_rebate_rate: u64,

    // ========== Gas Limits ==========
    /// Maximum gas budget for a transaction
    pub max_tx_gas: u64,
    /// Maximum gas price
    pub max_gas_price: u64,
    /// Maximum computation gas bucket
    pub max_gas_computation_bucket: u64,

    // ========== Gas Model ==========
    /// Gas model version (affects cost table selection and charging behavior)
    pub gas_model_version: u64,

    // ========== Execution Limits ==========
    /// Maximum number of Move events a transaction can emit
    pub max_num_event_emit: u64,
    /// Maximum total size of events
    pub max_event_emit_size: u64,
    /// Maximum number of new objects created
    pub max_num_new_move_object_ids: u64,
    /// Maximum number of objects deleted
    pub max_num_deleted_move_object_ids: u64,
    /// Maximum number of objects transferred
    pub max_num_transferred_move_object_ids: u64,

    // ========== VM Limits ==========
    /// Maximum function parameters
    pub max_function_parameters: u64,
    /// Maximum Move vector length
    pub max_move_vector_len: u64,
    /// Maximum type argument depth
    pub max_type_argument_depth: u64,
    /// Maximum type arguments
    pub max_type_arguments: u64,
    /// Maximum value stack depth
    pub max_value_stack_depth: u64,

    // ========== Native Function Costs ==========
    /// BCS serialization base cost
    pub bcs_per_byte_serialized_cost: u64,
    /// BCS deserialization base cost
    pub bcs_legacy_min_output_size_cost: u64,
    /// BCS failure cost
    pub bcs_failure_cost: u64,
}

impl GasParameters {
    /// Create GasParameters from a ProtocolConfig.
    ///
    /// This extracts all gas-related parameters from the protocol config,
    /// ensuring they match the on-chain values for that protocol version.
    pub fn from_protocol_config(config: &ProtocolConfig) -> Self {
        Self {
            // Transaction base costs
            base_tx_cost_fixed: config.base_tx_cost_fixed(),
            base_tx_cost_per_byte: config.base_tx_cost_per_byte(),

            // Package publish costs
            package_publish_cost_fixed: config.package_publish_cost_fixed(),
            package_publish_cost_per_byte: config.package_publish_cost_per_byte(),

            // Object access costs
            obj_access_cost_read_per_byte: config.obj_access_cost_read_per_byte(),
            obj_access_cost_mutate_per_byte: config.obj_access_cost_mutate_per_byte(),
            obj_access_cost_delete_per_byte: config.obj_access_cost_delete_per_byte(),
            obj_access_cost_verify_per_byte: config.obj_access_cost_verify_per_byte(),

            // Storage costs
            obj_data_cost_refundable: config.obj_data_cost_refundable(),
            obj_metadata_cost_non_refundable: config.obj_metadata_cost_non_refundable(),
            storage_rebate_rate: config.storage_rebate_rate(),

            // Gas limits
            max_tx_gas: config.max_tx_gas(),
            max_gas_price: config.max_gas_price(),
            max_gas_computation_bucket: config.max_gas_computation_bucket(),

            // Gas model
            gas_model_version: config.gas_model_version(),

            // Execution limits
            max_num_event_emit: config.max_num_event_emit(),
            max_event_emit_size: config.max_event_emit_size(),
            max_num_new_move_object_ids: config.max_num_new_move_object_ids(),
            max_num_deleted_move_object_ids: config.max_num_deleted_move_object_ids(),
            max_num_transferred_move_object_ids: config.max_num_transferred_move_object_ids(),

            // VM limits
            max_function_parameters: config.max_function_parameters(),
            max_move_vector_len: config.max_move_vector_len(),
            max_type_argument_depth: u64::from(config.max_type_argument_depth()),
            max_type_arguments: u64::from(config.max_type_arguments()),
            max_value_stack_depth: config.max_value_stack_size(),

            // BCS costs (used by native functions)
            bcs_per_byte_serialized_cost: config.bcs_per_byte_serialized_cost(),
            bcs_legacy_min_output_size_cost: config.bcs_legacy_min_output_size_cost(),
            bcs_failure_cost: config.bcs_failure_cost(),
        }
    }

    /// Get the gas model version.
    pub fn gas_model_version(&self) -> u64 {
        self.gas_model_version
    }

    /// Check if this gas model version should charge input as memory.
    /// Only applies to gas model version 4.
    pub fn charge_input_as_memory(&self) -> bool {
        self.gas_model_version == 4
    }

    /// Check if this gas model version should use legacy abstract size calculation.
    pub fn use_legacy_abstract_size(&self) -> bool {
        self.gas_model_version <= 7
    }

    /// Check if storage OOG should not charge the entire budget.
    pub fn dont_charge_budget_on_storage_oog(&self) -> bool {
        self.gas_model_version >= 4
    }

    /// Check if gas price too high check is enabled.
    pub fn check_for_gas_price_too_high(&self) -> bool {
        self.gas_model_version >= 4
    }

    /// Check if package upgrades should be charged differently.
    pub fn charge_upgrades(&self) -> bool {
        self.gas_model_version >= 7
    }
}

impl Default for GasParameters {
    fn default() -> Self {
        Self::from_protocol_config(&load_default_protocol_config())
    }
}

/// Gas rounding step (1000) for protocol version 14+.
/// When set, computation gas is rounded up to the nearest multiple of this value.
pub const GAS_ROUNDING_STEP: u64 = 1000;

/// Apply gas rounding to computation gas.
///
/// Since protocol version 14, Sui rounds computation gas up to the nearest 1000 gas units
/// before multiplying by gas price. This function implements that rounding.
///
/// # Arguments
/// * `gas_units` - Raw gas units consumed
///
/// # Returns
/// Rounded gas units (rounded UP to nearest 1000)
///
/// # Example
/// ```ignore
/// assert_eq!(apply_gas_rounding(0), 0);
/// assert_eq!(apply_gas_rounding(1), 1000);
/// assert_eq!(apply_gas_rounding(999), 1000);
/// assert_eq!(apply_gas_rounding(1000), 1000);
/// assert_eq!(apply_gas_rounding(1001), 2000);
/// ```
pub fn apply_gas_rounding(gas_units: u64) -> u64 {
    if gas_units == 0 {
        return 0;
    }
    #[allow(clippy::manual_is_multiple_of)]
    if gas_units % GAS_ROUNDING_STEP == 0 {
        gas_units
    } else {
        ((gas_units / GAS_ROUNDING_STEP) + 1) * GAS_ROUNDING_STEP
    }
}

/// Calculate the minimum transaction cost.
///
/// This is the base cost that every transaction must pay, regardless of what it does.
/// For protocol version 14+, this is `base_tx_cost_fixed * gas_price`.
///
/// # Arguments
/// * `protocol_config` - The protocol config to get base_tx_cost_fixed from
/// * `gas_price` - The transaction's gas price
///
/// # Returns
/// Minimum transaction cost in MIST
pub fn calculate_min_tx_cost(config: &ProtocolConfig, gas_price: u64) -> u64 {
    // Since protocol version 14, min_transaction_cost = base_tx_cost_fixed * gas_price
    // Before that, it was just base_tx_cost_fixed
    // We'll use the multiplier version which matches current mainnet behavior
    config.base_tx_cost_fixed().saturating_mul(gas_price)
}

/// Calculate final computation cost with rounding and minimum.
///
/// This applies Sui's computation cost finalization:
/// 1. Rounds gas units UP to nearest 1000 (gas rounding step)
/// 2. Multiplies by gas price to get MIST
/// 3. Ensures result is at least min_transaction_cost
///
/// # Arguments
/// * `gas_units` - Raw computation gas units consumed
/// * `gas_price` - Transaction's gas price (MIST per gas unit)
/// * `min_tx_cost` - Minimum transaction cost (from calculate_min_tx_cost)
///
/// # Returns
/// Final computation cost in MIST
pub fn finalize_computation_cost(gas_units: u64, gas_price: u64, min_tx_cost: u64) -> u64 {
    let rounded_gas_units = apply_gas_rounding(gas_units);
    let computation_cost = rounded_gas_units.saturating_mul(gas_price);
    computation_cost.max(min_tx_cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_protocol_config() {
        // Test loading different protocol versions
        let config_1 = load_protocol_config(1);
        assert!(config_1.gas_model_version() >= 1);

        let config_68 = load_protocol_config(68);
        assert!(config_68.gas_model_version() >= 8);
    }

    #[test]
    fn test_gas_parameters_from_config() {
        let config = load_protocol_config(68);
        let params = GasParameters::from_protocol_config(&config);

        // Verify key parameters are set
        assert!(params.base_tx_cost_fixed > 0);
        assert!(params.max_tx_gas > 0);
        assert!(params.gas_model_version > 0);
        assert!(params.storage_rebate_rate > 0);
        assert!(params.obj_access_cost_read_per_byte > 0);
    }

    #[test]
    fn test_gas_model_predicates() {
        // Gas model version 4
        let params_v4 = GasParameters {
            gas_model_version: 4,
            ..Default::default()
        };
        assert!(params_v4.charge_input_as_memory());
        assert!(params_v4.dont_charge_budget_on_storage_oog());
        assert!(!params_v4.charge_upgrades());

        // Gas model version 8
        let params_v8 = GasParameters {
            gas_model_version: 8,
            ..Default::default()
        };
        assert!(!params_v8.charge_input_as_memory());
        assert!(!params_v8.use_legacy_abstract_size());
        assert!(params_v8.charge_upgrades());
    }

    #[test]
    fn test_default_gas_parameters() {
        let params = GasParameters::default();
        assert!(
            params.gas_model_version >= 8,
            "Default should use recent protocol version"
        );
    }
}
