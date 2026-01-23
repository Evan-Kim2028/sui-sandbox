//! Constructor Graph: Enhanced constructor discovery using MM2.
//!
//! This module provides an MM2-based approach to finding constructors,
//! with better type analysis than the bytecode-only approach.
//!
//! ## Multi-Hop Constructor Chains
//!
//! The graph supports multi-hop constructor resolution via BFS. For example,
//! to construct type C which needs B which needs A:
//!
//! ```text
//! A (primitives only) -> B (needs A) -> C (needs B)
//! ```
//!
//! The `find_execution_chain()` method returns constructors in topological order
//! (A, B, C) so they can be executed sequentially with proper dependencies.

use crate::constructor_map::{ConstructorInfo, ParamKind};
use crate::mm2::model::{FunctionSignature, ReturnTypeArg, TypeModel};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{ModuleId, StructTag, TypeTag};
use move_model_2::summary::{self, Ability};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::LazyLock;

/// Safe fallback identifiers for error cases - validated at compile time via static initialization.
static UNKNOWN_MODULE: LazyLock<Identifier> =
    LazyLock::new(|| Identifier::new("unknown").expect("'unknown' is a valid identifier"));
static UNKNOWN_TYPE: LazyLock<Identifier> =
    LazyLock::new(|| Identifier::new("Unknown").expect("'Unknown' is a valid identifier"));

/// Safely create an Identifier, falling back to "unknown" if the input is invalid.
fn safe_identifier(name: &str, fallback: &Identifier) -> Identifier {
    Identifier::new(name).unwrap_or_else(|_| fallback.clone())
}

/// Maximum depth for constructor chain resolution
pub const MAX_CHAIN_DEPTH: usize = 5;

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
    /// Available constructors for this type (identified by naming patterns)
    pub constructors: Vec<Constructor>,
    /// Available producers for this type (identified by return type analysis)
    /// Each producer includes the return index where this type appears
    pub producers: Vec<(Producer, usize)>,
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

/// A producer function that returns one or more types (for return value chaining).
///
/// Unlike constructors which are identified by naming patterns (new_*, create_*),
/// producers are identified by analyzing actual return types. A producer can
/// return multiple types (e.g., `create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo)`).
#[derive(Debug, Clone)]
pub struct Producer {
    /// Module containing the producer
    pub module_addr: AccountAddress,
    pub module_name: String,
    /// Function name
    pub function_name: String,
    /// Parameter requirements
    pub params: Vec<ParamRequirement>,
    /// Number of type parameters
    pub type_param_count: usize,
    /// Types this function produces, with their return index
    /// (return_idx, type_key) where type_key is "addr::module::name"
    pub produces: Vec<(usize, ProducedType)>,
    /// Synthesis complexity score
    pub complexity: usize,
}

/// A type produced by a producer function.
#[derive(Debug, Clone)]
pub struct ProducedType {
    /// Full type key: "addr::module::name"
    pub type_key: String,
    /// Module address
    pub module_addr: AccountAddress,
    /// Module name
    pub module_name: String,
    /// Type name
    pub type_name: String,
    /// Type arguments (indices into producer's type params)
    pub type_args: Vec<ReturnTypeArg>,
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

    /// Check if this is a reference to a constructible type (not TxContext/Clock).
    pub fn is_constructible_reference(&self) -> bool {
        match self {
            ParamRequirement::Reference { inner, .. } => {
                matches!(inner.as_ref(), ParamRequirement::Type { .. })
            }
            _ => false,
        }
    }

    /// Get the inner type for a reference parameter.
    pub fn get_reference_inner(&self) -> Option<(&ParamRequirement, bool)> {
        match self {
            ParamRequirement::Reference { inner, is_mut } => Some((inner.as_ref(), *is_mut)),
            _ => None,
        }
    }
}

/// A producer-based chain for return value chaining.
///
/// Unlike ExecutionChain which uses naming-pattern constructors, ProducerChain
/// uses return type analysis to identify how to produce a type. This enables
/// chaining like: create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo)
#[derive(Debug, Clone)]
pub struct ProducerChain {
    /// Steps in topological order (dependencies first)
    pub steps: Vec<ProducerStep>,
    /// The final target type key
    pub target_type_key: String,
    /// Total depth of the chain
    pub depth: usize,
}

/// A single step in a producer chain.
#[derive(Debug, Clone)]
pub struct ProducerStep {
    /// The producer function to call
    pub producer: Producer,
    /// Which return value index contains the type we need
    pub target_return_idx: usize,
    /// Parameter dependencies: param_idx -> type_key from previous step
    pub dependencies: BTreeMap<usize, String>,
}

