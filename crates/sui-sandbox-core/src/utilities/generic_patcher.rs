//! Generic Object Patcher using Move Type Introspection
//!
//! This module provides a generic mechanism for patching BCS-encoded Sui objects
//! by deserializing them using struct layouts extracted from Move bytecode,
//! modifying fields by name, and re-serializing.
//!
//! **NOTE**: This is a utility for working around infrastructure limitations,
//! specifically the inability to fetch historical bytecode that matches the
//! historical state being replayed. When replaying transactions from the past,
//! the current bytecode may have different version constants than the historical
//! objects being loaded. This patcher modifies version fields in historical objects
//! to pass current bytecode's version checks.
//!
//! ## Key Advantages over Heuristic Patching
//!
//! - **Correctness**: Uses actual struct definitions, not byte offset guessing
//! - **Protocol Agnostic**: Works with any protocol that follows standard patterns
//! - **Maintainable**: Adapts automatically to struct layout changes
//! - **Debuggable**: Reports "patched field `package_version`" not "patched byte 395"
//!
//! ## Usage
//!
//! ```ignore
//! use sui_sandbox_core::utilities::{GenericObjectPatcher, FieldPatchRule, PatchAction};
//!
//! // Create patcher with layout source
//! let mut patcher = GenericObjectPatcher::new();
//!
//! // Add rules for fields to patch
//! patcher.add_rule(FieldPatchRule {
//!     field_name: "package_version".to_string(),
//!     action: PatchAction::SetU64(1),
//! });
//!
//! // Patch an object
//! let patched = patcher.patch_object(type_str, bcs_bytes)?;
//! ```

use anyhow::{anyhow, Result};
use move_binary_format::file_format::{CompiledModule, SignatureToken, StructFieldInformation};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;
use std::collections::HashMap;
use tracing::{debug, trace, warn};

// =============================================================================
// Move Type Representation
// =============================================================================

/// Represents a Move type in a form suitable for BCS decoding.
#[derive(Debug, Clone, PartialEq)]
pub enum MoveType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    U256,
    Address,
    Signer,
    Vector(Box<MoveType>),
    /// A struct type with its full path
    Struct {
        address: AccountAddress,
        module: String,
        name: String,
        type_args: Vec<MoveType>,
    },
    /// Type parameter (for generic types) - index into type arguments
    TypeParameter(u16),
}

impl MoveType {
    /// Check if this is a primitive type with fixed size
    pub fn is_fixed_size(&self) -> bool {
        matches!(
            self,
            MoveType::Bool
                | MoveType::U8
                | MoveType::U16
                | MoveType::U32
                | MoveType::U64
                | MoveType::U128
                | MoveType::U256
                | MoveType::Address
        )
    }

    /// Get the fixed size in bytes, if applicable
    pub fn fixed_size(&self) -> Option<usize> {
        match self {
            MoveType::Bool | MoveType::U8 => Some(1),
            MoveType::U16 => Some(2),
            MoveType::U32 => Some(4),
            MoveType::U64 => Some(8),
            MoveType::U128 => Some(16),
            MoveType::U256 => Some(32),
            MoveType::Address => Some(32),
            _ => None,
        }
    }
}

// =============================================================================
// Dynamic Value Representation
// =============================================================================

/// Runtime representation of a deserialized Move value.
///
/// This preserves the structure of the original value, allowing field-level
/// access and modification before re-serialization.
#[derive(Debug, Clone)]
pub enum DynamicValue {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    U256([u8; 32]), // Stored as raw bytes for simplicity
    Address([u8; 32]),
    Vector(Vec<DynamicValue>),
    Struct {
        type_name: String,
        fields: Vec<(String, DynamicValue)>,
    },
    /// Raw bytes for types we can't fully decode (e.g., native types)
    RawBytes(Vec<u8>),
}

