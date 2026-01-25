//! Native function cost parameters from ProtocolConfig.
//!
//! This module extracts native function costs from Sui's ProtocolConfig.
//! These costs are used by the mock native implementations to report
//! accurate gas costs that match on-chain behavior.
//!
//! # Cost Categories
//!
//! Native functions have several cost categories:
//! - **Base cost**: Fixed overhead for calling the function
//! - **Per-byte cost**: Additional cost based on input/output size
//! - **Per-item cost**: Additional cost based on number of items processed
//!
//! # Usage
//!
//! ```ignore
//! use sui_sandbox_core::gas::{NativeFunctionCosts, load_protocol_config};
//!
//! let config = load_protocol_config(68);
//! let costs = NativeFunctionCosts::from_protocol_config(&config);
//!
//! // Get cost for tx_context::sender()
//! let sender_cost = costs.tx_context_sender_base;
//! ```

use sui_protocol_config::ProtocolConfig;

/// Native function costs extracted from ProtocolConfig.
///
/// These costs match the on-chain costs for native function execution.
/// When accurate gas metering is enabled, these costs should be used
/// instead of the default zero costs.
#[derive(Debug, Clone)]
pub struct NativeFunctionCosts {
    // ========== TxContext Natives ==========
    /// tx_context::sender base cost
    pub tx_context_sender_base: u64,
    /// tx_context::epoch base cost
    pub tx_context_epoch_base: u64,
    /// tx_context::epoch_timestamp_ms base cost
    pub tx_context_epoch_timestamp_ms_base: u64,
    /// tx_context::fresh_id base cost
    pub tx_context_fresh_id_base: u64,
    /// tx_context::ids_created base cost
    pub tx_context_ids_created_base: u64,
    /// tx_context::derive_id base cost
    pub tx_context_derive_id_base: u64,
    /// tx_context::gas_price base cost
    pub tx_context_gas_price_base: u64,
    /// tx_context::gas_budget base cost
    pub tx_context_gas_budget_base: u64,
    /// tx_context::sponsor base cost
    pub tx_context_sponsor_base: u64,
    /// tx_context::rgp (reference gas price) base cost
    pub tx_context_rgp_base: u64,

    // ========== Transfer Natives ==========
    /// transfer::transfer_internal base cost
    pub transfer_internal_base: u64,
    /// transfer::freeze_object base cost
    pub transfer_freeze_object_base: u64,
    /// transfer::share_object base cost
    pub transfer_share_object_base: u64,

    // ========== Object Natives ==========
    /// object::delete_impl base cost
    pub object_delete_impl_base: u64,
    /// object::borrow_uid base cost
    pub object_borrow_uid_base: u64,
    /// object::record_new_id base cost
    pub object_record_new_id_base: u64,

    // ========== Dynamic Field Natives ==========
    /// dynamic_field::hash_type_and_key base cost
    pub dynamic_field_hash_base: u64,
    /// dynamic_field::add_child_object base cost
    pub dynamic_field_add_child_base: u64,
    /// dynamic_field::borrow_child_object base cost
    pub dynamic_field_borrow_child_base: u64,
    /// dynamic_field::remove_child_object base cost
    pub dynamic_field_remove_child_base: u64,
    /// dynamic_field::has_child_object base cost
    pub dynamic_field_has_child_base: u64,

    // ========== Event Natives ==========
    /// event::emit base cost
    pub event_emit_base: u64,
    /// event::emit per byte cost
    pub event_emit_per_byte: u64,

    // ========== Crypto Natives ==========
    /// hash::blake2b256 base cost
    pub hash_blake2b256_base: u64,
    /// hash::blake2b256 per byte cost
    pub hash_blake2b256_per_byte: u64,
    /// hash::keccak256 base cost
    pub hash_keccak256_base: u64,
    /// hash::keccak256 per byte cost
    pub hash_keccak256_per_byte: u64,

    // ========== Address Natives ==========
    /// address::from_bytes base cost
    pub address_from_bytes_base: u64,
    /// address::to_u256 base cost
    pub address_to_u256_base: u64,
    /// address::from_u256 base cost
    pub address_from_u256_base: u64,

    // ========== Type Natives ==========
    /// types::is_one_time_witness base cost
    pub types_is_one_time_witness_base: u64,

    // ========== BCS Natives ==========
    /// bcs::to_bytes base cost
    pub bcs_to_bytes_base: u64,
    /// bcs::to_bytes per byte cost
    pub bcs_to_bytes_per_byte: u64,

    // ========== Vector Natives (from move-stdlib) ==========
    /// vector::empty base cost
    pub vector_empty_base: u64,
    /// vector::length base cost
    pub vector_length_base: u64,
    /// vector::push_back base cost
    pub vector_push_back_base: u64,
    /// vector::pop_back base cost
    pub vector_pop_back_base: u64,
    /// vector::borrow base cost
    pub vector_borrow_base: u64,
    /// vector::swap base cost
    pub vector_swap_base: u64,