impl ProducerChain {
    /// Check if this chain is empty.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get all types produced by this chain (for multi-return functions).
    pub fn all_produced_types(&self) -> Vec<&ProducedType> {
        self.steps
            .iter()
            .flat_map(|s| s.producer.produces.iter().map(|(_, pt)| pt))
            .collect()
    }

    /// Check if executing this chain would also produce other types we might need.
    /// This is useful for planning - one producer call might give us multiple capabilities.
    pub fn bonus_types(&self) -> Vec<&ProducedType> {
        self.steps
            .iter()
            .flat_map(|s| {
                s.producer.produces.iter().filter_map(|(idx, pt)| {
                    if *idx != s.target_return_idx {
                        Some(pt)
                    } else {
                        None
                    }
                })
            })
            .collect()
    }
}

/// An execution-ready constructor chain with proper topological ordering.
///
/// This structure is designed to be consumed by runner.rs for Tier B execution.
/// The constructors are ordered such that dependencies come before dependents.
#[derive(Debug, Clone)]
pub struct ExecutionChain {
    /// Constructors in topological order (dependencies first).
    /// Each entry includes the type key it constructs for lookup.
    pub steps: Vec<ExecutionStep>,
    /// Total depth of the chain (1 = direct, 2 = single-hop, etc.)
    pub depth: usize,
}

/// A single step in an execution chain.
#[derive(Debug, Clone)]
pub struct ExecutionStep {
    /// The type key this step constructs (addr::module::name)
    pub type_key: String,
    /// The constructor to execute
    pub constructor: Constructor,
    /// Converted ConstructorInfo for runner.rs compatibility
    pub ctor_info: ConstructorInfo,
    /// Which parameters of this constructor depend on previously constructed types.
    /// Maps param_idx -> type_key of the dependency.
    pub dependencies: BTreeMap<usize, String>,
}

impl ExecutionChain {
    /// Check if this chain is empty (no constructors).
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get the final constructed type key.
    pub fn target_type(&self) -> Option<&str> {
        self.steps.last().map(|s| s.type_key.as_str())
    }