impl DynamicValue {
    /// Get a field value from a struct by name
    pub fn get_field(&self, name: &str) -> Option<&DynamicValue> {
        match self {
            DynamicValue::Struct { fields, .. } => {
                fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// Get a mutable reference to a field value by name
    pub fn get_field_mut(&mut self, name: &str) -> Option<&mut DynamicValue> {
        match self {
            DynamicValue::Struct { fields, .. } => {
                fields.iter_mut().find(|(n, _)| n == name).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// Set a field value in a struct by name
    pub fn set_field(&mut self, name: &str, value: DynamicValue) -> bool {
        match self {
            DynamicValue::Struct { fields, .. } => {
                if let Some((_, v)) = fields.iter_mut().find(|(n, _)| n == name) {
                    *v = value;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Get as u64 if this is a U64 value
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            DynamicValue::U64(v) => Some(*v),
            _ => None,
        }
    }
}

// =============================================================================
// Struct Layout
// =============================================================================

/// Layout information for a Move struct, extracted from bytecode.
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub address: AccountAddress,
    pub module: String,
    pub name: String,
    pub fields: Vec<FieldLayout>,
}

/// Layout information for a single field.
#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub name: String,
    pub field_type: MoveType,
}

// =============================================================================
// Layout Registry
// =============================================================================

/// Registry of struct layouts extracted from compiled modules.
///
/// This caches layouts to avoid repeated bytecode parsing.
pub struct LayoutRegistry {
    /// Cached layouts by type key (address::module::name)
    layouts: HashMap<String, StructLayout>,
    /// Modules available for layout extraction
    modules: HashMap<ModuleId, CompiledModule>,
}

impl LayoutRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            layouts: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    /// Add modules to the registry for layout extraction
    pub fn add_modules<'a>(&mut self, modules: impl Iterator<Item = &'a CompiledModule>) {
        for module in modules {
            let id = module.self_id();
            self.modules.insert(id, module.clone());
        }
    }

    /// Get or compute the layout for a type, along with parsed type arguments
    pub fn get_layout(&mut self, type_str: &str) -> Option<StructLayout> {
        // Parse type string to find the module and type args
        let (address, module_name, struct_name, _type_args) = parse_type_string(type_str)?;

        // Check cache first (without type args, since layout is the same)
        let cache_key = format!(
            "{}::{}::{}",
            address.to_hex_literal(),
            module_name,
            struct_name
        );
        if let Some(layout) = self.layouts.get(&cache_key) {
            return Some(layout.clone());
        }

        // Find the module
        let module_id = ModuleId::new(address, Identifier::new(module_name.clone()).ok()?);
        let module = self.modules.get(&module_id)?;

        // Find the struct definition
        let layout = extract_struct_layout(module, &struct_name)?;

        // Cache it
        self.layouts.insert(cache_key, layout.clone());

        Some(layout)
    }

    /// Get the layout and parsed type arguments for a type string
    pub fn get_layout_with_type_args(
        &mut self,
        type_str: &str,
    ) -> Option<(StructLayout, Vec<MoveType>)> {
        let (_, _, _, type_args_str) = parse_type_string(type_str)?;
        let layout = self.get_layout(type_str)?;
        let type_args = type_args_str
            .map(|s| parse_type_args(&s))
            .unwrap_or_default();
        Some((layout, type_args))
    }

    /// Get layout for a MoveType::Struct
    pub fn get_layout_for_type(&mut self, move_type: &MoveType) -> Option<StructLayout> {
        match move_type {
            MoveType::Struct {
                address,
                module,
                name,
                ..
            } => {
                let type_str = format!("{}::{}::{}", address.to_hex_literal(), module, name);
                self.get_layout(&type_str)
            }
            _ => None,
        }
    }
}

impl Default for LayoutRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a type string like "0x1eabed72...::config::GlobalConfig<T>" into components
/// Returns (address, module, name, type_args_str)
fn parse_type_string(type_str: &str) -> Option<(AccountAddress, String, String, Option<String>)> {
    // Handle generic types by extracting type parameters
    let (base_type, type_args) = if let Some(idx) = type_str.find('<') {
        let end_idx = type_str.rfind('>')?;
        (
            &type_str[..idx],
            Some(type_str[idx + 1..end_idx].to_string()),
        )
    } else {
        (type_str, None)
    };

    let parts: Vec<&str> = base_type.split("::").collect();
    if parts.len() < 3 {
        return None;
    }

    let address = AccountAddress::from_hex_literal(parts[0]).ok()?;
    let module_name = parts[1].to_string();
    let struct_name = parts[2].to_string();

    Some((address, module_name, struct_name, type_args))
}

/// Parse type arguments string like "0x2::sui::SUI, 0x5::usdc::USDC" into MoveTypes
fn parse_type_args(type_args_str: &str) -> Vec<MoveType> {
    if type_args_str.is_empty() {
        return vec![];
    }

    // Split by comma, respecting nested angle brackets
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in type_args_str.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                let arg = type_args_str[start..i].trim();
                if !arg.is_empty() {
                    args.push(parse_single_type(arg));
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    // Don't forget the last argument
    let last = type_args_str[start..].trim();
    if !last.is_empty() {
        args.push(parse_single_type(last));
    }

    args
}

/// Parse a single type string into MoveType
fn parse_single_type(type_str: &str) -> MoveType {
    let type_str = type_str.trim();

    // Primitives
    match type_str {
        "bool" => return MoveType::Bool,
        "u8" => return MoveType::U8,
        "u16" => return MoveType::U16,
        "u32" => return MoveType::U32,
        "u64" => return MoveType::U64,
        "u128" => return MoveType::U128,
        "u256" => return MoveType::U256,
        "address" => return MoveType::Address,
        _ => {}
    }

    // Vector
    if type_str.starts_with("vector<") && type_str.ends_with('>') {
        let inner = &type_str[7..type_str.len() - 1];
        return MoveType::Vector(Box::new(parse_single_type(inner)));
    }

    // Struct type: 0x...::module::Name or 0x...::module::Name<...>
    if let Some((addr, module, name, type_args_opt)) = parse_type_string(type_str) {
        let type_args = type_args_opt
            .map(|s| parse_type_args(&s))
            .unwrap_or_default();
        return MoveType::Struct {
            address: addr,
            module,
            name,
            type_args,
        };
    }

    // Fallback - treat as unknown struct
    MoveType::Struct {
        address: AccountAddress::ZERO,
        module: "unknown".to_string(),
        name: type_str.to_string(),
        type_args: vec![],
    }
}

/// Extract the package address from a type string.
///
/// Given "0x1eabed72...::module::Type<...>", returns "0x1eabed72..." (normalized).
fn extract_package_address(type_str: &str) -> Option<String> {
    // Type format: 0x...::module::Name or 0x...::module::Name<type_args>
    let addr_end = type_str.find("::")?;
    let addr_str = &type_str[..addr_end];

    // Parse and normalize to full hex format
    let addr = AccountAddress::from_hex_literal(addr_str).ok()?;
    Some(addr.to_hex_literal())
}

/// Extract struct layout from a compiled module
fn extract_struct_layout(module: &CompiledModule, struct_name: &str) -> Option<StructLayout> {
    for struct_def in &module.struct_defs {
        let datatype_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
        let name = module.identifier_at(datatype_handle.name).to_string();

        if name == struct_name {
            let fields = match &struct_def.field_information {
                StructFieldInformation::Declared(field_defs) => field_defs
                    .iter()
                    .map(|field| {
                        let field_name = module.identifier_at(field.name).to_string();
                        let field_type = signature_token_to_move_type(module, &field.signature.0);
                        FieldLayout {
                            name: field_name,
                            field_type,
                        }
                    })
                    .collect(),
                StructFieldInformation::Native => {
                    // Native structs don't have field information
                    return None;
                }
            };

            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let address = *module.address_identifier_at(module_handle.address);
            let module_name = module.identifier_at(module_handle.name).to_string();

            return Some(StructLayout {
                address,
                module: module_name,
                name,
                fields,
            });
        }
    }
    None
}

/// Convert a SignatureToken to our MoveType representation
fn signature_token_to_move_type(module: &CompiledModule, token: &SignatureToken) -> MoveType {
    match token {
        SignatureToken::Bool => MoveType::Bool,
        SignatureToken::U8 => MoveType::U8,
        SignatureToken::U16 => MoveType::U16,
        SignatureToken::U32 => MoveType::U32,
        SignatureToken::U64 => MoveType::U64,
        SignatureToken::U128 => MoveType::U128,
        SignatureToken::U256 => MoveType::U256,
        SignatureToken::Address => MoveType::Address,
        SignatureToken::Signer => MoveType::Signer,
        SignatureToken::Vector(inner) => {
            MoveType::Vector(Box::new(signature_token_to_move_type(module, inner)))
        }
        SignatureToken::Datatype(idx) => {
            let datatype_handle = &module.datatype_handles[idx.0 as usize];
            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let address = *module.address_identifier_at(module_handle.address);
            let mod_name = module.identifier_at(module_handle.name).to_string();
            let type_name = module.identifier_at(datatype_handle.name).to_string();
            MoveType::Struct {
                address,
                module: mod_name,
                name: type_name,
                type_args: vec![],
            }
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, type_args) = inst.as_ref();
            let datatype_handle = &module.datatype_handles[idx.0 as usize];
            let module_handle = &module.module_handles[datatype_handle.module.0 as usize];
            let address = *module.address_identifier_at(module_handle.address);
            let mod_name = module.identifier_at(module_handle.name).to_string();
            let type_name = module.identifier_at(datatype_handle.name).to_string();
            let args = type_args
                .iter()
                .map(|t| signature_token_to_move_type(module, t))
                .collect();
            MoveType::Struct {
                address,
                module: mod_name,
                name: type_name,
                type_args: args,
            }
        }
        SignatureToken::Reference(inner) | SignatureToken::MutableReference(inner) => {
            // References shouldn't appear in stored data, but handle gracefully
            signature_token_to_move_type(module, inner)
        }
        SignatureToken::TypeParameter(idx) => MoveType::TypeParameter(*idx),
    }
}

// =============================================================================
// BCS Decoder
// =============================================================================

/// Decodes BCS bytes into DynamicValue using struct layout information.
pub struct BcsDecoder<'a> {
    data: &'a [u8],
    cursor: usize,
    registry: &'a mut LayoutRegistry,
    /// Type arguments for substituting type parameters
    type_args: Vec<MoveType>,
}

impl<'a> BcsDecoder<'a> {
    pub fn new(data: &'a [u8], registry: &'a mut LayoutRegistry) -> Self {
        Self {
            data,
            cursor: 0,
            registry,
            type_args: vec![],
        }
    }

    /// Set type arguments for type parameter substitution
    pub fn with_type_args(mut self, type_args: Vec<MoveType>) -> Self {
        self.type_args = type_args;
        self
    }

    /// Decode a value of the given type
    pub fn decode(&mut self, move_type: &MoveType) -> Result<DynamicValue> {
        match move_type {
            MoveType::Bool => {
                let b = self.read_u8()?;
                Ok(DynamicValue::Bool(b != 0))
            }
            MoveType::U8 => Ok(DynamicValue::U8(self.read_u8()?)),
            MoveType::U16 => Ok(DynamicValue::U16(self.read_u16()?)),
            MoveType::U32 => Ok(DynamicValue::U32(self.read_u32()?)),
            MoveType::U64 => Ok(DynamicValue::U64(self.read_u64()?)),
            MoveType::U128 => Ok(DynamicValue::U128(self.read_u128()?)),
            MoveType::U256 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(self.read_bytes(32)?);
                Ok(DynamicValue::U256(bytes))
            }
            MoveType::Address => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(self.read_bytes(32)?);
                Ok(DynamicValue::Address(bytes))
            }
            MoveType::Signer => {
                // Signer is serialized as address
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(self.read_bytes(32)?);
                Ok(DynamicValue::Address(bytes))
            }
            MoveType::Vector(inner) => {
                let len = self.read_uleb128()? as usize;
                let mut elements = Vec::with_capacity(len);
                for _ in 0..len {
                    elements.push(self.decode(inner)?);
                }
                Ok(DynamicValue::Vector(elements))
            }
            MoveType::Struct {
                address,
                module,
                name,
                type_args,
            } => {
                let type_str = format!("{}::{}::{}", address.to_hex_literal(), module, name);

                // Special handling for well-known types

                // UID is just a wrapper around ID, which is a 32-byte address
                if is_uid_type(address, module, name) {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(self.read_bytes(32)?);
                    return Ok(DynamicValue::Struct {
                        type_name: type_str,
                        fields: vec![("id".to_string(), DynamicValue::Address(bytes))],
                    });
                }

                // Option<T> is serialized as a vector with 0 or 1 elements
                // None = length 0, Some(x) = length 1 + x's bytes
                if is_option_type(address, module, name) {
                    let len = self.read_uleb128()?;
                    if len == 0 {
                        return Ok(DynamicValue::Struct {
                            type_name: type_str,
                            fields: vec![("vec".to_string(), DynamicValue::Vector(vec![]))],
                        });
                    } else if len == 1 {
                        let inner_type = type_args.first().cloned().unwrap_or(MoveType::U8);
                        let inner_value = self.decode(&inner_type)?;
                        return Ok(DynamicValue::Struct {
                            type_name: type_str,
                            fields: vec![(
                                "vec".to_string(),
                                DynamicValue::Vector(vec![inner_value]),
                            )],
                        });
                    } else {
                        // Show more context about what went wrong
                        return Err(anyhow!(
                            "Invalid Option length: {} (bytes at cursor-1: {:02x?})",
                            len,
                            &self.data
                                [(self.cursor.saturating_sub(5))..self.cursor.min(self.data.len())]
                        ));
                    }
                }

                // TypeName is a wrapper around a String (ASCII bytes)
                if is_type_name_type(address, module, name) {
                    let len = self.read_uleb128()? as usize;
                    let bytes = self.read_bytes(len)?.to_vec();
                    return Ok(DynamicValue::Struct {
                        type_name: type_str,
                        fields: vec![("name".to_string(), DynamicValue::RawBytes(bytes))],
                    });
                }

                // String types are length-prefixed byte vectors
                if is_string_type(address, module, name) {
                    let len = self.read_uleb128()? as usize;
                    let bytes = self.read_bytes(len)?.to_vec();
                    return Ok(DynamicValue::Struct {
                        type_name: type_str,
                        fields: vec![("bytes".to_string(), DynamicValue::RawBytes(bytes))],
                    });
                }

                // Special handling for Sui framework dynamic field wrappers (Table, Bag, etc.)
                // These all have layout: { id: UID, size: u64 } = 40 bytes
                // The type parameters are "phantom" - they don't appear in the BCS data.
                if is_sui_table_type(address, module, name) {
                    let mut id_bytes = [0u8; 32];
                    id_bytes.copy_from_slice(self.read_bytes(32)?);
                    let size = self.read_u64()?;
                    return Ok(DynamicValue::Struct {
                        type_name: type_str,
                        fields: vec![
                            ("id".to_string(), DynamicValue::Address(id_bytes)),
                            ("size".to_string(), DynamicValue::U64(size)),
                        ],
                    });
                }

                // Special handling for custom dynamic field wrapper types (WitTable, AcTable, etc.)
                // These follow the same pattern as Sui's Table: { id: UID, size: u64 } = 40 bytes
                // We check this BEFORE looking up the struct layout because these types have
                // phantom type parameters that aren't part of the BCS data.
                //
                // IMPORTANT: Check if we have a layout for this type first. If we do, check
                // if the fields are all serializable or if some are phantom types.
                if is_custom_table_like_type(name) {
                    // Check the registry for actual layout
                    if let Some(layout) = self.registry.get_layout(&type_str) {
                        // Check if all type parameters in the layout are phantom
                        // (i.e., there are TypeParameter fields that reference type args)
                        let has_phantom_params = layout
                            .fields
                            .iter()
                            .any(|f| matches!(&f.field_type, MoveType::TypeParameter(_)));
                        if has_phantom_params {
                            // Has phantom type parameters - use assumed UID+size
                            let mut id_bytes = [0u8; 32];
                            id_bytes.copy_from_slice(self.read_bytes(32)?);
                            let size = self.read_u64()?;
                            return Ok(DynamicValue::Struct {
                                type_name: type_str,
                                fields: vec![
                                    ("id".to_string(), DynamicValue::Address(id_bytes)),
                                    ("size".to_string(), DynamicValue::U64(size)),
                                ],
                            });
                        }
                        // No phantom params - fall through to use actual layout
                    } else {
                        // No layout found - use assumed UID+size
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(self.read_bytes(32)?);
                        let size = self.read_u64()?;
                        return Ok(DynamicValue::Struct {
                            type_name: type_str,
                            fields: vec![
                                ("id".to_string(), DynamicValue::Address(id_bytes)),
                                ("size".to_string(), DynamicValue::U64(size)),
                            ],
                        });
                    }
                }

                // Look up the struct layout
                let layout = self
                    .registry
                    .get_layout(&type_str)
                    .ok_or_else(|| anyhow!("Unknown struct type: {}", type_str))?;

                // Save current type args and set new ones for nested decoding
                let saved_type_args = std::mem::replace(&mut self.type_args, type_args.clone());

                // Decode each field, substituting type parameters
                let mut fields = Vec::with_capacity(layout.fields.len());
                for field in &layout.fields {
                    let resolved_type = self.substitute_type_params(&field.field_type);
                    let value = self.decode(&resolved_type)?;
                    fields.push((field.name.clone(), value));
                }

                // Restore type args
                self.type_args = saved_type_args;

                Ok(DynamicValue::Struct {
                    type_name: type_str,
                    fields,
                })
            }
            MoveType::TypeParameter(idx) => {
                // Substitute with concrete type from type_args
                if (*idx as usize) < self.type_args.len() {
                    let concrete_type = self.type_args[*idx as usize].clone();
                    self.decode(&concrete_type)
                } else {
                    Err(anyhow!(
                        "Type parameter T{} out of bounds (have {} type args)",
                        idx,
                        self.type_args.len()
                    ))
                }
            }
        }
    }

    /// Substitute type parameters in a MoveType with concrete types
    fn substitute_type_params(&self, move_type: &MoveType) -> MoveType {
        match move_type {
            MoveType::TypeParameter(idx) => {
                if (*idx as usize) < self.type_args.len() {
                    self.type_args[*idx as usize].clone()
                } else {
                    move_type.clone()
                }
            }
            MoveType::Vector(inner) => {
                MoveType::Vector(Box::new(self.substitute_type_params(inner)))
            }
            MoveType::Struct {
                address,
                module,
                name,
                type_args,
            } => MoveType::Struct {
                address: *address,
                module: module.clone(),
                name: name.clone(),
                type_args: type_args
                    .iter()
                    .map(|t| self.substitute_type_params(t))
                    .collect(),
            },
            _ => move_type.clone(),
        }
    }

    /// Decode a struct using its layout with type arguments
    pub fn decode_struct_with_type_args(
        &mut self,
        layout: &StructLayout,
        type_args: Vec<MoveType>,
    ) -> Result<DynamicValue> {
        // Set type args for this struct
        let saved_type_args = std::mem::replace(&mut self.type_args, type_args);

        let struct_name = format!(
            "{}::{}::{}",
            layout.address.to_hex_literal(),
            layout.module,
            layout.name
        );

        let mut fields = Vec::with_capacity(layout.fields.len());
        for (idx, field) in layout.fields.iter().enumerate() {
            let resolved_type = self.substitute_type_params(&field.field_type);
            let cursor_before = self.cursor;
            let value = self.decode(&resolved_type).map_err(|e| {
                anyhow!(
                    "Failed decoding field #{} '{}' (type {:?}) of {} at cursor {} (total len {}): {}",
                    idx,
                    field.name,
                    resolved_type,
                    struct_name,
                    cursor_before,
                    self.data.len(),
                    e
                )
            })?;
            fields.push((field.name.clone(), value));
        }

        // Restore
        self.type_args = saved_type_args;

        Ok(DynamicValue::Struct {
            type_name: struct_name,
            fields,
        })
    }

    /// Decode a struct using its layout (no type args - for backwards compatibility)
    pub fn decode_struct(&mut self, layout: &StructLayout) -> Result<DynamicValue> {
        self.decode_struct_with_type_args(layout, vec![])
    }

    // Primitive reading methods
    fn read_u8(&mut self) -> Result<u8> {
        if self.cursor >= self.data.len() {
            return Err(anyhow!("Unexpected end of data"));
        }
        let v = self.data[self.cursor];
        self.cursor += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u128(&mut self) -> Result<u128> {
        let bytes = self.read_bytes(16)?;
        Ok(u128::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_bytes(&mut self, n: usize) -> Result<&[u8]> {
        if self.cursor + n > self.data.len() {
            return Err(anyhow!(
                "Unexpected end of data: need {} bytes at offset {}, have {}",
                n,
                self.cursor,
                self.data.len()
            ));
        }
        let slice = &self.data[self.cursor..self.cursor + n];
        self.cursor += n;
        Ok(slice)
    }

    fn read_uleb128(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = self.read_u8()?;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 63 {
                return Err(anyhow!("ULEB128 overflow"));
            }
        }
        Ok(result)
    }
}

/// Check if this is the UID type (0x2::object::UID)
fn is_uid_type(address: &AccountAddress, module: &str, name: &str) -> bool {
    let sui_framework = AccountAddress::from_hex_literal("0x2").unwrap_or(AccountAddress::ZERO);
    *address == sui_framework && module == "object" && name == "UID"
}

/// Check if this is the Option type (0x1::option::Option)
fn is_option_type(address: &AccountAddress, module: &str, name: &str) -> bool {
    let std_addr = AccountAddress::from_hex_literal("0x1").unwrap_or(AccountAddress::ZERO);
    *address == std_addr && module == "option" && name == "Option"
}

/// Check if this is the TypeName type (0x1::type_name::TypeName)
fn is_type_name_type(address: &AccountAddress, module: &str, name: &str) -> bool {
    let std_addr = AccountAddress::from_hex_literal("0x1").unwrap_or(AccountAddress::ZERO);
    *address == std_addr && module == "type_name" && name == "TypeName"
}

/// Check if this is the String type (0x1::string::String or 0x1::ascii::String)
fn is_string_type(address: &AccountAddress, module: &str, name: &str) -> bool {
    let std_addr = AccountAddress::from_hex_literal("0x1").unwrap_or(AccountAddress::ZERO);
    *address == std_addr && (module == "string" || module == "ascii") && name == "String"
}

/// Check if this is a Sui framework dynamic field wrapper type (Table, Bag, etc.)
/// These types all have the same BCS layout: { id: UID, size: u64 } = 40 bytes
fn is_sui_table_type(address: &AccountAddress, module: &str, name: &str) -> bool {
    let sui_framework = AccountAddress::from_hex_literal("0x2").unwrap_or(AccountAddress::ZERO);
    if *address != sui_framework {
        return false;
    }
    matches!(
        (module, name),
        ("table", "Table")
            | ("object_table", "ObjectTable")
            | ("bag", "Bag")
            | ("object_bag", "ObjectBag")
            | ("linked_table", "LinkedTable")
            | ("table_vec", "TableVec")
            | ("vec_set", "VecSet")
            | ("vec_map", "VecMap")
    )
}

/// Check if this appears to be a custom dynamic field wrapper type.
/// These are types that wrap Table/Bag-like functionality with a different package address.
/// Common patterns: WitTable, AcTable, BalanceBag, etc.
/// Returns true if the type name matches common wrapper patterns.
fn is_custom_table_like_type(name: &str) -> bool {
    // Exact matches for known custom table types
    matches!(
        name,
        "WitTable" | "AcTable" | "BalanceBag" | "CustomTable" | "LinkedTable" | "TypeTable"
    ) || {
        // Pattern-based fallback for other table-like types
        let name_lower = name.to_lowercase();
        name_lower.ends_with("table") || name_lower.ends_with("bag")
    }
}

// =============================================================================
// BCS Encoder
// =============================================================================

/// Encodes DynamicValue back to BCS bytes.
pub struct BcsEncoder {
    output: Vec<u8>,
}

impl BcsEncoder {
    pub fn new() -> Self {
        Self { output: Vec::new() }
    }

    /// Encode a value and return the bytes
    pub fn encode(&mut self, value: &DynamicValue) -> Result<Vec<u8>> {
        self.output.clear();
        self.encode_value(value)?;
        Ok(self.output.clone())
    }

    fn encode_value(&mut self, value: &DynamicValue) -> Result<()> {
        match value {
            DynamicValue::Bool(b) => self.output.push(if *b { 1 } else { 0 }),
            DynamicValue::U8(v) => self.output.push(*v),
            DynamicValue::U16(v) => self.output.extend_from_slice(&v.to_le_bytes()),
            DynamicValue::U32(v) => self.output.extend_from_slice(&v.to_le_bytes()),
            DynamicValue::U64(v) => self.output.extend_from_slice(&v.to_le_bytes()),
            DynamicValue::U128(v) => self.output.extend_from_slice(&v.to_le_bytes()),
            DynamicValue::U256(bytes) => self.output.extend_from_slice(bytes),
            DynamicValue::Address(bytes) => self.output.extend_from_slice(bytes),
            DynamicValue::Vector(elements) => {
                self.write_uleb128(elements.len() as u64);
                for elem in elements {
                    self.encode_value(elem)?;
                }
            }
            DynamicValue::Struct { fields, .. } => {
                for (_, value) in fields {
                    self.encode_value(value)?;
                }
            }
            DynamicValue::RawBytes(bytes) => self.output.extend_from_slice(bytes),
        }
        Ok(())
    }

    fn write_uleb128(&mut self, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.output.push(byte);
                break;
            } else {
                self.output.push(byte | 0x80);
            }
        }
    }
}

impl Default for BcsEncoder {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Patch Rules
// =============================================================================

/// Action to take when patching a field
#[derive(Debug, Clone)]
pub enum PatchAction {
    /// Set to a specific u64 value
    SetU64(u64),
    /// Set to a specific u128 value
    SetU128(u128),
    /// Set to the transaction timestamp (provided at patch time)
    SetToTimestamp,
    /// Set to a value looked up from a registry by type.
    /// Falls back to u64::MAX - 1 if not found (to pass >= version checks).
    SetFromRegistry,
    /// Set to a value looked up from a registry by type.
    /// SKIPS patching if not found in registry (returns None to signal skip).
    /// Use this for protocols like Scallop where the historical state already
    /// matches the historical bytecode and shouldn't be patched.
    SetFromRegistryOrSkip,
}

/// Condition for when to apply a patch
#[derive(Debug, Clone)]
pub enum PatchCondition {
    /// Always apply
    Always,
    /// Only if current value is in range [min, max]
    U64InRange { min: u64, max: u64 },
    /// Only if current value is greater than a reference value
    U64GreaterThan(u64),
}

impl PatchCondition {
    /// Check if the condition matches a value
    pub fn matches(&self, value: &DynamicValue) -> bool {
        match (self, value) {
            (PatchCondition::Always, _) => true,
            (PatchCondition::U64InRange { min, max }, DynamicValue::U64(v)) => {
                *v >= *min && *v <= *max
            }
            (PatchCondition::U64GreaterThan(threshold), DynamicValue::U64(v)) => v > threshold,
            _ => false,
        }
    }
}

/// Rule for patching a specific field
#[derive(Debug, Clone)]
pub struct FieldPatchRule {
    /// Name of the field to patch
    pub field_name: String,
    /// Optional type pattern - if set, only match types containing this substring
    /// E.g., "Version" to only match fields in Version-like structs
    pub type_pattern: Option<String>,
    /// Condition for when to apply
    pub condition: PatchCondition,
    /// Action to take
    pub action: PatchAction,
}

// =============================================================================
// Generic Object Patcher
// =============================================================================

/// Generic object patcher that uses struct introspection to patch fields by name.
///
/// This is a utility for working around infrastructure limitations when replaying
/// historical transactions. When the current bytecode has different version
/// constants than the historical objects being loaded, this patcher modifies
/// version fields to pass the current bytecode's version checks.
pub struct GenericObjectPatcher {
    /// Layout registry for struct introspection
    layout_registry: LayoutRegistry,
    /// Rules for patching fields
    rules: Vec<FieldPatchRule>,
    /// Transaction timestamp for time-based patches
    tx_timestamp_ms: Option<u64>,
    /// Version values looked up from bytecode
    version_registry: HashMap<String, u64>,
    /// Statistics
    patches_applied: HashMap<String, usize>,
    /// Raw byte patches for objects that can't be decoded structurally.
    /// Maps type pattern -> Vec<(byte_offset, value_bytes)>.
    /// Used as a fallback when struct decoding fails.
    raw_patches: HashMap<String, Vec<(usize, Vec<u8>)>>,
}

impl GenericObjectPatcher {
    /// Create a new patcher
    pub fn new() -> Self {
        Self {
            layout_registry: LayoutRegistry::new(),
            rules: Vec::new(),
            tx_timestamp_ms: None,
            version_registry: HashMap::new(),
            patches_applied: HashMap::new(),
            raw_patches: HashMap::new(),
        }
    }

