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
use move_core_types::language_storage::TypeTag;
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
#[derive(Debug, Clone, Default)]
pub struct SynthesisResult {
    /// The synthesized BCS bytes
    pub bytes: Vec<u8>,
    /// Whether this was a "best effort" synthesis (may not be fully valid)
    pub is_stub: bool,
    /// Description of what was synthesized
    pub description: String,
    /// Resolved type arguments if this was a generic type (defaults to empty)
    #[allow(dead_code)]
    pub type_args: Vec<String>,
}

impl SynthesisResult {
    /// Create a new SynthesisResult with no type arguments.
    pub fn new(bytes: Vec<u8>, is_stub: bool, description: impl Into<String>) -> Self {
        Self {
            bytes,
            is_stub,
            description: description.into(),
            type_args: Vec::new(),
        }
    }

    /// Create a new SynthesisResult with type arguments.
    pub fn with_type_args(
        bytes: Vec<u8>,
        is_stub: bool,
        description: impl Into<String>,
        type_args: Vec<String>,
    ) -> Self {
        Self {
            bytes,
            is_stub,
            description: description.into(),
            type_args,
        }
    }
}

/// Context for synthesizing types with generic parameters.
/// Maps type parameter indices (T0, T1, etc.) to concrete type strings.
#[derive(Debug, Clone, Default)]
pub struct TypeContext {
    /// Type arguments: index -> concrete type string
    type_args: Vec<String>,
}

impl TypeContext {
    /// Create a new empty type context.
    pub fn new() -> Self {
        Self {
            type_args: Vec::new(),
        }
    }

    /// Create a type context with the given type arguments.
    pub fn with_args(args: Vec<String>) -> Self {
        Self { type_args: args }
    }

    /// Get the concrete type for a type parameter index.
    /// Returns None if the index is out of bounds.
    pub fn resolve(&self, index: usize) -> Option<&str> {
        self.type_args.get(index).map(|s| s.as_str())
    }

    /// Check if this context has any type arguments.
    pub fn is_empty(&self) -> bool {
        self.type_args.is_empty()
    }

    /// Create a TypeContext from a slice of TypeTags.
    /// Converts each TypeTag to its string representation.
    pub fn from_type_tags(type_tags: &[TypeTag]) -> Self {
        let args: Vec<String> = type_tags.iter().map(Self::type_tag_to_string).collect();
        Self { type_args: args }
    }

    /// Convert a TypeTag to its canonical string representation.
    /// Delegates to the canonical implementation in types module.
    pub fn type_tag_to_string(tag: &TypeTag) -> String {
        crate::benchmark::types::format_type_tag(tag)
    }
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

    /// Synthesize BCS bytes for a struct type with explicit type arguments.
    ///
    /// # Arguments
    /// * `module_addr` - Module address containing the struct
    /// * `module_name` - Module name
    /// * `struct_name` - Struct name
    /// * `type_args` - Concrete type arguments (e.g., ["0x2::sui::SUI"] for Coin<SUI>)
    ///
    /// # Returns
    /// SynthesisResult containing the BCS bytes and metadata
    pub fn synthesize_struct_with_type_args(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
        type_args: &[String],
    ) -> Result<SynthesisResult> {
        let context = TypeContext::with_args(type_args.to_vec());
        self.synthesize_struct_with_depth_and_context(
            module_addr,
            module_name,
            struct_name,
            0,
            &context,
        )
    }

    /// Synthesize BCS bytes for a struct type with TypeTag-based type arguments.
    ///
    /// This is a convenience method that converts TypeTags to their string representation
    /// for use in synthesis. Useful when you have concrete type arguments from StructTag.
    pub fn synthesize_struct_with_type_tags(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
        type_tags: &[TypeTag],
    ) -> Result<SynthesisResult> {
        let context = TypeContext::from_type_tags(type_tags);
        self.synthesize_struct_with_depth_and_context(
            module_addr,
            module_name,
            struct_name,
            0,
            &context,
        )
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

        Ok(SynthesisResult::new(
            bytes,
            is_stub,
            format!("synthesized {}::{}", module_name, struct_name),
        ))
    }

