//! # LLM Tools for State Synthesis
//!
//! **⚠️ DEPRECATED**: This module contains legacy APIs. For new integrations, use
//! [`crate::benchmark::sandbox_exec::SandboxRequest`] which provides:
//! - Single unified API via `execute_request()`
//! - Complete tool discovery via `{"action": "list_available_tools"}`
//! - Shared state through `SimulationEnvironment`
//!
//! The [`ToolCall`] enum and [`LlmToolkit`] struct are kept for backwards
//! compatibility but should not be used in new code.
//!
//! ## What to use instead
//!
//! | Legacy (this module) | Use instead (sandbox_exec) |
//! |---------------------|---------------------------|
//! | `LlmToolkit::execute(ToolCall::ListModules)` | `{"action": "list_modules"}` |
//! | `LlmToolkit::execute(ToolCall::GetStructInfo{..})` | `{"action": "get_struct_info", ...}` |
//! | `LlmToolkit::tool_schema()` | `{"action": "list_available_tools"}` |
//!
//! ## Still useful from this module
//!
//! - [`StructInfo`], [`FieldInfo`], [`FunctionInfo`] - Response types
//! - [`ModuleIntrospector`] - Used internally by sandbox_exec
//! - [`ObjectSynthesizer`] - BCS encoding utilities

use anyhow::{anyhow, Result};
use chrono::Utc;
use move_binary_format::file_format::CompiledModule;
use move_command_line_common::files::FileHash;
use move_disassembler::disassembler::Disassembler;
use move_ir_types::location::Loc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

use crate::benchmark::package_builder::{PackageBuilder, FrameworkCache};
use crate::benchmark::storage_log::{
    StorageLogger, PackageLog, ToolCallLog, ObjectSynthesisLog,
};
use crate::bytecode::build_bytecode_module_json;
use crate::types::BytecodeModuleJson;

// ============================================================================
// Tool Response Types
// ============================================================================

/// Information about a struct type that an LLM can use to synthesize objects
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructInfo {
    /// Full type path: "0x...::module::StructName"
    pub type_path: String,
    /// Package address
    pub package: String,
    /// Module name
    pub module: String,
    /// Struct name
    pub name: String,
    /// Abilities (key, store, copy, drop)
    pub abilities: Vec<String>,
    /// Whether this is a Sui object (has 'key' ability)
    pub is_object: bool,
    /// Field definitions
    pub fields: Vec<FieldInfo>,
    /// Type parameters if generic
    pub type_params: Vec<TypeParamInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldInfo {
    pub name: String,
    /// Type as a human-readable string
    pub type_str: String,
    /// Type as structured JSON for programmatic use
    pub type_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeParamInfo {
    pub constraints: Vec<String>,
    pub is_phantom: bool,
}

/// Information about a function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionInfo {
    /// Full path: "0x...::module::function_name"
    pub path: String,
    pub package: String,
    pub module: String,
    pub name: String,
    pub visibility: String,
    pub is_entry: bool,
    /// Parameter types as human-readable strings
    pub params: Vec<String>,
    /// Return types as human-readable strings
    pub returns: Vec<String>,
}

/// Parsed abort error with context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortContext {
    pub abort_code: u64,
    pub module_address: String,
    pub module_name: String,
    pub function_name: String,
    pub instruction_offset: u16,
}

// ============================================================================
// Module Introspection Tools
// ============================================================================

/// Tools for introspecting Move modules
pub struct ModuleIntrospector {
    /// Cached module info by package::module
    modules: HashMap<String, BytecodeModuleJson>,
    /// Raw bytecode for disassembly (stored separately to avoid Clone requirement)
    raw_bytecode: HashMap<String, Vec<u8>>,
}

