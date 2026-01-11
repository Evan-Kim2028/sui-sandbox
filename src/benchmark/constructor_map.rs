//! Constructor Map: Finds functions that can construct struct types.
//!
//! For constructor chaining, we need to know which functions return which struct types
//! and whether their parameters are synthesizable (primitives, TxContext, or other
//! constructable structs).
//!
//! Also tracks One-Time Witness (OTW) types that can be used with coin::create_currency.

use move_binary_format::file_format::{
    Ability, CompiledModule, SignatureToken, Visibility,
};
use move_core_types::language_storage::{ModuleId, StructTag, TypeTag};
use move_core_types::account_address::AccountAddress;
use std::collections::HashMap;

/// Information about a constructor function
#[derive(Debug, Clone)]
pub struct ConstructorInfo {
    pub module_id: ModuleId,
    pub function_name: String,
    pub type_params: usize,
    pub params: Vec<ParamKind>,
    pub returns: StructTag,
}

/// What kind of parameter a constructor needs
#[derive(Debug, Clone)]
pub enum ParamKind {
    /// A primitive type (u8, u64, bool, address, etc.)
    Primitive(TypeTag),
    /// A vector of primitives
    PrimitiveVector(TypeTag),
    /// A reference to TxContext (&mut TxContext)
    TxContext,
    /// A reference to Clock (&Clock)
    Clock,
    /// A struct type that needs its own constructor
    Struct(StructTag),
    /// A type parameter (T) - will be instantiated with u64
    TypeParam(u16),
    /// Unsupported parameter type
    Unsupported(String),
}

/// Information about a One-Time Witness type
#[derive(Debug, Clone)]
pub struct OtwInfo {
    pub module_id: ModuleId,
    pub struct_name: String,
    pub type_tag: TypeTag,
}

/// Map from struct types to their constructors
pub struct ConstructorMap {
    /// StructTag (without type args) -> list of constructors
    constructors: HashMap<String, Vec<ConstructorInfo>>,
    /// OTW types found in modules (can be used with coin::create_currency)
    otw_types: Vec<OtwInfo>,
}

impl ConstructorMap {
    /// Build a constructor map from a set of modules
    pub fn from_modules(modules: &[CompiledModule]) -> Self {
        let mut constructors: HashMap<String, Vec<ConstructorInfo>> = HashMap::new();
        
        for module in modules {
            let module_id = module.self_id();
            
            for func_def in &module.function_defs {
                // Skip private functions - we can only call public ones
                if func_def.visibility == Visibility::Private {
                    continue;
                }
                
                let handle = module.function_handle_at(func_def.function);
                let func_name = module.identifier_at(handle.name).to_string();
                
                // Get return signature
                let return_sig = module.signature_at(handle.return_);
                if return_sig.0.is_empty() {
                    continue; // No return value
                }
                
                // Check if first return is a struct from this package
                let first_return = &return_sig.0[0];
                let struct_tag = match token_to_struct_tag(first_return, module) {
                    Some(tag) => tag,
                    None => continue, // Not a struct return
                };
                
                // Analyze parameters
                let params_sig = module.signature_at(handle.parameters);
                let params: Vec<ParamKind> = params_sig.0.iter()
                    .map(|token| classify_param(token, module))
                    .collect();
                
                // Create constructor info
                let info = ConstructorInfo {
                    module_id: module_id.clone(),
                    function_name: func_name,
                    type_params: handle.type_parameters.len(),
                    params,
                    returns: struct_tag.clone(),
                };
                
                // Key by struct name (module::name) without type args
                let key = format!("{}::{}", struct_tag.module_id(), struct_tag.name);
                constructors.entry(key).or_default().push(info);
            }
        }
        
        // Scan for OTW types
        let otw_types = Self::find_otw_types(modules);
        
        ConstructorMap { constructors, otw_types }
    }
    