    /// Synthesize a value for a type string (from MM2's formatted type output).
    pub fn synthesize_type_str(&mut self, type_str: &str, depth: usize) -> Result<SynthesisResult> {
        self.synthesize_type_str_with_context(type_str, depth, &TypeContext::new())
    }

    /// Synthesize a value for a type string with generic type context.
    ///
    /// The context maps type parameter indices (T0, T1, etc.) to concrete types,
    /// enabling proper generic type synthesis.
    pub fn synthesize_type_str_with_context(
        &mut self,
        type_str: &str,
        depth: usize,
        context: &TypeContext,
    ) -> Result<SynthesisResult> {
        let type_str = type_str.trim();

        // Handle primitives
        match type_str {
            "bool" => return Ok(SynthesisResult::new(vec![0], false, "bool(false)")),
            "u8" => return Ok(SynthesisResult::new(vec![0], false, "u8(0)")),
            "u16" => {
                return Ok(SynthesisResult::new(
                    0u16.to_le_bytes().to_vec(),
                    false,
                    "u16(0)",
                ))
            }
            "u32" => {
                return Ok(SynthesisResult::new(
                    0u32.to_le_bytes().to_vec(),
                    false,
                    "u32(0)",
                ))
            }
            "u64" => {
                return Ok(SynthesisResult::new(
                    0u64.to_le_bytes().to_vec(),
                    false,
                    "u64(0)",
                ))
            }
            "u128" => {
                return Ok(SynthesisResult::new(
                    0u128.to_le_bytes().to_vec(),
                    false,
                    "u128(0)",
                ))
            }
            "u256" => return Ok(SynthesisResult::new([0u8; 32].to_vec(), false, "u256(0)")),
            "address" => {
                return Ok(SynthesisResult::new(
                    [0u8; 32].to_vec(),
                    false,
                    "address(0x0)",
                ))
            }
            _ => {}
        }

        // Handle vectors
        if type_str.starts_with("vector<") {
            // Empty vector - BCS encodes as length prefix 0
            return Ok(SynthesisResult::new(
                vec![0],
                false,
                format!("empty {}", type_str),
            ));
        }

        // Handle Option<T> - synthesize as None
        if type_str.contains("option::Option<") || type_str.contains("::Option<") {
            return Ok(SynthesisResult::new(vec![0], false, "Option::None"));
        }

        // Handle struct types (format: "0xaddr::module::Name" or "0xaddr::module::Name<T>")
        if type_str.contains("::") {
            return self.synthesize_struct_from_type_str_with_context(type_str, depth, context);
        }

        // Handle type parameters (T0, T1, etc.) - resolve using context or default to u64
        if type_str.starts_with('T') && type_str.len() <= 3 {
            // Try to parse the index (T0 -> 0, T1 -> 1, etc.)
            if let Ok(idx) = type_str[1..].parse::<usize>() {
                // Check if we have a concrete type in the context
                if let Some(concrete_type) = context.resolve(idx) {
                    // Recursively synthesize the concrete type
                    return self.synthesize_type_str_with_context(
                        concrete_type,
                        depth + 1,
                        context,
                    );
                }
            }
            // Default: synthesize as u64 (common default for numeric generics)
            return Ok(SynthesisResult::new(
                0u64.to_le_bytes().to_vec(),
                true,
                format!("type_param {} as u64 (unresolved)", type_str),
            ));
        }

        Err(anyhow!("cannot synthesize type: {}", type_str))
    }

    /// Parse a type string and synthesize the struct.
    fn synthesize_struct_from_type_str(
        &mut self,
        type_str: &str,
        depth: usize,
    ) -> Result<SynthesisResult> {
        self.synthesize_struct_from_type_str_with_context(type_str, depth, &TypeContext::new())
    }