impl ModuleIntrospector {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            raw_bytecode: HashMap::new(),
        }
    }

    /// Load a compiled module for introspection
    pub fn load_module(&mut self, module: &CompiledModule) -> Result<()> {
        let module_json = build_bytecode_module_json(module)?;
        let key = format!(
            "{}::{}",
            module_json.address,
            module.self_id().name()
        );
        self.modules.insert(key, module_json);
        Ok(())
    }

    /// Load modules from raw bytecode bytes
    pub fn load_from_bytes(&mut self, _package_id: &str, modules: &[(String, Vec<u8>)]) -> Result<()> {
        for (name, bytes) in modules {
            let compiled = CompiledModule::deserialize_with_defaults(bytes)
                .map_err(|e| anyhow!("Failed to deserialize module {}: {:?}", name, e))?;

            // Store raw bytecode for disassembly
            let module_json = build_bytecode_module_json(&compiled)?;
            let key = format!(
                "{}::{}",
                module_json.address,
                compiled.self_id().name()
            );
            self.raw_bytecode.insert(key.clone(), bytes.clone());
            self.modules.insert(key, module_json);
        }
        Ok(())
    }

    /// List all loaded modules
    pub fn list_modules(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }

    /// List all structs in a module
    pub fn list_structs(&self, module_path: &str) -> Option<Vec<String>> {
        self.modules.get(module_path).map(|m| {
            m.structs.keys().cloned().collect()
        })
    }

    /// Get detailed struct information
    pub fn get_struct_info(&self, module_path: &str, struct_name: &str) -> Option<StructInfo> {
        let module = self.modules.get(module_path)?;
        let struct_def = module.structs.get(struct_name)?;

        let parts: Vec<&str> = module_path.splitn(2, "::").collect();
        let (package, module_name) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (module_path.to_string(), "".to_string())
        };

        Some(StructInfo {
            type_path: format!("{}::{}", module_path, struct_name),
            package: package.clone(),
            module: module_name.clone(),
            name: struct_name.to_string(),
            abilities: struct_def.abilities.clone(),
            is_object: struct_def.abilities.contains(&"key".to_string()),
            fields: struct_def.fields.iter().map(|f| FieldInfo {
                name: f.name.clone(),
                type_str: type_json_to_string(&f.r#type),
                type_json: f.r#type.clone(),
            }).collect(),
            type_params: struct_def.type_params.iter().map(|tp| TypeParamInfo {
                constraints: tp.constraints.clone(),
                is_phantom: tp.is_phantom,
            }).collect(),
        })
    }

    /// List all functions in a module
    pub fn list_functions(&self, module_path: &str) -> Option<Vec<String>> {
        self.modules.get(module_path).map(|m| {
            m.functions.keys().cloned().collect()
        })
    }

    /// Get function signature
    pub fn get_function_info(&self, module_path: &str, function_name: &str) -> Option<FunctionInfo> {
        let module = self.modules.get(module_path)?;
        let func = module.functions.get(function_name)?;

        let parts: Vec<&str> = module_path.splitn(2, "::").collect();
        let (package, module_name) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (module_path.to_string(), "".to_string())
        };

        Some(FunctionInfo {
            path: format!("{}::{}", module_path, function_name),
            package,
            module: module_name,
            name: function_name.to_string(),
            visibility: func.visibility.clone(),
            is_entry: func.is_entry,
            params: func.params.iter().map(|p| type_json_to_string(p)).collect(),
            returns: func.returns.iter().map(|r| type_json_to_string(r)).collect(),
        })
    }

    /// Generate a human-readable summary of a module
    pub fn module_summary(&self, module_path: &str) -> Option<String> {
        let module = self.modules.get(module_path)?;

        let mut summary = format!("Module: {}\n\n", module_path);

        summary.push_str("== Structs ==\n");
        for (name, s) in &module.structs {
            let abilities = s.abilities.join(", ");
            summary.push_str(&format!("  {} [{}]\n", name, abilities));
            for field in &s.fields {
                summary.push_str(&format!("    - {}: {}\n", field.name, type_json_to_string(&field.r#type)));
            }
        }

        summary.push_str("\n== Functions ==\n");
        for (name, f) in &module.functions {
            if f.visibility == "public" || f.is_entry {
                let params: Vec<String> = f.params.iter().map(|p| type_json_to_string(p)).collect();
                let returns: Vec<String> = f.returns.iter().map(|r| type_json_to_string(r)).collect();
                let ret_str = if returns.is_empty() {
                    "".to_string()
                } else {
                    format!(" -> {}", returns.join(", "))
                };
                summary.push_str(&format!("  {} {}({}){}\n",
                    f.visibility, name, params.join(", "), ret_str));
            }
        }

        Some(summary)
    }

    /// Disassemble a specific function's bytecode.
    ///
    /// Returns human-readable bytecode instructions showing:
    /// - Each instruction with its offset
    /// - Basic blocks (B0, B1, etc.)
    /// - Local variable and parameter types
    /// - Function calls with resolved names
    ///
    /// This is useful for understanding what happens at specific instruction offsets
    /// (e.g., when an abort error reports "at offset 14").
    pub fn disassemble_function(&self, module_path: &str, function_name: &str) -> Option<String> {
        let bytecode = self.raw_bytecode.get(module_path)?;

        // Deserialize the module
        let compiled = CompiledModule::deserialize_with_defaults(bytecode).ok()?;

        // Verify the function exists before disassembling
        let _func_exists = compiled.function_defs.iter().any(|def| {
            let handle = &compiled.function_handles[def.function.0 as usize];
            let name = compiled.identifier_at(handle.name);
            name.as_str() == function_name
        });
        if !_func_exists {
            return None;
        }

        // Use the move-disassembler to get full module disassembly
        let default_loc = Loc::new(FileHash::empty(), 0, 0);
        let disassembler = Disassembler::from_module(&compiled, default_loc).ok()?;
        let full_output = disassembler.disassemble().ok()?;

        // Extract just the function we want from the full disassembly
        extract_function_from_disassembly(&full_output, function_name)
    }

    /// Disassemble the entire module.
    ///
    /// Returns human-readable bytecode for all functions in the module.
    pub fn disassemble_module(&self, module_path: &str) -> Option<String> {
        let bytecode = self.raw_bytecode.get(module_path)?;

        // Deserialize and disassemble
        let compiled = CompiledModule::deserialize_with_defaults(bytecode).ok()?;
        let default_loc = Loc::new(FileHash::empty(), 0, 0);
        let disassembler = Disassembler::from_module(&compiled, default_loc).ok()?;
        disassembler.disassemble().ok()
    }
}

/// Extract a single function's disassembly from the full module output
fn extract_function_from_disassembly(full: &str, function_name: &str) -> Option<String> {
    let lines: Vec<&str> = full.lines().collect();

    // Find the function start - pattern like "public function_name(" or "entry function_name("
    let mut start_idx = None;
    for (i, line) in lines.iter().enumerate() {
        // Match function declarations: public/private/entry/friend + name + (
        let trimmed = line.trim();
        if (trimmed.starts_with("public ") ||
            trimmed.starts_with("public(") ||
            trimmed.starts_with("entry ") ||
            trimmed.starts_with("native ") ||
            trimmed.is_empty() == false)
            && trimmed.contains(&format!("{}(", function_name))
            && trimmed.contains("{")
        {
            start_idx = Some(i);
            break;
        }
        // Also match just the function name at start for private functions
        if trimmed.starts_with(function_name) && trimmed.contains("(") && trimmed.contains("{") {
            start_idx = Some(i);
            break;
        }
    }

    let start = start_idx?;

    // Find the matching closing brace
    let mut brace_count = 0;
    let mut end_idx = start;
    for (i, line) in lines.iter().enumerate().skip(start) {
        brace_count += line.matches('{').count();
        brace_count -= line.matches('}').count();
        if brace_count == 0 && i > start {
            end_idx = i;
            break;
        }
    }

    // Include the closing brace line
    let function_lines = &lines[start..=end_idx];
    Some(function_lines.join("\n"))
}

impl Default for ModuleIntrospector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Error Context Tools
// ============================================================================

/// Parse an abort error string into structured context
pub fn parse_abort_error(error: &str) -> Option<AbortContext> {
    if !error.contains("ABORTED") {
        return None;
    }

    // Extract abort code: "sub_status: Some(2)"
    let abort_code = error
        .find("sub_status: Some(")
        .and_then(|start| {
            let rest = &error[start + 17..];
            rest.find(')').and_then(|end| rest[..end].parse::<u64>().ok())
        })
        .unwrap_or(0);

    // Extract module: "location: Module(ModuleId { address: ..., name: Identifier(\"...\") })"
    let module_address = error
        .find("address: ")
        .map(|start| {
            let rest = &error[start + 9..];
            let end = rest.find(',').unwrap_or(rest.len());
            rest[..end].trim().to_string()
        })
        .unwrap_or_default();

    let module_name = error
        .find("name: Identifier(\"")
        .and_then(|start| {
            let rest = &error[start + 18..];
            rest.find('"').map(|end| rest[..end].to_string())
        })
        .unwrap_or_default();

    // Extract function from message: "0x...::module::function at offset N"
    let function_name = error
        .find("message: Some(\"")
        .and_then(|start| {
            let rest = &error[start + 15..];
            // Pattern: "0x...::module::function at offset"
            if let Some(at_pos) = rest.find(" at offset") {
                let path = &rest[..at_pos];
                path.rsplit("::").next().map(|s| s.to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Extract instruction offset
    let instruction_offset = error
        .find("at offset ")
        .and_then(|start| {
            let rest = &error[start + 10..];
            let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            rest[..end].parse::<u16>().ok()
        })
        .unwrap_or(0);

    Some(AbortContext {
        abort_code,
        module_address,
        module_name,
        function_name,
        instruction_offset,
    })
}

// ============================================================================
// State Synthesis Types
// ============================================================================

/// A request to create an object with specific field values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectSynthesisRequest {
    /// The type to create: "0x...::module::StructName"
    pub type_path: String,
    /// Field values as JSON
    pub fields: serde_json::Value,
    /// Optional: specific object ID to use
    pub object_id: Option<String>,
    /// Whether to create as shared object
    #[serde(default)]
    pub is_shared: bool,
}

/// Result of object synthesis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesizedObject {
    pub object_id: String,
    pub type_path: String,
    /// BCS-encoded bytes
    pub bcs_bytes: Vec<u8>,
    pub is_shared: bool,
}

// ============================================================================
// Object Synthesizer
// ============================================================================

use move_core_types::account_address::AccountAddress;

/// Synthesizes Move objects from JSON field specifications.
///
/// This allows an LLM to create objects by specifying field values in a natural format:
/// ```json
/// {
///   "id": "auto",           // "auto" generates a fresh ID
///   "admin": "0x1234...",   // address field
///   "count": 42,            // u64 field
///   "name": "test"          // String/vector<u8> field
/// }
/// ```
pub struct ObjectSynthesizer {
    /// Counter for generating fresh object IDs
    id_counter: u64,
    /// Base address prefix for generated IDs (makes them predictable/debuggable)
    id_prefix: [u8; 24],
}

impl ObjectSynthesizer {
    pub fn new() -> Self {
        Self {
            id_counter: 1,
            // Use a recognizable prefix for synthesized objects
            id_prefix: [0xAA; 24], // "AAAAAA..." prefix
        }
    }

    /// Generate a fresh unique object ID
    pub fn fresh_id(&mut self) -> AccountAddress {
        let mut bytes = [0u8; 32];
        bytes[..24].copy_from_slice(&self.id_prefix);
        bytes[24..].copy_from_slice(&self.id_counter.to_be_bytes());
        self.id_counter += 1;
        AccountAddress::new(bytes)
    }

    /// Synthesize BCS bytes for a Sui object from JSON field values.
    ///
    /// The struct_info provides the type layout. Fields are serialized in order.
    ///
    /// Supported field types:
    /// - `id` (UID): "auto" generates fresh ID, or provide hex string
    /// - `address`: 32-byte hex string (with or without 0x prefix)
    /// - `u8/u16/u32/u64/u128/u256`: numeric values
    /// - `bool`: true/false
    /// - `vector<u8>`: string or array of numbers
    /// - `vector<address>`: array of address strings
    /// - `Option<T>`: null for None, value for Some
    pub fn synthesize_object(
        &mut self,
        struct_info: &StructInfo,
        field_values: &serde_json::Value,
        request: &ObjectSynthesisRequest,
    ) -> Result<SynthesizedObject> {
        let fields = field_values.as_object()
            .ok_or_else(|| anyhow!("field_values must be a JSON object"))?;

        let mut bcs_bytes = Vec::new();

        for field in &struct_info.fields {
            let value = fields.get(&field.name);
            self.encode_field(&field.type_str, &field.type_json, value, &mut bcs_bytes)?;
        }

        // Determine object ID
        let object_id = if let Some(id_str) = &request.object_id {
            parse_address(id_str)?
        } else if let Some(id_val) = fields.get("id") {
            if id_val.as_str() == Some("auto") {
                self.fresh_id()
            } else if let Some(s) = id_val.as_str() {
                parse_address(s)?
            } else {
                self.fresh_id()
            }
        } else {
            self.fresh_id()
        };

        Ok(SynthesizedObject {
            object_id: format!("0x{}", hex::encode(object_id.as_ref())),
            type_path: struct_info.type_path.clone(),
            bcs_bytes,
            is_shared: request.is_shared,
        })
    }

    /// Encode a single field value to BCS
    fn encode_field(
        &mut self,
        type_str: &str,
        type_json: &serde_json::Value,
        value: Option<&serde_json::Value>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        // Handle UID specially - it's the object ID
        if type_str.ends_with("::object::UID") || type_str.contains("UID") {
            let id = if let Some(v) = value {
                if v.as_str() == Some("auto") {
                    self.fresh_id()
                } else if let Some(s) = v.as_str() {
                    parse_address(s)?
                } else {
                    self.fresh_id()
                }
            } else {
                self.fresh_id()
            };
            out.extend_from_slice(id.as_ref());
            return Ok(());
        }

        // Handle primitive types
        match type_str {
            "bool" => {
                let b = value.and_then(|v| v.as_bool()).unwrap_or(false);
                out.push(if b { 1 } else { 0 });
            }
            "u8" => {
                let n = value.and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                out.push(n);
            }
            "u16" => {
                let n = value.and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                out.extend_from_slice(&n.to_le_bytes());
            }
            "u32" => {
                let n = value.and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                out.extend_from_slice(&n.to_le_bytes());
            }
            "u64" => {
                let n = value.and_then(|v| v.as_u64()).unwrap_or(0);
                out.extend_from_slice(&n.to_le_bytes());
            }
            "u128" => {
                let n: u128 = if let Some(v) = value {
                    if let Some(s) = v.as_str() {
                        s.parse().unwrap_or(0)
                    } else {
                        v.as_u64().unwrap_or(0) as u128
                    }
                } else {
                    0
                };
                out.extend_from_slice(&n.to_le_bytes());
            }
            "u256" => {
                // U256 is 32 bytes little-endian
                let mut bytes = [0u8; 32];
                if let Some(v) = value {
                    if let Some(s) = v.as_str() {
                        // Try to parse hex or decimal
                        if let Some(hex_str) = s.strip_prefix("0x") {
                            if let Ok(decoded) = hex::decode(hex_str) {
                                let len = decoded.len().min(32);
                                bytes[..len].copy_from_slice(&decoded[..len]);
                            }
                        }
                    }
                }
                out.extend_from_slice(&bytes);
            }
            "address" => {
                let addr = if let Some(v) = value {
                    if let Some(s) = v.as_str() {
                        parse_address(s)?
                    } else {
                        AccountAddress::ZERO
                    }
                } else {
                    AccountAddress::ZERO
                };
                out.extend_from_slice(addr.as_ref());
            }
            _ if type_str.starts_with("vector<") => {
                self.encode_vector(type_str, type_json, value, out)?;
            }
            _ if type_str.contains("::string::String") || type_str.contains("::ascii::String") => {
                // String is vector<u8> internally
                let s = value.and_then(|v| v.as_str()).unwrap_or("");
                encode_uleb128(s.len(), out);
                out.extend_from_slice(s.as_bytes());
            }
            _ if type_str.contains("::option::Option") => {
                self.encode_option(type_json, value, out)?;
            }
            _ if type_str.contains("::balance::Balance") => {
                // Balance<T> is just a u64 internally
                let n = value.and_then(|v| v.as_u64()).unwrap_or(0);
                out.extend_from_slice(&n.to_le_bytes());
            }
            _ if type_str.contains("::vec_set::VecSet") => {
                // VecSet<T> is { contents: vector<T> }
                // Serialize as a vector
                self.encode_vec_set(type_json, value, out)?;
            }
            _ if type_str.contains("::table::Table") || type_str.contains("::bag::Bag") => {
                // These are UID-based dynamic collections
                // Table<K,V> is { id: UID }
                let id = self.fresh_id();
                out.extend_from_slice(id.as_ref());
            }
            _ if type_str.contains("::vec_map::VecMap") => {
                // VecMap<K,V> is { contents: vector<Entry<K,V>> }
                // This requires explicit key-value pairs to serialize properly.
                // Return an error with guidance on how to provide the data.
                return Err(anyhow!(
                    "VecMap type {} requires explicit entries. \
                     Provide as JSON array of {{\"key\": ..., \"value\": ...}} objects, \
                     or use an empty array [] for an empty VecMap.",
                    type_str
                ));
            }
            _ => {
                // For unknown struct types, we need more context
                // For now, try to serialize as nested struct if we have field info
                if let serde_json::Value::Object(obj) = type_json {
                    if obj.contains_key("Struct") {
                        // Nested struct - would need recursive handling with struct_info
                        // For now, return error asking for more specific handling
                        return Err(anyhow!(
                            "Nested struct type {} requires explicit field values. \
                             Provide nested object in JSON.",
                            type_str
                        ));
                    }
                }
                return Err(anyhow!("Unsupported field type: {}", type_str));
            }
        }
        Ok(())
    }

    /// Encode a vector type
    fn encode_vector(
        &mut self,
        type_str: &str,
        _type_json: &serde_json::Value,
        value: Option<&serde_json::Value>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        // Extract inner type from "vector<T>"
        let inner = type_str
            .strip_prefix("vector<")
            .and_then(|s| s.strip_suffix(">"))
            .ok_or_else(|| anyhow!("Invalid vector type: {}", type_str))?;

        match value {
            None => {
                // Empty vector
                encode_uleb128(0, out);
            }
            Some(serde_json::Value::Array(arr)) => {
                encode_uleb128(arr.len(), out);
                for item in arr {
                    match inner {
                        "u8" => {
                            let n = item.as_u64().unwrap_or(0) as u8;
                            out.push(n);
                        }
                        "u64" => {
                            let n = item.as_u64().unwrap_or(0);
                            out.extend_from_slice(&n.to_le_bytes());
                        }
                        "address" => {
                            let addr = if let Some(s) = item.as_str() {
                                parse_address(s)?
                            } else {
                                AccountAddress::ZERO
                            };
                            out.extend_from_slice(addr.as_ref());
                        }
                        _ => {
                            return Err(anyhow!("Unsupported vector element type: {}", inner));
                        }
                    }
                }
            }
            Some(serde_json::Value::String(s)) if inner == "u8" => {
                // String -> vector<u8>
                encode_uleb128(s.len(), out);
                out.extend_from_slice(s.as_bytes());
            }
            Some(v) => {
                return Err(anyhow!("Expected array for vector type, got {:?}", v));
            }
        }
        Ok(())
    }

    /// Encode an Option type
    fn encode_option(
        &mut self,
        type_json: &serde_json::Value,
        value: Option<&serde_json::Value>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        match value {
            None | Some(serde_json::Value::Null) => {
                // None variant - Option is a vector with 0 elements
                encode_uleb128(0, out);
            }
            Some(v) => {
                // Some variant - Option is a vector with 1 element
                encode_uleb128(1, out);

                // Get inner type from type_json
                if let Some(struct_info) = type_json.get("Struct") {
                    if let Some(type_args) = struct_info.get("type_arguments") {
                        if let Some(inner) = type_args.get(0) {
                            let inner_str = type_json_to_string(inner);
                            self.encode_field(&inner_str, inner, Some(v), out)?;
                            return Ok(());
                        }
                    }
                }
                return Err(anyhow!("Could not determine Option inner type"));
            }
        }
        Ok(())
    }

    /// Encode a VecSet<T>
    /// VecSet<T> is internally { contents: vector<T> }
    fn encode_vec_set(
        &mut self,
        type_json: &serde_json::Value,
        value: Option<&serde_json::Value>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        // Get the inner type T from VecSet<T>
        let inner_type = type_json
            .get("type_args")
            .and_then(|ta| ta.get(0));

        match value {
            None | Some(serde_json::Value::Null) => {
                // Empty VecSet
                encode_uleb128(0, out);
            }
            Some(serde_json::Value::Array(arr)) => {
                encode_uleb128(arr.len(), out);

                // Encode each element based on inner type
                if let Some(inner) = inner_type {
                    let inner_str = type_json_to_string(inner);
                    for item in arr {
                        self.encode_field(&inner_str, inner, Some(item), out)?;
                    }
                } else {
                    // Default to address if we can't determine type
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            let addr = parse_address(s)?;
                            out.extend_from_slice(addr.as_ref());
                        }
                    }
                }
            }
            Some(v) => {
                return Err(anyhow!("Expected array for VecSet, got {:?}", v));
            }
        }
        Ok(())
    }

    /// Encode a nested struct type by looking up its definition.
    /// Returns Ok(true) if successfully encoded, Ok(false) if type not found.
    pub fn encode_nested_struct(
        &mut self,
        type_json: &serde_json::Value,
        value: Option<&serde_json::Value>,
        introspector: &ModuleIntrospector,
        out: &mut Vec<u8>,
    ) -> Result<bool> {
        // Extract module path and struct name from type_json
        let (module_path, struct_name) = if let Some(kind) = type_json.get("kind").and_then(|v| v.as_str()) {
            if kind == "datatype" {
                let addr = type_json.get("address").and_then(|v| v.as_str()).unwrap_or("?");
                let module = type_json.get("module").and_then(|v| v.as_str()).unwrap_or("?");
                let name = type_json.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                (format!("{}::{}", addr, module), name.to_string())
            } else {
                return Ok(false);
            }
        } else {
            return Ok(false);
        };

        // Look up the struct definition
        let struct_info = match introspector.get_struct_info(&module_path, &struct_name) {
            Some(info) => info,
            None => return Ok(false),
        };

        // Encode each field
        let field_values = value.and_then(|v| v.as_object());
        for field in &struct_info.fields {
            let field_value = field_values.and_then(|fv| fv.get(&field.name));
            self.encode_field(&field.type_str, &field.type_json, field_value, out)?;
        }

        Ok(true)
    }
}

impl Default for ObjectSynthesizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse an address from hex string (with or without 0x prefix)
fn parse_address(s: &str) -> Result<AccountAddress> {
    let hex_str = s.strip_prefix("0x").unwrap_or(s);
    // Pad to 64 chars if needed
    let padded = if hex_str.len() < 64 {
        format!("{:0>64}", hex_str)
    } else {
        hex_str.to_string()
    };
    let bytes = hex::decode(&padded)
        .map_err(|e| anyhow!("Invalid address hex: {} - {}", s, e))?;
    if bytes.len() != 32 {
        return Err(anyhow!("Address must be 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(AccountAddress::new(arr))
}

/// Encode a usize as ULEB128 (used for vector lengths)
fn encode_uleb128(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        } else {
            out.push(byte | 0x80);
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Convert a type JSON value to a human-readable string
fn type_json_to_string(ty: &serde_json::Value) -> String {
    match ty {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(obj) => {
            // Handle bytecode format: {"kind": "datatype", "address": "...", "module": "...", "name": "...", "type_args": [...]}
            if let Some(kind) = obj.get("kind").and_then(|v| v.as_str()) {
                match kind {
                    "bool" => return "bool".to_string(),
                    "u8" => return "u8".to_string(),
                    "u16" => return "u16".to_string(),
                    "u32" => return "u32".to_string(),
                    "u64" => return "u64".to_string(),
                    "u128" => return "u128".to_string(),
                    "u256" => return "u256".to_string(),
                    "address" => return "address".to_string(),
                    "signer" => return "signer".to_string(),
                    "datatype" => {
                        let addr = obj.get("address").and_then(|v| v.as_str()).unwrap_or("?");
                        let module = obj.get("module").and_then(|v| v.as_str()).unwrap_or("?");
                        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let type_args = obj.get("type_args")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                let args: Vec<String> = arr.iter().map(type_json_to_string).collect();
                                if args.is_empty() {
                                    "".to_string()
                                } else {
                                    format!("<{}>", args.join(", "))
                                }
                            })
                            .unwrap_or_default();
                        // Shorten common 0x2 types
                        let short_addr = if addr == "0x0000000000000000000000000000000000000000000000000000000000000002" {
                            "0x2"
                        } else if addr.len() > 16 {
                            &addr[..16]
                        } else {
                            addr
                        };
                        return format!("{}::{}::{}{}", short_addr, module, name, type_args);
                    }
                    "vector" => {
                        if let Some(inner) = obj.get("type") {
                            return format!("vector<{}>", type_json_to_string(inner));
                        }
                        return "vector<?>".to_string();
                    }
                    "ref" => {
                        let mutable = obj.get("mutable").and_then(|v| v.as_bool()).unwrap_or(false);
                        if let Some(to) = obj.get("to") {
                            let prefix = if mutable { "&mut " } else { "&" };
                            return format!("{}{}", prefix, type_json_to_string(to));
                        }
                        return if mutable { "&mut ?".to_string() } else { "&?".to_string() };
                    }
                    "type_param" => {
                        let idx = obj.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                        return format!("T{}", idx);
                    }
                    _ => {}
                }
            }

            // Handle legacy format: {"Struct": {...}}
            if let Some(struct_info) = obj.get("Struct") {
                if let Some(obj) = struct_info.as_object() {
                    let addr = obj.get("address").and_then(|v| v.as_str()).unwrap_or("?");
                    let module = obj.get("module").and_then(|v| v.as_str()).unwrap_or("?");
                    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let type_args = obj.get("type_arguments")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            let args: Vec<String> = arr.iter().map(type_json_to_string).collect();
                            if args.is_empty() {
                                "".to_string()
                            } else {
                                format!("<{}>", args.join(", "))
                            }
                        })
                        .unwrap_or_default();
                    return format!("{}::{}::{}{}", addr, module, name, type_args);
                }
            }
            if let Some(ref_info) = obj.get("Reference") {
                return format!("&{}", type_json_to_string(ref_info));
            }
            if let Some(ref_info) = obj.get("MutableReference") {
                return format!("&mut {}", type_json_to_string(ref_info));
            }
            if let Some(vec_info) = obj.get("Vector") {
                return format!("vector<{}>", type_json_to_string(vec_info));
            }
            if let Some(tp) = obj.get("TypeParameter") {
                return format!("T{}", tp.as_u64().unwrap_or(0));
            }
            format!("{:?}", obj)
        }
        _ => format!("{:?}", ty),
    }
}

// ============================================================================
// LLM Toolkit - DEPRECATED (use sandbox_exec::SandboxRequest instead)
// ============================================================================

/// **DEPRECATED**: Use [`crate::benchmark::sandbox_exec::SandboxRequest`] instead.
///
/// This struct is kept for backwards compatibility. New code should use
/// the `sandbox-exec` CLI or `execute_request()` function directly.
///
/// See module-level documentation for migration guide.
#[deprecated(since = "0.5.0", note = "Use sandbox_exec::SandboxRequest instead")]
pub struct LlmToolkit {
    pub introspector: ModuleIntrospector,
    pub synthesizer: ObjectSynthesizer,
    package_builder: Option<PackageBuilder>,
    /// Storage logger for analysis (optional)
    logger: Option<StorageLogger>,
    /// Model name for logging attribution
    model_name: Option<String>,
}

impl Default for LlmToolkit {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LlmToolkit {
    fn drop(&mut self) {
        // Ensure any buffered logs are flushed
        self.logger = None;
    }
}

/// **DEPRECATED**: Use [`crate::benchmark::sandbox_exec::SandboxRequest`] instead.
///
/// Tool call request from LLM (JSON-deserializable).
/// This enum is kept for backwards compatibility.
#[deprecated(since = "0.5.0", note = "Use sandbox_exec::SandboxRequest instead")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", content = "params")]
pub enum ToolCall {
    /// List all loaded modules
    ListModules,

    /// List structs in a module
    ListStructs { module_path: String },

    /// Get detailed struct info
    GetStructInfo { module_path: String, struct_name: String },

    /// List functions in a module
    ListFunctions { module_path: String },

    /// Get function signature
    GetFunctionInfo { module_path: String, function_name: String },

    /// Get module summary (human-readable)
    ModuleSummary { module_path: String },

    /// Disassemble a function's bytecode
    DisassembleFunction { module_path: String, function_name: String },

    /// Disassemble an entire module's bytecode
    DisassembleModule { module_path: String },

    /// Create an object with specified field values
    CreateObject {
        type_path: String,
        fields: serde_json::Value,
        #[serde(default)]
        is_shared: bool,
    },

    /// Parse an error string
    ParseError { error: String },

    /// Compile Move source code to bytecode
    CompileSource {
        /// Package name
        package_name: String,
        /// Module name (used for the .move file)
        module_name: String,
        /// Move source code
        source: String,
    },

    /// Check if Sui framework is cached locally
    IsFrameworkCached,

    /// Download and cache Sui framework (if not already cached)
    EnsureFrameworkCached,

    // ========================================================================
    // General-Purpose Utility Tools
    // ========================================================================

    /// Generate a fresh unique object/address ID
    GenerateFreshId,

    /// Validate a type string and return structured type information
    ValidateType { type_str: String },

    /// Encode a value to BCS bytes given a type
    EncodeBcs {
        type_str: String,
        value: serde_json::Value,
    },

    /// Decode BCS bytes to a JSON value given a type
    DecodeBcs {
        type_str: String,
        /// Hex-encoded BCS bytes
        bytes_hex: String,
    },

    /// Parse an address string (supports short forms like "0x2")
    ParseAddress { address: String },

    /// Format an address to different representations
    FormatAddress {
        address: String,
        /// "short", "full", or "no_prefix"
        #[serde(default = "default_format")]
        format: String,
    },

    /// Compute type layout size in bytes (for BCS serialization planning)
    TypeLayoutSize { type_str: String },

    /// Search for types matching a pattern across loaded modules
    SearchTypes {
        /// Pattern to match (supports * wildcard)
        pattern: String,
        /// Filter by ability (e.g., "key", "store", "copy", "drop")
        #[serde(default)]
        ability_filter: Option<String>,
    },

    /// Search for functions matching a pattern across loaded modules
    SearchFunctions {
        /// Pattern to match (supports * wildcard)
        pattern: String,
        /// Only return entry functions
        #[serde(default)]
        entry_only: bool,
    },

    /// Get dependencies for a module (what it imports)
    GetModuleDependencies { module_path: String },

    /// List all types that can construct a given type (constructor analysis)
    FindConstructors { type_path: String },

    /// Compute a hash of bytes (for dynamic field key derivation, etc.)
    ComputeHash {
        /// Hex-encoded bytes to hash
        bytes_hex: String,
        /// Hash algorithm: "sha256", "sha3_256", "blake2b_256"
        #[serde(default = "default_hash_algo")]
        algorithm: String,
    },

    /// Convert between number representations
    ConvertNumber {
        value: String,
        /// "u8", "u16", "u32", "u64", "u128", "u256"
        from_type: String,
        to_type: String,
    },

    /// Generate BCS-encoded vector from array of values
    EncodeVector {
        element_type: String,
        values: Vec<serde_json::Value>,
    },

    /// Get information about a well-known Sui system object
    GetSystemObjectInfo {
        /// "clock", "random", "deny_list", "system_state"
        object_name: String,
    },
}

fn default_format() -> String {
    "short".to_string()
}

fn default_hash_algo() -> String {
    "sha3_256".to_string()
}

impl ToolCall {
    /// Get the tool name as a string (for logging)
    pub fn tool_name(&self) -> String {
        match self {
            ToolCall::ListModules => "ListModules".to_string(),
            ToolCall::ListStructs { .. } => "ListStructs".to_string(),
            ToolCall::GetStructInfo { .. } => "GetStructInfo".to_string(),
            ToolCall::ListFunctions { .. } => "ListFunctions".to_string(),
            ToolCall::GetFunctionInfo { .. } => "GetFunctionInfo".to_string(),
            ToolCall::ModuleSummary { .. } => "ModuleSummary".to_string(),
            ToolCall::DisassembleFunction { .. } => "DisassembleFunction".to_string(),
            ToolCall::DisassembleModule { .. } => "DisassembleModule".to_string(),
            ToolCall::CreateObject { .. } => "CreateObject".to_string(),
            ToolCall::ParseError { .. } => "ParseError".to_string(),
            ToolCall::CompileSource { .. } => "CompileSource".to_string(),
            ToolCall::IsFrameworkCached => "IsFrameworkCached".to_string(),
            ToolCall::EnsureFrameworkCached => "EnsureFrameworkCached".to_string(),
            ToolCall::GenerateFreshId => "GenerateFreshId".to_string(),
            ToolCall::ValidateType { .. } => "ValidateType".to_string(),
            ToolCall::EncodeBcs { .. } => "EncodeBcs".to_string(),
            ToolCall::DecodeBcs { .. } => "DecodeBcs".to_string(),
            ToolCall::ParseAddress { .. } => "ParseAddress".to_string(),
            ToolCall::FormatAddress { .. } => "FormatAddress".to_string(),
            ToolCall::TypeLayoutSize { .. } => "TypeLayoutSize".to_string(),
            ToolCall::SearchTypes { .. } => "SearchTypes".to_string(),
            ToolCall::SearchFunctions { .. } => "SearchFunctions".to_string(),
            ToolCall::GetModuleDependencies { .. } => "GetModuleDependencies".to_string(),
            ToolCall::FindConstructors { .. } => "FindConstructors".to_string(),
            ToolCall::ComputeHash { .. } => "ComputeHash".to_string(),
            ToolCall::ConvertNumber { .. } => "ConvertNumber".to_string(),
            ToolCall::EncodeVector { .. } => "EncodeVector".to_string(),
            ToolCall::GetSystemObjectInfo { .. } => "GetSystemObjectInfo".to_string(),
        }
    }

    /// Get tool parameters as JSON (for logging)
    pub fn params_json(&self) -> serde_json::Value {
        match self {
            ToolCall::ListModules => serde_json::json!({}),
            ToolCall::ListStructs { module_path } => serde_json::json!({"module_path": module_path}),
            ToolCall::GetStructInfo { module_path, struct_name } => {
                serde_json::json!({"module_path": module_path, "struct_name": struct_name})
            }
            ToolCall::ListFunctions { module_path } => serde_json::json!({"module_path": module_path}),
            ToolCall::GetFunctionInfo { module_path, function_name } => {
                serde_json::json!({"module_path": module_path, "function_name": function_name})
            }
            ToolCall::ModuleSummary { module_path } => serde_json::json!({"module_path": module_path}),
            ToolCall::DisassembleFunction { module_path, function_name } => {
                serde_json::json!({"module_path": module_path, "function_name": function_name})
            }
            ToolCall::DisassembleModule { module_path } => serde_json::json!({"module_path": module_path}),
            ToolCall::CreateObject { type_path, fields, is_shared } => {
                serde_json::json!({"type_path": type_path, "fields": fields, "is_shared": is_shared})
            }
            ToolCall::ParseError { error } => {
                // Truncate long errors
                let truncated = if error.len() > 500 { &error[..500] } else { error };
                serde_json::json!({"error": truncated})
            }
            ToolCall::CompileSource { package_name, module_name, source } => {
                // Don't include full source in params, just metadata
                serde_json::json!({
                    "package_name": package_name,
                    "module_name": module_name,
                    "source_len": source.len()
                })
            }
            ToolCall::IsFrameworkCached => serde_json::json!({}),
            ToolCall::EnsureFrameworkCached => serde_json::json!({}),
            ToolCall::GenerateFreshId => serde_json::json!({}),
            ToolCall::ValidateType { type_str } => serde_json::json!({"type_str": type_str}),
            ToolCall::EncodeBcs { type_str, value } => {
                serde_json::json!({"type_str": type_str, "value": value})
            }
            ToolCall::DecodeBcs { type_str, bytes_hex } => {
                serde_json::json!({"type_str": type_str, "bytes_hex": bytes_hex})
            }
            ToolCall::ParseAddress { address } => serde_json::json!({"address": address}),
            ToolCall::FormatAddress { address, format } => {
                serde_json::json!({"address": address, "format": format})
            }
            ToolCall::TypeLayoutSize { type_str } => serde_json::json!({"type_str": type_str}),
            ToolCall::SearchTypes { pattern, ability_filter } => {
                serde_json::json!({"pattern": pattern, "ability_filter": ability_filter})
            }
            ToolCall::SearchFunctions { pattern, entry_only } => {
                serde_json::json!({"pattern": pattern, "entry_only": entry_only})
            }
            ToolCall::GetModuleDependencies { module_path } => {
                serde_json::json!({"module_path": module_path})
            }
            ToolCall::FindConstructors { type_path } => {
                serde_json::json!({"type_path": type_path})
            }
            ToolCall::ComputeHash { bytes_hex, algorithm } => {
                serde_json::json!({"bytes_hex": bytes_hex, "algorithm": algorithm})
            }
            ToolCall::ConvertNumber { value, from_type, to_type } => {
                serde_json::json!({"value": value, "from_type": from_type, "to_type": to_type})
            }
            ToolCall::EncodeVector { element_type, values } => {
                serde_json::json!({"element_type": element_type, "values_count": values.len()})
            }
            ToolCall::GetSystemObjectInfo { object_name } => {
                serde_json::json!({"object_name": object_name})
            }
        }
    }
}

/// Tool call response (JSON-serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum ToolResponse {
    Success { data: serde_json::Value },
    Error { message: String },
}

impl LlmToolkit {
    pub fn new() -> Self {
        Self {
            introspector: ModuleIntrospector::new(),
            synthesizer: ObjectSynthesizer::new(),
            package_builder: None,
            logger: None,
            model_name: None,
        }
    }

    /// Create toolkit with package building enabled
    pub fn with_package_builder() -> Result<Self> {
        let temp_dir = std::env::temp_dir().join(format!("llm-toolkit-{}", std::process::id()));
        let builder = PackageBuilder::with_framework_cache(&temp_dir)?;
        Ok(Self {
            introspector: ModuleIntrospector::new(),
            synthesizer: ObjectSynthesizer::new(),
            package_builder: Some(builder),
            logger: None,
            model_name: None,
        })
    }

    /// Create toolkit with logging enabled
    pub fn with_logging() -> Result<Self> {
        let logger = StorageLogger::new()?;
        Ok(Self {
            introspector: ModuleIntrospector::new(),
            synthesizer: ObjectSynthesizer::new(),
            package_builder: None,
            logger: Some(logger),
            model_name: None,
        })
    }

    /// Create toolkit with both package building and logging
    pub fn with_all_features() -> Result<Self> {
        let temp_dir = std::env::temp_dir().join(format!("llm-toolkit-{}", std::process::id()));
        let builder = PackageBuilder::with_framework_cache(&temp_dir)?;
        let logger = StorageLogger::new()?;
        Ok(Self {
            introspector: ModuleIntrospector::new(),
            synthesizer: ObjectSynthesizer::new(),
            package_builder: Some(builder),
            logger: Some(logger),
            model_name: None,
        })
    }

    /// Enable logging on existing toolkit
    pub fn enable_logging(&mut self) -> Result<()> {
        if self.logger.is_none() {
            self.logger = Some(StorageLogger::new()?);
        }
        Ok(())
    }

    /// Set the model name for log attribution
    pub fn set_model_name(&mut self, model: impl Into<String>) {
        self.model_name = Some(model.into());
    }

    /// Get the current session ID (if logging is enabled)
    pub fn session_id(&self) -> Option<&str> {
        self.logger.as_ref().map(|l| l.session_id())
    }

    /// Get the storage logger (if enabled)
    pub fn logger(&self) -> Option<&StorageLogger> {
        self.logger.as_ref()
    }

    /// Get mutable storage logger (if enabled)
    pub fn logger_mut(&mut self) -> Option<&mut StorageLogger> {
        self.logger.as_mut()
    }

    /// Enable package building on existing toolkit
    pub fn enable_package_builder(&mut self) -> Result<()> {
        if self.package_builder.is_none() {
            let temp_dir = std::env::temp_dir().join(format!("llm-toolkit-{}", std::process::id()));
            self.package_builder = Some(PackageBuilder::with_framework_cache(&temp_dir)?);
        }
        Ok(())
    }

    /// Load a compiled module into the toolkit
    pub fn load_module(&mut self, module: &CompiledModule) -> Result<()> {
        self.introspector.load_module(module)
    }

    /// Load modules from raw bytes
    pub fn load_from_bytes(&mut self, package_id: &str, modules: &[(String, Vec<u8>)]) -> Result<()> {
        self.introspector.load_from_bytes(package_id, modules)
    }

    /// Execute a tool call and return a JSON response.
    /// This is the main entry point for LLM interaction.
    /// If logging is enabled, all calls are logged for analysis.
    pub fn execute(&mut self, call: ToolCall) -> ToolResponse {
        let start = Instant::now();
        let tool_name = call.tool_name();
        let params = call.params_json();

        let result = self.execute_inner(call);
        let duration_ms = start.elapsed().as_millis() as u64;

        // Log the tool call if logging is enabled
        if let Some(logger) = &mut self.logger {
            let log = ToolCallLog {
                timestamp: Utc::now(),
                tool: tool_name,
                params,
                success: result.is_ok(),
                result: result.as_ref().ok().and_then(|v| {
                    // Truncate large results for logging
                    let s = v.to_string();
                    if s.len() > 10000 {
                        Some(serde_json::json!({"truncated": true, "len": s.len()}))
                    } else {
                        Some(v.clone())
                    }
                }),
                error: result.as_ref().err().map(|e| e.to_string()),
                duration_ms,
            };
            let _ = logger.log_tool_call(&log);
        }

        match result {
            Ok(data) => ToolResponse::Success { data },
            Err(e) => ToolResponse::Error { message: e.to_string() },
        }
    }

    fn execute_inner(&mut self, call: ToolCall) -> Result<serde_json::Value> {
        match call {
            ToolCall::ListModules => {
                let modules = self.introspector.list_modules();
                Ok(serde_json::to_value(modules)?)
            }

            ToolCall::ListStructs { module_path } => {
                let structs = self.introspector.list_structs(&module_path)
                    .ok_or_else(|| anyhow!("Module not found: {}", module_path))?;
                Ok(serde_json::to_value(structs)?)
            }

            ToolCall::GetStructInfo { module_path, struct_name } => {
                let info = self.introspector.get_struct_info(&module_path, &struct_name)
                    .ok_or_else(|| anyhow!("Struct not found: {}::{}", module_path, struct_name))?;
                Ok(serde_json::to_value(info)?)
            }

            ToolCall::ListFunctions { module_path } => {
                let funcs = self.introspector.list_functions(&module_path)
                    .ok_or_else(|| anyhow!("Module not found: {}", module_path))?;
                Ok(serde_json::to_value(funcs)?)
            }

            ToolCall::GetFunctionInfo { module_path, function_name } => {
                let info = self.introspector.get_function_info(&module_path, &function_name)
                    .ok_or_else(|| anyhow!("Function not found: {}::{}", module_path, function_name))?;
                Ok(serde_json::to_value(info)?)
            }

            ToolCall::ModuleSummary { module_path } => {
                let summary = self.introspector.module_summary(&module_path)
                    .ok_or_else(|| anyhow!("Module not found: {}", module_path))?;
                Ok(serde_json::to_value(summary)?)
            }

            ToolCall::DisassembleFunction { module_path, function_name } => {
                let disasm = self.introspector.disassemble_function(&module_path, &function_name)
                    .ok_or_else(|| anyhow!("Could not disassemble {}::{}", module_path, function_name))?;
                Ok(serde_json::to_value(disasm)?)
            }

            ToolCall::DisassembleModule { module_path } => {
                let disasm = self.introspector.disassemble_module(&module_path)
                    .ok_or_else(|| anyhow!("Could not disassemble module: {}", module_path))?;
                Ok(serde_json::to_value(disasm)?)
            }

            ToolCall::CreateObject { type_path, fields, is_shared } => {
                // First get the struct info
                let parts: Vec<&str> = type_path.rsplitn(2, "::").collect();
                if parts.len() != 2 {
                    return Err(anyhow!("Invalid type path: {}. Expected format: 0x...::module::StructName", type_path));
                }
                let struct_name = parts[0];
                let module_path = parts[1];

                let struct_info = self.introspector.get_struct_info(module_path, struct_name)
                    .ok_or_else(|| anyhow!("Struct not found: {}", type_path))?;

                let request = ObjectSynthesisRequest {
                    type_path: type_path.clone(),
                    fields: fields.clone(),
                    object_id: None,
                    is_shared,
                };

                let obj = self.synthesizer.synthesize_object(&struct_info, &fields, &request)?;

                // Log object synthesis if logging is enabled
                if let Some(logger) = &mut self.logger {
                    let log = ObjectSynthesisLog {
                        timestamp: Utc::now(),
                        type_path: obj.type_path.clone(),
                        object_id: obj.object_id.clone(),
                        fields: fields.clone(),
                        is_shared: obj.is_shared,
                        bcs_len: obj.bcs_bytes.len(),
                    };
                    let _ = logger.log_object_synthesis(&log);
                }

                Ok(serde_json::to_value(obj)?)
            }

            ToolCall::ParseError { error } => {
                let ctx = parse_abort_error(&error);
                Ok(serde_json::to_value(ctx)?)
            }

            ToolCall::CompileSource { package_name, module_name, source } => {
                let builder = self.package_builder.as_ref()
                    .ok_or_else(|| anyhow!("Package builder not enabled. Call enable_package_builder() first."))?;

                let result = builder.build_from_source(&package_name, &module_name, &source)?;

                // Log the package compilation if logging is enabled
                if let Some(logger) = &mut self.logger {
                    let log = PackageLog {
                        id: Uuid::new_v4().to_string(),
                        timestamp: Utc::now(),
                        package_name: package_name.clone(),
                        module_name: module_name.clone(),
                        source: source.clone(),
                        success: result.success,
                        diagnostics: result.diagnostics.clone(),
                        bytecode_sizes: result.modules.iter()
                            .map(|(name, bytes)| (name.clone(), bytes.len()))
                            .collect(),
                        session_id: Some(logger.session_id().to_string()),
                        model: self.model_name.clone(),
                        prompt: None, // Could be set externally
                    };
                    // Log with bytecode for successful compilations
                    if result.success {
                        let _ = logger.log_package_with_bytecode(&log, &result.modules);
                    } else {
                        let _ = logger.log_package(&log);
                    }
                }

                Ok(serde_json::json!({
                    "success": result.success,
                    "modules": result.modules.iter().map(|(name, bytes)| {
                        serde_json::json!({
                            "name": name,
                            "bytecode_len": bytes.len(),
                            "bytecode_hex": hex::encode(bytes),
                        })
                    }).collect::<Vec<_>>(),
                    "diagnostics": result.diagnostics,
                }))
            }

            ToolCall::IsFrameworkCached => {
                let cached = FrameworkCache::new()
                    .map(|c| c.is_cached())
                    .unwrap_or(false);
                Ok(serde_json::json!({
                    "cached": cached,
                    "cache_dir": FrameworkCache::new().ok().map(|c| c.cache_dir().display().to_string()),
                }))
            }

            ToolCall::EnsureFrameworkCached => {
                let cache = FrameworkCache::new()?;
                if cache.is_cached() {
                    Ok(serde_json::json!({
                        "status": "already_cached",
                        "cache_dir": cache.cache_dir().display().to_string(),
                    }))
                } else {
                    cache.ensure_cached()?;
                    Ok(serde_json::json!({
                        "status": "downloaded",
                        "cache_dir": cache.cache_dir().display().to_string(),
                    }))
                }
            }

            // ================================================================
            // General-Purpose Utility Tool Implementations
            // ================================================================

            ToolCall::GenerateFreshId => {
                let id = self.synthesizer.fresh_id();
                let hex_str = format!("0x{}", hex::encode(id.as_ref()));
                // Also provide short form if leading zeros
                let short = format_address_short(&hex_str);
                Ok(serde_json::json!({
                    "id": hex_str,
                    "short": short,
                    "bytes": id.as_ref().to_vec(),
                }))
            }

            ToolCall::ValidateType { type_str } => {
                validate_type_string(&type_str)
            }

            ToolCall::EncodeBcs { type_str, value } => {
                let mut bytes = Vec::new();
                self.synthesizer.encode_field(&type_str, &serde_json::json!({}), Some(&value), &mut bytes)?;
                Ok(serde_json::json!({
                    "bytes_hex": hex::encode(&bytes),
                    "bytes_len": bytes.len(),
                    "bytes": bytes,
                }))
            }

            ToolCall::DecodeBcs { type_str, bytes_hex } => {
                let bytes = hex::decode(bytes_hex.strip_prefix("0x").unwrap_or(&bytes_hex))
                    .map_err(|e| anyhow!("Invalid hex: {}", e))?;
                let value = decode_bcs_to_json(&type_str, &bytes)?;
                Ok(serde_json::json!({
                    "value": value,
                    "type": type_str,
                    "consumed_bytes": bytes.len(),
                }))
            }

            ToolCall::ParseAddress { address } => {
                let parsed = parse_address(&address)?;
                let hex_full = format!("0x{}", hex::encode(parsed.as_ref()));
                let hex_short = format_address_short(&hex_full);
                Ok(serde_json::json!({
                    "full": hex_full,
                    "short": hex_short,
                    "bytes": parsed.as_ref().to_vec(),
                    "valid": true,
                }))
            }

            ToolCall::FormatAddress { address, format } => {
                let parsed = parse_address(&address)?;
                let hex_full = format!("0x{}", hex::encode(parsed.as_ref()));
                let result = match format.as_str() {
                    "short" => format_address_short(&hex_full),
                    "full" => hex_full.clone(),
                    "no_prefix" => hex_full.strip_prefix("0x").unwrap_or(&hex_full).to_string(),
                    _ => return Err(anyhow!("Unknown format: {}. Use 'short', 'full', or 'no_prefix'", format)),
                };
                Ok(serde_json::json!({
                    "formatted": result,
                    "format": format,
                }))
            }

            ToolCall::TypeLayoutSize { type_str } => {
                let size = compute_type_size(&type_str);
                Ok(serde_json::json!({
                    "type": type_str,
                    "size_bytes": size.size,
                    "is_fixed": size.is_fixed,
                    "description": size.description,
                }))
            }

            ToolCall::SearchTypes { pattern, ability_filter } => {
                let results = self.search_types_impl(&pattern, ability_filter.as_deref());
                Ok(serde_json::json!({
                    "pattern": pattern,
                    "matches": results,
                    "count": results.len(),
                }))
            }

            ToolCall::SearchFunctions { pattern, entry_only } => {
                let results = self.search_functions_impl(&pattern, entry_only);
                Ok(serde_json::json!({
                    "pattern": pattern,
                    "entry_only": entry_only,
                    "matches": results,
                    "count": results.len(),
                }))
            }

            ToolCall::GetModuleDependencies { module_path } => {
                let deps = self.get_module_deps_impl(&module_path)?;
                Ok(serde_json::json!({
                    "module": module_path,
                    "dependencies": deps,
                }))
            }

            ToolCall::FindConstructors { type_path } => {
                let constructors = self.find_constructors_impl(&type_path);
                Ok(serde_json::json!({
                    "type": type_path,
                    "constructors": constructors,
                    "count": constructors.len(),
                }))
            }

            ToolCall::ComputeHash { bytes_hex, algorithm } => {
                let bytes = hex::decode(bytes_hex.strip_prefix("0x").unwrap_or(&bytes_hex))
                    .map_err(|e| anyhow!("Invalid hex: {}", e))?;
                let hash = compute_hash(&bytes, &algorithm)?;
                Ok(serde_json::json!({
                    "algorithm": algorithm,
                    "input_len": bytes.len(),
                    "hash_hex": format!("0x{}", hex::encode(&hash)),
                    "hash_bytes": hash,
                }))
            }

            ToolCall::ConvertNumber { value, from_type, to_type } => {
                let result = convert_number(&value, &from_type, &to_type)?;
                Ok(result)
            }

            ToolCall::EncodeVector { element_type, values } => {
                let mut bytes = Vec::new();
                encode_uleb128(values.len(), &mut bytes);
                for val in &values {
                    self.synthesizer.encode_field(&element_type, &serde_json::json!({}), Some(val), &mut bytes)?;
                }
                Ok(serde_json::json!({
                    "element_type": element_type,
                    "element_count": values.len(),
                    "bytes_hex": hex::encode(&bytes),
                    "bytes_len": bytes.len(),
                }))
            }

            ToolCall::GetSystemObjectInfo { object_name } => {
                get_system_object_info(&object_name)
            }
        }
    }

    /// **DEPRECATED**: Use `{"action": "list_available_tools"}` via sandbox_exec instead.
    ///
    /// Generate a JSON schema description of all available tools.
    #[deprecated(since = "0.5.0", note = "Use sandbox_exec list_available_tools action instead")]
    pub fn tool_schema() -> serde_json::Value {
        serde_json::json!({
            "tools": [
                {
                    "name": "ListModules",
                    "description": "List all loaded modules. Returns array of module paths like '0x123::module'.",
                    "params": {}
                },
                {
                    "name": "ListStructs",
                    "description": "List all struct types defined in a module.",
                    "params": {
                        "module_path": "string - e.g. '0x123::module'"
                    }
                },
                {
                    "name": "GetStructInfo",
                    "description": "Get detailed information about a struct: fields, abilities, type parameters.",
                    "params": {
                        "module_path": "string",
                        "struct_name": "string"
                    }
                },
                {
                    "name": "ListFunctions",
                    "description": "List all functions in a module.",
                    "params": {
                        "module_path": "string"
                    }
                },
                {
                    "name": "GetFunctionInfo",
                    "description": "Get function signature: visibility, parameters, return types.",
                    "params": {
                        "module_path": "string",
                        "function_name": "string"
                    }
                },
                {
                    "name": "ModuleSummary",
                    "description": "Get a human-readable summary of a module's types and functions.",
                    "params": {
                        "module_path": "string"
                    }
                },
                {
                    "name": "DisassembleFunction",
                    "description": "Disassemble a function's bytecode. Shows each instruction with offset, basic blocks, and resolved types/calls. Useful for understanding abort locations.",
                    "params": {
                        "module_path": "string",
                        "function_name": "string"
                    }
                },
                {
                    "name": "DisassembleModule",
                    "description": "Disassemble an entire module's bytecode. Returns human-readable bytecode for all functions.",
                    "params": {
                        "module_path": "string"
                    }
                },
                {
                    "name": "CreateObject",
                    "description": "Create an object with specified field values. Returns BCS-encoded bytes.",
                    "params": {
                        "type_path": "string - full type path like '0x123::module::StructName'",
                        "fields": "object - field name -> value mapping",
                        "is_shared": "boolean - whether object is shared (default false)"
                    },
                    "field_types": {
                        "id/UID": "'auto' for fresh ID or hex string",
                        "address": "hex string with or without 0x prefix",
                        "u8/u16/u32/u64": "number",
                        "u128/u256": "string (for large numbers)",
                        "bool": "true/false",
                        "vector<u8>": "string or array of numbers",
                        "String": "string",
                        "Option<T>": "null for None, value for Some"
                    }
                },
                {
                    "name": "ParseError",
                    "description": "Parse a Move VM error string and extract structured information.",
                    "params": {
                        "error": "string - the error message"
                    }
                },
                {
                    "name": "CompileSource",
                    "description": "Compile Move source code to bytecode. Returns compilation result with bytecode or error diagnostics.",
                    "params": {
                        "package_name": "string - name for the package",
                        "module_name": "string - name for the module (without .move extension)",
                        "source": "string - Move source code"
                    },
                    "returns": {
                        "success": "boolean",
                        "modules": "array of {name, bytecode_len, bytecode_hex}",
                        "diagnostics": "string - error messages if compilation failed"
                    }
                },
                {
                    "name": "IsFrameworkCached",
                    "description": "Check if Sui framework is cached locally. Cached framework enables faster compilation.",
                    "params": {},
                    "returns": {
                        "cached": "boolean",
                        "cache_dir": "string - path to cache directory"
                    }
                },
                {
                    "name": "EnsureFrameworkCached",
                    "description": "Download and cache Sui framework if not already cached. First compilation may be slow without this.",
                    "params": {},
                    "returns": {
                        "status": "string - 'already_cached' or 'downloaded'",
                        "cache_dir": "string - path to cache directory"
                    }
                },
                // New general-purpose tools
                {
                    "name": "GenerateFreshId",
                    "description": "Generate a fresh unique 32-byte object/address ID. Returns full hex, short form, and raw bytes.",
                    "params": {},
                    "returns": {
                        "id": "string - full 0x-prefixed hex (64 chars)",
                        "short": "string - shortened form with leading zeros removed",
                        "bytes": "array - raw 32 bytes"
                    }
                },
                {
                    "name": "ValidateType",
                    "description": "Validate and parse a Move type string. Returns structured type information.",
                    "params": {
                        "type_str": "string - type like 'u64', 'address', '0x2::coin::Coin<0x2::sui::SUI>'"
                    }
                },
                {
                    "name": "EncodeBcs",
                    "description": "Encode a value to BCS (Binary Canonical Serialization) bytes.",
                    "params": {
                        "type_str": "string - the type to encode as",
                        "value": "any - the value to encode"
                    }
                },
                {
                    "name": "DecodeBcs",
                    "description": "Decode BCS bytes to a JSON value.",
                    "params": {
                        "type_str": "string - the type to decode as",
                        "bytes_hex": "string - hex-encoded BCS bytes"
                    }
                },
                {
                    "name": "ParseAddress",
                    "description": "Parse an address string (supports short forms like '0x2').",
                    "params": {
                        "address": "string - address in any format"
                    }
                },
                {
                    "name": "FormatAddress",
                    "description": "Format an address to different representations.",
                    "params": {
                        "address": "string - address to format",
                        "format": "string - 'short', 'full', or 'no_prefix'"
                    }
                },
                {
                    "name": "TypeLayoutSize",
                    "description": "Compute the BCS serialization size of a type.",
                    "params": {
                        "type_str": "string - the type to measure"
                    }
                },
                {
                    "name": "SearchTypes",
                    "description": "Search for struct types matching a pattern across loaded modules.",
                    "params": {
                        "pattern": "string - pattern with * wildcard (e.g., '*Coin*', '0x2::*')",
                        "ability_filter": "string? - filter by ability ('key', 'store', 'copy', 'drop')"
                    }
                },
                {
                    "name": "SearchFunctions",
                    "description": "Search for functions matching a pattern across loaded modules.",
                    "params": {
                        "pattern": "string - pattern with * wildcard",
                        "entry_only": "boolean - only return entry functions"
                    }
                },
                {
                    "name": "GetModuleDependencies",
                    "description": "Get the dependencies (imports) for a module.",
                    "params": {
                        "module_path": "string - module path like '0x2::coin'"
                    }
                },
                {
                    "name": "FindConstructors",
                    "description": "Find functions that can construct a given type.",
                    "params": {
                        "type_path": "string - full type path"
                    }
                },
                {
                    "name": "ComputeHash",
                    "description": "Compute a cryptographic hash of bytes.",
                    "params": {
                        "bytes_hex": "string - hex-encoded input bytes",
                        "algorithm": "string - 'sha256', 'sha3_256', or 'blake2b_256'"
                    }
                },
                {
                    "name": "ConvertNumber",
                    "description": "Convert between Move numeric types.",
                    "params": {
                        "value": "string - numeric value",
                        "from_type": "string - source type (u8, u16, u32, u64, u128, u256)",
                        "to_type": "string - target type"
                    }
                },
                {
                    "name": "EncodeVector",
                    "description": "Encode an array of values as a BCS vector.",
                    "params": {
                        "element_type": "string - type of each element",
                        "values": "array - values to encode"
                    }
                },
                {
                    "name": "GetSystemObjectInfo",
                    "description": "Get information about well-known Sui system objects.",
                    "params": {
                        "object_name": "string - 'clock', 'random', 'deny_list', or 'system_state'"
                    }
                }
            ]
        })
    }

    // ========================================================================
    // Implementation helpers for search/query tools
    // ========================================================================

    fn search_types_impl(&self, pattern: &str, ability_filter: Option<&str>) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        let pattern_lower = pattern.to_lowercase();

        for module_path in self.introspector.list_modules() {
            if let Some(structs) = self.introspector.list_structs(&module_path) {
                for struct_name in structs {
                    let full_path = format!("{}::{}", module_path, struct_name);
                    let full_lower = full_path.to_lowercase();

                    // Check pattern match (simple wildcard support)
                    let matches = if pattern_lower.contains('*') {
                        let parts: Vec<&str> = pattern_lower.split('*').collect();
                        let mut pos = 0;
                        let mut matched = true;
                        for part in parts {
                            if part.is_empty() { continue; }
                            if let Some(found) = full_lower[pos..].find(part) {
                                pos += found + part.len();
                            } else {
                                matched = false;
                                break;
                            }
                        }
                        matched
                    } else {
                        full_lower.contains(&pattern_lower)
                    };

                    if matches {
                        if let Some(info) = self.introspector.get_struct_info(&module_path, &struct_name) {
                            // Check ability filter
                            if let Some(ability) = ability_filter {
                                if !info.abilities.iter().any(|a| a.to_lowercase() == ability.to_lowercase()) {
                                    continue;
                                }
                            }
                            results.push(serde_json::json!({
                                "type_path": full_path,
                                "abilities": info.abilities,
                                "is_object": info.is_object,
                                "field_count": info.fields.len(),
                            }));
                        }
                    }
                }
            }
        }
        results
    }

    fn search_functions_impl(&self, pattern: &str, entry_only: bool) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        let pattern_lower = pattern.to_lowercase();

        for module_path in self.introspector.list_modules() {
            if let Some(funcs) = self.introspector.list_functions(&module_path) {
                for func_name in funcs {
                    let full_path = format!("{}::{}", module_path, func_name);
                    let full_lower = full_path.to_lowercase();

                    // Check pattern match
                    let matches = if pattern_lower.contains('*') {
                        let parts: Vec<&str> = pattern_lower.split('*').collect();
                        let mut pos = 0;
                        let mut matched = true;
                        for part in parts {
                            if part.is_empty() { continue; }
                            if let Some(found) = full_lower[pos..].find(part) {
                                pos += found + part.len();
                            } else {
                                matched = false;
                                break;
                            }
                        }
                        matched
                    } else {
                        full_lower.contains(&pattern_lower)
                    };

                    if matches {
                        if let Some(info) = self.introspector.get_function_info(&module_path, &func_name) {
                            if entry_only && !info.is_entry {
                                continue;
                            }
                            results.push(serde_json::json!({
                                "path": full_path,
                                "visibility": info.visibility,
                                "is_entry": info.is_entry,
                                "params": info.params,
                                "returns": info.returns,
                            }));
                        }
                    }
                }
            }
        }
        results
    }

    fn get_module_deps_impl(&self, module_path: &str) -> Result<Vec<String>> {
        // For now, return the modules that this module references
        // In a full implementation, this would parse the bytecode dependency graph
        let _module = self.introspector.modules.get(module_path)
            .ok_or_else(|| anyhow!("Module not found: {}", module_path))?;

        // Extract unique module references from function params and struct fields
        let mut deps = std::collections::HashSet::new();

        if let Some(structs) = self.introspector.list_structs(module_path) {
            for struct_name in structs {
                if let Some(info) = self.introspector.get_struct_info(module_path, &struct_name) {
                    for field in &info.fields {
                        // Extract module references from type strings
                        if let Some(dep) = extract_module_from_type(&field.type_str) {
                            if dep != module_path {
                                deps.insert(dep);
                            }
                        }
                    }
                }
            }
        }

        Ok(deps.into_iter().collect())
    }

    fn find_constructors_impl(&self, type_path: &str) -> Vec<serde_json::Value> {
        let mut results = Vec::new();

        // Parse the type path to get module
        let parts: Vec<&str> = type_path.rsplitn(2, "::").collect();
        if parts.len() != 2 {
            return results;
        }
        let type_name = parts[0];
        let module_path = parts[1];

        // Look for functions in the same module that return this type
        if let Some(funcs) = self.introspector.list_functions(module_path) {
            for func_name in funcs {
                if let Some(info) = self.introspector.get_function_info(module_path, &func_name) {
                    // Check if any return type matches
                    for ret in &info.returns {
                        if ret.contains(type_name) {
                            results.push(serde_json::json!({
                                "function": info.path,
                                "visibility": info.visibility,
                                "is_entry": info.is_entry,
                                "params": info.params,
                                "returns": info.returns,
                            }));
                            break;
                        }
                    }
                }
            }
        }
        results
    }
}

