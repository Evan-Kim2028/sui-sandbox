//! Phase 1: Resolution
//!
//! This phase handles loading bytecode and verifying that target functions exist.

use crate::errors::{ErrorCode, Failure, FailureContext};
use crate::mm2::{TypeModel, TypeValidator};
use crate::resolver::LocalModuleResolver;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;

/// Context built during resolution phase.
///
/// Contains target function info and type model.
/// Note: Does not own the resolver - caller manages its lifetime.
pub struct ResolutionContext {
    /// The type model built from all loaded modules
    pub type_model: TypeModel,
    /// Target function's module address
    pub target_module_addr: AccountAddress,
    /// Target function's module name
    pub target_module_name: String,
    /// Target function name
    pub target_function_name: String,
}

impl ResolutionContext {
    /// Create a type validator from this context.
    pub fn type_validator(&self) -> TypeValidator<'_> {
        TypeValidator::new(&self.type_model)
    }
}

/// Configuration for the resolution phase.
pub struct ResolutionConfig<'a> {
    /// The module resolver with loaded bytecode
    pub resolver: &'a LocalModuleResolver,
    /// Target module address
    pub module_addr: AccountAddress,
    /// Target module name
    pub module_name: &'a str,
    /// Target function name
    pub function_name: &'a str,
}

/// Resolve and validate the target function exists.
///
/// This is Phase 1 of the pipeline. It:
/// 1. Verifies the target module exists in the resolver
/// 2. Builds an MM2 TypeModel from all loaded modules
/// 3. Validates the target function exists and is callable
///
/// # Arguments
/// * `config` - Resolution configuration
///
/// # Returns
/// A ResolutionContext containing the type model and target info,
/// or a Phase 1 error (E101, E102, E103).
pub fn resolve(config: ResolutionConfig<'_>) -> Result<ResolutionContext, Failure> {
    // Check module exists
    let module_id = ModuleId::new(
        config.module_addr,
        Identifier::new(config.module_name).map_err(|e| {
            Failure::new(
                ErrorCode::ModuleNotFound,
                format!("Invalid module name '{}': {}", config.module_name, e),
            )
        })?,
    );

    // Verify module is loaded
    if config.resolver.get_module_struct(&module_id).is_none() {
        return Err(Failure::with_context(
            ErrorCode::ModuleNotFound,
            format!("Module not found: {}", module_id),
            FailureContext {
                module: Some(format!("{}", module_id)),
                function: None,
                type_name: None,
                param_index: None,
            },
        ));
    }

    // Build type model from all modules
    let all_modules: Vec<CompiledModule> = config.resolver.iter_modules().cloned().collect();
    let type_model = TypeModel::from_modules(all_modules)?;

    // Validate function exists using type model
    let validator = TypeValidator::new(&type_model);
    let sig = validator.validate_function_exists(
        &config.module_addr,
        config.module_name,
        config.function_name,
    )?;

    // Check function is callable (public or entry)
    if !sig.is_public && !sig.is_entry {
        return Err(Failure::with_context(
            ErrorCode::NotCallable,
            format!(
                "Function {}::{}::{} is not public or entry",
                config.module_addr, config.module_name, config.function_name
            ),
            FailureContext {
                module: Some(format!("{}::{}", config.module_addr, config.module_name)),
                function: Some(config.function_name.to_string()),
                type_name: None,
                param_index: None,
            },
        ));
    }

    Ok(ResolutionContext {
        type_model,
        target_module_addr: config.module_addr,
        target_module_name: config.module_name.to_string(),
        target_function_name: config.function_name.to_string(),
    })
}

/// Quick check if a function exists without building full context.
///
/// Useful for filtering functions before full resolution.
pub fn function_exists(
    resolver: &LocalModuleResolver,
    module_addr: &AccountAddress,
    module_name: &str,
    function_name: &str,
) -> bool {
    let module_id = match Identifier::new(module_name) {
        Ok(ident) => ModuleId::new(*module_addr, ident),
        Err(_) => return false,
    };

    if let Some(module) = resolver.get_module_struct(&module_id) {
        // Check if function exists in the module
        let func_ident = match Identifier::new(function_name) {
            Ok(ident) => ident,
            Err(_) => return false,
        };

        module.function_defs().iter().any(|fd| {
            module.identifier_at(module.function_handle_at(fd.function).name)
                == func_ident.as_ident_str()
        })
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    // Tests would require fixture modules
}