    /// Find all One-Time Witness types in the modules.
    /// OTW requirements: name == UPPERCASE(module_name), one bool field, only drop ability
    fn find_otw_types(modules: &[CompiledModule]) -> Vec<OtwInfo> {
        let mut otw_types = Vec::new();
        
        for module in modules {
            let module_id = module.self_id();
            let module_name = module_id.name().as_str();
            let expected_otw_name = module_name.to_ascii_uppercase();
            
            for struct_def in &module.struct_defs {
                let struct_handle = module.datatype_handle_at(struct_def.struct_handle);
                let struct_name = module.identifier_at(struct_handle.name).to_string();
                
                // Check if name matches OTW pattern
                if struct_name != expected_otw_name {
                    continue;
                }
                
                // Check abilities: must have only drop
                let abilities = struct_handle.abilities;
                let drop_only = move_binary_format::file_format::AbilitySet::singleton(Ability::Drop);
                if abilities != drop_only {
                    continue;
                }
                
                // Check fields: must have exactly one bool field
                let field_count = match struct_def.declared_field_count() {
                    Ok(count) => count,
                    Err(_) => continue, // Native struct
                };
                
                if field_count != 1 {
                    continue;
                }
                
                // Check if the single field is bool
                if let Some(field) = struct_def.field(0) {
                    if field.signature.0 != SignatureToken::Bool {
                        continue;
                    }
                } else {
                    continue;
                }
                
                // This is a valid OTW type!
                let type_tag = TypeTag::Struct(Box::new(StructTag {
                    address: *module_id.address(),
                    module: module_id.name().to_owned(),
                    name: move_core_types::identifier::Identifier::new(struct_name.clone()).unwrap(),
                    type_params: vec![],
                }));
                
                otw_types.push(OtwInfo {
                    module_id: module_id.clone(),
                    struct_name,
                    type_tag,
                });
            }
        }
        
        otw_types
    }
    
    /// Find constructors for a struct type
    pub fn find_constructors(&self, struct_tag: &StructTag) -> Option<&Vec<ConstructorInfo>> {
        let key = format!("{}::{}", struct_tag.module_id(), struct_tag.name);
        self.constructors.get(&key)
    }
    
    /// Find a constructor that can be synthesized with only primitives and TxContext
    pub fn find_synthesizable_constructor(&self, struct_tag: &StructTag) -> Option<&ConstructorInfo> {
        let ctors = self.find_constructors(struct_tag)?;
        
        ctors.iter().find(|ctor| {
            ctor.params.iter().all(|p| matches!(p, 
                ParamKind::Primitive(_) | 
                ParamKind::PrimitiveVector(_) |
                ParamKind::TxContext | 
                ParamKind::Clock |
                ParamKind::TypeParam(_)
            ))
        })
    }
    
    /// Find a constructor that needs only one level of struct construction
    pub fn find_single_hop_constructor(&self, struct_tag: &StructTag) -> Option<(&ConstructorInfo, Vec<&ConstructorInfo>)> {
        let ctors = self.find_constructors(struct_tag)?;
        
        for ctor in ctors {
            let mut dependencies = Vec::new();
            let mut all_resolvable = true;
            
            for param in &ctor.params {
                match param {
                    ParamKind::Primitive(_) | 
                    ParamKind::PrimitiveVector(_) |
                    ParamKind::TxContext | 
                    ParamKind::Clock |
                    ParamKind::TypeParam(_) => {
                        // These are directly synthesizable
                    }
                    ParamKind::Struct(dep_tag) => {
                        // Check if this dependency has a synthesizable constructor
                        if let Some(dep_ctor) = self.find_synthesizable_constructor(dep_tag) {
                            dependencies.push(dep_ctor);
                        } else {
                            all_resolvable = false;
                            break;
                        }
                    }
                    ParamKind::Unsupported(_) => {
                        all_resolvable = false;
                        break;
                    }
                }
            }
            
            if all_resolvable {
                return Some((ctor, dependencies));
            }
        }
        
        None
    }
    
    /// Get all constructors (for debugging)
    pub fn all_constructors(&self) -> impl Iterator<Item = (&String, &Vec<ConstructorInfo>)> {
        self.constructors.iter()
    }
    
    /// Get all OTW types found in modules
    pub fn get_otw_types(&self) -> &[OtwInfo] {
        &self.otw_types
    }
    
    /// Get the first available OTW type (if any)
    pub fn get_first_otw(&self) -> Option<&OtwInfo> {
        self.otw_types.first()
    }
    
    /// Check if a struct is TreasuryCap from sui::coin
    pub fn is_treasury_cap(struct_tag: &StructTag) -> bool {
        struct_tag.address == AccountAddress::TWO 
            && struct_tag.module.as_str() == "coin"
            && struct_tag.name.as_str() == "TreasuryCap"
    }
}

/// Convert a signature token to a StructTag if it's a struct
fn token_to_struct_tag(token: &SignatureToken, module: &CompiledModule) -> Option<StructTag> {
    match token {
        SignatureToken::Datatype(idx) => {
            let handle = module.datatype_handle_at(*idx);
            let module_handle = module.module_handle_at(handle.module);
            let address = *module.address_identifier_at(module_handle.address);
            let module_name = module.identifier_at(module_handle.name).to_owned();
            let name = module.identifier_at(handle.name).to_owned();
            
            Some(StructTag {
                address,
                module: module_name,
                name,
                type_params: vec![],
            })
        }
        SignatureToken::DatatypeInstantiation(inner) => {
            let (idx, type_args) = inner.as_ref();
            let handle = module.datatype_handle_at(*idx);
            let module_handle = module.module_handle_at(handle.module);
            let address = *module.address_identifier_at(module_handle.address);
            let module_name = module.identifier_at(module_handle.name).to_owned();
            let name = module.identifier_at(handle.name).to_owned();
            
            // Convert type args (simplified - just mark as having type args)
            let converted_type_args: Vec<TypeTag> = type_args.iter()
                .filter_map(|t| token_to_type_tag(t, module))
                .collect();
            
            Some(StructTag {
                address,
                module: module_name,
                name,
                type_params: converted_type_args,
            })
        }
        _ => None,
    }
}

