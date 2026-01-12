//! TypeModel: Wrapper around MM2's Model for type inhabitation analysis.
//!
//! This module provides a simplified interface to `move-model-2` for the
//! specific needs of the type inhabitation pipeline.

use crate::benchmark::errors::{ErrorCode, Failure};
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_model_2::{compiled_model, summary};
use move_symbol_pool::Symbol;
use std::collections::BTreeMap;

// Re-export Symbol and AbilitySet for convenience
pub use move_model_2::summary::AbilitySet;
pub use move_symbol_pool::Symbol as MoveSymbol;

/// Wrapper around MM2's Model providing type analysis capabilities.
///
/// TypeModel is the core abstraction for Phase 2 (TypeCheck) of the pipeline.
/// It builds a semantic model from compiled bytecode and provides methods
/// to query type information.
pub struct TypeModel {
    /// The underlying MM2 model built from bytecode
    model: compiled_model::Model,
    /// Named address reverse map for prettier error messages
    named_addresses: BTreeMap<AccountAddress, Symbol>,
}

/// Information about a function's signature from the model.
///
/// Note: Types are represented as formatted strings since summary::Type
/// doesn't implement Clone. For detailed type analysis, use the model directly.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    /// Module containing the function
    pub module_addr: AccountAddress,
    pub module_name: String,
    /// Function name
    pub name: String,
    /// Type parameters with their constraints
    pub type_parameters: Vec<TypeParam>,
    /// Parameter info (names and formatted type strings)
    pub parameters: Vec<ParamInfo>,
    /// Return type info (for return value chaining)
    pub returns: Vec<ReturnInfo>,
    /// Number of return values (kept for backwards compatibility)
    pub return_count: usize,
    /// Whether the function is public
    pub is_public: bool,
    /// Whether the function is an entry function
    pub is_entry: bool,
}

/// Information about a function return type.
#[derive(Debug, Clone)]
pub struct ReturnInfo {
    /// Formatted type string
    pub type_str: String,
    /// Parsed struct info if this is a struct type (for return value chaining)
    pub struct_type: Option<ReturnStructType>,
}

/// Parsed struct type information from a return value.
#[derive(Debug, Clone)]
pub struct ReturnStructType {
    /// Module address containing the struct
    pub module_addr: AccountAddress,
    /// Module name
    pub module_name: String,
    /// Struct name
    pub struct_name: String,
    /// Type arguments (as indices into function type params, or concrete types)
    pub type_args: Vec<ReturnTypeArg>,
}

/// Type argument in a return type.
#[derive(Debug, Clone)]
pub enum ReturnTypeArg {
    /// Reference to a function type parameter by index
    TypeParam(usize),
    /// A concrete type (formatted string)
    Concrete(String),
}

/// Type parameter with constraints.
#[derive(Debug, Clone)]
pub struct TypeParam {
    /// Parameter name (if available from source)
    pub name: Option<String>,
    /// Ability constraints (copy, drop, store, key)
    pub constraints: AbilitySet,
}

/// Information about a function parameter.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    /// Parameter name (if available from source)
    pub name: Option<String>,
    /// Formatted type string
    pub type_str: String,
    /// Whether this is a reference
    pub is_reference: bool,
    /// Whether this is a mutable reference
    pub is_mut_reference: bool,
}

/// Information about a struct definition.
#[derive(Debug, Clone)]
pub struct StructInfo {
    /// Module containing the struct
    pub module_addr: AccountAddress,
    pub module_name: String,
    /// Struct name
    pub name: String,
    /// Declared abilities
    pub abilities: AbilitySet,
    /// Type parameters
    pub type_parameters: Vec<DatatypeTypeParam>,
    /// Fields
    pub fields: Vec<FieldInfo>,
}

/// Type parameter for a datatype (struct/enum).
#[derive(Debug, Clone)]
pub struct DatatypeTypeParam {
    /// Whether this is a phantom type parameter
    pub phantom: bool,
    /// Parameter name (if available)
    pub name: Option<String>,
    /// Ability constraints
    pub constraints: AbilitySet,
}

