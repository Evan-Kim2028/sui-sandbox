//! JSON to BCS Reconstruction Utility
//!
//! This module converts Sui object JSON (the decoded Move object representation
//! used by the Sui RPC, GraphQL, Snowflake, and other data sources) back to
//! BCS bytes using struct layouts extracted from Move bytecode.
//!
//! ## How It Works
//!
//! 1. Parse the object type string to identify the struct
//! 2. Load bytecode modules and build a LayoutRegistry
//! 3. Get the StructLayout which contains field names, types, and ORDER
//! 4. Convert JSON fields to DynamicValue following the layout
//! 5. Serialize using BcsEncoder
//!
//! ## Limitations
//!
//! - Requires the same bytecode version that produced the object JSON
//! - Native structs (like some framework types) may not work
//! - Complex nested types require recursive layout resolution

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use super::generic_patcher::{BcsEncoder, DynamicValue, LayoutRegistry, MoveType, StructLayout};

/// Reconstructs BCS bytes from Sui object JSON using bytecode layouts.
///
/// Accepts the standard Sui object JSON format used by the Sui RPC,
/// GraphQL API, Snowflake data warehouse, and other data providers.
pub struct JsonToBcsConverter {
    layout_registry: LayoutRegistry,
}

impl JsonToBcsConverter {
    /// Create a new converter with an empty layout registry.
    pub fn new() -> Self {
        Self {
            layout_registry: LayoutRegistry::new(),
        }
    }

    /// Add compiled modules to the layout registry.
    /// These modules provide struct definitions needed for layout resolution.
    pub fn add_modules(&mut self, modules: &[CompiledModule]) {
        self.layout_registry.add_modules(modules.iter());
    }

    /// Add modules from raw bytecode bytes.
    pub fn add_modules_from_bytes(&mut self, bytecode_list: &[Vec<u8>]) -> Result<()> {
        for bytecode in bytecode_list {
            let module = CompiledModule::deserialize_with_defaults(bytecode)
                .map_err(|e| anyhow!("Failed to deserialize module: {:?}", e))?;
            self.layout_registry.add_modules(std::iter::once(&module));
        }
        Ok(())
    }

    /// Convert Sui object JSON to BCS bytes.
    ///
    /// # Arguments
    /// * `type_str` - The full Sui type string (e.g., "0x97d...::margin_manager::MarginManager<...>")
    /// * `object_json` - The decoded object data (standard Sui object JSON format)
    ///
    /// # Returns
    /// The BCS-encoded bytes that can be loaded into the VM.
    pub fn convert(&mut self, type_str: &str, object_json: &JsonValue) -> Result<Vec<u8>> {
        // Get the struct layout AND type args from bytecode
        let (layout, type_args) = self
            .layout_registry
            .get_layout_with_type_args(type_str)
            .ok_or_else(|| {
                // Extract the package address from the type string to give a helpful hint
                let pkg_hint = type_str
                    .split("::")
                    .next()
                    .filter(|s| s.starts_with("0x"))
                    .map(|addr| format!(" Ensure bytecode for package {} has been loaded via add_modules_from_bytes().", addr))
                    .unwrap_or_default();
                anyhow!("Could not find layout for type: {}.{}", type_str, pkg_hint)
            })?;

        // Convert JSON to DynamicValue following the layout
        let value = self.json_to_dynamic_value_with_type_args(object_json, &layout, &type_args)?;

        // Encode to BCS
        let mut encoder = BcsEncoder::new();
        let bcs_bytes = encoder
            .encode(&value)
            .with_context(|| format!("Failed to encode {} to BCS", type_str))?;

        Ok(bcs_bytes)
    }

    /// Substitute type parameters in a MoveType using the provided type arguments.
    fn substitute_type_params(&self, move_type: &MoveType, type_args: &[MoveType]) -> MoveType {
        match move_type {
            MoveType::TypeParameter(idx) => {
                if (*idx as usize) < type_args.len() {
                    type_args[*idx as usize].clone()
                } else {
                    move_type.clone()
                }
            }
            MoveType::Vector(inner) => {
                MoveType::Vector(Box::new(self.substitute_type_params(inner, type_args)))
            }
            MoveType::Struct {
                address,
                module,
                name,
                type_args: nested_type_args,
            } => MoveType::Struct {
                address: *address,
                module: module.clone(),
                name: name.clone(),
                type_args: nested_type_args
                    .iter()
                    .map(|t| self.substitute_type_params(t, type_args))
                    .collect(),
            },
            _ => move_type.clone(),
        }
    }