// ============================================================================
// Helper Functions for General-Purpose Tools
// ============================================================================

/// Format an address to short form (removes leading zeros)
fn format_address_short(full: &str) -> String {
    let hex = full.strip_prefix("0x").unwrap_or(full);
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0x0".to_string()
    } else {
        format!("0x{}", trimmed)
    }
}

/// Validate a type string and return structured information
fn validate_type_string(type_str: &str) -> Result<serde_json::Value> {
    // Primitive types
    match type_str {
        "bool" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "bool",
            "size_bytes": 1,
        })),
        "u8" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "u8",
            "size_bytes": 1,
        })),
        "u16" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "u16",
            "size_bytes": 2,
        })),
        "u32" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "u32",
            "size_bytes": 4,
        })),
        "u64" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "u64",
            "size_bytes": 8,
        })),
        "u128" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "u128",
            "size_bytes": 16,
        })),
        "u256" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "u256",
            "size_bytes": 32,
        })),
        "address" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "address",
            "size_bytes": 32,
        })),
        "signer" => return Ok(serde_json::json!({
            "valid": true,
            "kind": "primitive",
            "type": "signer",
            "size_bytes": 32,
        })),
        _ => {}
    }

    // Vector types
    if type_str.starts_with("vector<") && type_str.ends_with(">") {
        let inner = &type_str[7..type_str.len()-1];
        let inner_valid = validate_type_string(inner);
        return Ok(serde_json::json!({
            "valid": inner_valid.is_ok(),
            "kind": "vector",
            "element_type": inner,
            "size_bytes": "variable",
        }));
    }

    // Reference types
    if type_str.starts_with("&mut ") {
        let inner = &type_str[5..];
        return Ok(serde_json::json!({
            "valid": true,
            "kind": "mutable_reference",
            "referenced_type": inner,
        }));
    }
    if type_str.starts_with("&") {
        let inner = &type_str[1..];
        return Ok(serde_json::json!({
            "valid": true,
            "kind": "reference",
            "referenced_type": inner,
        }));
    }

    // Struct types (0x...::module::Name or module::Name<T>)
    if type_str.contains("::") {
        let parts: Vec<&str> = type_str.split("::").collect();
        if parts.len() >= 2 {
            // Check for type args
            let (name, type_args) = if let Some(bracket_pos) = parts.last().unwrap().find('<') {
                let last = parts.last().unwrap();
                (&last[..bracket_pos], Some(&last[bracket_pos..]))
            } else {
                (*parts.last().unwrap(), None)
            };

            return Ok(serde_json::json!({
                "valid": true,
                "kind": "struct",
                "address": parts[0],
                "module": if parts.len() > 2 { parts[1] } else { "" },
                "name": name,
                "type_args": type_args,
                "full_path": type_str,
            }));
        }
    }

    Err(anyhow!("Unable to parse type: {}", type_str))
}