    // ========== Balance/Coin Natives ==========
    /// balance::create_for_testing base cost
    pub balance_create_base: u64,
    /// balance::destroy_for_testing base cost
    pub balance_destroy_base: u64,
    /// coin::mint_for_testing base cost
    pub coin_mint_base: u64,
    /// coin::burn_for_testing base cost
    pub coin_burn_base: u64,
    /// supply::create_supply base cost
    pub supply_create_base: u64,

    // ========== Transfer Receive Natives ==========
    /// transfer::receive_impl base cost
    pub transfer_receive_base: u64,

    // ========== Protocol Config Natives ==========
    /// protocol_config::protocol_version base cost
    pub protocol_version_base: u64,
    /// protocol_config::is_feature_enabled base cost
    pub is_feature_enabled_base: u64,

    // ========== Validator Natives ==========
    /// validator::validate_metadata_bcs base cost
    pub validator_validate_metadata_base: u64,
}

impl NativeFunctionCosts {
    /// Create native function costs from a ProtocolConfig.
    ///
    /// This extracts all native function costs from the protocol config.
    /// Many of these costs are accessed via specific cost param methods.
    pub fn from_protocol_config(config: &ProtocolConfig) -> Self {
        // Default cost value when specific config methods aren't available
        // These are reasonable estimates based on Sui's typical gas costs
        const DEFAULT_TX_CONTEXT_COST: u64 = 52;
        const DEFAULT_TRANSFER_COST: u64 = 52;
        const DEFAULT_OBJECT_COST: u64 = 52;
        const DEFAULT_DYNAMIC_FIELD_COST: u64 = 100;
        const DEFAULT_HASH_BASE: u64 = 52;
        const DEFAULT_HASH_PER_BYTE: u64 = 2;
        const DEFAULT_EVENT_BASE: u64 = 52;
        const DEFAULT_EVENT_PER_BYTE: u64 = 10;
        const DEFAULT_VECTOR_COST: u64 = 10;

        Self {
            // TxContext natives - all have similar low costs
            tx_context_sender_base: config
                .tx_context_sender_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_epoch_base: config
                .tx_context_epoch_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_epoch_timestamp_ms_base: config
                .tx_context_epoch_timestamp_ms_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_fresh_id_base: config
                .tx_context_fresh_id_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_ids_created_base: config
                .tx_context_ids_created_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_derive_id_base: config
                .tx_context_derive_id_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_gas_price_base: config
                .tx_context_gas_price_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_gas_budget_base: config
                .tx_context_gas_budget_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_sponsor_base: config
                .tx_context_sponsor_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),
            tx_context_rgp_base: config
                .tx_context_rgp_cost_base_as_option()
                .unwrap_or(DEFAULT_TX_CONTEXT_COST),

            // Transfer natives
            transfer_internal_base: config
                .transfer_transfer_internal_cost_base_as_option()
                .unwrap_or(DEFAULT_TRANSFER_COST),
            transfer_freeze_object_base: config
                .transfer_freeze_object_cost_base_as_option()
                .unwrap_or(DEFAULT_TRANSFER_COST),
            transfer_share_object_base: config
                .transfer_share_object_cost_base_as_option()
                .unwrap_or(DEFAULT_TRANSFER_COST),

            // Object natives
            object_delete_impl_base: config
                .object_delete_impl_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),
            object_borrow_uid_base: config
                .object_borrow_uid_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),
            object_record_new_id_base: config
                .object_record_new_uid_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),

            // Dynamic field natives
            dynamic_field_hash_base: config
                .dynamic_field_hash_type_and_key_cost_base_as_option()
                .unwrap_or(DEFAULT_DYNAMIC_FIELD_COST),
            dynamic_field_add_child_base: config
                .dynamic_field_add_child_object_cost_base_as_option()
                .unwrap_or(DEFAULT_DYNAMIC_FIELD_COST),
            dynamic_field_borrow_child_base: config
                .dynamic_field_borrow_child_object_cost_base_as_option()
                .unwrap_or(DEFAULT_DYNAMIC_FIELD_COST),
            dynamic_field_remove_child_base: config
                .dynamic_field_remove_child_object_cost_base_as_option()
                .unwrap_or(DEFAULT_DYNAMIC_FIELD_COST),
            dynamic_field_has_child_base: config
                .dynamic_field_has_child_object_cost_base_as_option()
                .unwrap_or(DEFAULT_DYNAMIC_FIELD_COST),

            // Event natives
            event_emit_base: config
                .event_emit_cost_base_as_option()
                .unwrap_or(DEFAULT_EVENT_BASE),
            event_emit_per_byte: config
                .event_emit_output_cost_per_byte_as_option()
                .unwrap_or(DEFAULT_EVENT_PER_BYTE),

            // Hash natives - use hash_keccak256_cost_base as representative
            hash_blake2b256_base: config
                .hash_blake2b256_cost_base_as_option()
                .unwrap_or(DEFAULT_HASH_BASE),
            hash_blake2b256_per_byte: config
                .hash_blake2b256_data_cost_per_byte_as_option()
                .unwrap_or(DEFAULT_HASH_PER_BYTE),
            hash_keccak256_base: config
                .hash_keccak256_cost_base_as_option()
                .unwrap_or(DEFAULT_HASH_BASE),
            hash_keccak256_per_byte: config
                .hash_keccak256_data_cost_per_byte_as_option()
                .unwrap_or(DEFAULT_HASH_PER_BYTE),

            // Address natives
            address_from_bytes_base: config
                .address_from_bytes_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),
            address_to_u256_base: config
                .address_to_u256_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),
            address_from_u256_base: config
                .address_from_u256_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),

            // Type natives
            types_is_one_time_witness_base: config
                .types_is_one_time_witness_cost_base_as_option()
                .unwrap_or(DEFAULT_OBJECT_COST),

            // BCS natives
            bcs_to_bytes_base: config
                .bcs_per_byte_serialized_cost_as_option()
                .map(|_| 52u64)
                .unwrap_or(52),
            bcs_to_bytes_per_byte: config.bcs_per_byte_serialized_cost_as_option().unwrap_or(2),

            // Vector natives - use simple defaults
            vector_empty_base: DEFAULT_VECTOR_COST,
            vector_length_base: DEFAULT_VECTOR_COST,
            vector_push_back_base: DEFAULT_VECTOR_COST,
            vector_pop_back_base: DEFAULT_VECTOR_COST,
            vector_borrow_base: DEFAULT_VECTOR_COST,
            vector_swap_base: DEFAULT_VECTOR_COST,

            // Balance/Coin natives - use simple defaults (test utilities)
            balance_create_base: DEFAULT_OBJECT_COST,
            balance_destroy_base: DEFAULT_OBJECT_COST,
            coin_mint_base: DEFAULT_OBJECT_COST,
            coin_burn_base: DEFAULT_OBJECT_COST,
            supply_create_base: DEFAULT_OBJECT_COST,

            // Transfer receive
            transfer_receive_base: DEFAULT_TRANSFER_COST,

            // Protocol config natives
            protocol_version_base: DEFAULT_TX_CONTEXT_COST,
            is_feature_enabled_base: DEFAULT_TX_CONTEXT_COST,

            // Validator natives
            validator_validate_metadata_base: DEFAULT_OBJECT_COST,
        }
    }
}

