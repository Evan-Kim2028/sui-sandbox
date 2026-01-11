//! Phase 2: Type Check
//!
//! This phase performs static type validation using MM2.

use crate::benchmark::errors::{ErrorCode, Failure};
use crate::benchmark::mm2::type_validator::{is_potential_object_type, is_type_synthesizable};
use crate::benchmark::phases::ResolutionContext;
use move_model_2::summary;

/// Result of type checking a function.
#[derive(Debug)]
pub struct TypeCheckResult {
    /// Number of type parameters expected
    pub type_param_count: usize,
    /// Number of parameters
    pub param_count: usize,
    /// Which parameters might be object types
    pub object_param_indices: Vec<usize>,
    /// Which parameters are unsynthesizable
    pub unsynthesizable_indices: Vec<usize>,
    /// Parameter type descriptions
    pub param_types: Vec<String>,
}

/// Perform static type checking on a resolved function.
///
/// This is Phase 2 of the pipeline. It:
/// 1. Validates function signature
/// 2. Identifies object parameters (need special handling)
/// 3. Checks parameter synthesizability
///
/// # Arguments
/// * `ctx` - Resolution context from Phase 1
///
/// # Returns
/// TypeCheckResult with validation info, or a Phase 2 error (E201-E205).
pub fn validate(ctx: &ResolutionContext) -> Result<TypeCheckResult, Failure> {
    let validator = ctx.type_validator();

    // Get function signature
    let sig = validator.validate_function_exists(
        &ctx.target_module_addr,
        &ctx.target_module_name,
        &ctx.target_function_name,
    )?;

    // Analyze parameters
    let mut object_param_indices = Vec::new();
    let mut unsynthesizable_indices = Vec::new();
    let mut param_types = Vec::new();

    for (i, param) in sig.parameters.iter().enumerate() {
        param_types.push(param.type_str.clone());

        // Check if this might be an object parameter
        if validator.might_be_object_type(&param.type_str) {
            object_param_indices.push(i);
        }

        // Check basic synthesizability from the type string
        if !validator.is_synthesizable_type_str(&param.type_str) {
            unsynthesizable_indices.push(i);
        }
    }

    Ok(TypeCheckResult {
        type_param_count: sig.type_parameters.len(),
        param_count: sig.parameters.len(),
        object_param_indices,
        unsynthesizable_indices,
        param_types,
    })
}

/// Validate type arguments for a generic function.
///
/// # Arguments
/// * `ctx` - Resolution context
/// * `type_arg_count` - Number of type arguments provided
///
/// # Returns
/// Ok if valid, or E203 error if count mismatch.
pub fn validate_type_args(ctx: &ResolutionContext, type_arg_count: usize) -> Result<(), Failure> {
    let validator = ctx.type_validator();

    validator.validate_call(
        &ctx.target_module_addr,
        &ctx.target_module_name,
        &ctx.target_function_name,
        type_arg_count,
    )?;

    Ok(())
}

/// Check if all parameters can be synthesized.
///
/// # Arguments
/// * `result` - TypeCheckResult from validate()
///
/// # Returns
/// Ok if all params are synthesizable, or E303 error listing problematic params.
pub fn check_synthesizability(result: &TypeCheckResult) -> Result<(), Failure> {
    if result.unsynthesizable_indices.is_empty() {
        return Ok(());
    }

    let problematic: Vec<String> = result
        .unsynthesizable_indices
        .iter()
        .map(|&i| format!("param {} ({})", i, result.param_types.get(i).map(|s| s.as_str()).unwrap_or("?")))
        .collect();

    Err(Failure::new(
        ErrorCode::UnsupportedConstructorParam,
        format!(
            "Cannot synthesize values for parameters: {}",
            problematic.join(", ")
        ),
    ))
}

/// Analyze parameter types directly from the summary.
///
/// This provides more detailed analysis when you have access to the full
/// type information (not just formatted strings).
pub fn analyze_param_type(ty: &summary::Type) -> ParamAnalysis {
    ParamAnalysis {
        is_synthesizable: is_type_synthesizable(ty),
        is_object: is_potential_object_type(ty),
        is_reference: matches!(ty, summary::Type::Reference(_, _)),
        is_mutable_ref: matches!(ty, summary::Type::Reference(true, _)),
        type_desc: crate::benchmark::mm2::model::format_type(ty),
    }
}

/// Analysis of a parameter type.
#[derive(Debug, Clone)]
pub struct ParamAnalysis {
    /// Can this type be synthesized from primitives?
    pub is_synthesizable: bool,
    /// Is this potentially an object type (has key ability)?
    pub is_object: bool,
    /// Is this a reference type?
    pub is_reference: bool,
    /// Is this a mutable reference?
    pub is_mutable_ref: bool,
    /// Human-readable type description
    pub type_desc: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_synthesizability_all_ok() {
        let result = TypeCheckResult {
            type_param_count: 0,
            param_count: 2,
            object_param_indices: vec![],
            unsynthesizable_indices: vec![],
            param_types: vec!["u64".to_string(), "bool".to_string()],
        };

        assert!(check_synthesizability(&result).is_ok());
    }

    #[test]
    fn test_check_synthesizability_has_unsynthesizable() {
        let result = TypeCheckResult {
            type_param_count: 0,
            param_count: 2,
            object_param_indices: vec![],
            unsynthesizable_indices: vec![1],
            param_types: vec!["u64".to_string(), "signer".to_string()],
        };

        let err = check_synthesizability(&result).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedConstructorParam);
    }
}