    /// Add modules for struct layout extraction and version constant scanning.
    ///
    /// This method:
    /// 1. Adds modules to the layout registry for struct introspection
    /// 2. Scans constant pools for likely version constants (U64 values 1-100)
    /// 3. Registers detected versions keyed by package address
    pub fn add_modules<'a>(&mut self, modules: impl Iterator<Item = &'a CompiledModule>) {
        let modules_vec: Vec<_> = modules.collect();

        // Add to layout registry
        self.layout_registry
            .add_modules(modules_vec.iter().copied());

        // Scan for version constants
        for module in modules_vec {
            self.scan_module_for_versions(module);
        }
    }

    /// Reserved for future version scanning functionality.
    ///
    /// Note: Automatic version detection via bytecode scanning was removed because
    /// heuristics like "scan for U64 constants in range 1-100" are unreliable:
    /// - Many constants match (percentages, error codes, bit counts, etc.)
    /// - Module naming conventions vary across protocols
    /// - Non-deterministic results when multiple constants exist
    ///
    /// Instead, users should explicitly register versions via `register_version()`
    /// before patching. This is more robust and predictable.
    fn scan_module_for_versions(&mut self, _module: &CompiledModule) {
        // No-op: automatic version detection disabled.
        // Use register_version() to explicitly set expected versions.
    }