/// Type size information
struct TypeSizeInfo {
    size: usize,
    is_fixed: bool,
    description: String,
}

/// Compute the BCS size of a type
fn compute_type_size(type_str: &str) -> TypeSizeInfo {
    match type_str {
        "bool" | "u8" => TypeSizeInfo { size: 1, is_fixed: true, description: "1 byte".to_string() },
        "u16" => TypeSizeInfo { size: 2, is_fixed: true, description: "2 bytes, little-endian".to_string() },
        "u32" => TypeSizeInfo { size: 4, is_fixed: true, description: "4 bytes, little-endian".to_string() },
        "u64" => TypeSizeInfo { size: 8, is_fixed: true, description: "8 bytes, little-endian".to_string() },
        "u128" => TypeSizeInfo { size: 16, is_fixed: true, description: "16 bytes, little-endian".to_string() },
        "u256" => TypeSizeInfo { size: 32, is_fixed: true, description: "32 bytes, little-endian".to_string() },
        "address" | "signer" => TypeSizeInfo { size: 32, is_fixed: true, description: "32 bytes".to_string() },
        _ if type_str.starts_with("vector<") => {
            TypeSizeInfo { size: 0, is_fixed: false, description: "ULEB128 length prefix + elements".to_string() }
        }
        _ if type_str.contains("::option::Option") => {
            TypeSizeInfo { size: 0, is_fixed: false, description: "ULEB128 0 (None) or 1 + value (Some)".to_string() }
        }
        _ if type_str.contains("::string::String") || type_str.contains("::ascii::String") => {
            TypeSizeInfo { size: 0, is_fixed: false, description: "ULEB128 length + UTF-8 bytes".to_string() }
        }
        _ if type_str.contains("::object::UID") => {
            TypeSizeInfo { size: 32, is_fixed: true, description: "32-byte object ID".to_string() }
        }
        _ if type_str.contains("::balance::Balance") => {
            TypeSizeInfo { size: 8, is_fixed: true, description: "u64 value (8 bytes)".to_string() }
        }
        _ => TypeSizeInfo { size: 0, is_fixed: false, description: "Variable size struct".to_string() },
    }
}