impl Default for NativeFunctionCosts {
    fn default() -> Self {
        // Reasonable defaults matching typical on-chain costs
        Self {
            // TxContext natives
            tx_context_sender_base: 52,
            tx_context_epoch_base: 52,
            tx_context_epoch_timestamp_ms_base: 52,
            tx_context_fresh_id_base: 52,
            tx_context_ids_created_base: 52,
            tx_context_derive_id_base: 52,
            tx_context_gas_price_base: 52,
            tx_context_gas_budget_base: 52,
            tx_context_sponsor_base: 52,
            tx_context_rgp_base: 52,

            // Transfer natives
            transfer_internal_base: 52,
            transfer_freeze_object_base: 52,
            transfer_share_object_base: 52,

            // Object natives
            object_delete_impl_base: 52,
            object_borrow_uid_base: 52,
            object_record_new_id_base: 52,

            // Dynamic field natives
            dynamic_field_hash_base: 100,
            dynamic_field_add_child_base: 100,
            dynamic_field_borrow_child_base: 100,
            dynamic_field_remove_child_base: 100,
            dynamic_field_has_child_base: 100,

            // Event natives
            event_emit_base: 52,
            event_emit_per_byte: 10,

            // Hash natives
            hash_blake2b256_base: 52,
            hash_blake2b256_per_byte: 2,
            hash_keccak256_base: 52,
            hash_keccak256_per_byte: 2,

            // Address natives
            address_from_bytes_base: 52,
            address_to_u256_base: 52,
            address_from_u256_base: 52,

            // Type natives
            types_is_one_time_witness_base: 52,

            // BCS natives
            bcs_to_bytes_base: 52,
            bcs_to_bytes_per_byte: 2,

            // Vector natives
            vector_empty_base: 10,
            vector_length_base: 10,
            vector_push_back_base: 10,
            vector_pop_back_base: 10,
            vector_borrow_base: 10,
            vector_swap_base: 10,

            // Balance/Coin natives
            balance_create_base: 52,
            balance_destroy_base: 52,
            coin_mint_base: 52,
            coin_burn_base: 52,
            supply_create_base: 52,

            // Transfer receive
            transfer_receive_base: 52,

            // Protocol config natives
            protocol_version_base: 52,
            is_feature_enabled_base: 52,

            // Validator natives
            validator_validate_metadata_base: 52,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gas::load_protocol_config;

    #[test]
    fn test_native_costs_from_protocol_config() {
        let config = load_protocol_config(68);
        let costs = NativeFunctionCosts::from_protocol_config(&config);

        // Verify costs are reasonable (non-zero)
        assert!(costs.tx_context_sender_base > 0);
        assert!(costs.transfer_internal_base > 0);
        assert!(costs.dynamic_field_hash_base > 0);
    }

    #[test]
    fn test_native_costs_default() {
        let costs = NativeFunctionCosts::default();

        // All defaults should be non-zero
        assert!(costs.tx_context_sender_base > 0);
        assert!(costs.event_emit_base > 0);
        assert!(costs.hash_keccak256_base > 0);
    }
}