/// Convert a signature token to a TypeTag
fn token_to_type_tag(token: &SignatureToken, module: &CompiledModule) -> Option<TypeTag> {
    match token {
        SignatureToken::Bool => Some(TypeTag::Bool),
        SignatureToken::U8 => Some(TypeTag::U8),
        SignatureToken::U16 => Some(TypeTag::U16),
        SignatureToken::U32 => Some(TypeTag::U32),
        SignatureToken::U64 => Some(TypeTag::U64),
        SignatureToken::U128 => Some(TypeTag::U128),
        SignatureToken::U256 => Some(TypeTag::U256),
        SignatureToken::Address => Some(TypeTag::Address),
        SignatureToken::Signer => Some(TypeTag::Signer),
        SignatureToken::Vector(inner) => {
            token_to_type_tag(inner, module).map(|t| TypeTag::Vector(Box::new(t)))
        }
        SignatureToken::Datatype(_) | SignatureToken::DatatypeInstantiation(_) => {
            token_to_struct_tag(token, module).map(|s| TypeTag::Struct(Box::new(s)))
        }
        SignatureToken::TypeParameter(_idx) => {
            // Type parameter - we'll instantiate with u64
            Some(TypeTag::U64)
        }
        _ => None,
    }
}

/// Classify a parameter token
fn classify_param(token: &SignatureToken, module: &CompiledModule) -> ParamKind {
    match token {
        // Primitives
        SignatureToken::Bool => ParamKind::Primitive(TypeTag::Bool),
        SignatureToken::U8 => ParamKind::Primitive(TypeTag::U8),
        SignatureToken::U16 => ParamKind::Primitive(TypeTag::U16),
        SignatureToken::U32 => ParamKind::Primitive(TypeTag::U32),
        SignatureToken::U64 => ParamKind::Primitive(TypeTag::U64),
        SignatureToken::U128 => ParamKind::Primitive(TypeTag::U128),
        SignatureToken::U256 => ParamKind::Primitive(TypeTag::U256),
        SignatureToken::Address => ParamKind::Primitive(TypeTag::Address),
        
        // Vectors of primitives
        SignatureToken::Vector(inner) => {
            if let Some(tag) = token_to_type_tag(inner, module) {
                if matches!(tag, TypeTag::Bool | TypeTag::U8 | TypeTag::U16 | TypeTag::U32 | 
                           TypeTag::U64 | TypeTag::U128 | TypeTag::U256 | TypeTag::Address) {
                    return ParamKind::PrimitiveVector(tag);
                }
            }
            ParamKind::Unsupported("complex vector".to_string())
        }
        
        // Type parameters
        SignatureToken::TypeParameter(idx) => ParamKind::TypeParam(*idx),
        
        // References - check for TxContext and Clock
        SignatureToken::Reference(inner) | SignatureToken::MutableReference(inner) => {
            if let Some(struct_tag) = token_to_struct_tag(inner, module) {
                let full_name = format!("{}::{}::{}", 
                    struct_tag.address.to_hex_literal(),
                    struct_tag.module,
                    struct_tag.name
                );
                if full_name.ends_with("::tx_context::TxContext") {
                    return ParamKind::TxContext;
                }
                if full_name.ends_with("::clock::Clock") {
                    return ParamKind::Clock;
                }
                // Other object references are unsupported for now
                return ParamKind::Unsupported(format!("object ref: {}", struct_tag.name));
            }
            ParamKind::Unsupported("unknown reference".to_string())
        }
        
        // Struct types (by value)
        SignatureToken::Datatype(_) | SignatureToken::DatatypeInstantiation(_) => {
            if let Some(struct_tag) = token_to_struct_tag(token, module) {
                ParamKind::Struct(struct_tag)
            } else {
                ParamKind::Unsupported("unknown struct".to_string())
            }
        }
        
        _ => ParamKind::Unsupported(format!("{:?}", token)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_constructor_map_empty() {
        let map = ConstructorMap::from_modules(&[]);
        assert!(map.constructors.is_empty());
    }
}
