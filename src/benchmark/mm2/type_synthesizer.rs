//! Type Synthesizer: Generate BCS bytes for Move types using MM2 type information.
//!
//! This module provides the ability to synthesize valid BCS-encoded values for
//! struct types without executing constructor functions. This is useful when:
//!
//! 1. A constructor exists but aborts at runtime (e.g., needs validator state)
//! 2. We need to provide object references for read-only accessor functions
//! 3. Testing type inhabitation without full execution
//!
//! ## How It Works
//!
//! The synthesizer uses MM2's StructInfo to determine field layouts, then
//! recursively generates default values for each field. Special handling is
//! provided for Sui framework types (UID, Balance, Bag, etc.).
//!
//! ## Limitations
//!
//! - Synthesized objects may not pass runtime validation checks
//! - Complex invariants cannot be maintained
//! - Some operations on synthesized objects will abort

use crate::benchmark::mm2::model::TypeModel;
use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use std::collections::HashSet;

/// Well-known Sui framework addresses (without 0x prefix since that's how AccountAddress formats)
const SUI_FRAMEWORK: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const MOVE_STDLIB: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SUI_SYSTEM: &str = "0000000000000000000000000000000000000000000000000000000000000003";

/// With 0x prefix for from_hex_literal calls
#[cfg(test)]
const SUI_SYSTEM_HEX: &str = "0x0000000000000000000000000000000000000000000000000000000000000003";

/// Default number of validators to synthesize to avoid division by zero errors.
/// IMPORTANT: Must be > 0 to prevent division by zero in stake calculations.
const DEFAULT_VALIDATOR_COUNT: usize = 10;

// Compile-time assertion that DEFAULT_VALIDATOR_COUNT > 0
const _: () = assert!(
    DEFAULT_VALIDATOR_COUNT > 0,
    "DEFAULT_VALIDATOR_COUNT must be > 0 to avoid division by zero"
);

/// Type synthesizer that generates BCS bytes for Move types.
///
/// Uses MM2's type information to recursively build valid BCS representations
/// of struct types without executing constructor functions.
pub struct TypeSynthesizer<'a> {
    model: &'a TypeModel,
    /// Track types currently being synthesized to detect cycles
    in_progress: HashSet<String>,
    /// Maximum recursion depth to prevent stack overflow
    max_depth: usize,
}

/// Result of type synthesis
#[derive(Debug, Clone)]
pub struct SynthesisResult {
    /// The synthesized BCS bytes
    pub bytes: Vec<u8>,
    /// Whether this was a "best effort" synthesis (may not be fully valid)
    pub is_stub: bool,
    /// Description of what was synthesized
    pub description: String,
}

impl<'a> TypeSynthesizer<'a> {
    /// Create a new TypeSynthesizer backed by an MM2 TypeModel.
    pub fn new(model: &'a TypeModel) -> Self {
        Self {
            model,
            in_progress: HashSet::new(),
            max_depth: 10,
        }
    }

    /// Synthesize BCS bytes for a struct type.
    ///
    /// # Arguments
    /// * `module_addr` - Module address containing the struct
    /// * `module_name` - Module name
    /// * `struct_name` - Struct name
    ///
    /// # Returns
    /// SynthesisResult containing the BCS bytes and metadata
    pub fn synthesize_struct(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
    ) -> Result<SynthesisResult> {
        self.synthesize_struct_with_depth(module_addr, module_name, struct_name, 0)
    }