    /// Convert a JSON object to DynamicValue using the struct layout with type parameter substitution.
    fn json_to_dynamic_value_with_type_args(
        &mut self,
        json: &JsonValue,
        layout: &StructLayout,
        type_args: &[MoveType],
    ) -> Result<DynamicValue> {
        let json_obj = json
            .as_object()
            .ok_or_else(|| anyhow!("Expected JSON object for struct {}", layout.name))?;

        let mut fields = Vec::new();

        // Process fields in the ORDER defined by the struct layout (critical for BCS!)
        for field_layout in &layout.fields {
            let field_name = &field_layout.name;
            let field_type = self.substitute_type_params(&field_layout.field_type, type_args);

            let json_value = json_obj.get(field_name).ok_or_else(|| {
                anyhow!(
                    "Missing field '{}' in JSON for struct {}",
                    field_name,
                    layout.name
                )
            })?;

            let value = self.convert_field(json_value, &field_type, field_name)?;
            fields.push((field_name.clone(), value));
        }

        Ok(DynamicValue::Struct {
            type_name: layout.name.clone(),
            fields,
        })
    }

    /// Convert a single field from JSON to DynamicValue.
    fn convert_field(
        &mut self,
        json: &JsonValue,
        move_type: &MoveType,
        field_name: &str,
    ) -> Result<DynamicValue> {
        match move_type {
            MoveType::Bool => {
                let v = json
                    .as_bool()
                    .ok_or_else(|| anyhow!("Expected bool for field {}", field_name))?;
                Ok(DynamicValue::Bool(v))
            }
            MoveType::U8 => {
                let v = parse_json_number_u64(json, field_name)? as u8;
                Ok(DynamicValue::U8(v))
            }
            MoveType::U16 => {
                let v = parse_json_number_u64(json, field_name)? as u16;
                Ok(DynamicValue::U16(v))
            }
            MoveType::U32 => {
                let v = parse_json_number_u64(json, field_name)? as u32;
                Ok(DynamicValue::U32(v))
            }
            MoveType::U64 => {
                let v = parse_json_number_u64(json, field_name)?;
                Ok(DynamicValue::U64(v))
            }
            MoveType::U128 => {
                let v = parse_json_number_u128(json, field_name)?;
                Ok(DynamicValue::U128(v))
            }
            MoveType::U256 => {
                let bytes = parse_json_u256(json, field_name)?;
                Ok(DynamicValue::U256(bytes))
            }
            MoveType::Address => {
                let addr_bytes = parse_json_address(json, field_name)?;
                Ok(DynamicValue::Address(addr_bytes))
            }
            MoveType::Signer => {
                let addr_bytes = parse_json_address(json, field_name)?;
                Ok(DynamicValue::Address(addr_bytes))
            }
            MoveType::Vector(inner_type) => self.convert_vector(json, inner_type, field_name),
            MoveType::Struct {
                address,
                module,
                name,
                type_args,
            } => self.convert_struct(json, address, module, name, type_args, field_name),
            MoveType::TypeParameter(_) => {
                Err(anyhow!("Unresolved type parameter in field {}", field_name))
            }
        }
    }

    /// Convert a vector field.
    fn convert_vector(
        &mut self,
        json: &JsonValue,
        inner_type: &MoveType,
        field_name: &str,
    ) -> Result<DynamicValue> {
        // Special case: vector<u8> might be stored as hex string or base64
        if matches!(inner_type, MoveType::U8) {
            if let Some(s) = json.as_str() {
                if let Some(hex_str) = s.strip_prefix("0x") {
                    let bytes = hex::decode(hex_str)
                        .with_context(|| format!("Invalid hex in field {}", field_name))?;
                    return Ok(DynamicValue::Vector(
                        bytes.into_iter().map(DynamicValue::U8).collect(),
                    ));
                }
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(s) {
                    return Ok(DynamicValue::Vector(
                        bytes.into_iter().map(DynamicValue::U8).collect(),
                    ));
                }
            }
        }

        let arr = json
            .as_array()
            .ok_or_else(|| anyhow!("Expected array for field {}", field_name))?;

        let mut elements = Vec::new();
        for (i, elem) in arr.iter().enumerate() {
            let elem_name = format!("{}[{}]", field_name, i);
            let value = self.convert_field(elem, inner_type, &elem_name)?;
            elements.push(value);
        }

        Ok(DynamicValue::Vector(elements))
    }