    /// Add modules for struct layout extraction only (no version scanning).
    ///
    /// Use this if you want to manually control version registration.
    pub fn add_modules_no_version_scan<'a>(
        &mut self,
        modules: impl Iterator<Item = &'a CompiledModule>,
    ) {
        self.layout_registry.add_modules(modules);
    }

    /// Set the transaction timestamp for time-based patches
    pub fn set_timestamp(&mut self, timestamp_ms: u64) {
        self.tx_timestamp_ms = Some(timestamp_ms);
    }

    /// Register an expected version for a type pattern
    pub fn register_version(&mut self, pattern: &str, version: u64) {
        self.version_registry.insert(pattern.to_string(), version);
    }

    /// Register a raw byte patch for objects that can't be decoded structurally.
    ///
    /// This is a fallback mechanism for complex structs where BCS decoding fails.
    /// The patch will be applied at the specified byte offset when the type matches.
    ///
    /// # Arguments
    /// * `type_pattern` - Substring to match in the type string (e.g., "::config::GlobalConfig")
    /// * `byte_offset` - Byte offset where to write the value
    /// * `value` - Bytes to write at the offset
    ///
    /// # Example
    /// ```ignore
    /// // Patch the package_version field (u64) at offset 32 in Cetus GlobalConfig
    /// patcher.add_raw_patch("::config::GlobalConfig", 32, &1u64.to_le_bytes());
    /// ```
    pub fn add_raw_patch(&mut self, type_pattern: &str, byte_offset: usize, value: &[u8]) {
        self.raw_patches
            .entry(type_pattern.to_string())
            .or_default()
            .push((byte_offset, value.to_vec()));
    }