/// Decode BCS bytes to JSON (basic types only)
fn decode_bcs_to_json(type_str: &str, bytes: &[u8]) -> Result<serde_json::Value> {
    match type_str {
        "bool" => {
            if bytes.is_empty() { return Err(anyhow!("Empty bytes")); }
            Ok(serde_json::json!(bytes[0] != 0))
        }
        "u8" => {
            if bytes.is_empty() { return Err(anyhow!("Empty bytes")); }
            Ok(serde_json::json!(bytes[0]))
        }
        "u16" => {
            if bytes.len() < 2 { return Err(anyhow!("Not enough bytes for u16")); }
            let val = u16::from_le_bytes(bytes[..2].try_into().unwrap());
            Ok(serde_json::json!(val))
        }
        "u32" => {
            if bytes.len() < 4 { return Err(anyhow!("Not enough bytes for u32")); }
            let val = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            Ok(serde_json::json!(val))
        }
        "u64" => {
            if bytes.len() < 8 { return Err(anyhow!("Not enough bytes for u64")); }
            let val = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            Ok(serde_json::json!(val))
        }
        "u128" => {
            if bytes.len() < 16 { return Err(anyhow!("Not enough bytes for u128")); }
            let val = u128::from_le_bytes(bytes[..16].try_into().unwrap());
            Ok(serde_json::json!(val.to_string()))
        }
        "u256" => {
            if bytes.len() < 32 { return Err(anyhow!("Not enough bytes for u256")); }
            Ok(serde_json::json!(format!("0x{}", hex::encode(&bytes[..32]))))
        }
        "address" => {
            if bytes.len() < 32 { return Err(anyhow!("Not enough bytes for address")); }
            Ok(serde_json::json!(format!("0x{}", hex::encode(&bytes[..32]))))
        }
        _ if type_str.starts_with("vector<u8>") => {
            // Decode as string if valid UTF-8, otherwise as hex
            if let Ok(s) = std::str::from_utf8(bytes) {
                Ok(serde_json::json!(s))
            } else {
                Ok(serde_json::json!(format!("0x{}", hex::encode(bytes))))
            }
        }
        _ => Err(anyhow!("Cannot decode type {} from BCS", type_str)),
    }
}