    /// Convert a struct field.
    fn convert_struct(
        &mut self,
        json: &JsonValue,
        address: &AccountAddress,
        module: &str,
        name: &str,
        type_args: &[MoveType],
        field_name: &str,
    ) -> Result<DynamicValue> {
        // Unwrap type-annotated format: {"fields": {...}, "type": "0x..."}
        let json = if let Some(obj) = json.as_object() {
            if obj.contains_key("fields") && obj.contains_key("type") && obj.len() == 2 {
                obj.get("fields").unwrap_or(json)
            } else {
                json
            }
        } else {
            json
        };

        let base_type = format!("{}::{}::{}", address.to_hex_literal(), module, name);

        let full_type = if type_args.is_empty() {
            base_type.clone()
        } else {
            let type_args_str = type_args
                .iter()
                .map(format_move_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}<{}>", base_type, type_args_str)
        };

        // Handle well-known types
        if base_type.contains("object::UID") || name == "UID" {
            return self.convert_uid(json, field_name);
        }
        if base_type.contains("object::ID") || name == "ID" {
            return self.convert_id(json, field_name);
        }
        if base_type.contains("balance::Balance") || name == "Balance" {
            return self.convert_balance(json, field_name);
        }
        if base_type.contains("option::Option") || name == "Option" {
            return self.convert_option(json, type_args, field_name);
        }
        if name == "VecSet" {
            return self.convert_vec_set(json, field_name);
        }
        if name == "VecMap" {
            return self.convert_vec_map(json, field_name);
        }
        if name == "Table" || name == "Bag" || name == "ObjectTable" || name == "ObjectBag" {
            return self.convert_table_or_bag(json, field_name);
        }
        if name == "String" && (module == "string" || module == "ascii") {
            return self.convert_string(json, field_name);
        }
        if name == "TypeName" && module == "type_name" {
            return self.convert_type_name(json, field_name);
        }

        // Generic struct - try to get layout and recurse
        if let Some((layout, nested_type_args)) =
            self.layout_registry.get_layout_with_type_args(&full_type)
        {
            return self.json_to_dynamic_value_with_type_args(json, &layout, &nested_type_args);
        }

        // Fallback: try to process as generic object
        if let Some(obj) = json.as_object() {
            let mut fields = Vec::new();
            for (k, v) in obj {
                let value = self.infer_and_convert(v, k)?;
                fields.push((k.clone(), value));
            }
            return Ok(DynamicValue::Struct {
                type_name: name.to_string(),
                fields,
            });
        }

        Err(anyhow!(
            "Cannot convert struct {} for field {}",
            full_type,
            field_name
        ))
    }

    fn convert_uid(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let id_obj = json
            .as_object()
            .ok_or_else(|| anyhow!("Expected object for UID in {}", field_name))?;

        let id_value = id_obj
            .get("id")
            .ok_or_else(|| anyhow!("Missing 'id' in UID for {}", field_name))?;

        let addr_bytes = parse_json_address(id_value, &format!("{}.id", field_name))?;

        Ok(DynamicValue::Struct {
            type_name: "UID".to_string(),
            fields: vec![(
                "id".to_string(),
                DynamicValue::Struct {
                    type_name: "ID".to_string(),
                    fields: vec![("bytes".to_string(), DynamicValue::Address(addr_bytes))],
                },
            )],
        })
    }

    fn convert_id(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let addr_bytes = if let Some(obj) = json.as_object() {
            let id_value = obj
                .get("id")
                .ok_or_else(|| anyhow!("Missing 'id' in ID for {}", field_name))?;
            parse_json_address(id_value, &format!("{}.id", field_name))?
        } else {
            parse_json_address(json, field_name)?
        };

        Ok(DynamicValue::Struct {
            type_name: "ID".to_string(),
            fields: vec![("bytes".to_string(), DynamicValue::Address(addr_bytes))],
        })
    }

    fn convert_balance(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let value = if let Some(obj) = json.as_object() {
            let v = obj
                .get("value")
                .ok_or_else(|| anyhow!("Missing 'value' in Balance for {}", field_name))?;
            parse_json_number_u64(v, &format!("{}.value", field_name))?
        } else {
            parse_json_number_u64(json, field_name)?
        };

        Ok(DynamicValue::Struct {
            type_name: "Balance".to_string(),
            fields: vec![("value".to_string(), DynamicValue::U64(value))],
        })
    }