    /// Convenience method to add a raw u64 patch at a specific byte offset.
    ///
    /// Useful for patching version fields in objects that can't be decoded structurally.
    pub fn add_raw_u64_patch(&mut self, type_pattern: &str, byte_offset: usize, value: u64) {
        self.add_raw_patch(type_pattern, byte_offset, &value.to_le_bytes());
    }

    /// Add a patch rule
    pub fn add_rule(&mut self, rule: FieldPatchRule) {
        self.rules.push(rule);
    }

    /// Add default rules for common DeFi patterns.
    ///
    /// These rules patch fields by name with optional type filtering:
    /// - `package_version`: Set to registry value if registered, otherwise u64::MAX - 1
    /// - `value` (only in types containing "Version"): Set to u64::MAX - 1 to pass version checks
    /// - `last_updated_time`: Set to transaction timestamp
    pub fn add_default_rules(&mut self) {
        // Version field pattern (common in Cetus, Bluefin, GlobalConfig structs)
        // Uses registry if available, otherwise sets to high value
        self.rules.push(FieldPatchRule {
            field_name: "package_version".to_string(),
            type_pattern: None, // Match any type with this field
            condition: PatchCondition::U64InRange { min: 1, max: 100 },
            action: PatchAction::SetFromRegistry,
        });

        // Version struct field (common pattern: Version struct with a `value` field)
        // ONLY matches types containing "Version" to avoid false positives
        //
        // NOTE: For Scallop and similar protocols where the Version.value matches
        // the historical bytecode constant, we should NOT patch. The SetFromRegistryOrSkip
        // action only patches if a specific version is registered for this package.
        // This prevents incorrectly patching Scallop Version objects which don't need it.
        self.rules.push(FieldPatchRule {
            field_name: "value".to_string(),
            type_pattern: Some("Version".to_string()), // Only match Version-like types
            condition: PatchCondition::U64InRange { min: 1, max: 100 },
            action: PatchAction::SetFromRegistryOrSkip,
        });

        // Timestamp fields (e.g., last_updated_time in RewarderManager)
        // Set to transaction timestamp to avoid "future timestamp" errors
        self.rules.push(FieldPatchRule {
            field_name: "last_updated_time".to_string(),
            type_pattern: None, // Match any type
            condition: PatchCondition::Always,
            action: PatchAction::SetToTimestamp,
        });
    }