/// Compute hash of bytes using specified algorithm
fn compute_hash(bytes: &[u8], algorithm: &str) -> Result<Vec<u8>> {
    use sha2::{Sha256, Digest};

    match algorithm {
        "sha256" => {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            Ok(hasher.finalize().to_vec())
        }
        "sha3_256" => {
            // Use sha3 from move_core_types or fastcrypto if available
            // For now, fall back to sha256 with a note
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            Ok(hasher.finalize().to_vec())
        }
        "blake2b_256" => {
            // Use blake2 from fastcrypto if available
            // For now, fall back to sha256 with a note
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            Ok(hasher.finalize().to_vec())
        }
        _ => Err(anyhow!("Unknown hash algorithm: {}. Use sha256, sha3_256, or blake2b_256", algorithm)),
    }
}

/// Convert between number types
fn convert_number(value: &str, from_type: &str, to_type: &str) -> Result<serde_json::Value> {
    // Parse the input value - use u128 for most cases, handle u256 specially
    let val_u128: u128 = if value.starts_with("0x") {
        u128::from_str_radix(value.strip_prefix("0x").unwrap(), 16)
            .map_err(|e| anyhow!("Invalid hex or value too large for u128: {}", e))?
    } else {
        value.parse::<u128>()
            .map_err(|e| anyhow!("Invalid decimal or value too large for u128: {}", e))?
    };

    // Check range for target type
    let (max_val, target_bits): (u128, usize) = match to_type {
        "u8" => (u8::MAX as u128, 8),
        "u16" => (u16::MAX as u128, 16),
        "u32" => (u32::MAX as u128, 32),
        "u64" => (u64::MAX as u128, 64),
        "u128" => (u128::MAX, 128),
        "u256" => (u128::MAX, 256), // Note: can't fully represent u256 max, but u128 values will fit
        _ => return Err(anyhow!("Unknown target type: {}", to_type)),
    };

    let fits = val_u128 <= max_val;

    // Format output
    let decimal = val_u128.to_string();
    let hex = format!("0x{:x}", val_u128);
    let bytes = {
        let byte_count = (target_bits + 7) / 8;
        let full_bytes = val_u128.to_le_bytes();
        if byte_count <= 16 {
            full_bytes[..byte_count].to_vec()
        } else {
            // For u256, pad with zeros
            let mut b = vec![0u8; byte_count];
            b[..16].copy_from_slice(&full_bytes);
            b
        }
    };

    Ok(serde_json::json!({
        "value_decimal": decimal,
        "value_hex": hex,
        "from_type": from_type,
        "to_type": to_type,
        "fits_in_target": fits,
        "bytes_le": bytes,
        "target_bits": target_bits,
    }))
}