    fn convert_option(
        &mut self,
        json: &JsonValue,
        type_args: &[MoveType],
        field_name: &str,
    ) -> Result<DynamicValue> {
        if json.is_null() {
            Ok(DynamicValue::Vector(vec![]))
        } else {
            // Use the inner type from type_args if available for correct typed conversion
            let inner = if let Some(inner_type) = type_args.first() {
                self.convert_field(json, inner_type, field_name)?
            } else {
                self.infer_and_convert(json, field_name)?
            };
            Ok(DynamicValue::Vector(vec![inner]))
        }
    }

    fn convert_string(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let s = json
            .as_str()
            .ok_or_else(|| anyhow!("Expected string for String in {}", field_name))?;

        let bytes: Vec<DynamicValue> = s.as_bytes().iter().map(|&b| DynamicValue::U8(b)).collect();
        Ok(DynamicValue::Struct {
            type_name: "String".to_string(),
            fields: vec![("bytes".to_string(), DynamicValue::Vector(bytes))],
        })
    }

    fn convert_type_name(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let name_json = if let Some(obj) = json.as_object() {
            obj.get("name")
                .ok_or_else(|| anyhow!("Missing 'name' in TypeName for {}", field_name))?
        } else {
            json
        };

        let name_str = name_json
            .as_str()
            .ok_or_else(|| anyhow!("Expected string for TypeName.name in {}", field_name))?;

        let name_value = self.convert_string(
            &serde_json::Value::String(name_str.to_string()),
            &format!("{}.name", field_name),
        )?;

        Ok(DynamicValue::Struct {
            type_name: "TypeName".to_string(),
            fields: vec![("name".to_string(), name_value)],
        })
    }

    fn convert_vec_set(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let obj = json
            .as_object()
            .ok_or_else(|| anyhow!("Expected object for VecSet in {}", field_name))?;

        let contents = obj
            .get("contents")
            .ok_or_else(|| anyhow!("Missing 'contents' in VecSet for {}", field_name))?;

        let arr = contents
            .as_array()
            .ok_or_else(|| anyhow!("Expected array in VecSet.contents for {}", field_name))?;

        let mut elements = Vec::new();
        for (i, elem) in arr.iter().enumerate() {
            let value = self.infer_and_convert(elem, &format!("{}.contents[{}]", field_name, i))?;
            elements.push(value);
        }

        Ok(DynamicValue::Struct {
            type_name: "VecSet".to_string(),
            fields: vec![("contents".to_string(), DynamicValue::Vector(elements))],
        })
    }

    fn convert_vec_map(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let obj = json
            .as_object()
            .ok_or_else(|| anyhow!("Expected object for VecMap in {}", field_name))?;

        let contents = obj
            .get("contents")
            .ok_or_else(|| anyhow!("Missing 'contents' in VecMap for {}", field_name))?;

        let arr = contents
            .as_array()
            .ok_or_else(|| anyhow!("Expected array in VecMap.contents for {}", field_name))?;

        let mut elements = Vec::new();
        for (i, entry) in arr.iter().enumerate() {
            let entry_obj = entry.as_object().ok_or_else(|| {
                anyhow!(
                    "Expected object in VecMap entry for {}.contents[{}]",
                    field_name,
                    i
                )
            })?;

            let key = entry_obj.get("key").ok_or_else(|| {
                anyhow!(
                    "Missing 'key' in VecMap entry {}.contents[{}]",
                    field_name,
                    i
                )
            })?;
            let value = entry_obj.get("value").ok_or_else(|| {
                anyhow!(
                    "Missing 'value' in VecMap entry {}.contents[{}]",
                    field_name,
                    i
                )
            })?;

            let key_val =
                self.infer_and_convert(key, &format!("{}.contents[{}].key", field_name, i))?;
            let val_val =
                self.infer_and_convert(value, &format!("{}.contents[{}].value", field_name, i))?;

            elements.push(DynamicValue::Struct {
                type_name: "Entry".to_string(),
                fields: vec![("key".to_string(), key_val), ("value".to_string(), val_val)],
            });
        }

        Ok(DynamicValue::Struct {
            type_name: "VecMap".to_string(),
            fields: vec![("contents".to_string(), DynamicValue::Vector(elements))],
        })
    }