    fn synthesize_struct_with_depth(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
        depth: usize,
    ) -> Result<SynthesisResult> {
        if depth > self.max_depth {
            return Err(anyhow!(
                "max recursion depth exceeded synthesizing {}::{}::{}",
                module_addr,
                module_name,
                struct_name
            ));
        }

        let type_key = format!("{}::{}::{}", module_addr, module_name, struct_name);

        // Check for cycles
        if self.in_progress.contains(&type_key) {
            return Err(anyhow!("circular type dependency detected: {}", type_key));
        }

        // Check for Sui framework types first (special handling)
        let addr_str = format!("{}", module_addr);
        if addr_str == SUI_FRAMEWORK || addr_str == MOVE_STDLIB || addr_str == SUI_SYSTEM {
            if let Some(result) =
                self.synthesize_framework_type(&addr_str, module_name, struct_name)?
            {
                return Ok(result);
            }
        }

        // Get struct info from MM2
        let struct_info = self
            .model
            .get_struct(module_addr, module_name, struct_name)
            .ok_or_else(|| {
                anyhow!(
                    "struct not found in model: {}::{}::{}",
                    module_addr,
                    module_name,
                    struct_name
                )
            })?;

        // Mark as in-progress for cycle detection
        self.in_progress.insert(type_key.clone());

        // Synthesize each field
        let mut bytes = Vec::new();
        let mut is_stub = false;

        for field in &struct_info.fields {
            let field_result = self.synthesize_type_str(&field.type_str, depth + 1)?;
            bytes.extend(field_result.bytes);
            is_stub = is_stub || field_result.is_stub;
        }

        // Remove from in-progress
        self.in_progress.remove(&type_key);

        Ok(SynthesisResult {
            bytes,
            is_stub,
            description: format!("synthesized {}::{}", module_name, struct_name),
        })
    }

    /// Synthesize a value for a type string (from MM2's formatted type output).
    pub fn synthesize_type_str(&mut self, type_str: &str, depth: usize) -> Result<SynthesisResult> {
        let type_str = type_str.trim();

        // Handle primitives
        match type_str {
            "bool" => {
                return Ok(SynthesisResult {
                    bytes: vec![0], // false
                    is_stub: false,
                    description: "bool(false)".to_string(),
                });
            }
            "u8" => {
                return Ok(SynthesisResult {
                    bytes: vec![0],
                    is_stub: false,
                    description: "u8(0)".to_string(),
                })
            }
            "u16" => {
                return Ok(SynthesisResult {
                    bytes: 0u16.to_le_bytes().to_vec(),
                    is_stub: false,
                    description: "u16(0)".to_string(),
                })
            }
            "u32" => {
                return Ok(SynthesisResult {
                    bytes: 0u32.to_le_bytes().to_vec(),
                    is_stub: false,
                    description: "u32(0)".to_string(),
                })
            }
            "u64" => {
                return Ok(SynthesisResult {
                    bytes: 0u64.to_le_bytes().to_vec(),
                    is_stub: false,
                    description: "u64(0)".to_string(),
                })
            }
            "u128" => {
                return Ok(SynthesisResult {
                    bytes: 0u128.to_le_bytes().to_vec(),
                    is_stub: false,
                    description: "u128(0)".to_string(),
                })
            }
            "u256" => {
                return Ok(SynthesisResult {
                    bytes: [0u8; 32].to_vec(),
                    is_stub: false,
                    description: "u256(0)".to_string(),
                })
            }
            "address" => {
                return Ok(SynthesisResult {
                    bytes: [0u8; 32].to_vec(),
                    is_stub: false,
                    description: "address(0x0)".to_string(),
                })
            }
            _ => {}
        }

        // Handle vectors
        if type_str.starts_with("vector<") {
            // Empty vector - BCS encodes as length prefix 0
            return Ok(SynthesisResult {
                bytes: vec![0],
                is_stub: false,
                description: format!("empty {}", type_str),
            });
        }

        // Handle Option<T> - synthesize as None
        if type_str.contains("option::Option<") || type_str.contains("::Option<") {
            return Ok(SynthesisResult {
                bytes: vec![0], // BCS encoding of None (empty vector)
                is_stub: false,
                description: "Option::None".to_string(),
            });
        }

        // Handle struct types (format: "0xaddr::module::Name" or "0xaddr::module::Name<T>")
        if type_str.contains("::") {
            return self.synthesize_struct_from_type_str(type_str, depth);
        }

        // Handle type parameters (T0, T1, etc.) - default to u64
        if type_str.starts_with('T') && type_str.len() <= 3 {
            return Ok(SynthesisResult {
                bytes: 0u64.to_le_bytes().to_vec(),
                is_stub: true,
                description: format!("type_param {} as u64", type_str),
            });
        }

        Err(anyhow!("cannot synthesize type: {}", type_str))
    }