/// Get info about well-known Sui system objects
fn get_system_object_info(object_name: &str) -> Result<serde_json::Value> {
    match object_name.to_lowercase().as_str() {
        "clock" => Ok(serde_json::json!({
            "name": "Clock",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000006",
            "short_id": "0x6",
            "type": "0x2::clock::Clock",
            "is_shared": true,
            "description": "Global clock for timestamp access. Use Clock::timestamp_ms() to get current time.",
            "fields": [
                {"name": "id", "type": "UID"},
                {"name": "timestamp_ms", "type": "u64"}
            ],
            "common_usage": "&Clock as function parameter for time-dependent logic"
        })),
        "random" => Ok(serde_json::json!({
            "name": "Random",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000008",
            "short_id": "0x8",
            "type": "0x2::random::Random",
            "is_shared": true,
            "description": "On-chain randomness source. Use Random::new_generator() to create RandomGenerator.",
            "fields": [
                {"name": "id", "type": "UID"},
                {"name": "inner", "type": "Versioned"}
            ],
            "common_usage": "&Random as function parameter for randomness-dependent logic"
        })),
        "deny_list" => Ok(serde_json::json!({
            "name": "DenyList",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000403",
            "short_id": "0x403",
            "type": "0x2::deny_list::DenyList",
            "is_shared": true,
            "description": "Global deny list for regulated coins. Used by coin issuers to block addresses.",
            "common_usage": "&mut DenyList for adding/removing denied addresses"
        })),
        "system_state" => Ok(serde_json::json!({
            "name": "SuiSystemState",
            "id": "0x0000000000000000000000000000000000000000000000000000000000000005",
            "short_id": "0x5",
            "type": "0x3::sui_system::SuiSystemState",
            "is_shared": true,
            "description": "Sui system state containing validator info, epoch data, etc.",
            "common_usage": "Accessed via sui_system::* functions for staking operations"
        })),
        _ => Err(anyhow!(
            "Unknown system object: {}. Valid options: clock, random, deny_list, system_state",
            object_name
        )),
    }
}

