//! Constructor Graph: Enhanced constructor discovery using MM2.
//!
//! This module provides an MM2-based approach to finding constructors,
//! with better type analysis than the bytecode-only approach.

use crate::benchmark::mm2::model::{FunctionSignature, TypeModel};
use move_core_types::account_address::AccountAddress;
use move_model_2::summary::{self, Ability};
use std::collections::{BTreeSet, HashMap, VecDeque};

/// Maximum depth for constructor chain resolution
const MAX_CHAIN_DEPTH: usize = 5;

/// A node in the constructor graph representing a type and how to construct it.
#[derive(Debug, Clone)]
pub struct TypeNode {
    /// Module address
    pub module_addr: AccountAddress,
    /// Module name
    pub module_name: String,
    /// Type name
    pub type_name: String,
    /// Abilities of this type
    pub abilities: summary::AbilitySet,
    /// Available constructors for this type
    pub constructors: Vec<Constructor>,
    /// Whether this is a primitive type
    pub is_primitive: bool,
}

/// A constructor function that produces a type.
#[derive(Debug, Clone)]
pub struct Constructor {
    /// Module containing the constructor
    pub module_addr: AccountAddress,
    pub module_name: String,
    /// Function name
    pub function_name: String,
    /// Parameter requirements
    pub params: Vec<ParamRequirement>,
    /// Number of type parameters
    pub type_param_count: usize,
    /// Is this an entry function?
    pub is_entry: bool,
    /// Synthesis complexity score (lower = easier to construct)
    pub complexity: usize,
}

/// What a constructor parameter requires.
#[derive(Debug, Clone)]
pub enum ParamRequirement {
    /// A primitive value (can synthesize directly)
    Primitive(String),
    /// A vector of a synthesizable type
    Vector(Box<ParamRequirement>),
    /// A TxContext reference
    TxContext,
    /// A Clock reference
    Clock,
    /// A type that needs its own constructor
    Type {
        module_addr: AccountAddress,
        module_name: String,
        type_name: String,
    },
    /// A type parameter (will be instantiated)
    TypeParam(u16),
    /// Reference to another type
    Reference {
        is_mut: bool,
        inner: Box<ParamRequirement>,
    },
    /// Unsupported parameter
    Unsupported(String),
}

impl ParamRequirement {
    /// Check if this parameter can be directly synthesized.
    pub fn is_synthesizable(&self) -> bool {
        match self {
            ParamRequirement::Primitive(_) => true,
            ParamRequirement::Vector(inner) => inner.is_synthesizable(),
            ParamRequirement::TxContext => true,
            ParamRequirement::Clock => true,
            ParamRequirement::TypeParam(_) => true, // Will be instantiated with u64
            ParamRequirement::Reference { inner, .. } => {
                // References to TxContext/Clock are synthesizable
                matches!(
                    inner.as_ref(),
                    ParamRequirement::TxContext | ParamRequirement::Clock
                )
            }
            ParamRequirement::Type { .. } => false, // Needs constructor
            ParamRequirement::Unsupported(_) => false,
        }
    }
}

/// Constructor graph built from MM2 model.
pub struct ConstructorGraph {
    /// Type nodes indexed by "addr::module::name"
    types: HashMap<String, TypeNode>,
    /// Cache of resolved constructor chains
    chain_cache: HashMap<String, Option<Vec<Constructor>>>,
}

impl ConstructorGraph {
    /// Build a constructor graph from an MM2 TypeModel.
    pub fn from_model(model: &TypeModel) -> Self {
        let mut types = HashMap::new();

        // Process all modules
        for (addr, module_name) in model.modules() {
            // Process structs in this module
            for struct_name in model.structs_in_module(&addr, &module_name) {
                if let Some(struct_info) = model.get_struct(&addr, &module_name, &struct_name) {
                    let key = format!("{}::{}::{}", addr, module_name, struct_name);

                    let node = TypeNode {
                        module_addr: addr,
                        module_name: module_name.clone(),
                        type_name: struct_name.clone(),
                        abilities: struct_info.abilities.clone(),
                        constructors: Vec::new(),
                        is_primitive: false,
                    };

                    types.insert(key, node);
                }
            }
        }

        // Now find constructors for each type
        for (addr, module_name) in model.modules() {
            for func_name in model.functions_in_module(&addr, &module_name) {
                if let Some(sig) = model.get_function(&addr, &module_name, &func_name) {
                    // Only consider public/entry functions
                    if !sig.is_public && !sig.is_entry {
                        continue;
                    }

                    // Check if this function returns a struct type
                    if sig.return_count == 0 {
                        continue;
                    }

                    // Analyze the function to see what type it constructs
                    // We use the function name and parameter patterns to infer this
                    if let Some(ctor) = Self::analyze_constructor(&sig, model) {
                        let _return_type_key = format!(
                            "{}::{}::{}",
                            ctor.module_addr, ctor.module_name, ctor.function_name
                        );

                        // Find the target type this constructor produces
                        // This is heuristic - we look for the struct this function likely creates
                        if let Some(target_key) = Self::infer_return_type(&sig, &addr, &module_name) {
                            if let Some(node) = types.get_mut(&target_key) {
                                node.constructors.push(ctor);
                            }
                        }
                    }
                }
            }
        }

        // Sort constructors by complexity
        for node in types.values_mut() {
            node.constructors.sort_by_key(|c| c.complexity);
        }

        ConstructorGraph {
            types,
            chain_cache: HashMap::new(),
        }
    }