/// Information about a struct field.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// Field name
    pub name: String,
    /// Formatted type string
    pub type_str: String,
}

impl TypeModel {
    /// Build a TypeModel from compiled modules.
    ///
    /// This is the primary constructor, using MM2's `Model::from_compiled()` to
    /// build a semantic model directly from bytecode without source code.
    ///
    /// # Arguments
    /// * `modules` - Compiled Move modules to analyze
    ///
    /// # Returns
    /// A TypeModel ready for type analysis, or a Failure if model building fails.
    pub fn from_modules(modules: Vec<CompiledModule>) -> Result<Self, Failure> {
        Self::from_modules_with_addresses(BTreeMap::new(), modules)
    }

    /// Build a TypeModel with named address mappings.
    ///
    /// Named addresses improve error messages by showing package names instead
    /// of raw addresses (e.g., "sui::object" vs "0x2::object").
    ///
    /// # Arguments
    /// * `named_addresses` - Map from addresses to symbolic names
    /// * `modules` - Compiled Move modules to analyze
    pub fn from_modules_with_addresses(
        named_addresses: BTreeMap<AccountAddress, Symbol>,
        modules: Vec<CompiledModule>,
    ) -> Result<Self, Failure> {
        if modules.is_empty() {
            return Err(Failure::new(
                ErrorCode::ModuleNotFound,
                "No modules provided to TypeModel",
            ));
        }

        let model = compiled_model::Model::from_compiled(&named_addresses, modules);

        Ok(Self {
            model,
            named_addresses,
        })
    }