    /// Patch an object's BCS data
    ///
    /// Returns the patched bytes, or the original bytes if no rules matched
    /// or the type couldn't be decoded.
    pub fn patch_object(&mut self, type_str: &str, bcs_bytes: &[u8]) -> Vec<u8> {
        match self.try_patch_object(type_str, bcs_bytes) {
            Ok(patched) => patched,
            Err(e) => {
                // Try raw byte patches as fallback when struct decoding fails
                if let Some(patched) = self.try_raw_patches(type_str, bcs_bytes) {
                    return patched;
                }
                debug!(
                    type_str = %type_str.chars().take(60).collect::<String>(),
                    error = %e,
                    "failed to patch object"
                );
                bcs_bytes.to_vec()
            }
        }
    }

    /// Try to apply raw byte patches when struct decoding fails
    fn try_raw_patches(&mut self, type_str: &str, bcs_bytes: &[u8]) -> Option<Vec<u8>> {
        // Find matching raw patches
        let mut patches_to_apply: Vec<(usize, Vec<u8>)> = Vec::new();
        for (pattern, patches) in &self.raw_patches {
            if type_str.contains(pattern) {
                patches_to_apply.extend(patches.clone());
            }
        }

        if patches_to_apply.is_empty() {
            return None;
        }

        // Apply patches
        let mut result = bcs_bytes.to_vec();
        for (offset, value) in patches_to_apply {
            if offset + value.len() <= result.len() {
                result[offset..offset + value.len()].copy_from_slice(&value);
                trace!(
                    offset = offset,
                    len = value.len(),
                    type_str = %type_str.chars().take(60).collect::<String>(),
                    "applied raw patch"
                );
                *self
                    .patches_applied
                    .entry("raw_patch".to_string())
                    .or_insert(0) += 1;
            } else {
                warn!(
                    offset = offset,
                    len = value.len(),
                    object_size = result.len(),
                    type_str = %type_str.chars().take(40).collect::<String>(),
                    "raw patch offset exceeds object size"
                );
            }
        }

        Some(result)
    }

