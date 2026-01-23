//! TypeValidator: Static type checking using MM2.
//!
//! This module implements Phase 2 (TypeCheck) of the v0.4.0 pipeline,
//! validating types statically before VM execution.

use crate::errors::{ErrorCode, Failure, FailureContext};
use crate::mm2::model::{FunctionSignature, TypeModel};
use move_core_types::account_address::AccountAddress;
use move_model_2::summary::{self, Ability};

/// Static type validator using MM2.
///
/// TypeValidator performs compile-time type checking, catching errors
/// before attempting VM execution.
pub struct TypeValidator<'a> {
    model: &'a TypeModel,
}

/// Result of validating a function call.
#[derive(Debug)]
pub struct CallValidation {
    /// The validated function signature
    pub function: FunctionSignature,
    /// Number of type parameters expected
    pub type_param_count: usize,
    /// Number of parameters expected
    pub param_count: usize,
}

impl<'a> TypeValidator<'a> {
    /// Create a new TypeValidator for the given model.
    pub fn new(model: &'a TypeModel) -> Self {
        Self { model }
    }

    /// Validate that a function exists and can be called.
    ///
    /// This performs basic resolution checks (Phase 1 overlap) to ensure
    /// the function exists before doing type checking.
    ///
    /// # Arguments
    /// * `module_addr` - Address of the module
    /// * `module_name` - Name of the module
    /// * `function_name` - Name of the function
    ///
    /// # Returns
    /// The function signature if valid, or a Failure.
    pub fn validate_function_exists(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
    ) -> Result<FunctionSignature, Failure> {
        self.model
            .get_function(module_addr, module_name, function_name)
            .ok_or_else(|| {
                Failure::with_context(
                    ErrorCode::FunctionNotFound,
                    format!(
                        "Function not found: {}::{}",
                        self.model.format_module(module_addr, module_name),
                        function_name
                    ),
                    FailureContext {
                        module: Some(self.model.format_module(module_addr, module_name)),
                        function: Some(function_name.to_string()),
                        type_name: None,
                        param_index: None,
                    },
                )
            })
    }

    /// Validate a function call with type argument count.
    ///
    /// Checks:
    /// - Function exists
    /// - Correct number of type arguments
    ///
    /// # Arguments
    /// * `module_addr` - Address of the module
    /// * `module_name` - Name of the module
    /// * `function_name` - Name of the function
    /// * `type_arg_count` - Number of type arguments provided
    ///
    /// # Returns
    /// CallValidation with function info, or a Failure.
    pub fn validate_call(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        function_name: &str,
        type_arg_count: usize,
    ) -> Result<CallValidation, Failure> {
        let sig = self.validate_function_exists(module_addr, module_name, function_name)?;

        // Check type argument count
        if type_arg_count != sig.type_parameters.len() {
            return Err(Failure::new(
                ErrorCode::GenericBoundsViolation,
                format!(
                    "Function {}::{} expects {} type arguments, got {}",
                    self.model.format_module(module_addr, module_name),
                    function_name,
                    sig.type_parameters.len(),
                    type_arg_count
                ),
            ));
        }

        Ok(CallValidation {
            type_param_count: sig.type_parameters.len(),
            param_count: sig.parameters.len(),
            function: sig,
        })
    }

    /// Check if a type string represents a synthesizable type.
    ///
    /// Returns true if:
    /// - Type is a primitive (u8, u64, bool, address, etc.)
    /// - Type is a vector of synthesizable types
    pub fn is_synthesizable_type_str(&self, type_str: &str) -> bool {
        match type_str {
            "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address" => true,
            s if s.starts_with("vector<") => {
                // Extract inner type
                if let Some(inner) = s.strip_prefix("vector<").and_then(|s| s.strip_suffix('>')) {
                    self.is_synthesizable_type_str(inner)
                } else {
                    false
                }
            }
            "signer" => false,
            s if s.starts_with('&') => false, // References
            _ => {
                // For structs, we'd need to check if there's a constructor
                // For now, assume they might be synthesizable
                true
            }
        }
    }