    /// Analyze a function to determine if it's a constructor and what params it needs.
    fn analyze_constructor(sig: &FunctionSignature, _model: &TypeModel) -> Option<Constructor> {
        let mut params = Vec::new();
        let mut complexity = 0;

        for param in &sig.parameters {
            let req = Self::classify_param(&param.type_str);
            if !req.is_synthesizable() {
                complexity += 10; // Non-synthesizable params increase complexity
            }
            params.push(req);
        }

        // Add base complexity for type parameters
        complexity += sig.type_parameters.len() * 2;

        Some(Constructor {
            module_addr: sig.module_addr,
            module_name: sig.module_name.clone(),
            function_name: sig.name.clone(),
            params,
            type_param_count: sig.type_parameters.len(),
            is_entry: sig.is_entry,
            complexity,
        })
    }

    /// Classify a parameter type string into a ParamRequirement.
    fn classify_param(type_str: &str) -> ParamRequirement {
        match type_str {
            "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address" => {
                ParamRequirement::Primitive(type_str.to_string())
            }
            s if s.starts_with("vector<") => {
                if let Some(inner) = s.strip_prefix("vector<").and_then(|s| s.strip_suffix('>')) {
                    ParamRequirement::Vector(Box::new(Self::classify_param(inner)))
                } else {
                    ParamRequirement::Unsupported(type_str.to_string())
                }
            }
            s if s.starts_with("&mut ") => {
                let inner = &s[5..];
                if inner.contains("tx_context::TxContext") {
                    ParamRequirement::Reference {
                        is_mut: true,
                        inner: Box::new(ParamRequirement::TxContext),
                    }
                } else if inner.contains("clock::Clock") {
                    ParamRequirement::Reference {
                        is_mut: true,
                        inner: Box::new(ParamRequirement::Clock),
                    }
                } else {
                    ParamRequirement::Reference {
                        is_mut: true,
                        inner: Box::new(Self::classify_param(inner)),
                    }
                }
            }
            s if s.starts_with('&') => {
                let inner = &s[1..];
                if inner.contains("tx_context::TxContext") {
                    ParamRequirement::Reference {
                        is_mut: false,
                        inner: Box::new(ParamRequirement::TxContext),
                    }
                } else if inner.contains("clock::Clock") {
                    ParamRequirement::Reference {
                        is_mut: false,
                        inner: Box::new(ParamRequirement::Clock),
                    }
                } else {
                    ParamRequirement::Reference {
                        is_mut: false,
                        inner: Box::new(Self::classify_param(inner)),
                    }
                }
            }
            s if s.starts_with('T') && s.len() <= 3 => {
                // Type parameter like T0, T1
                let idx = s[1..].parse::<u16>().unwrap_or(0);
                ParamRequirement::TypeParam(idx)
            }
            s if s.contains("::") => {
                // Struct type like "0x2::coin::Coin"
                let parts: Vec<&str> = s.split("::").collect();
                if parts.len() >= 3 {
                    // Try to parse the address
                    if let Ok(addr) = AccountAddress::from_hex_literal(parts[0]) {
                        ParamRequirement::Type {
                            module_addr: addr,
                            module_name: parts[1].to_string(),
                            type_name: parts[2].split('<').next().unwrap_or(parts[2]).to_string(),
                        }
                    } else {
                        ParamRequirement::Unsupported(type_str.to_string())
                    }
                } else {
                    ParamRequirement::Unsupported(type_str.to_string())
                }
            }
            _ => ParamRequirement::Unsupported(type_str.to_string()),
        }
    }

    /// Infer the return type key for a function (heuristic).
    fn infer_return_type(
        sig: &FunctionSignature,
        addr: &AccountAddress,
        module_name: &str,
    ) -> Option<String> {
        // Common patterns:
        // - new_foo() returns Foo from same module
        // - create_foo() returns Foo from same module
        // - init() might return the module's main type

        let func_name = &sig.name;

        // Try to extract type name from function name
        let type_name = if func_name.starts_with("new_") {
            Some(to_pascal_case(&func_name[4..]))
        } else if func_name.starts_with("create_") {
            Some(to_pascal_case(&func_name[7..]))
        } else if func_name == "new" || func_name == "create" || func_name == "init" {
            // Return module name as type name
            Some(to_pascal_case(module_name))
        } else {
            None
        };

        type_name.map(|tn| format!("{}::{}::{}", addr, module_name, tn))
    }