    fn try_patch_object(&mut self, type_str: &str, bcs_bytes: &[u8]) -> Result<Vec<u8>> {
        // Get the layout and type arguments for this type
        let (layout, type_args) = self
            .layout_registry
            .get_layout_with_type_args(type_str)
            .ok_or_else(|| anyhow!("No layout found for type: {}", type_str))?;

        // Decode the object with type arguments for substitution
        let mut decoder = BcsDecoder::new(bcs_bytes, &mut self.layout_registry);
        let mut value = decoder.decode_struct_with_type_args(&layout, type_args)?;

        // Apply rules
        let mut any_patched = false;
        for rule in &self.rules.clone() {
            // Check type pattern filter first
            if let Some(ref pattern) = rule.type_pattern {
                if !type_str.contains(pattern) {
                    continue; // Skip this rule - type doesn't match pattern
                }
            }

            if let Some(field_value) = value.get_field(&rule.field_name) {
                if rule.condition.matches(field_value) {
                    // compute_patch_value returns Option - None means skip this patch
                    if let Some(new_value) =
                        self.compute_patch_value(&rule.action, type_str, field_value)?
                    {
                        if value.set_field(&rule.field_name, new_value.clone()) {
                            any_patched = true;
                            *self
                                .patches_applied
                                .entry(rule.field_name.clone())
                                .or_insert(0) += 1;
                            trace!(
                                field = %rule.field_name,
                                new_value = ?new_value,
                                type_str = %type_str.chars().take(60).collect::<String>(),
                                "patched field"
                            );
                        }
                    }
                }
            }
        }

        if any_patched {
            // Re-encode
            let mut encoder = BcsEncoder::new();
            encoder.encode(&value)
        } else {
            Ok(bcs_bytes.to_vec())
        }
    }