    fn convert_table_or_bag(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        let obj = json
            .as_object()
            .ok_or_else(|| anyhow!("Expected object for Table/Bag in {}", field_name))?;

        let id_json = obj
            .get("id")
            .ok_or_else(|| anyhow!("Missing 'id' in Table/Bag for {}", field_name))?;
        let id_value = self.convert_uid(id_json, &format!("{}.id", field_name))?;

        let size_json = obj
            .get("size")
            .ok_or_else(|| anyhow!("Missing 'size' in Table/Bag for {}", field_name))?;
        let size = parse_json_number_u64(size_json, &format!("{}.size", field_name))?;

        Ok(DynamicValue::Struct {
            type_name: "Table".to_string(),
            fields: vec![
                ("id".to_string(), id_value),
                ("size".to_string(), DynamicValue::U64(size)),
            ],
        })
    }

    /// Infer type from JSON value and convert.
    fn infer_and_convert(&mut self, json: &JsonValue, field_name: &str) -> Result<DynamicValue> {
        match json {
            JsonValue::Null => Ok(DynamicValue::Vector(vec![])),
            JsonValue::Bool(b) => Ok(DynamicValue::Bool(*b)),
            JsonValue::Number(n) => {
                if let Some(v) = n.as_u64() {
                    Ok(DynamicValue::U64(v))
                } else if let Some(v) = n.as_i64() {
                    Ok(DynamicValue::U64(v as u64))
                } else {
                    Err(anyhow!("Cannot convert number for {}", field_name))
                }
            }
            JsonValue::String(s) => {
                if s.starts_with("0x") && s.len() == 66 {
                    let bytes = parse_hex_address(s)?;
                    Ok(DynamicValue::Address(bytes))
                } else if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
                    // Try u64 first, then u128, then u256 for large numeric strings
                    if let Ok(v) = s.parse::<u64>() {
                        Ok(DynamicValue::U64(v))
                    } else if let Ok(v) = s.parse::<u128>() {
                        Ok(DynamicValue::U128(v))
                    } else {
                        let bytes = decimal_str_to_u256_le(s, field_name)?;
                        Ok(DynamicValue::U256(bytes))
                    }
                } else {
                    Ok(DynamicValue::Vector(
                        s.as_bytes().iter().map(|b| DynamicValue::U8(*b)).collect(),
                    ))
                }
            }
            JsonValue::Array(arr) => {
                let mut elements = Vec::new();
                for (i, elem) in arr.iter().enumerate() {
                    let value = self.infer_and_convert(elem, &format!("{}[{}]", field_name, i))?;
                    elements.push(value);
                }
                Ok(DynamicValue::Vector(elements))
            }
            JsonValue::Object(_) => {
                if let Some(obj) = json.as_object() {
                    if obj.contains_key("id") && obj.len() == 1 {
                        if let Some(id_val) = obj.get("id") {
                            if id_val.is_string() || id_val.is_object() {
                                return self.convert_uid(json, field_name);
                            }
                        }
                    }
                    // Note: {value: ...} with 1 field could be Balance OR Decimal or other types.
                    // Don't assume Balance here — let the generic handler infer the value type.
                    if obj.contains_key("contents") {
                        if let Some(contents) = obj.get("contents") {
                            if contents.is_array() {
                                if let Some(first) = contents.as_array().and_then(|a| a.first()) {
                                    if first.is_object()
                                        && first.as_object().is_some_and(|o| o.contains_key("key"))
                                    {
                                        return self.convert_vec_map(json, field_name);
                                    }
                                }
                                return self.convert_vec_set(json, field_name);
                            }
                        }
                    }
                    if obj.contains_key("id") && obj.contains_key("size") {
                        return self.convert_table_or_bag(json, field_name);
                    }
                }

                let obj = json.as_object().unwrap();
                let mut fields = Vec::new();
                for (k, v) in obj {
                    let value = self.infer_and_convert(v, &format!("{}.{}", field_name, k))?;
                    fields.push((k.clone(), value));
                }
                Ok(DynamicValue::Struct {
                    type_name: "Unknown".to_string(),
                    fields,
                })
            }
        }
    }
}