    /// Check if a type string might be an object type.
    ///
    /// Object types in Sui typically have "key" ability and are in specific packages.
    pub fn might_be_object_type(&self, type_str: &str) -> bool {
        // Common Sui object patterns
        type_str.contains("::object::") || type_str.contains("::coin::Coin")
    }

    /// Validate a struct exists.
    pub fn validate_struct_exists(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
    ) -> Result<(), Failure> {
        self.model
            .get_struct(module_addr, module_name, struct_name)
            .map(|_| ())
            .ok_or_else(|| {
                Failure::new(
                    ErrorCode::UnknownType,
                    format!(
                        "Struct not found: {}::{}",
                        self.model.format_module(module_addr, module_name),
                        struct_name
                    ),
                )
            })
    }

    /// Check if a struct has the key ability (is an object type).
    pub fn struct_has_key_ability(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        struct_name: &str,
    ) -> bool {
        self.model
            .get_struct(module_addr, module_name, struct_name)
            .map(|info| info.abilities.0.contains(&Ability::Key))
            .unwrap_or(false)
    }

    /// Get the model reference.
    pub fn model(&self) -> &TypeModel {
        self.model
    }
}

/// Analyze a summary Type and check if it's synthesizable.
pub fn is_type_synthesizable(ty: &summary::Type) -> bool {
    match ty {
        // Primitives are always synthesizable
        summary::Type::Bool
        | summary::Type::U8
        | summary::Type::U16
        | summary::Type::U32
        | summary::Type::U64
        | summary::Type::U128
        | summary::Type::U256
        | summary::Type::Address => true,

        // Signer is not synthesizable in our sandbox
        summary::Type::Signer => false,

        // Vectors are synthesizable if their element type is
        summary::Type::Vector(inner) => is_type_synthesizable(inner),

        // References are not directly synthesizable
        summary::Type::Reference(_, _) => false,

        // Type parameters depend on instantiation - conservatively say yes
        // (the concrete type at instantiation determines synthesizability)
        summary::Type::TypeParameter(_) | summary::Type::NamedTypeParameter(_) => true,

        // Tuples: all elements must be synthesizable
        summary::Type::Tuple(elems) => elems.iter().all(is_type_synthesizable),

        // Function types are not synthesizable
        summary::Type::Fun(_, _) => false,

        // Any type - unknown, say no
        summary::Type::Any => false,

        // Datatypes (structs/enums) - need constructor lookup
        // For now, assume they might be synthesizable
        summary::Type::Datatype(_) => true,
    }
}

/// Check if a summary Type is an object type (has key ability potential).
pub fn is_potential_object_type(ty: &summary::Type) -> bool {
    match ty {
        summary::Type::Datatype(dt) => {
            // Check common Sui object patterns
            let module_name = dt.module.name.as_str();
            let type_name = dt.name.as_str();

            module_name == "object"
                || type_name == "Coin"
                || type_name == "UID"
                || type_name.ends_with("Cap")
        }
        summary::Type::Reference(_, inner) => is_potential_object_type(inner),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_synthesizable_type_str() {
        // This test doesn't need a real model for the basic type string checks
    }

    #[test]
    fn test_is_type_synthesizable_primitives() {
        assert!(is_type_synthesizable(&summary::Type::Bool));
        assert!(is_type_synthesizable(&summary::Type::U64));
        assert!(is_type_synthesizable(&summary::Type::Address));
        assert!(!is_type_synthesizable(&summary::Type::Signer));
    }

    #[test]
    fn test_is_type_synthesizable_vector() {
        let vec_u8 = summary::Type::Vector(Box::new(summary::Type::U8));
        assert!(is_type_synthesizable(&vec_u8));

        let vec_signer = summary::Type::Vector(Box::new(summary::Type::Signer));
        assert!(!is_type_synthesizable(&vec_signer));
    }

    #[test]
    fn test_is_type_synthesizable_reference() {
        let ref_u64 = summary::Type::Reference(false, Box::new(summary::Type::U64));
        assert!(!is_type_synthesizable(&ref_u64));
    }
}