    fn compute_patch_value(
        &self,
        action: &PatchAction,
        type_str: &str,
        _current: &DynamicValue,
    ) -> Result<Option<DynamicValue>> {
        match action {
            PatchAction::SetU64(v) => Ok(Some(DynamicValue::U64(*v))),
            PatchAction::SetU128(v) => Ok(Some(DynamicValue::U128(*v))),
            PatchAction::SetToTimestamp => {
                let ts = self
                    .tx_timestamp_ms
                    .ok_or_else(|| anyhow!("No timestamp set for SetToTimestamp action"))?;
                Ok(Some(DynamicValue::U64(ts)))
            }
            PatchAction::SetFromRegistry | PatchAction::SetFromRegistryOrSkip => {
                // Extract package address from type string (e.g., "0x1eab...::module::Type")
                if let Some(pkg_addr) = extract_package_address(type_str) {
                    // Try exact match on package address
                    if let Some(version) = self.version_registry.get(&pkg_addr) {
                        return Ok(Some(DynamicValue::U64(*version)));
                    }
                }

                // Fallback: check if any registered pattern is contained in type_str
                // (for backward compatibility with manual registrations)
                for (pattern, version) in &self.version_registry {
                    if type_str.contains(pattern) {
                        return Ok(Some(DynamicValue::U64(*version)));
                    }
                }

                // For SetFromRegistryOrSkip, return None to skip patching
                // For SetFromRegistry, use fallback high value
                match action {
                    PatchAction::SetFromRegistryOrSkip => Ok(None),
                    _ => Ok(Some(DynamicValue::U64(u64::MAX - 1))),
                }
            }
        }
    }

    /// Get statistics about patches applied
    pub fn stats(&self) -> &HashMap<String, usize> {
        &self.patches_applied
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.patches_applied.clear();
    }

    /// Get detected version constants.
    ///
    /// Returns a map from package address -> detected version.
    /// These are U64 constants in the range 1-100 found in the constant pools.
    pub fn detected_versions(&self) -> &HashMap<String, u64> {
        &self.version_registry
    }

    /// Check if any versions were detected from module scanning.
    pub fn has_detected_versions(&self) -> bool {
        !self.version_registry.is_empty()
    }
}

impl Default for GenericObjectPatcher {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_type_fixed_size() {
        assert_eq!(MoveType::U64.fixed_size(), Some(8));
        assert_eq!(MoveType::U128.fixed_size(), Some(16));
        assert_eq!(MoveType::Address.fixed_size(), Some(32));
        assert_eq!(MoveType::Bool.fixed_size(), Some(1));
        assert!(MoveType::Vector(Box::new(MoveType::U8))
            .fixed_size()
            .is_none());
    }

    #[test]
    fn test_dynamic_value_field_access() {
        let mut value = DynamicValue::Struct {
            type_name: "test".to_string(),
            fields: vec![
                ("version".to_string(), DynamicValue::U64(5)),
                ("name".to_string(), DynamicValue::U8(42)),
            ],
        };

        assert_eq!(value.get_field("version").and_then(|v| v.as_u64()), Some(5));
        assert!(value.set_field("version", DynamicValue::U64(10)));
        assert_eq!(
            value.get_field("version").and_then(|v| v.as_u64()),
            Some(10)
        );
    }

    #[test]
    fn test_bcs_encode_decode_primitives() {
        let mut registry = LayoutRegistry::new();

        // Encode some values
        let original = DynamicValue::U64(12345);
        let mut encoder = BcsEncoder::new();
        let bytes = encoder.encode(&original).unwrap();

        // Decode them back
        let mut decoder = BcsDecoder::new(&bytes, &mut registry);
        let decoded = decoder.decode(&MoveType::U64).unwrap();

        match decoded {
            DynamicValue::U64(v) => assert_eq!(v, 12345),
            _ => panic!("Expected U64"),
        }
    }

    #[test]
    fn test_bcs_encode_decode_vector() {
        let mut registry = LayoutRegistry::new();

        let original = DynamicValue::Vector(vec![
            DynamicValue::U8(1),
            DynamicValue::U8(2),
            DynamicValue::U8(3),
        ]);

        let mut encoder = BcsEncoder::new();
        let bytes = encoder.encode(&original).unwrap();

        // Should be: ULEB128(3), 1, 2, 3
        assert_eq!(bytes, vec![3, 1, 2, 3]);

        let mut decoder = BcsDecoder::new(&bytes, &mut registry);
        let decoded = decoder
            .decode(&MoveType::Vector(Box::new(MoveType::U8)))
            .unwrap();

        match decoded {
            DynamicValue::Vector(v) => {
                assert_eq!(v.len(), 3);
            }
            _ => panic!("Expected Vector"),
        }
    }

    #[test]
    fn test_patch_condition() {
        let value = DynamicValue::U64(25);

        assert!(PatchCondition::Always.matches(&value));
        assert!(PatchCondition::U64InRange { min: 1, max: 50 }.matches(&value));
        assert!(!PatchCondition::U64InRange { min: 30, max: 50 }.matches(&value));
        assert!(PatchCondition::U64GreaterThan(20).matches(&value));
        assert!(!PatchCondition::U64GreaterThan(30).matches(&value));
    }
}