    /// Get function signature by module and name.
    ///
    /// # Arguments
    /// * `module_addr` - Address of the module containing the function
    /// * `module_name` - Name of the module
    /// * `function_name` - Name of the function
    ///
    /// # Returns
    /// Function signature if found, or None.
    pub fn get_function(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Option<FunctionSignature> {
        let module_name_sym = Symbol::from(module_name);
        let function_name_sym = Symbol::from(function_name);

        // Try to get the module
        let package = self.model.maybe_package(module_addr)?;
        let module = package.maybe_module(module_name_sym)?;
        let function = module.maybe_function(function_name_sym)?;

        let summary = function.summary();

        // Parse return types for return value chaining
        let returns: Vec<ReturnInfo> = summary
            .return_
            .iter()
            .map(|ret_type| {
                let type_str = format_type(ret_type);
                let struct_type = parse_return_struct_type(ret_type);
                ReturnInfo {
                    type_str,
                    struct_type,
                }
            })
            .collect();

        Some(FunctionSignature {
            module_addr: *module_addr,
            module_name: module_name.to_string(),
            name: function_name.to_string(),
            type_parameters: summary
                .type_parameters
                .iter()
                .map(|tp| TypeParam {
                    name: tp.name.map(|s| s.to_string()),
                    constraints: tp.constraints.clone(),
                })
                .collect(),
            parameters: summary
                .parameters
                .iter()
                .map(|p| {
                    let (is_ref, is_mut) = match &p.type_ {
                        summary::Type::Reference(is_mut, _) => (true, *is_mut),
                        _ => (false, false),
                    };
                    ParamInfo {
                        name: p.name.map(|s| s.to_string()),
                        type_str: format_type(&p.type_),
                        is_reference: is_ref,
                        is_mut_reference: is_mut,
                    }
                })
                .collect(),
            returns,
            return_count: summary.return_.len(),
            is_public: matches!(
                summary.visibility,
                summary::Visibility::Public | summary::Visibility::Friend
            ),
            is_entry: summary.entry,
        })
    }

    /// Get struct definition by module and name.
    ///
    /// # Arguments
    /// * `module_addr` - Address of the module containing the struct
    /// * `module_name` - Name of the module
    /// * `struct_name` - Name of the struct
    ///
    /// # Returns
    /// Struct info if found, or None.
    pub fn get_struct(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
    ) -> Option<StructInfo> {
        let module_name_sym = Symbol::from(module_name);
        let struct_name_sym = Symbol::from(struct_name);

        let package = self.model.maybe_package(module_addr)?;
        let module = package.maybe_module(module_name_sym)?;
        let struct_ = module.maybe_struct(struct_name_sym)?;

        let summary = struct_.summary();

        Some(StructInfo {
            module_addr: *module_addr,
            module_name: module_name.to_string(),
            name: struct_name.to_string(),
            abilities: summary.abilities.clone(),
            type_parameters: summary
                .type_parameters
                .iter()
                .map(|tp| DatatypeTypeParam {
                    phantom: tp.phantom,
                    name: tp.tparam.name.map(|s| s.to_string()),
                    constraints: tp.tparam.constraints.clone(),
                })
                .collect(),
            fields: summary
                .fields
                .fields
                .iter()
                .map(|(name, field)| FieldInfo {
                    name: name.to_string(),
                    type_str: format_type(&field.type_),
                })
                .collect(),
        })
    }

    /// List all modules in the model.
    pub fn modules(&self) -> Vec<(AccountAddress, String)> {
        self.model
            .modules()
            .map(|m| (m.package().address(), m.name().to_string()))
            .collect()
    }

    /// List all functions in a module.
    pub fn functions_in_module(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
    ) -> Vec<String> {
        let module_name_sym = Symbol::from(module_name);

        self.model
            .maybe_package(module_addr)
            .and_then(|p| p.maybe_module(module_name_sym))
            .map(|m| m.functions().map(|f| f.name().to_string()).collect())
            .unwrap_or_default()
    }

    /// List all structs in a module.
    pub fn structs_in_module(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
    ) -> Vec<String> {
        let module_name_sym = Symbol::from(module_name);

        self.model
            .maybe_package(module_addr)
            .and_then(|p| p.maybe_module(module_name_sym))
            .map(|m| m.structs().map(|s| s.name().to_string()).collect())
            .unwrap_or_default()
    }

    /// Get the summary for direct access to MM2 types.
    pub fn summary(&self) -> &summary::Packages {
        self.model.summary_without_source()
    }

    /// Get the underlying MM2 model for advanced use cases.
    pub fn inner(&self) -> &compiled_model::Model {
        &self.model
    }

    /// Get named address if available.
    pub fn get_named_address(&self, addr: &AccountAddress) -> Option<Symbol> {
        self.named_addresses.get(addr).copied()
    }

    /// Format an address for display (using named address if available).
    pub fn format_address(&self, addr: &AccountAddress) -> String {
        if let Some(name) = self.named_addresses.get(addr) {
            name.to_string()
        } else {
            format!("{:#x}", addr)
        }
    }

    /// Format a module ID for display.
    pub fn format_module(&self, addr: &AccountAddress, name: &str) -> String {
        format!("{}::{}", self.format_address(addr), name)
    }
}

/// Format a summary::Type for display.
pub fn format_type(ty: &summary::Type) -> String {
    match ty {
        summary::Type::Bool => "bool".to_string(),
        summary::Type::U8 => "u8".to_string(),
        summary::Type::U16 => "u16".to_string(),
        summary::Type::U32 => "u32".to_string(),
        summary::Type::U64 => "u64".to_string(),
        summary::Type::U128 => "u128".to_string(),
        summary::Type::U256 => "u256".to_string(),
        summary::Type::Address => "address".to_string(),
        summary::Type::Signer => "signer".to_string(),
        summary::Type::Vector(inner) => format!("vector<{}>", format_type(inner)),
        summary::Type::Reference(is_mut, inner) => {
            if *is_mut {
                format!("&mut {}", format_type(inner))
            } else {
                format!("&{}", format_type(inner))
            }
        }
        summary::Type::TypeParameter(idx) => format!("T{}", idx),
        summary::Type::NamedTypeParameter(name) => name.to_string(),
        summary::Type::Datatype(dt) => {
            let base = format!("{}::{}::{}", dt.module.address, dt.module.name, dt.name);
            if dt.type_arguments.is_empty() {
                base
            } else {
                let args: Vec<String> = dt
                    .type_arguments
                    .iter()
                    .map(|ta| format_type(&ta.argument))
                    .collect();
                format!("{}<{}>", base, args.join(", "))
            }
        }
        summary::Type::Tuple(elems) => {
            let formatted: Vec<String> = elems.iter().map(format_type).collect();
            format!("({})", formatted.join(", "))
        }
        summary::Type::Fun(args, ret) => {
            let arg_str: Vec<String> = args.iter().map(format_type).collect();
            format!("|{}| -> {}", arg_str.join(", "), format_type(ret))
        }
        summary::Type::Any => "_".to_string(),
    }
}

/// Parse a return type to extract struct information for return value chaining.
///
/// This is used to identify functions that produce capability types (AdminCap, etc.)
/// that can be used as inputs to other functions.
fn parse_return_struct_type(ty: &summary::Type) -> Option<ReturnStructType> {
    match ty {
        summary::Type::Datatype(dt) => {
            // dt.module.address is a Symbol (formatted string), need to parse it
            let addr_str = dt.module.address.as_str();
            let module_addr = AccountAddress::from_hex_literal(addr_str).ok()?;
            let module_name = dt.module.name.to_string();
            let struct_name = dt.name.to_string();

            // Parse type arguments
            let type_args: Vec<ReturnTypeArg> = dt
                .type_arguments
                .iter()
                .map(|ta| parse_type_arg(&ta.argument))
                .collect();

            Some(ReturnStructType {
                module_addr,
                module_name,
                struct_name,
                type_args,
            })
        }
        // For references, extract the inner type
        summary::Type::Reference(_, inner) => parse_return_struct_type(inner),
        _ => None,
    }
}

/// Parse a type argument from a return type.
fn parse_type_arg(ty: &summary::Type) -> ReturnTypeArg {
    match ty {
        summary::Type::TypeParameter(idx) => ReturnTypeArg::TypeParam(*idx as usize),
        summary::Type::NamedTypeParameter(name) => {
            // Try to extract index from name like "T0", "T1"
            if let Some(idx_str) = name.as_str().strip_prefix('T') {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    return ReturnTypeArg::TypeParam(idx);
                }
            }
            ReturnTypeArg::Concrete(name.to_string())
        }
        _ => ReturnTypeArg::Concrete(format_type(ty)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::errors::Phase;

    #[test]
    fn test_empty_modules_error() {
        let result = TypeModel::from_modules(vec![]);
        assert!(result.is_err());
        // Use match since TypeModel doesn't impl Debug
        match result {
            Err(failure) => {
                assert_eq!(failure.phase, Phase::Resolution);
                assert_eq!(failure.code, ErrorCode::ModuleNotFound);
            }
            Ok(_) => panic!("Expected error"),
        }
    }

    #[test]
    fn test_format_type_primitives() {
        assert_eq!(format_type(&summary::Type::Bool), "bool");
        assert_eq!(format_type(&summary::Type::U64), "u64");
        assert_eq!(format_type(&summary::Type::Address), "address");
    }

    #[test]
    fn test_format_type_vector() {
        let vec_u8 = summary::Type::Vector(Box::new(summary::Type::U8));
        assert_eq!(format_type(&vec_u8), "vector<u8>");
    }

    #[test]
    fn test_format_type_reference() {
        let ref_u64 = summary::Type::Reference(false, Box::new(summary::Type::U64));
        assert_eq!(format_type(&ref_u64), "&u64");

        let mut_ref_u64 = summary::Type::Reference(true, Box::new(summary::Type::U64));
        assert_eq!(format_type(&mut_ref_u64), "&mut u64");
    }
}