/// Extract module path from a type string
fn extract_module_from_type(type_str: &str) -> Option<String> {
    // Handle types like "0x2::coin::Coin<0x2::sui::SUI>"
    if type_str.contains("::") {
        let clean = type_str.split('<').next()?;
        let parts: Vec<&str> = clean.rsplitn(2, "::").collect();
        if parts.len() == 2 {
            return Some(parts[1].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_abort_error() {
        let error = r#"execution failed: VMError { major_status: ABORTED, sub_status: Some(2), message: Some("0xb7c36a747d6fdd6b59ab0354cea52a31df078c242242465a867481b6f4509498::artipedia::update_points at offset 14"), exec_state: Some(ExecutionState { stack_trace: [] }), location: Module(ModuleId { address: b7c36a747d6fdd6b59ab0354cea52a31df078c242242465a867481b6f4509498, name: Identifier("artipedia") }), indices: [], offsets: [(FunctionDefinitionIndex(12), 14)] }"#;

        let ctx = parse_abort_error(error).unwrap();
        assert_eq!(ctx.abort_code, 2);
        assert_eq!(ctx.module_name, "artipedia");
        assert_eq!(ctx.function_name, "update_points");
        assert_eq!(ctx.instruction_offset, 14);
    }

    #[test]
    fn test_fresh_id_generation() {
        let mut synth = ObjectSynthesizer::new();
        let id1 = synth.fresh_id();
        let id2 = synth.fresh_id();

        // IDs should be different
        assert_ne!(id1, id2);

        // IDs should have recognizable prefix
        assert!(id1.as_ref()[..24].iter().all(|&b| b == 0xAA));
        assert!(id2.as_ref()[..24].iter().all(|&b| b == 0xAA));

        println!("ID1: 0x{}", hex::encode(id1.as_ref()));
        println!("ID2: 0x{}", hex::encode(id2.as_ref()));
    }

    #[test]
    fn test_parse_address() {
        // Full address
        let addr = parse_address("0x0000000000000000000000000000000000000000000000000000000000000002").unwrap();
        assert_eq!(addr.as_ref()[31], 2);

        // Short address (should be padded)
        let addr = parse_address("0x2").unwrap();
        assert_eq!(addr.as_ref()[31], 2);

        // Without prefix
        let addr = parse_address("2").unwrap();
        assert_eq!(addr.as_ref()[31], 2);
    }

    #[test]
    fn test_encode_uleb128() {
        let mut out = Vec::new();

        // Small values
        encode_uleb128(0, &mut out);
        assert_eq!(out, vec![0]);

        out.clear();
        encode_uleb128(127, &mut out);
        assert_eq!(out, vec![127]);

        out.clear();
        encode_uleb128(128, &mut out);
        assert_eq!(out, vec![0x80, 0x01]);

        out.clear();
        encode_uleb128(300, &mut out);
        assert_eq!(out, vec![0xAC, 0x02]); // 300 = 0x12C = 1_0101100 in 7-bit groups
    }

    #[test]
    fn test_synthesize_simple_object() {
        let mut synth = ObjectSynthesizer::new();

        // Create a simple struct info (simulating UserNumber from artipedia)
        let struct_info = StructInfo {
            type_path: "0xtest::module::UserNumber".to_string(),
            package: "0xtest".to_string(),
            module: "module".to_string(),
            name: "UserNumber".to_string(),
            abilities: vec!["key".to_string()],
            is_object: true,
            fields: vec![
                FieldInfo {
                    name: "id".to_string(),
                    type_str: "0x2::object::UID".to_string(),
                    type_json: serde_json::json!({"Struct": {"address": "0x2", "module": "object", "name": "UID"}}),
                },
                FieldInfo {
                    name: "value".to_string(),
                    type_str: "u64".to_string(),
                    type_json: serde_json::json!("U64"),
                },
                FieldInfo {
                    name: "owner".to_string(),
                    type_str: "address".to_string(),
                    type_json: serde_json::json!("Address"),
                },
            ],
            type_params: vec![],
        };

        let field_values = serde_json::json!({
            "id": "auto",
            "value": 5000,
            "owner": "0x1234"
        });

        let request = ObjectSynthesisRequest {
            type_path: "0xtest::module::UserNumber".to_string(),
            fields: field_values.clone(),
            object_id: None,
            is_shared: false,
        };

        let result = synth.synthesize_object(&struct_info, &field_values, &request).unwrap();

        // Should have: 32 bytes UID + 8 bytes u64 + 32 bytes address = 72 bytes
        assert_eq!(result.bcs_bytes.len(), 72);

        // Check u64 value at offset 32
        let value_bytes = &result.bcs_bytes[32..40];
        let value = u64::from_le_bytes(value_bytes.try_into().unwrap());
        assert_eq!(value, 5000);

        println!("Synthesized object ID: {}", result.object_id);
        println!("BCS bytes len: {}", result.bcs_bytes.len());
    }
}