impl Default for JsonToBcsConverter {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Parse a JSON value as u64 (handles both numbers and strings).
pub fn parse_json_number_u64(json: &JsonValue, field_name: &str) -> Result<u64> {
    if let Some(n) = json.as_u64() {
        return Ok(n);
    }
    if let Some(n) = json.as_i64() {
        return Ok(n as u64);
    }
    if let Some(s) = json.as_str() {
        return s
            .parse()
            .with_context(|| format!("Failed to parse '{}' as u64 for {}", s, field_name));
    }
    Err(anyhow!(
        "Expected number or numeric string for {}, got {:?}",
        field_name,
        json
    ))
}

/// Parse a JSON value as u128.
pub fn parse_json_number_u128(json: &JsonValue, field_name: &str) -> Result<u128> {
    if let Some(s) = json.as_str() {
        return s
            .parse()
            .with_context(|| format!("Failed to parse '{}' as u128 for {}", s, field_name));
    }
    if let Some(n) = json.as_u64() {
        return Ok(n as u128);
    }
    Err(anyhow!(
        "Expected numeric string for u128 field {}",
        field_name
    ))
}

/// Parse a JSON value as U256 (32 bytes, little-endian).
pub fn parse_json_u256(json: &JsonValue, field_name: &str) -> Result<[u8; 32]> {
    if let Some(s) = json.as_str() {
        if let Some(hex_str) = s.strip_prefix("0x") {
            let bytes = hex::decode(hex_str)
                .with_context(|| format!("Invalid hex for U256 {}", field_name))?;
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                return Ok(arr);
            }
        }
        return decimal_str_to_u256_le(s, field_name);
    }
    if let Some(n) = json.as_u64() {
        let mut arr = [0u8; 32];
        arr[..8].copy_from_slice(&n.to_le_bytes());
        return Ok(arr);
    }
    Err(anyhow!("Expected string for U256 field {}", field_name))
}

/// Convert a decimal string to 32-byte little-endian U256.
/// Handles values from 0 up to 2^256-1.
fn decimal_str_to_u256_le(s: &str, field_name: &str) -> Result<[u8; 32]> {
    // Try u128 first (covers most cases)
    if let Ok(n) = s.parse::<u128>() {
        let mut arr = [0u8; 32];
        arr[..16].copy_from_slice(&n.to_le_bytes());
        return Ok(arr);
    }

    // Full u256 decimal parsing via long multiplication
    let mut result = [0u8; 32];
    for ch in s.chars() {
        if !ch.is_ascii_digit() {
            return Err(anyhow!(
                "Invalid character '{}' in U256 decimal for {}",
                ch,
                field_name
            ));
        }
        let digit = ch as u8 - b'0';
        // result = result * 10 + digit
        let mut carry: u16 = digit as u16;
        for byte in result.iter_mut() {
            let val = (*byte as u16) * 10 + carry;
            *byte = val as u8;
            carry = val >> 8;
        }
        if carry != 0 {
            return Err(anyhow!("U256 overflow for {}", field_name));
        }
    }
    Ok(result)
}

/// Parse a JSON value as an address (32 bytes).
pub fn parse_json_address(json: &JsonValue, field_name: &str) -> Result<[u8; 32]> {
    if let Some(s) = json.as_str() {
        return parse_hex_address(s).with_context(|| format!("Invalid address for {}", field_name));
    }
    if let Some(obj) = json.as_object() {
        if let Some(id) = obj.get("id") {
            return parse_json_address(id, field_name);
        }
    }
    Err(anyhow!(
        "Expected hex string for address field {}",
        field_name
    ))
}

/// Parse a hex address string to 32 bytes.
pub fn parse_hex_address(s: &str) -> Result<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let padded = format!("{:0>64}", s);
    let bytes = hex::decode(&padded).with_context(|| format!("Invalid hex address: 0x{}", s))?;
    if bytes.len() != 32 {
        return Err(anyhow!("Address must be 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Format a MoveType back to a type string.
pub fn format_move_type(move_type: &MoveType) -> String {
    match move_type {
        MoveType::Bool => "bool".to_string(),
        MoveType::U8 => "u8".to_string(),
        MoveType::U16 => "u16".to_string(),
        MoveType::U32 => "u32".to_string(),
        MoveType::U64 => "u64".to_string(),
        MoveType::U128 => "u128".to_string(),
        MoveType::U256 => "u256".to_string(),
        MoveType::Address => "address".to_string(),
        MoveType::Signer => "signer".to_string(),
        MoveType::Vector(inner) => format!("vector<{}>", format_move_type(inner)),
        MoveType::Struct {
            address,
            module,
            name,
            type_args,
        } => {
            let base = format!("{}::{}::{}", address.to_hex_literal(), module, name);
            if type_args.is_empty() {
                base
            } else {
                let args_str = type_args
                    .iter()
                    .map(format_move_type)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", base, args_str)
            }
        }
        MoveType::TypeParameter(idx) => format!("T{}", idx),
    }
}

/// One object reconstruction input row used by generic JSON->BCS validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonBcsValidationObject {
    pub object_id: String,
    pub version: u64,
    pub object_type: String,
    pub object_json: JsonValue,
}

/// Validation plan for reconstructing object JSON and comparing to historical BCS.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JsonBcsValidationPlan {
    #[serde(default)]
    pub package_roots: Vec<AccountAddress>,
    #[serde(default)]
    pub type_refs: Vec<String>,
    #[serde(default)]
    pub objects: Vec<JsonBcsValidationObject>,
    #[serde(default)]
    pub historical_mode: bool,
}

/// Validation status for one object row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JsonBcsValidationStatus {
    Match,
    Mismatch,
    MissingBaseline,
    ConversionError,
}