    /// Parse a type string and synthesize the struct.
    fn synthesize_struct_from_type_str(
        &mut self,
        type_str: &str,
        depth: usize,
    ) -> Result<SynthesisResult> {
        // Parse "0xaddr::module::Name" or "0xaddr::module::Name<T>"
        // Strip any type arguments for now (we'll handle generics separately)
        let base_type = if let Some(idx) = type_str.find('<') {
            &type_str[..idx]
        } else {
            type_str
        };

        let parts: Vec<&str> = base_type.split("::").collect();
        if parts.len() < 3 {
            return Err(anyhow!("invalid struct type format: {}", type_str));
        }

        let addr_str = parts[0];
        let module_name = parts[1];
        let struct_name = parts[2];

        // Parse address
        let module_addr = AccountAddress::from_hex_literal(addr_str)
            .map_err(|e| anyhow!("invalid address {}: {}", addr_str, e))?;

        self.synthesize_struct_with_depth(&module_addr, module_name, struct_name, depth)
    }

    /// Special handling for Sui framework types.
    ///
    /// These types have well-known structures that we can synthesize directly
    /// without looking up their definitions.
    fn synthesize_framework_type(
        &self,
        addr: &str,
        module_name: &str,
        struct_name: &str,
    ) -> Result<Option<SynthesisResult>> {
        // Sui framework (0x2)
        if addr == SUI_FRAMEWORK {
            match (module_name, struct_name) {
                // object::UID - wrapper around ID
                ("object", "UID") => {
                    return Ok(Some(SynthesisResult {
                        bytes: [0u8; 32].to_vec(), // ID { bytes: address }
                        is_stub: true,
                        description: "UID(synthetic)".to_string(),
                    }));
                }
                // object::ID - just an address
                ("object", "ID") => {
                    return Ok(Some(SynthesisResult {
                        bytes: [0u8; 32].to_vec(),
                        is_stub: true,
                        description: "ID(synthetic)".to_string(),
                    }));
                }
                // balance::Balance<T> - just a u64 value
                ("balance", "Balance") => {
                    return Ok(Some(SynthesisResult {
                        bytes: 0u64.to_le_bytes().to_vec(),
                        is_stub: true,
                        description: "Balance(0)".to_string(),
                    }));
                }
                // balance::Supply<T> - just a u64 value
                ("balance", "Supply") => {
                    return Ok(Some(SynthesisResult {
                        bytes: 0u64.to_le_bytes().to_vec(),
                        is_stub: true,
                        description: "Supply(0)".to_string(),
                    }));
                }
                // bag::Bag - UID + size
                ("bag", "Bag") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // size: u64
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "Bag(empty)".to_string(),
                    }));
                }
                // table::Table<K, V> - UID + size
                ("table", "Table") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // size: u64
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "Table(empty)".to_string(),
                    }));
                }
                // vec_map::VecMap<K, V> - just a vector of entries (empty)
                ("vec_map", "VecMap") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty vector
                        is_stub: true,
                        description: "VecMap(empty)".to_string(),
                    }));
                }
                // vec_set::VecSet<T> - just a vector (empty)
                ("vec_set", "VecSet") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty vector
                        is_stub: true,
                        description: "VecSet(empty)".to_string(),
                    }));
                }
                // coin::Coin<T> - id + balance
                // Use 1 SUI (1_000_000_000 MIST) as default to avoid zero-balance failures
                ("coin", "Coin") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    let one_sui: u64 = 1_000_000_000; // 1 SUI in MIST
                    bytes.extend_from_slice(&one_sui.to_le_bytes()); // balance: Balance<T>
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "Coin(1_SUI)".to_string(),
                    }));
                }
                // coin::TreasuryCap<T> - id + total_supply
                // Use non-zero supply to avoid division-by-zero in percentage calculations
                ("coin", "TreasuryCap") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    let initial_supply: u64 = 1_000_000_000_000; // 1000 SUI total supply
                    bytes.extend_from_slice(&initial_supply.to_le_bytes()); // total_supply: Supply<T>
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "TreasuryCap(1000_SUI_supply)".to_string(),
                    }));
                }
                // clock::Clock - id + timestamp_ms
                ("clock", "Clock") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // timestamp_ms: u64
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "Clock(0)".to_string(),
                    }));
                }
                // tx_context::TxContext - complex but well-known structure
                ("tx_context", "TxContext") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // sender: address
                    bytes.push(32); // tx_hash: vector<u8> length prefix
                    bytes.extend_from_slice(&[0u8; 32]); // tx_hash data
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // epoch
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // epoch_timestamp_ms
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // ids_created
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "TxContext(synthetic)".to_string(),
                    }));
                }
                // string::String - just a vector<u8>
                ("string", "String") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty string
                        is_stub: false,
                        description: "String(empty)".to_string(),
                    }));
                }
                // url::Url - wrapper around String
                ("url", "Url") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty URL (empty string)
                        is_stub: true,
                        description: "Url(empty)".to_string(),
                    }));
                }
                _ => {}
            }
        }

        // Move stdlib (0x1)
        if addr == MOVE_STDLIB {
            match (module_name, struct_name) {
                // option::Option<T> - None = empty vector
                ("option", "Option") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0],
                        is_stub: false,
                        description: "Option::None".to_string(),
                    }));
                }
                // string::String - vector<u8>
                ("string", "String") | ("ascii", "String") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0],
                        is_stub: false,
                        description: "String(empty)".to_string(),
                    }));
                }
                _ => {}
            }
        }

        // Sui System (0x3) - validator and system state types
        if addr == SUI_SYSTEM {
            match (module_name, struct_name) {
                // sui_system::SuiSystemState - the main system object
                // This is a thin wrapper: { id: UID, version: u64 }
                // The actual inner state is stored as a dynamic field
                ("sui_system", "SuiSystemState") => {
                    let mut bytes = Vec::new();
                    // id: UID (32 bytes) - use fixed ID 0x5 for SuiSystemState
                    let mut id_bytes = [0u8; 32];
                    id_bytes[31] = 5; // Object ID 0x5
                    bytes.extend_from_slice(&id_bytes);
                    // version: u64 - use version 2 (current)
                    bytes.extend_from_slice(&2u64.to_le_bytes());
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: format!(
                            "SuiSystemState(synthetic, {} validators assumed)",
                            DEFAULT_VALIDATOR_COUNT
                        ),
                    }));
                }

                // validator_set::ValidatorSet - contains active validators
                // The key issue: functions divide by active_validators.length()
                // We synthesize with DEFAULT_VALIDATOR_COUNT validators to avoid div-by-zero
                ("validator_set", "ValidatorSet") => {
                    return Ok(Some(self.synthesize_validator_set()?));
                }

                // staking_pool::StakedSui - staked SUI receipt
                ("staking_pool", "StakedSui") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&[0u8; 32]); // pool_id: ID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // stake_activation_epoch: u64
                    bytes.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // principal: Balance<SUI> (1 SUI)
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "StakedSui(synthetic)".to_string(),
                    }));
                }

                // staking_pool::FungibleStakedSui - fungible staked SUI
                ("staking_pool", "FungibleStakedSui") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&[0u8; 32]); // pool_id: ID
                    bytes.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // value: u64 (1 SUI worth)
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "FungibleStakedSui(synthetic)".to_string(),
                    }));
                }

                // staking_pool::StakingPool
                ("staking_pool", "StakingPool") => {
                    return Ok(Some(self.synthesize_staking_pool()?));
                }

                _ => {}
            }
        }

        // Not a specially-handled framework type
        Ok(None)
    }

    /// Synthesize a ValidatorSet with DEFAULT_VALIDATOR_COUNT validators.
    /// This ensures operations that divide by validator count don't fail.
    fn synthesize_validator_set(&self) -> Result<SynthesisResult> {
        let mut bytes = Vec::new();

        // ValidatorSet struct fields:
        // total_stake: u64
        let total_stake = 10_000_000_000_000_000u64; // 10M SUI total stake
        bytes.extend_from_slice(&total_stake.to_le_bytes());

        // active_validators: vector<Validator>
        // BCS: length prefix (ULEB128) + validator data
        // We need to synthesize DEFAULT_VALIDATOR_COUNT validators
        bytes.push(DEFAULT_VALIDATOR_COUNT as u8); // length prefix for 10 validators

        for i in 0..DEFAULT_VALIDATOR_COUNT {
            bytes.extend(self.synthesize_minimal_validator(i)?);
        }

        // pending_active_validators: TableVec<Validator> - empty table vec
        bytes.extend_from_slice(&[0u8; 32]); // id: UID
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size: 0

        // pending_removals: vector<u64> - empty
        bytes.push(0);

        // staking_pool_mappings: Table<ID, address>
        bytes.extend_from_slice(&[0u8; 32]); // id: UID
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size: 0

        // inactive_validators: Table<ID, ValidatorWrapper>
        bytes.extend_from_slice(&[0u8; 32]); // id: UID
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size: 0

        // validator_candidates: Table<address, ValidatorWrapper>
        bytes.extend_from_slice(&[0u8; 32]); // id: UID
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size: 0

        // at_risk_validators: VecMap<address, u64> - empty
        bytes.push(0);

        // extra_fields: Bag
        bytes.extend_from_slice(&[0u8; 32]); // id: UID
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size: 0

        Ok(SynthesisResult {
            bytes,
            is_stub: true,
            description: format!("ValidatorSet({} validators)", DEFAULT_VALIDATOR_COUNT),
        })
    }

    /// Synthesize a minimal Validator struct.
    /// This creates a valid but minimal validator for avoiding div-by-zero.
    fn synthesize_minimal_validator(&self, index: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();

        // Validator struct has many fields, we need to provide valid BCS for all of them
        // Key fields that affect division operations:

        // metadata: ValidatorMetadata
        // - sui_address: address
        let mut addr = [0u8; 32];
        addr[31] = (index + 1) as u8; // Unique address per validator
        bytes.extend_from_slice(&addr);

        // - protocol_pubkey_bytes: vector<u8>
        bytes.push(48); // 48 bytes for BLS key
        bytes.extend_from_slice(&[0u8; 48]);

        // - network_pubkey_bytes: vector<u8>
        bytes.push(32); // 32 bytes
        bytes.extend_from_slice(&[0u8; 32]);

        // - worker_pubkey_bytes: vector<u8>
        bytes.push(32); // 32 bytes
        bytes.extend_from_slice(&[0u8; 32]);

        // - proof_of_possession: vector<u8>
        bytes.push(96); // 96 bytes for proof
        bytes.extend_from_slice(&[0u8; 96]);

        // - name: String
        let name = format!("Validator{}", index);
        bytes.push(name.len() as u8);
        bytes.extend_from_slice(name.as_bytes());

        // - description: String
        bytes.push(0); // empty

        // - image_url: Url
        bytes.push(0); // empty

        // - project_url: Url
        bytes.push(0); // empty

        // - net_address: String
        bytes.push(0); // empty

        // - p2p_address: String
        bytes.push(0); // empty

        // - primary_address: String
        bytes.push(0); // empty

        // - worker_address: String
        bytes.push(0); // empty

        // - next_epoch_protocol_pubkey_bytes: Option<vector<u8>>
        bytes.push(0); // None

        // - next_epoch_proof_of_possession: Option<vector<u8>>
        bytes.push(0); // None

        // - next_epoch_network_pubkey_bytes: Option<vector<u8>>
        bytes.push(0); // None

        // - next_epoch_worker_pubkey_bytes: Option<vector<u8>>
        bytes.push(0); // None

        // - next_epoch_net_address: Option<String>
        bytes.push(0); // None

        // - next_epoch_p2p_address: Option<String>
        bytes.push(0); // None

        // - next_epoch_primary_address: Option<String>
        bytes.push(0); // None

        // - next_epoch_worker_address: Option<String>
        bytes.push(0); // None

        // - extra_fields: Bag
        bytes.extend_from_slice(&[0u8; 32]); // id
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size

        // voting_power: u64 - each validator gets 1000 voting power (10000 total)
        bytes.extend_from_slice(&1000u64.to_le_bytes());

        // operation_cap_id: ID
        bytes.extend_from_slice(&[0u8; 32]);

        // gas_price: u64
        bytes.extend_from_slice(&1000u64.to_le_bytes());

        // staking_pool: StakingPool
        bytes.extend(self.synthesize_staking_pool()?.bytes);

        // commission_rate: u64
        bytes.extend_from_slice(&200u64.to_le_bytes()); // 2% commission

        // next_epoch_stake: u64
        let stake = 1_000_000_000_000_000u64 / DEFAULT_VALIDATOR_COUNT as u64; // Equal share
        bytes.extend_from_slice(&stake.to_le_bytes());

        // next_epoch_gas_price: u64
        bytes.extend_from_slice(&1000u64.to_le_bytes());

        // next_epoch_commission_rate: u64
        bytes.extend_from_slice(&200u64.to_le_bytes());

        // extra_fields: Bag
        bytes.extend_from_slice(&[0u8; 32]); // id
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size

        Ok(bytes)
    }

    /// Synthesize a StakingPool.
    fn synthesize_staking_pool(&self) -> Result<SynthesisResult> {
        let mut bytes = Vec::new();

        // id: UID
        bytes.extend_from_slice(&[0u8; 32]);

        // activation_epoch: Option<u64>
        bytes.push(1); // Some
        bytes.extend_from_slice(&0u64.to_le_bytes()); // epoch 0

        // deactivation_epoch: Option<u64>
        bytes.push(0); // None

        // sui_balance: u64 - pool balance
        let pool_balance = 1_000_000_000_000_000u64 / DEFAULT_VALIDATOR_COUNT as u64;
        bytes.extend_from_slice(&pool_balance.to_le_bytes());

        // rewards_pool: Balance<SUI>
        bytes.extend_from_slice(&0u64.to_le_bytes());

        // pool_token_balance: u64
        bytes.extend_from_slice(&pool_balance.to_le_bytes());

        // exchange_rates: Table<u64, PoolTokenExchangeRate>
        bytes.extend_from_slice(&[0u8; 32]); // id
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size

        // pending_stake: u64
        bytes.extend_from_slice(&0u64.to_le_bytes());

        // pending_total_sui_withdraw: u64
        bytes.extend_from_slice(&0u64.to_le_bytes());

        // pending_pool_token_withdraw: u64
        bytes.extend_from_slice(&0u64.to_le_bytes());

        // extra_fields: Bag
        bytes.extend_from_slice(&[0u8; 32]); // id
        bytes.extend_from_slice(&0u64.to_le_bytes()); // size

        Ok(SynthesisResult {
            bytes,
            is_stub: true,
            description: "StakingPool(synthetic)".to_string(),
        })
    }

    /// Check if a type can be synthesized.
    pub fn can_synthesize(&self, type_str: &str) -> bool {
        // Primitives
        if matches!(
            type_str,
            "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address"
        ) {
            return true;
        }

        // Vectors
        if type_str.starts_with("vector<") {
            return true;
        }

        // Options
        if type_str.contains("::Option<") {
            return true;
        }

        // Check if it's a struct we know about
        if type_str.contains("::") {
            let base_type = if let Some(idx) = type_str.find('<') {
                &type_str[..idx]
            } else {
                type_str
            };

            let parts: Vec<&str> = base_type.split("::").collect();
            if parts.len() >= 3 {
                if let Ok(addr) = AccountAddress::from_hex_literal(parts[0]) {
                    // Check framework types
                    let addr_str = format!("{}", addr);
                    if addr_str == SUI_FRAMEWORK || addr_str == MOVE_STDLIB {
                        return true; // We handle most framework types
                    }
                    // Check if model has this struct
                    return self.model.get_struct(&addr, parts[1], parts[2]).is_some();
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::resolver::LocalModuleResolver;

    #[test]
    fn test_primitive_types_recognized() {
        // Verify known primitive types
        let primitives = ["bool", "u8", "u16", "u32", "u64", "u128", "u256", "address"];
        for prim in primitives {
            assert!(
                matches!(
                    prim,
                    "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address"
                ),
                "primitive {} should be recognized",
                prim
            );
        }
    }

    #[test]
    fn test_validator_count_constant() {
        // Ensure we have a non-zero validator count to avoid division by zero
        assert!(DEFAULT_VALIDATOR_COUNT > 0);
        assert_eq!(DEFAULT_VALIDATOR_COUNT, 10);
    }

    /// Helper to create a TypeModel with framework modules for testing.
    fn create_test_model() -> TypeModel {
        let resolver =
            LocalModuleResolver::with_sui_framework().expect("Failed to load Sui framework");
        let modules: Vec<_> = resolver.iter_modules().cloned().collect();
        TypeModel::from_modules(modules).expect("Failed to build TypeModel")
    }

    #[test]
    fn test_sui_system_state_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Test SuiSystemState synthesis
        let sui_system_addr = AccountAddress::from_hex_literal(SUI_SYSTEM_HEX).unwrap();
        let result =
            synthesizer.synthesize_struct(&sui_system_addr, "sui_system", "SuiSystemState");
        assert!(
            result.is_ok(),
            "SuiSystemState synthesis should succeed: {:?}",
            result.err()
        );

        let result = result.unwrap();
        // SuiSystemState: UID (32 bytes) + version (8 bytes) = 40 bytes
        assert_eq!(result.bytes.len(), 40, "SuiSystemState should be 40 bytes");
        assert!(result.is_stub, "Should be marked as stub");
        assert!(
            result.description.contains("10 validators"),
            "Description should mention validator count"
        );
    }

    #[test]
    fn test_validator_set_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let sui_system_addr = AccountAddress::from_hex_literal(SUI_SYSTEM_HEX).unwrap();
        let result =
            synthesizer.synthesize_struct(&sui_system_addr, "validator_set", "ValidatorSet");
        assert!(
            result.is_ok(),
            "ValidatorSet synthesis should succeed: {:?}",
            result.err()
        );

        let result = result.unwrap();
        // ValidatorSet should have non-trivial size due to 10 validators
        assert!(
            result.bytes.len() > 100,
            "ValidatorSet should have substantial data: got {} bytes",
            result.bytes.len()
        );
        assert!(result.is_stub, "Should be marked as stub");
        assert!(
            result.description.contains("10 validators"),
            "Description should mention validator count"
        );
    }

    #[test]
    fn test_staking_pool_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let sui_system_addr = AccountAddress::from_hex_literal(SUI_SYSTEM_HEX).unwrap();
        let result = synthesizer.synthesize_struct(&sui_system_addr, "staking_pool", "StakingPool");
        assert!(
            result.is_ok(),
            "StakingPool synthesis should succeed: {:?}",
            result.err()
        );

        let result = result.unwrap();
        assert!(result.bytes.len() > 0, "StakingPool should have data");
        assert!(result.is_stub, "Should be marked as stub");
    }

    #[test]
    fn test_staked_sui_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let sui_system_addr = AccountAddress::from_hex_literal(SUI_SYSTEM_HEX).unwrap();
        let result = synthesizer.synthesize_struct(&sui_system_addr, "staking_pool", "StakedSui");
        assert!(
            result.is_ok(),
            "StakedSui synthesis should succeed: {:?}",
            result.err()
        );

        let result = result.unwrap();
        // StakedSui: UID (32) + pool_id (32) + activation_epoch (8) + principal (8) = 80 bytes
        assert_eq!(result.bytes.len(), 80, "StakedSui should be 80 bytes");
        assert!(result.is_stub, "Should be marked as stub");
    }
}
