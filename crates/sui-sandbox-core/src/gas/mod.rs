//! Accurate gas metering module for Sui Move VM execution.
//!
//! # ⚠️ Beta Status
//!
//! **This gas metering implementation is currently in beta.** While it provides
//! accurate gas estimates for many transaction types, there are known limitations:
//!
//! ## Current Accuracy
//!
//! - **Simple transactions**: ~100% accuracy (exact match with on-chain)
//! - **Standard DeFi operations**: ~70-100% accuracy
//! - **Complex multi-package calls**: ~50-100% accuracy (varies by transaction)
//!
//! ## Known Limitations
//!
//! 1. **Complex transactions may show 50% accuracy** - Some transactions involving
//!    multiple package calls or complex DeFi operations may show only ~50% of
//!    actual on-chain gas. The root cause is still under investigation.
//!
//! 2. **Failed local execution** - If a transaction fails locally but succeeded
//!    on-chain (due to missing prefetched data), gas will be underreported.
//!
//! 3. **Advanced crypto natives** - Some advanced cryptographic operations
//!    (BLS signatures, VDF, zkLogin) report zero gas cost.
//!
//! 4. **Storage costs are approximate** - Storage gas tracking is heuristic-based
//!    and not fully integrated with the ObjectRuntime.
//!
//! ## What Works Well
//!
//! - Bytecode instruction costs (tiered pricing from Sui's cost tables)
//! - Input object read costs (`obj_access_cost_read_per_byte`)
//! - Gas rounding (rounds UP to nearest 1000 gas units)
//! - Minimum transaction cost (`base_tx_cost_fixed * gas_price`)
//! - Core native function costs (tx_context, transfer, object, dynamic_field, event)
//!
//! ## Recommended Usage
//!
//! Use gas estimates for:
//! - **Rough budgeting** - Get a ballpark estimate for transaction costs
//! - **Relative comparisons** - Compare gas usage between different approaches
//! - **Development/testing** - Catch obvious gas issues early
//!
//! Do NOT use for:
//! - **Exact cost predictions** - On-chain costs may differ significantly
//! - **Financial calculations** - Always verify with actual on-chain execution
//!
//! # Architecture
//!
//! The gas system has several layers:
//!
//! 1. **Protocol Configuration** - Loads gas parameters from `ProtocolConfig`
//! 2. **Cost Tables** - Tiered instruction costs based on gas model version
//! 3. **Storage Tracking** - Tracks object read/write/delete costs
//! 4. **Gas Meter** - Implements Move VM's `GasMeter` trait
//! 5. **Gas Charger** - Orchestrates all gas operations
//!
//! # Usage
//!
//! ```ignore
//! use sui_sandbox_core::gas::{AccurateGasCharger, GasSummary};
//! use sui_protocol_config::ProtocolConfig;
//!
//! // Load protocol config for a specific version
//! let protocol_config = load_protocol_config(68);
//!
//! // Create gas charger with budget
//! let mut charger = AccurateGasCharger::new(
//!     50_000_000_000, // budget
//!     1000,           // gas_price
//!     1000,           // reference_gas_price
//!     protocol_config,
//! );
//!
//! // Use charger.gas_meter() with Move VM
//! // Use charger.storage_tracker() for object operations
//!
//! // Get final summary
//! let summary = charger.finalize();
//! println!("Total gas: {}", summary.total_cost);
//! ```

mod charger;
mod cost_table;
mod meter;
mod native_costs;
mod protocol;
mod storage;
mod summary;

pub use charger::*;
pub use cost_table::*;
pub use meter::*;
pub use native_costs::*;
pub use protocol::{
    apply_gas_rounding, calculate_min_tx_cost, finalize_computation_cost,
    load_default_protocol_config, load_protocol_config, load_protocol_config_arc, GasParameters,
    DEFAULT_GAS_BALANCE, DEFAULT_GAS_BUDGET, DEFAULT_GAS_PRICE, DEFAULT_PROTOCOL_VERSION,
    DEFAULT_REFERENCE_GAS_PRICE, GAS_ROUNDING_STEP,
};
pub use storage::*;
pub use summary::*;

#[cfg(test)]
mod tests;