    /// Find a constructor chain that can synthesize a given type.
    pub fn find_chain(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
    ) -> Option<Vec<Constructor>> {
        let key = format!("{}::{}::{}", module_addr, module_name, type_name);

        // Check cache
        if let Some(cached) = self.chain_cache.get(&key) {
            return cached.clone();
        }

        // BFS to find shortest constructor chain
        let result = self.bfs_find_chain(&key, MAX_CHAIN_DEPTH);

        // Cache result
        self.chain_cache.insert(key, result.clone());

        result
    }

    /// BFS to find constructor chain.
    fn bfs_find_chain(&self, target_key: &str, max_depth: usize) -> Option<Vec<Constructor>> {
        let target_node = self.types.get(target_key)?;

        // Try direct constructors first
        for ctor in &target_node.constructors {
            if ctor.params.iter().all(|p| p.is_synthesizable()) {
                return Some(vec![ctor.clone()]);
            }
        }

        // BFS for multi-hop chains
        let mut queue: VecDeque<(String, Vec<Constructor>, usize)> = VecDeque::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();

        queue.push_back((target_key.to_string(), Vec::new(), 0));
        visited.insert(target_key.to_string());

        while let Some((current_key, chain, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            let Some(node) = self.types.get(&current_key) else {
                continue;
            };

            for ctor in &node.constructors {
                let mut new_chain = chain.clone();
                new_chain.push(ctor.clone());

                // Check if all params are now resolvable
                let mut all_resolvable = true;
                let mut unresolved_types = Vec::new();

                for param in &ctor.params {
                    if !param.is_synthesizable() {
                        if let ParamRequirement::Type {
                            module_addr,
                            module_name,
                            type_name,
                        } = param
                        {
                            let dep_key =
                                format!("{}::{}::{}", module_addr, module_name, type_name);
                            if !visited.contains(&dep_key) {
                                unresolved_types.push(dep_key);
                            } else {
                                // Already visited but not resolved - circular dep
                                all_resolvable = false;
                                break;
                            }
                        } else {
                            all_resolvable = false;
                            break;
                        }
                    }
                }

                if all_resolvable && unresolved_types.is_empty() {
                    // Found a complete chain!
                    return Some(new_chain);
                }

                // Add unresolved types to queue
                for dep_key in unresolved_types {
                    if !visited.contains(&dep_key) {
                        visited.insert(dep_key.clone());
                        queue.push_back((dep_key, new_chain.clone(), depth + 1));
                    }
                }
            }
        }

        None
    }

    /// Get a type node by key.
    pub fn get_type(&self, key: &str) -> Option<&TypeNode> {
        self.types.get(key)
    }

    /// Check if a type has the key ability (is an object).
    pub fn is_object_type(&self, module_addr: &AccountAddress, module_name: &str, type_name: &str) -> bool {
        let key = format!("{}::{}::{}", module_addr, module_name, type_name);
        self.types
            .get(&key)
            .map(|n| n.abilities.0.contains(&Ability::Key))
            .unwrap_or(false)
    }

    /// Get all types in the graph.
    pub fn all_types(&self) -> impl Iterator<Item = &TypeNode> {
        self.types.values()
    }

    /// Get statistics about the constructor graph.
    pub fn stats(&self) -> ConstructorGraphStats {
        let total_types = self.types.len();
        let types_with_constructors = self.types.values().filter(|n| !n.constructors.is_empty()).count();
        let total_constructors: usize = self.types.values().map(|n| n.constructors.len()).sum();
        let object_types = self.types.values().filter(|n| n.abilities.0.contains(&Ability::Key)).count();

        ConstructorGraphStats {
            total_types,
            types_with_constructors,
            total_constructors,
            object_types,
        }
    }
}

/// Statistics about the constructor graph.
#[derive(Debug, Clone)]
pub struct ConstructorGraphStats {
    pub total_types: usize,
    pub types_with_constructors: usize,
    pub total_constructors: usize,
    pub object_types: usize,
}

/// Convert snake_case to PascalCase.
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_param_primitives() {
        assert!(matches!(
            ConstructorGraph::classify_param("u64"),
            ParamRequirement::Primitive(_)
        ));
        assert!(matches!(
            ConstructorGraph::classify_param("bool"),
            ParamRequirement::Primitive(_)
        ));
        assert!(matches!(
            ConstructorGraph::classify_param("address"),
            ParamRequirement::Primitive(_)
        ));
    }

    #[test]
    fn test_classify_param_vector() {
        let req = ConstructorGraph::classify_param("vector<u8>");
        assert!(matches!(req, ParamRequirement::Vector(_)));
        assert!(req.is_synthesizable());
    }

    #[test]
    fn test_classify_param_reference() {
        let req = ConstructorGraph::classify_param("&mut tx_context::TxContext");
        assert!(matches!(req, ParamRequirement::Reference { is_mut: true, .. }));
        assert!(req.is_synthesizable());
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("my_type"), "MyType");
        assert_eq!(to_pascal_case("foo"), "Foo");
        assert_eq!(to_pascal_case("test_struct_name"), "TestStructName");
    }
}