/// Per-object validation output row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonBcsValidationEntry {
    pub object_id: String,
    pub object_type: String,
    pub status: JsonBcsValidationStatus,
    pub reconstructed_len: Option<usize>,
    pub baseline_len: Option<usize>,
    pub mismatch_offset: Option<usize>,
    pub error: Option<String>,
}

/// Aggregate validation counters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JsonBcsValidationSummary {
    pub total: usize,
    pub matched: usize,
    pub mismatched: usize,
    pub missing_baseline: usize,
    pub conversion_errors: usize,
}

/// Output report for a full JSON->BCS validation pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonBcsValidationReport {
    pub grpc_endpoint: String,
    pub resolved_package_roots: usize,
    pub package_count: usize,
    pub module_count: usize,
    pub entries: Vec<JsonBcsValidationEntry>,
    pub summary: JsonBcsValidationSummary,
}

/// Reconstruct object JSON into BCS and compare with historical on-chain BCS.
///
/// This is generic and protocol-agnostic: callers provide package roots/type refs
/// plus object rows `{id, version, type, object_json}`.
pub async fn validate_json_bcs_reconstruction(
    plan: &JsonBcsValidationPlan,
) -> Result<JsonBcsValidationReport> {
    if plan.objects.is_empty() {
        return Err(anyhow!("json bcs validation plan has no objects"));
    }

    let provider = crate::bootstrap::create_mainnet_provider(plan.historical_mode).await?;
    let endpoint = provider.grpc_endpoint().to_string();

    let mut type_refs = plan.type_refs.clone();
    if type_refs.is_empty() {
        type_refs = plan
            .objects
            .iter()
            .map(|object| object.object_type.clone())
            .collect();
    }
    let package_roots: Vec<AccountAddress> =
        super::package_roots::collect_required_package_roots_from_type_strings(
            &plan.package_roots,
            &type_refs,
        )?
        .into_iter()
        .collect();
    if package_roots.is_empty() {
        return Err(anyhow!(
            "no package roots resolved for json bcs validation; provide package_roots and/or type_refs"
        ));
    }

    let packages = provider
        .fetch_packages_with_deps(&package_roots, None, None)
        .await
        .context("failed to fetch package closure for json bcs validation")?;

    let mut converter = JsonToBcsConverter::new();
    let mut module_count = 0usize;
    for pkg in packages.values() {
        let bytecode: Vec<Vec<u8>> = pkg.modules.iter().map(|(_, bytes)| bytes.clone()).collect();
        converter.add_modules_from_bytes(&bytecode)?;
        module_count += bytecode.len();
    }

    let grpc = provider.grpc();
    let mut entries = Vec::with_capacity(plan.objects.len());
    for object in &plan.objects {
        let baseline = grpc
            .get_object_at_version(&object.object_id, Some(object.version))
            .await
            .ok()
            .flatten()
            .and_then(|obj| obj.bcs);

        match converter.convert(&object.object_type, &object.object_json) {
            Ok(reconstructed) => {
                if let Some(expected) = baseline.as_ref() {
                    if reconstructed == *expected {
                        entries.push(JsonBcsValidationEntry {
                            object_id: object.object_id.clone(),
                            object_type: object.object_type.clone(),
                            status: JsonBcsValidationStatus::Match,
                            reconstructed_len: Some(reconstructed.len()),
                            baseline_len: Some(expected.len()),
                            mismatch_offset: None,
                            error: None,
                        });
                    } else {
                        let mismatch_offset = reconstructed
                            .iter()
                            .zip(expected.iter())
                            .position(|(left, right)| left != right)
                            .or_else(|| {
                                if reconstructed.len() != expected.len() {
                                    Some(reconstructed.len().min(expected.len()))
                                } else {
                                    None
                                }
                            });
                        entries.push(JsonBcsValidationEntry {
                            object_id: object.object_id.clone(),
                            object_type: object.object_type.clone(),
                            status: JsonBcsValidationStatus::Mismatch,
                            reconstructed_len: Some(reconstructed.len()),
                            baseline_len: Some(expected.len()),
                            mismatch_offset,
                            error: None,
                        });
                    }
                } else {
                    entries.push(JsonBcsValidationEntry {
                        object_id: object.object_id.clone(),
                        object_type: object.object_type.clone(),
                        status: JsonBcsValidationStatus::MissingBaseline,
                        reconstructed_len: Some(reconstructed.len()),
                        baseline_len: None,
                        mismatch_offset: None,
                        error: None,
                    });
                }
            }
            Err(err) => {
                entries.push(JsonBcsValidationEntry {
                    object_id: object.object_id.clone(),
                    object_type: object.object_type.clone(),
                    status: JsonBcsValidationStatus::ConversionError,
                    reconstructed_len: None,
                    baseline_len: baseline.as_ref().map(Vec::len),
                    mismatch_offset: None,
                    error: Some(err.to_string()),
                });
            }
        }
    }

    let summary = entries
        .iter()
        .fold(JsonBcsValidationSummary::default(), |mut summary, entry| {
            summary.total += 1;
            match entry.status {
                JsonBcsValidationStatus::Match => summary.matched += 1,
                JsonBcsValidationStatus::Mismatch => summary.mismatched += 1,
                JsonBcsValidationStatus::MissingBaseline => summary.missing_baseline += 1,
                JsonBcsValidationStatus::ConversionError => summary.conversion_errors += 1,
            }
            summary
        });

    Ok(JsonBcsValidationReport {
        grpc_endpoint: endpoint,
        resolved_package_roots: package_roots.len(),
        package_count: packages.len(),
        module_count,
        entries,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_address() {
        let addr = parse_hex_address("0x2").unwrap();
        assert_eq!(addr[31], 2);
        assert!(addr[..31].iter().all(|&b| b == 0));

        let full = "0xbcb8ee0447179ea67787dfca1d4d0c54ff82ffe67794f851a0329e40306bfa60";
        let addr = parse_hex_address(full).unwrap();
        assert_eq!(addr[0], 0xbc);
    }

    #[test]
    fn test_parse_json_number() {
        assert_eq!(
            parse_json_number_u64(&serde_json::json!(123), "test").unwrap(),
            123
        );
        assert_eq!(
            parse_json_number_u64(&serde_json::json!("456"), "test").unwrap(),
            456
        );
        assert_eq!(
            parse_json_number_u64(&serde_json::json!("6531093430"), "test").unwrap(),
            6531093430
        );
    }

    #[test]
    fn test_parse_json_u128() {
        let v =
            parse_json_number_u128(&serde_json::json!("787937890670812057358292"), "test").unwrap();
        assert_eq!(v, 787937890670812057358292u128);
    }

    #[test]
    fn test_parse_json_u256_from_decimal() {
        // Test the Suilend Decimal value that was failing
        let json = serde_json::json!("787937890670812057358292");
        let bytes = parse_json_u256(&json, "test").unwrap();
        // Verify it's little-endian: reconstruct u128 from low 16 bytes
        let mut le_bytes = [0u8; 16];
        le_bytes.copy_from_slice(&bytes[..16]);
        let reconstructed = u128::from_le_bytes(le_bytes);
        assert_eq!(reconstructed, 787937890670812057358292u128);
        // High 16 bytes should be zero
        assert!(bytes[16..].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_decimal_str_to_u256_le_zero() {
        let bytes = decimal_str_to_u256_le("0", "test").unwrap();
        assert!(bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_decimal_str_to_u256_le_small() {
        let bytes = decimal_str_to_u256_le("1000000000", "test").unwrap();
        let mut le_bytes = [0u8; 8];
        le_bytes.copy_from_slice(&bytes[..8]);
        let v = u64::from_le_bytes(le_bytes);
        assert_eq!(v, 1_000_000_000);
    }
}