    /// Convert to a list of ConstructorInfo for backwards compatibility.
    pub fn to_ctor_infos(&self) -> Vec<ConstructorInfo> {
        self.steps.iter().map(|s| s.ctor_info.clone()).collect()
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
                        producers: Vec::new(),
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
                        if let Some(target_key) = Self::infer_return_type(&sig, &addr, &module_name)
                        {
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

        // Third pass: analyze return types to find producers
        // This enables return value chaining (e.g., create_lst() -> (AdminCap, CollectionFeeCap, LiquidStakingInfo))
        for (addr, module_name) in model.modules() {
            for func_name in model.functions_in_module(&addr, &module_name) {
                if let Some(sig) = model.get_function(&addr, &module_name, &func_name) {
                    // Only consider public/entry functions
                    if !sig.is_public && !sig.is_entry {
                        continue;
                    }

                    // Skip functions with no returns
                    if sig.returns.is_empty() {
                        continue;
                    }

                    // Analyze return types to find struct types we can produce
                    let mut produced_types: Vec<(usize, ProducedType)> = Vec::new();

                    for (return_idx, ret_info) in sig.returns.iter().enumerate() {
                        if let Some(struct_type) = &ret_info.struct_type {
                            let type_key = format!(
                                "{}::{}::{}",
                                struct_type.module_addr,
                                struct_type.module_name,
                                struct_type.struct_name
                            );

                            // Only consider types we know about
                            if types.contains_key(&type_key) {
                                produced_types.push((
                                    return_idx,
                                    ProducedType {
                                        type_key: type_key.clone(),
                                        module_addr: struct_type.module_addr,
                                        module_name: struct_type.module_name.clone(),
                                        type_name: struct_type.struct_name.clone(),
                                        type_args: struct_type.type_args.clone(),
                                    },
                                ));
                            }
                        }
                    }

                    // If this function produces any struct types, create a producer
                    if !produced_types.is_empty() {
                        // Analyze parameters
                        let params: Vec<ParamRequirement> = sig
                            .parameters
                            .iter()
                            .map(|p| Self::classify_param(&p.type_str))
                            .collect();

                        let complexity = params.iter().filter(|p| !p.is_synthesizable()).count()
                            * 10
                            + sig.type_parameters.len() * 2;

                        let producer = Producer {
                            module_addr: addr,
                            module_name: module_name.clone(),
                            function_name: func_name.clone(),
                            params,
                            type_param_count: sig.type_parameters.len(),
                            produces: produced_types.clone(),
                            complexity,
                        };

                        // Register this producer with each type it produces
                        for (return_idx, produced_type) in &produced_types {
                            if let Some(node) = types.get_mut(&produced_type.type_key) {
                                node.producers.push((producer.clone(), *return_idx));
                            }
                        }
                    }
                }
            }
        }

        // Sort producers by complexity
        for node in types.values_mut() {
            node.producers.sort_by_key(|(p, _)| p.complexity);
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
        let type_name = if let Some(suffix) = func_name.strip_prefix("new_") {
            Some(to_pascal_case(suffix))
        } else if let Some(suffix) = func_name.strip_prefix("create_") {
            Some(to_pascal_case(suffix))
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

    /// Find a producer that can synthesize a given type.
    ///
    /// This method looks at all functions that return the target type (via return type analysis)
    /// rather than relying on naming patterns. This is especially useful for capability types
    /// like AdminCap, CollectionFeeCap, etc.
    ///
    /// Returns (Producer, return_index) if found.
    pub fn find_producer(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
    ) -> Option<(Producer, usize)> {
        let key = format!("{}::{}::{}", module_addr, module_name, type_name);
        let node = self.types.get(&key)?;

        // Find a producer whose params are all synthesizable
        for (producer, return_idx) in &node.producers {
            if producer.params.iter().all(|p| p.is_synthesizable()) {
                return Some((producer.clone(), *return_idx));
            }
        }

        None
    }

    /// Find all producers for a given type (including those with non-synthesizable params).
    pub fn find_all_producers(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
    ) -> Vec<(Producer, usize)> {
        let key = format!("{}::{}::{}", module_addr, module_name, type_name);
        self.types
            .get(&key)
            .map(|n| n.producers.clone())
            .unwrap_or_default()
    }

    /// Find a producer chain that can synthesize a given type.
    ///
    /// Similar to find_chain() but uses producers (return type analysis) instead of
    /// constructors (naming patterns). This enables return value chaining.
    ///
    /// Returns a ProducerChain that specifies exactly which return value to use.
    pub fn find_producer_chain(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
    ) -> Option<ProducerChain> {
        let key = format!("{}::{}::{}", module_addr, module_name, type_name);
        let node = self.types.get(&key)?;

        // Try direct producers first (all params synthesizable)
        for (producer, return_idx) in &node.producers {
            if producer.params.iter().all(|p| p.is_synthesizable()) {
                return Some(ProducerChain {
                    steps: vec![ProducerStep {
                        producer: producer.clone(),
                        target_return_idx: *return_idx,
                        dependencies: BTreeMap::new(),
                    }],
                    target_type_key: key,
                    depth: 1,
                });
            }
        }

        // BFS for multi-hop producer chains
        self.bfs_find_producer_chain(&key, MAX_CHAIN_DEPTH)
    }

    /// BFS to find producer chain.
    fn bfs_find_producer_chain(&self, target_key: &str, max_depth: usize) -> Option<ProducerChain> {
        // Verify target type exists in graph (early return if not)
        let _target_node = self.types.get(target_key)?;

        // BFS state: (current_type_key, chain_so_far, depth)
        let mut queue: VecDeque<(String, Vec<ProducerStep>, usize)> = VecDeque::new();
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

            for (producer, return_idx) in &node.producers {
                // Build dependency map for this step
                let mut dependencies = BTreeMap::new();
                let mut all_resolvable = true;
                let mut unresolved_types = Vec::new();

                for (param_idx, param) in producer.params.iter().enumerate() {
                    if !param.is_synthesizable() {
                        if let ParamRequirement::Type {
                            module_addr,
                            module_name,
                            type_name,
                        } = param
                        {
                            let dep_key =
                                format!("{}::{}::{}", module_addr, module_name, type_name);

                            // Check if this dependency is already in our chain
                            let already_in_chain = chain.iter().any(|step| {
                                step.producer
                                    .produces
                                    .iter()
                                    .any(|(_, pt)| pt.type_key == dep_key)
                            });

                            if already_in_chain {
                                dependencies.insert(param_idx, dep_key);
                            } else if !visited.contains(&dep_key) {
                                unresolved_types.push(dep_key);
                            } else {
                                // Already visited but not resolved - might be circular
                                all_resolvable = false;
                                break;
                            }
                        } else if let ParamRequirement::Reference { inner, .. } = param {
                            // Handle reference to constructible type
                            if let ParamRequirement::Type {
                                module_addr,
                                module_name,
                                type_name,
                            } = inner.as_ref()
                            {
                                let dep_key =
                                    format!("{}::{}::{}", module_addr, module_name, type_name);
                                let already_in_chain = chain.iter().any(|step| {
                                    step.producer
                                        .produces
                                        .iter()
                                        .any(|(_, pt)| pt.type_key == dep_key)
                                });
                                if already_in_chain {
                                    dependencies.insert(param_idx, dep_key);
                                } else if !visited.contains(&dep_key) {
                                    unresolved_types.push(dep_key);
                                } else {
                                    all_resolvable = false;
                                    break;
                                }
                            } else {
                                all_resolvable = false;
                                break;
                            }
                        } else {
                            all_resolvable = false;
                            break;
                        }
                    }
                }

                if !all_resolvable {
                    continue;
                }

                let mut new_chain = chain.clone();
                new_chain.push(ProducerStep {
                    producer: producer.clone(),
                    target_return_idx: *return_idx,
                    dependencies,
                });

                if unresolved_types.is_empty() {
                    // Found a complete chain - return in proper order (dependencies first)
                    new_chain.reverse();
                    return Some(ProducerChain {
                        steps: new_chain,
                        target_type_key: target_key.to_string(),
                        depth: depth + 1,
                    });
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
    pub fn is_object_type(
        &self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
    ) -> bool {
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

    /// Find an execution-ready constructor chain for a type.
    ///
    /// Unlike `find_chain()`, this method returns constructors in proper
    /// topological order (dependencies first) with all necessary metadata
    /// for execution by runner.rs.
    ///
    /// Returns `None` if no chain can be found within `MAX_CHAIN_DEPTH`.
    pub fn find_execution_chain(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
    ) -> Option<ExecutionChain> {
        let key = format!("{}::{}::{}", module_addr, module_name, type_name);

        // Use existing find_chain which does BFS
        let chain = self.find_chain(module_addr, module_name, type_name)?;

        if chain.is_empty() {
            return None;
        }

        // Build execution steps with proper topological ordering
        // The chain from find_chain is already in the right order (target last),
        // but we need to build dependency information
        let mut steps = Vec::new();
        let mut constructed_types: BTreeSet<String> = BTreeSet::new();

        for ctor in chain.iter() {
            // Determine what type this constructor builds
            // We need to infer from the constructor's context
            let type_key = format!(
                "{}::{}::{}",
                ctor.module_addr,
                ctor.module_name,
                infer_return_type_from_ctor(&ctor.function_name, &ctor.module_name)
            );

            // Build dependency map
            let mut dependencies = BTreeMap::new();
            for (param_idx, param) in ctor.params.iter().enumerate() {
                if let ParamRequirement::Type {
                    module_addr,
                    module_name,
                    type_name,
                } = param
                {
                    let dep_key = format!("{}::{}::{}", module_addr, module_name, type_name);
                    if constructed_types.contains(&dep_key) {
                        dependencies.insert(param_idx, dep_key);
                    }
                }
            }

            // Convert to ConstructorInfo
            let ctor_info = constructor_to_info(ctor, &key);

            steps.push(ExecutionStep {
                type_key: type_key.clone(),
                constructor: ctor.clone(),
                ctor_info,
                dependencies,
            });

            constructed_types.insert(type_key);
        }

        Some(ExecutionChain {
            depth: steps.len(),
            steps,
        })
    }

    /// Find an execution chain for a reference parameter.
    ///
    /// This handles the case where we have `&T` or `&mut T` and need to
    /// construct `T` first, then pass a reference to it.
    pub fn find_execution_chain_for_ref(
        &mut self,
        module_addr: &AccountAddress,
        module_name: &str,
        type_name: &str,
        is_mut: bool,
    ) -> Option<(ExecutionChain, bool)> {
        self.find_execution_chain(module_addr, module_name, type_name)
            .map(|chain| (chain, is_mut))
    }

    /// Get statistics about the constructor graph.
    pub fn stats(&self) -> ConstructorGraphStats {
        let total_types = self.types.len();
        let types_with_constructors = self
            .types
            .values()
            .filter(|n| !n.constructors.is_empty())
            .count();
        let total_constructors: usize = self.types.values().map(|n| n.constructors.len()).sum();
        let object_types = self
            .types
            .values()
            .filter(|n| n.abilities.0.contains(&Ability::Key))
            .count();
        let types_with_producers = self
            .types
            .values()
            .filter(|n| !n.producers.is_empty())
            .count();
        let total_producers: usize = self.types.values().map(|n| n.producers.len()).sum();

        ConstructorGraphStats {
            total_types,
            types_with_constructors,
            total_constructors,
            object_types,
            types_with_producers,
            total_producers,
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
    pub types_with_producers: usize,
    pub total_producers: usize,
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

/// Infer return type name from constructor function name.
///
/// Common patterns:
/// - `new_foo` -> `Foo`
/// - `create_foo` -> `Foo`
/// - `new` / `create` / `init` -> module name as PascalCase
fn infer_return_type_from_ctor(func_name: &str, module_name: &str) -> String {
    if let Some(suffix) = func_name.strip_prefix("new_") {
        to_pascal_case(suffix)
    } else if let Some(suffix) = func_name.strip_prefix("create_") {
        to_pascal_case(suffix)
    } else if func_name == "new" || func_name == "create" || func_name == "init" {
        to_pascal_case(module_name)
    } else {
        // Default: use module name
        to_pascal_case(module_name)
    }
}

/// Convert a ParamRequirement to a ParamKind for runner.rs compatibility.
fn param_requirement_to_kind(req: &ParamRequirement) -> ParamKind {
    match req {
        ParamRequirement::Primitive(s) => {
            let tag = match s.as_str() {
                "bool" => TypeTag::Bool,
                "u8" => TypeTag::U8,
                "u16" => TypeTag::U16,
                "u32" => TypeTag::U32,
                "u64" => TypeTag::U64,
                "u128" => TypeTag::U128,
                "u256" => TypeTag::U256,
                "address" => TypeTag::Address,
                _ => TypeTag::U64, // Default
            };
            ParamKind::Primitive(tag)
        }
        ParamRequirement::Vector(inner) => {
            if let ParamKind::Primitive(tag) = param_requirement_to_kind(inner) {
                ParamKind::PrimitiveVector(tag)
            } else {
                ParamKind::Unsupported("complex vector".to_string())
            }
        }
        ParamRequirement::TxContext => ParamKind::TxContext,
        ParamRequirement::Clock => ParamKind::Clock,
        ParamRequirement::TypeParam(idx) => ParamKind::TypeParam(*idx),
        ParamRequirement::Type {
            module_addr,
            module_name,
            type_name,
        } => {
            let struct_tag = StructTag {
                address: *module_addr,
                module: safe_identifier(module_name, &UNKNOWN_MODULE),
                name: safe_identifier(type_name, &UNKNOWN_TYPE),
                type_params: vec![],
            };
            ParamKind::Struct(struct_tag)
        }
        ParamRequirement::Reference { inner, .. } => {
            // References are handled specially - this shouldn't be called for them
            param_requirement_to_kind(inner)
        }
        ParamRequirement::Unsupported(s) => ParamKind::Unsupported(s.clone()),
    }
}

/// Convert a Constructor to ConstructorInfo for runner.rs compatibility.
fn constructor_to_info(ctor: &Constructor, target_key: &str) -> ConstructorInfo {
    // Parse target_key to get StructTag
    let parts: Vec<&str> = target_key.split("::").collect();
    let (addr, module_name, type_name) = if parts.len() >= 3 {
        let addr = AccountAddress::from_hex_literal(parts[0]).unwrap_or(AccountAddress::ZERO);
        (addr, parts[1].to_string(), parts[2].to_string())
    } else {
        (
            ctor.module_addr,
            ctor.module_name.clone(),
            infer_return_type_from_ctor(&ctor.function_name, &ctor.module_name),
        )
    };

    let module_id = ModuleId::new(
        ctor.module_addr,
        safe_identifier(&ctor.module_name, &UNKNOWN_MODULE),
    );

    let returns = StructTag {
        address: addr,
        module: safe_identifier(&module_name, &UNKNOWN_MODULE),
        name: safe_identifier(&type_name, &UNKNOWN_TYPE),
        type_params: vec![],
    };

    let params: Vec<ParamKind> = ctor.params.iter().map(param_requirement_to_kind).collect();

    ConstructorInfo {
        module_id,
        function_name: ctor.function_name.clone(),
        type_params: ctor.type_param_count,
        params,
        returns,
    }
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
        assert!(matches!(
            req,
            ParamRequirement::Reference { is_mut: true, .. }
        ));
        assert!(req.is_synthesizable());
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("my_type"), "MyType");
        assert_eq!(to_pascal_case("foo"), "Foo");
        assert_eq!(to_pascal_case("test_struct_name"), "TestStructName");
    }
}