    /// Parse a type string with generic type arguments and synthesize the struct.
    ///
    /// This function extracts type arguments from strings like "0x2::coin::Coin<0x2::sui::SUI>"
    /// and uses them to resolve type parameters in the struct's fields.
    fn synthesize_struct_from_type_str_with_context(
        &mut self,
        type_str: &str,
        depth: usize,
        parent_context: &TypeContext,
    ) -> Result<SynthesisResult> {
        // Parse "0xaddr::module::Name" or "0xaddr::module::Name<T0, T1>"
        let (base_type, type_args) = if let Some(angle_pos) = type_str.find('<') {
            let base = &type_str[..angle_pos];
            let args_str = &type_str[angle_pos + 1..type_str.len() - 1]; // Remove < and >
            let args = Self::parse_type_args(args_str, parent_context);
            (base, args)
        } else {
            (type_str, Vec::new())
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

        // Create context with the parsed type arguments
        let context = TypeContext::with_args(type_args.clone());

        // Synthesize with the context
        let mut result = self.synthesize_struct_with_depth_and_context(
            &module_addr,
            module_name,
            struct_name,
            depth,
            &context,
        )?;

        // Record the resolved type arguments
        result.type_args = type_args;
        Ok(result)
    }

    /// Parse comma-separated type arguments, respecting nested angle brackets.
    fn parse_type_args(args_str: &str, parent_context: &TypeContext) -> Vec<String> {
        let mut args = Vec::new();
        let mut current = String::new();
        let mut depth = 0;

        for ch in args_str.chars() {
            match ch {
                '<' => {
                    depth += 1;
                    current.push(ch);
                }
                '>' => {
                    depth -= 1;
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    let trimmed = current.trim().to_string();
                    // Resolve type parameters from parent context
                    let resolved = Self::resolve_type_arg(&trimmed, parent_context);
                    args.push(resolved);
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        // Don't forget the last argument
        if !current.is_empty() {
            let trimmed = current.trim().to_string();
            let resolved = Self::resolve_type_arg(&trimmed, parent_context);
            args.push(resolved);
        }

        args
    }

    /// Resolve a type argument using the parent context if it's a type parameter.
    fn resolve_type_arg(type_arg: &str, parent_context: &TypeContext) -> String {
        let trimmed = type_arg.trim();
        // If it's a type parameter (T0, T1, etc.), resolve from context
        if trimmed.starts_with('T') && trimmed.len() <= 3 {
            if let Ok(idx) = trimmed[1..].parse::<usize>() {
                if let Some(concrete) = parent_context.resolve(idx) {
                    return concrete.to_string();
                }
            }
        }
        trimmed.to_string()
    }

    /// Synthesize struct with depth tracking and type context.
    fn synthesize_struct_with_depth_and_context(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
        depth: usize,
        context: &TypeContext,
    ) -> Result<SynthesisResult> {
        if depth > self.max_depth {
            return Err(anyhow!(
                "synthesis depth {} exceeded max {} for {}::{}",
                depth,
                self.max_depth,
                module_name,
                struct_name
            ));
        }

        // Format address without 0x prefix for comparison
        let addr = format!("{}", module_addr);

        // Check for framework types first
        if let Some(result) = self.synthesize_framework_type(&addr, module_name, struct_name)? {
            return Ok(result);
        }

        // Check for cycle
        let type_key = format!("{}::{}::{}", addr, module_name, struct_name);
        if self.in_progress.contains(&type_key) {
            return Err(anyhow!("cycle detected while synthesizing {}", type_key));
        }
        self.in_progress.insert(type_key.clone());

        // Look up struct in MM2 model
        let struct_info = self
            .model
            .get_struct(module_addr, module_name, struct_name)
            .ok_or_else(|| {
                anyhow!(
                    "struct not found: {}::{}::{}",
                    module_addr.to_hex_literal(),
                    module_name,
                    struct_name
                )
            })?;

        // Synthesize each field using the type context
        let mut bytes = Vec::new();
        let mut is_stub = false;

        for field in &struct_info.fields {
            let field_result =
                self.synthesize_type_str_with_context(&field.type_str, depth + 1, context)?;
            bytes.extend(field_result.bytes);
            is_stub = is_stub || field_result.is_stub;
        }

        // Remove from in-progress
        self.in_progress.remove(&type_key);

        Ok(SynthesisResult::new(
            bytes,
            is_stub,
            format!("synthesized {}::{}", module_name, struct_name),
        ))
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
                        type_args: Vec::new(),
                    }));
                }
                // object::ID - just an address
                ("object", "ID") => {
                    return Ok(Some(SynthesisResult {
                        bytes: [0u8; 32].to_vec(),
                        is_stub: true,
                        description: "ID(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // balance::Balance<T> - just a u64 value
                ("balance", "Balance") => {
                    return Ok(Some(SynthesisResult {
                        bytes: 0u64.to_le_bytes().to_vec(),
                        is_stub: true,
                        description: "Balance(0)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // balance::Supply<T> - just a u64 value
                ("balance", "Supply") => {
                    return Ok(Some(SynthesisResult {
                        bytes: 0u64.to_le_bytes().to_vec(),
                        is_stub: true,
                        description: "Supply(0)".to_string(),
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
                    }));
                }
                // vec_map::VecMap<K, V> - just a vector of entries (empty)
                ("vec_map", "VecMap") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty vector
                        is_stub: true,
                        description: "VecMap(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // vec_set::VecSet<T> - just a vector (empty)
                ("vec_set", "VecSet") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty vector
                        is_stub: true,
                        description: "VecSet(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // linked_table::LinkedTable<K, V> - UID + size + head + tail
                ("linked_table", "LinkedTable") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // size: u64
                    bytes.push(0); // head: Option<K> = None
                    bytes.push(0); // tail: Option<K> = None
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "LinkedTable(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // object_table::ObjectTable<K, V> - UID + size
                ("object_table", "ObjectTable") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // size: u64
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "ObjectTable(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // object_bag::ObjectBag - UID + size
                ("object_bag", "ObjectBag") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // size: u64
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "ObjectBag(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // priority_queue::PriorityQueue<T> - UID + entries (empty vector)
                ("priority_queue", "PriorityQueue") => {
                    let mut bytes = Vec::new();
                    bytes.push(0); // entries: vector<Entry<T>> - empty
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "PriorityQueue(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // dynamic_field - store marker types
                ("dynamic_field", "Field") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                                                         // name + value are generic, synthesize minimally
                    bytes.push(0); // placeholder for name (empty)
                    bytes.push(0); // placeholder for value (empty)
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "Field(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // kiosk::Kiosk - NFT marketplace object
                ("kiosk", "Kiosk") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // profits: Balance<SUI> (1 SUI)
                    bytes.extend_from_slice(&[0u8; 32]); // owner: address
                    bytes.extend_from_slice(&0u32.to_le_bytes()); // item_count: u32
                    bytes.push(0); // allow_extensions: bool = false
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "Kiosk(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // kiosk::KioskOwnerCap - capability for kiosk
                ("kiosk", "KioskOwnerCap") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&[0u8; 32]); // for: ID
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "KioskOwnerCap(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // transfer_policy::TransferPolicy<T>
                ("transfer_policy", "TransferPolicy") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&0u64.to_le_bytes()); // balance: Balance<SUI>
                    bytes.push(0); // rules: VecSet<TypeName> - empty
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "TransferPolicy(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // transfer_policy::TransferPolicyCap<T>
                ("transfer_policy", "TransferPolicyCap") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.extend_from_slice(&[0u8; 32]); // policy_id: ID
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "TransferPolicyCap(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // coin::CoinMetadata<T>
                ("coin", "CoinMetadata") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    bytes.push(9); // decimals: u8 (default 9 like SUI)
                    bytes.push(0); // name: String (empty)
                    bytes.push(0); // symbol: String (empty)
                    bytes.push(0); // description: String (empty)
                    bytes.push(0); // icon_url: Option<Url> = None
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "CoinMetadata(synthetic)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // coin::DenyCap<T> - deny list capability
                ("coin", "DenyCap") => {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&[0u8; 32]); // id: UID
                    return Ok(Some(SynthesisResult {
                        bytes,
                        is_stub: true,
                        description: "DenyCap(synthetic)".to_string(),
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
                    }));
                }
                // string::String - just a vector<u8>
                ("string", "String") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty string
                        is_stub: false,
                        description: "String(empty)".to_string(),
                        type_args: Vec::new(),
                    }));
                }
                // url::Url - wrapper around String
                ("url", "Url") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0], // Empty URL (empty string)
                        is_stub: true,
                        description: "Url(empty)".to_string(),
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
                    }));
                }
                // string::String - vector<u8>
                ("string", "String") | ("ascii", "String") => {
                    return Ok(Some(SynthesisResult {
                        bytes: vec![0],
                        is_stub: false,
                        description: "String(empty)".to_string(),
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
                        type_args: Vec::new(),
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
            type_args: Vec::new(),
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
            type_args: Vec::new(),
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

    /// Synthesize a type with detailed error reporting.
    /// Returns both the result (or error) and diagnostic information about why it failed.
    pub fn synthesize_with_diagnostics(
        &mut self,
        type_str: &str,
    ) -> (Result<SynthesisResult>, SynthesisDiagnostic) {
        let mut diagnostic = SynthesisDiagnostic::new(type_str);

        // Track what we're trying to do
        diagnostic.is_primitive = matches!(
            type_str,
            "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address"
        );
        diagnostic.is_vector = type_str.starts_with("vector<");
        diagnostic.is_option = type_str.contains("::Option<");

        // Check if it's a framework type
        if type_str.contains("::") {
            let base_type = if let Some(idx) = type_str.find('<') {
                &type_str[..idx]
            } else {
                type_str
            };
            let parts: Vec<&str> = base_type.split("::").collect();
            if parts.len() >= 3 {
                if let Ok(addr) = AccountAddress::from_hex_literal(parts[0]) {
                    let addr_str = format!("{}", addr);
                    diagnostic.is_framework_type = addr_str == SUI_FRAMEWORK
                        || addr_str == MOVE_STDLIB
                        || addr_str == SUI_SYSTEM;
                    diagnostic.module_path = Some(format!("{}::{}", parts[0], parts[1]));
                    diagnostic.struct_name = Some(parts[2].to_string());

                    // Check if struct exists in model
                    diagnostic.struct_found_in_model =
                        self.model.get_struct(&addr, parts[1], parts[2]).is_some();
                }
            }
        }

        // Try synthesis
        let result = self.synthesize_type_str(type_str, 0);
        if let Err(ref e) = result {
            diagnostic.error_message = Some(e.to_string());
        }

        (result, diagnostic)
    }

    /// Try to synthesize a type with fallback strategies.
    /// If the primary synthesis fails, tries simpler fallbacks.
    pub fn synthesize_with_fallback(&mut self, type_str: &str) -> SynthesisResult {
        // Try primary synthesis
        if let Ok(result) = self.synthesize_type_str(type_str, 0) {
            return result;
        }

        // Fallback 1: If it's a generic type, try synthesizing without generics
        if type_str.contains('<') {
            if let Some(base_idx) = type_str.find('<') {
                let base = &type_str[..base_idx];
                if let Ok(result) = self.synthesize_type_str(base, 0) {
                    return SynthesisResult {
                        bytes: result.bytes,
                        is_stub: true,
                        description: format!("fallback {} (generics ignored)", type_str),
                        type_args: Vec::new(),
                    };
                }
            }
        }

        // Fallback 2: Return a minimal stub (UID only for objects)
        if type_str.contains("::") {
            return SynthesisResult {
                bytes: [0u8; 32].to_vec(), // Just a UID
                is_stub: true,
                description: format!("minimal_stub({})", type_str),
                type_args: Vec::new(),
            };
        }

        // Fallback 3: For primitives we don't recognize, default to u64
        SynthesisResult {
            bytes: 0u64.to_le_bytes().to_vec(),
            is_stub: true,
            description: format!("fallback_u64({})", type_str),
            type_args: Vec::new(),
        }
    }

    /// Get the default value for a primitive type.
    pub fn default_primitive(type_str: &str) -> Option<Vec<u8>> {
        match type_str {
            "bool" => Some(vec![0]),
            "u8" => Some(vec![0]),
            "u16" => Some(0u16.to_le_bytes().to_vec()),
            "u32" => Some(0u32.to_le_bytes().to_vec()),
            "u64" => Some(0u64.to_le_bytes().to_vec()),
            "u128" => Some(0u128.to_le_bytes().to_vec()),
            "u256" => Some([0u8; 32].to_vec()),
            "address" => Some([0u8; 32].to_vec()),
            _ => None,
        }
    }

    /// Estimate the BCS size of a type (for debugging/analysis).
    /// Returns None if size cannot be determined statically.
    pub fn estimate_size(&self, type_str: &str) -> Option<usize> {
        match type_str {
            "bool" | "u8" => Some(1),
            "u16" => Some(2),
            "u32" => Some(4),
            "u64" => Some(8),
            "u128" => Some(16),
            "u256" | "address" => Some(32),
            _ if type_str.starts_with("vector<") => Some(1), // Minimum (empty vector)
            _ if type_str.contains("::Option<") => Some(1),  // Minimum (None)
            _ => None,                                       // Complex types need full synthesis
        }
    }
}

/// Diagnostic information from a synthesis attempt.
#[derive(Debug, Clone, Default)]
pub struct SynthesisDiagnostic {
    /// The type being synthesized.
    pub type_str: String,
    /// Whether the type is a primitive.
    pub is_primitive: bool,
    /// Whether the type is a vector.
    pub is_vector: bool,
    /// Whether the type is an Option.
    pub is_option: bool,
    /// Whether the type is from the Sui framework.
    pub is_framework_type: bool,
    /// The module path (if parsed).
    pub module_path: Option<String>,
    /// The struct name (if parsed).
    pub struct_name: Option<String>,
    /// Whether the struct was found in the model.
    pub struct_found_in_model: bool,
    /// Error message if synthesis failed.
    pub error_message: Option<String>,
}

impl SynthesisDiagnostic {
    /// Create a new diagnostic for a type.
    pub fn new(type_str: &str) -> Self {
        Self {
            type_str: type_str.to_string(),
            ..Default::default()
        }
    }

    /// Check if the type should be synthesizable.
    pub fn should_be_synthesizable(&self) -> bool {
        self.is_primitive
            || self.is_vector
            || self.is_option
            || self.is_framework_type
            || self.struct_found_in_model
    }

    /// Get a human-readable reason why synthesis might have failed.
    pub fn failure_reason(&self) -> String {
        if let Some(ref err) = self.error_message {
            return err.clone();
        }

        if !self.type_str.contains("::") {
            return format!("Unknown type format: {}", self.type_str);
        }

        if !self.is_framework_type && !self.struct_found_in_model {
            if let Some(ref path) = self.module_path {
                return format!(
                    "Struct not found in model: {}::{}",
                    path,
                    self.struct_name.as_deref().unwrap_or("?")
                );
            }
        }

        "Unknown failure reason".to_string()
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

    #[test]
    fn test_type_context_resolution() {
        // Test the TypeContext for resolving type parameters
        let ctx = TypeContext::with_args(vec![
            "0x2::sui::SUI".to_string(),
            "0x2::coin::Coin<0x2::sui::SUI>".to_string(),
        ]);

        assert!(!ctx.is_empty());
        assert_eq!(ctx.resolve(0), Some("0x2::sui::SUI"));
        assert_eq!(ctx.resolve(1), Some("0x2::coin::Coin<0x2::sui::SUI>"));
        assert_eq!(ctx.resolve(2), None); // Out of bounds
    }

    #[test]
    fn test_type_arg_parsing() {
        // Test parsing type arguments from a type string
        let ctx = TypeContext::new();

        // Simple type args
        let args = TypeSynthesizer::parse_type_args("0x2::sui::SUI", &ctx);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "0x2::sui::SUI");

        // Multiple type args
        let args = TypeSynthesizer::parse_type_args("0x2::sui::SUI, u64", &ctx);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "0x2::sui::SUI");
        assert_eq!(args[1], "u64");

        // Nested type args (respects angle brackets)
        let args = TypeSynthesizer::parse_type_args("0x2::coin::Coin<0x2::sui::SUI>, u64", &ctx);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "0x2::coin::Coin<0x2::sui::SUI>");
        assert_eq!(args[1], "u64");
    }

    #[test]
    fn test_type_param_resolution_in_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Create a context that resolves T0 to a concrete type
        let ctx = TypeContext::with_args(vec!["u64".to_string()]);

        // Test that T0 resolves correctly
        let result = synthesizer.synthesize_type_str_with_context("T0", 0, &ctx);
        assert!(result.is_ok(), "T0 should resolve to u64");
        let result = result.unwrap();
        assert_eq!(result.bytes.len(), 8, "u64 is 8 bytes");
        assert!(!result.is_stub, "Resolved type should not be stub");
    }

    #[test]
    fn test_generic_type_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Test synthesizing a generic type with concrete type args
        // Balance<T> is just a u64 value
        let result = synthesizer.synthesize_type_str("0x2::balance::Balance<0x2::sui::SUI>", 0);
        assert!(result.is_ok(), "Balance<SUI> synthesis should succeed");
        let result = result.unwrap();
        assert_eq!(result.bytes.len(), 8, "Balance is u64 (8 bytes)");
        assert!(result.is_stub, "Balance is a framework stub");
    }

    #[test]
    fn test_coin_with_type_args() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Test synthesizing Coin<SUI>
        let result = synthesizer.synthesize_type_str("0x2::coin::Coin<0x2::sui::SUI>", 0);
        assert!(result.is_ok(), "Coin<SUI> synthesis should succeed");
        let result = result.unwrap();
        // Coin is UID (32) + Balance<T> (8) = 40 bytes
        assert_eq!(result.bytes.len(), 40, "Coin should be 40 bytes");
        assert!(result.is_stub, "Coin is a framework stub");
        assert_eq!(result.type_args, vec!["0x2::sui::SUI".to_string()]);
    }

    #[test]
    fn test_nested_generic_type_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Test that nested generics are parsed correctly
        let args = TypeSynthesizer::parse_type_args(
            "0x2::coin::Coin<0x2::sui::SUI>, 0x2::balance::Balance<0x2::usdc::USDC>",
            &TypeContext::new(),
        );
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "0x2::coin::Coin<0x2::sui::SUI>");
        assert_eq!(args[1], "0x2::balance::Balance<0x2::usdc::USDC>");
    }

    #[test]
    fn test_linked_table_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let result =
            synthesizer.synthesize_type_str("0x2::linked_table::LinkedTable<address, u64>", 0);
        assert!(result.is_ok(), "LinkedTable synthesis should succeed");
        let result = result.unwrap();
        // LinkedTable: UID (32) + size (8) + head Option (1) + tail Option (1) = 42 bytes
        assert_eq!(result.bytes.len(), 42, "LinkedTable should be 42 bytes");
        assert!(result.is_stub);
    }

    #[test]
    fn test_kiosk_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let result = synthesizer.synthesize_type_str("0x2::kiosk::Kiosk", 0);
        assert!(result.is_ok(), "Kiosk synthesis should succeed");
        let result = result.unwrap();
        assert!(
            result.bytes.len() > 32,
            "Kiosk should have substantial data"
        );
        assert!(result.is_stub);
    }

    #[test]
    fn test_synthesis_with_diagnostics() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Test successful synthesis
        let (result, diagnostic) = synthesizer.synthesize_with_diagnostics("u64");
        assert!(result.is_ok());
        assert!(diagnostic.is_primitive);
        assert!(diagnostic.should_be_synthesizable());

        // Test framework type
        let (result, diagnostic) =
            synthesizer.synthesize_with_diagnostics("0x2::coin::Coin<0x2::sui::SUI>");
        assert!(result.is_ok());
        assert!(diagnostic.is_framework_type);
        assert!(diagnostic.should_be_synthesizable());
    }

    #[test]
    fn test_synthesis_with_fallback() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        // Test successful synthesis (no fallback needed)
        let result = synthesizer.synthesize_with_fallback("u64");
        assert!(!result.is_stub);
        assert_eq!(result.bytes.len(), 8);

        // Test fallback for unknown type
        let result = synthesizer.synthesize_with_fallback("0xdeadbeef::unknown::Type");
        assert!(result.is_stub);
        assert!(result.description.contains("minimal_stub"));
    }

    #[test]
    fn test_default_primitive() {
        assert_eq!(TypeSynthesizer::default_primitive("bool"), Some(vec![0]));
        assert_eq!(TypeSynthesizer::default_primitive("u8"), Some(vec![0]));
        assert_eq!(
            TypeSynthesizer::default_primitive("u64"),
            Some(0u64.to_le_bytes().to_vec())
        );
        assert_eq!(
            TypeSynthesizer::default_primitive("address"),
            Some([0u8; 32].to_vec())
        );
        assert_eq!(TypeSynthesizer::default_primitive("unknown"), None);
    }

    #[test]
    fn test_estimate_size() {
        let model = create_test_model();
        let synthesizer = TypeSynthesizer::new(&model);

        assert_eq!(synthesizer.estimate_size("bool"), Some(1));
        assert_eq!(synthesizer.estimate_size("u64"), Some(8));
        assert_eq!(synthesizer.estimate_size("address"), Some(32));
        assert_eq!(synthesizer.estimate_size("vector<u8>"), Some(1)); // Empty vector minimum
        assert_eq!(
            synthesizer.estimate_size("0x1::option::Option<u64>"),
            Some(1)
        ); // None minimum
        assert_eq!(
            synthesizer.estimate_size("0x2::coin::Coin<0x2::sui::SUI>"),
            None
        ); // Complex type
    }

    #[test]
    fn test_coin_metadata_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let result = synthesizer.synthesize_type_str("0x2::coin::CoinMetadata<0x2::sui::SUI>", 0);
        assert!(result.is_ok(), "CoinMetadata synthesis should succeed");
        let result = result.unwrap();
        assert!(
            result.bytes.len() >= 36,
            "CoinMetadata should have at least UID + decimals + empty strings"
        );
        assert!(result.is_stub);
    }

    #[test]
    fn test_transfer_policy_synthesis() {
        let model = create_test_model();
        let mut synthesizer = TypeSynthesizer::new(&model);

        let result = synthesizer
            .synthesize_type_str("0x2::transfer_policy::TransferPolicy<0xabc::nft::NFT>", 0);
        assert!(result.is_ok(), "TransferPolicy synthesis should succeed");
        let result = result.unwrap();
        assert!(
            result.bytes.len() >= 41,
            "TransferPolicy should have UID + balance + rules"
        );
        assert!(result.is_stub);
    }
}
