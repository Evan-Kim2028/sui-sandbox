//! # PTB Executor: Programmable Transaction Block Execution
//!
//! This module implements a local PTB executor that allows multi-command
//! transaction execution with result chaining, matching Sui's PTB semantics.
//!
//! ## Overview
//!
//! PTBs (Programmable Transaction Blocks) are Sui's mechanism for batching
//! multiple operations into a single atomic transaction. Commands can:
//! - Call Move functions and capture return values
//! - Split and merge coins
//! - Transfer objects
//! - Create vectors from elements
//!
//! Results from earlier commands can be used as inputs to later commands,
//! enabling complex multi-step operations in a single transaction.
//!
//! ## Example
//!
//! ```
//! use sui_sandbox_core::ptb::{Command, Argument};
//! use move_core_types::account_address::AccountAddress;
//! use move_core_types::identifier::Identifier;
//!
//! // Define commands for a PTB
//! let package_addr = AccountAddress::from_hex_literal("0x2").unwrap();
//! let commands = vec![
//!     Command::MoveCall {
//!         package: package_addr,
//!         module: Identifier::new("my_module").unwrap(),
//!         function: Identifier::new("create_thing").unwrap(),
//!         type_args: vec![],
//!         args: vec![Argument::Input(0)],
//!     },
//!     Command::TransferObjects {
//!         objects: vec![Argument::Result(0)],
//!         address: Argument::Input(1),
//!     },
//! ];
//! assert_eq!(commands.len(), 2);
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{ModuleId, TypeTag};
use std::collections::{HashMap, HashSet};

use crate::natives::EmittedEvent;
use crate::vm::{gas_costs, VMHarness};
use crate::well_known;

// Re-export format_type_tag from types module for backward compatibility
pub use crate::types::format_type_tag;

// =============================================================================
// PTB Causality Validation
// =============================================================================

/// Result of PTB validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the PTB is valid.
    pub valid: bool,
    /// List of validation errors found.
    pub errors: Vec<ValidationError>,
    /// List of validation warnings (non-fatal issues).
    pub warnings: Vec<String>,
}

impl ValidationResult {
    /// Create a successful validation result.
    pub fn ok() -> Self {
        Self {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Create a failed validation result with errors.
    pub fn failed(errors: Vec<ValidationError>) -> Self {
        Self {
            valid: false,
            errors,
            warnings: Vec::new(),
        }
    }

    /// Add a warning to the result.
    pub fn with_warning(mut self, warning: String) -> Self {
        self.warnings.push(warning);
        self
    }
}

/// A specific validation error in a PTB.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// The command index where the error was found.
    pub command_index: usize,
    /// The type of validation error.
    pub kind: ValidationErrorKind,
    /// Human-readable description of the error.
    pub message: String,
}

/// Types of validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationErrorKind {
    /// Reference to a result that doesn't exist yet (forward reference).
    ForwardReference,
    /// Reference to a result index that's out of bounds.
    ResultOutOfBounds,
    /// Reference to an input index that's out of bounds.
    InputOutOfBounds,
    /// Circular dependency detected in result references.
    CircularDependency,
    /// Self-reference (command references its own result).
    SelfReference,
    /// Invalid nested result index.
    InvalidNestedIndex,
    /// Other validation error.
    Other,
}

/// Validate a PTB before execution.
///
/// This performs static validation to catch issues like:
/// - Forward references (referencing results that haven't been produced yet)
/// - Out of bounds references
/// - Self-references (command using its own result)
///
/// # Arguments
/// * `commands` - The commands to validate
/// * `num_inputs` - Number of transaction inputs available
///
/// # Returns
/// A `ValidationResult` indicating whether the PTB is valid.
pub fn validate_ptb(commands: &[Command], num_inputs: usize) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for (cmd_idx, cmd) in commands.iter().enumerate() {
        let args = extract_arguments(cmd);

        for arg in args {
            match arg {
                Argument::Input(idx) => {
                    if (idx as usize) >= num_inputs {
                        errors.push(ValidationError {
                            command_index: cmd_idx,
                            kind: ValidationErrorKind::InputOutOfBounds,
                            message: format!(
                                "Command {} references Input({}) but only {} inputs available",
                                cmd_idx, idx, num_inputs
                            ),
                        });
                    }
                }
                Argument::Result(result_idx) => {
                    let result_idx = result_idx as usize;
                    if result_idx >= cmd_idx {
                        if result_idx == cmd_idx {
                            errors.push(ValidationError {
                                command_index: cmd_idx,
                                kind: ValidationErrorKind::SelfReference,
                                message: format!(
                                    "Command {} references its own result Result({})",
                                    cmd_idx, result_idx
                                ),
                            });
                        } else {
                            errors.push(ValidationError {
                                command_index: cmd_idx,
                                kind: ValidationErrorKind::ForwardReference,
                                message: format!(
                                    "Command {} references Result({}) which hasn't been produced yet",
                                    cmd_idx, result_idx
                                ),
                            });
                        }
                    }
                }
                Argument::NestedResult(result_idx, _nested_idx) => {
                    let result_idx = result_idx as usize;
                    if result_idx >= cmd_idx {
                        if result_idx == cmd_idx {
                            errors.push(ValidationError {
                                command_index: cmd_idx,
                                kind: ValidationErrorKind::SelfReference,
                                message: format!(
                                    "Command {} references its own result NestedResult({}, _)",
                                    cmd_idx, result_idx
                                ),
                            });
                        } else {
                            errors.push(ValidationError {
                                command_index: cmd_idx,
                                kind: ValidationErrorKind::ForwardReference,
                                message: format!(
                                    "Command {} references NestedResult({}, _) which hasn't been produced yet",
                                    cmd_idx, result_idx
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // Check for unused results (warning only)
    let mut result_used = vec![false; commands.len()];
    for cmd in commands.iter() {
        for arg in extract_arguments(cmd) {
            match arg {
                Argument::Result(idx) => {
                    if (idx as usize) < result_used.len() {
                        result_used[idx as usize] = true;
                    }
                }
                Argument::NestedResult(idx, _) => {
                    if (idx as usize) < result_used.len() {
                        result_used[idx as usize] = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Last command result doesn't need to be used (it's the transaction return)
    if !result_used.is_empty() {
        let last_idx = result_used.len() - 1;
        result_used[last_idx] = true;
    }

    for (idx, used) in result_used.iter().enumerate() {
        if !*used && idx < commands.len() - 1 {
            // Skip this warning for commands that don't produce useful results
            match &commands[idx] {
                Command::TransferObjects { .. } => {} // Transfer doesn't produce usable results
                _ => {
                    warnings.push(format!(
                        "Command {} result is never used (potential dead code)",
                        idx
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        let mut result = ValidationResult::ok();
        for w in warnings {
            result = result.with_warning(w);
        }
        result
    } else {
        ValidationResult::failed(errors)
    }
}

/// Extract all arguments from a command.
fn extract_arguments(cmd: &Command) -> Vec<Argument> {
    match cmd {
        Command::MoveCall { args, .. } => args.clone(),
        Command::SplitCoins { coin, amounts } => {
            let mut args = vec![*coin];
            args.extend(amounts.iter().copied());
            args
        }
        Command::MergeCoins {
            destination,
            sources,
        } => {
            let mut args = vec![*destination];
            args.extend(sources.iter().copied());
            args
        }
        Command::TransferObjects { objects, address } => {
            let mut args = objects.clone();
            args.push(*address);
            args
        }
        Command::MakeMoveVec { elements, .. } => elements.clone(),
        Command::Publish { .. } => Vec::new(),
        Command::Upgrade { ticket, .. } => vec![*ticket],
        Command::Receive { .. } => Vec::new(),
    }
}

/// Compute the dependency graph of commands in a PTB.
///
/// Returns a map from command index to the set of command indices it depends on.
pub fn compute_dependency_graph(commands: &[Command]) -> HashMap<usize, HashSet<usize>> {
    let mut deps: HashMap<usize, HashSet<usize>> = HashMap::new();

    for (cmd_idx, cmd) in commands.iter().enumerate() {
        let mut cmd_deps = HashSet::new();

        for arg in extract_arguments(cmd) {
            match arg {
                Argument::Result(idx) => {
                    cmd_deps.insert(idx as usize);
                }
                Argument::NestedResult(idx, _) => {
                    cmd_deps.insert(idx as usize);
                }
                Argument::Input(_) => {} // Inputs don't create dependencies
            }
        }

        deps.insert(cmd_idx, cmd_deps);
    }

    deps
}

/// Perform a topological sort of commands based on their dependencies.
///
/// Returns Ok with the sorted indices if no cycles, or Err with the cycle path if a cycle exists.
pub fn topological_sort(commands: &[Command]) -> Result<Vec<usize>, Vec<usize>> {
    let deps = compute_dependency_graph(commands);
    let n = commands.len();

    // States: 0 = unvisited, 1 = visiting, 2 = visited
    let mut state = vec![0u8; n];
    let mut result = Vec::with_capacity(n);
    let mut path = Vec::new();

    fn visit(
        node: usize,
        deps: &HashMap<usize, HashSet<usize>>,
        state: &mut [u8],
        result: &mut Vec<usize>,
        path: &mut Vec<usize>,
    ) -> Result<(), Vec<usize>> {
        if state[node] == 2 {
            return Ok(());
        }
        if state[node] == 1 {
            // Found a cycle - return the path
            path.push(node);
            return Err(path.clone());
        }

        state[node] = 1;
        path.push(node);

        if let Some(node_deps) = deps.get(&node) {
            for &dep in node_deps {
                if dep < state.len() {
                    visit(dep, deps, state, result, path)?;
                }
            }
        }

        state[node] = 2;
        path.pop();
        result.push(node);
        Ok(())
    }

    for i in 0..n {
        path.clear();
        visit(i, &deps, &mut state, &mut result, &mut path)?;
    }

    Ok(result)
}

/// Unique identifier for objects in the PTB context.
pub type ObjectID = AccountAddress;

/// A command in a Programmable Transaction Block.
#[derive(Debug, Clone)]
pub enum Command {
    /// Call a Move function
    MoveCall {
        package: AccountAddress,
        module: Identifier,
        function: Identifier,
        type_args: Vec<TypeTag>,
        args: Vec<Argument>,
    },

    /// Split a coin into multiple coins with specified amounts.
    /// Returns a vector of the split coins.
    SplitCoins {
        coin: Argument,
        amounts: Vec<Argument>,
    },

    /// Merge multiple coins into a destination coin.
    /// The source coins are destroyed.
    MergeCoins {
        destination: Argument,
        sources: Vec<Argument>,
    },

    /// Transfer objects to an address.
    TransferObjects {
        objects: Vec<Argument>,
        address: Argument,
    },

    /// Create a vector from elements.
    /// If type_tag is None, it's inferred from elements.
    MakeMoveVec {
        type_tag: Option<TypeTag>,
        elements: Vec<Argument>,
    },

    /// Publish new modules (optional, may not be fully supported)
    Publish {
        modules: Vec<Vec<u8>>,
        dep_ids: Vec<ObjectID>,
    },

    /// Upgrade an existing package (optional, may not be fully supported)
    Upgrade {
        modules: Vec<Vec<u8>>,
        package: ObjectID,
        ticket: Argument,
    },

    /// Receive an object that was sent to this transaction.
    /// Used for transaction chaining where objects are passed between PTBs.
    /// The object must have been transferred to the sender in a previous transaction.
    Receive {
        /// The object ID to receive
        object_id: ObjectID,
        /// The expected type of the object (for validation)
        object_type: Option<TypeTag>,
    },
}

/// Reference to a value in a PTB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Argument {
    /// Reference to a transaction input (by index)
    Input(u16),

    /// Reference to the result of a previous command (by command index)
    /// For commands with a single return value.
    Result(u16),

    /// Reference to a specific value in a multi-return command result.
    /// (command_index, value_index)
    NestedResult(u16, u16),
}

// =============================================================================
// Object Lifecycle Tracking
// =============================================================================

/// Tracks the provenance and lifecycle of an object during PTB execution.
#[derive(Debug, Clone)]
pub struct ObjectProvenance {
    /// The object ID.
    pub object_id: ObjectID,
    /// How this object came to exist in the transaction.
    pub origin: ObjectOrigin,
    /// Current state of the object.
    pub state: ObjectState,
    /// History of operations on this object.
    pub history: Vec<ObjectOperation>,
    /// The type of the object, if known.
    pub type_tag: Option<TypeTag>,
}

/// How an object originated in the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectOrigin {
    /// Object was provided as a transaction input.
    Input { input_index: u16 },
    /// Object was created by a command in this transaction.
    Created { command_index: usize },
    /// Object was received from a previous transaction.
    Received,
    /// Object was split from another coin.
    Split {
        source_id: ObjectID,
        command_index: usize,
    },
}

/// Current state of an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectState {
    /// Object is available for use.
    Available,
    /// Object has been consumed (passed by value).
    Consumed,
    /// Object has been transferred to another address.
    Transferred,
    /// Object has been deleted/destroyed.
    Deleted,
    /// Object has been wrapped (stored inside another object).
    Wrapped,
    /// Object has been frozen (made immutable).
    Frozen,
}

/// A single operation performed on an object.
#[derive(Debug, Clone)]
pub struct ObjectOperation {
    /// The command index that performed this operation.
    pub command_index: usize,
    /// The type of operation.
    pub operation: OperationType,
}

/// Types of operations that can be performed on objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationType {
    /// Object was read (immutable borrow).
    Read,
    /// Object was mutated (mutable borrow).
    Mutate,
    /// Object was consumed (passed by value).
    Consume,
    /// Object was transferred to an address.
    Transfer { to: AccountAddress },
    /// Object was deleted.
    Delete,
    /// Object was wrapped inside another object.
    Wrap,
    /// Object was frozen (made immutable).
    Freeze,
    /// Object was merged into another coin.
    MergeInto { destination: ObjectID },
}

/// Tracks all object lifecycles during PTB execution.
#[derive(Debug, Clone, Default)]
pub struct ObjectLifecycleTracker {
    /// Provenance for each object by ID.
    objects: HashMap<ObjectID, ObjectProvenance>,
    /// Errors detected during lifecycle tracking.
    errors: Vec<LifecycleError>,
}

/// An error in object lifecycle (e.g., double-use, use after consume).
#[derive(Debug, Clone)]
pub struct LifecycleError {
    /// The object involved.
    pub object_id: ObjectID,
    /// The command that caused the error.
    pub command_index: usize,
    /// Description of the error.
    pub message: String,
    /// The kind of lifecycle error.
    pub kind: LifecycleErrorKind,
}

/// Types of lifecycle errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleErrorKind {
    /// Object was used after being consumed.
    UseAfterConsume,
    /// Object was used after being transferred.
    UseAfterTransfer,
    /// Object was used after being deleted.
    UseAfterDelete,
    /// Object was used after being wrapped.
    UseAfterWrap,
    /// Mutable borrow of immutable object.
    MutateImmutable,
    /// Object not found.
    ObjectNotFound,
}

impl ObjectLifecycleTracker {
    /// Create a new lifecycle tracker.
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Register an object from a transaction input.
    pub fn register_input(
        &mut self,
        object_id: ObjectID,
        input_index: u16,
        type_tag: Option<TypeTag>,
    ) {
        self.objects.insert(
            object_id,
            ObjectProvenance {
                object_id,
                origin: ObjectOrigin::Input { input_index },
                state: ObjectState::Available,
                history: Vec::new(),
                type_tag,
            },
        );
    }

    /// Register a newly created object.
    pub fn register_created(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
        type_tag: Option<TypeTag>,
    ) {
        self.objects.insert(
            object_id,
            ObjectProvenance {
                object_id,
                origin: ObjectOrigin::Created { command_index },
                state: ObjectState::Available,
                history: Vec::new(),
                type_tag,
            },
        );
    }

    /// Register a received object.
    pub fn register_received(&mut self, object_id: ObjectID, type_tag: Option<TypeTag>) {
        self.objects.insert(
            object_id,
            ObjectProvenance {
                object_id,
                origin: ObjectOrigin::Received,
                state: ObjectState::Available,
                history: Vec::new(),
                type_tag,
            },
        );
    }

    /// Record a read operation on an object.
    pub fn record_read(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
    ) -> Result<(), LifecycleError> {
        self.check_available(object_id, command_index)?;
        if let Some(prov) = self.objects.get_mut(&object_id) {
            prov.history.push(ObjectOperation {
                command_index,
                operation: OperationType::Read,
            });
        }
        Ok(())
    }

    /// Record a mutation on an object.
    pub fn record_mutate(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
    ) -> Result<(), LifecycleError> {
        self.check_available(object_id, command_index)?;
        if let Some(prov) = self.objects.get_mut(&object_id) {
            prov.history.push(ObjectOperation {
                command_index,
                operation: OperationType::Mutate,
            });
        }
        Ok(())
    }

    /// Record consumption of an object (passed by value).
    pub fn record_consume(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
    ) -> Result<(), LifecycleError> {
        self.check_available(object_id, command_index)?;
        if let Some(prov) = self.objects.get_mut(&object_id) {
            prov.state = ObjectState::Consumed;
            prov.history.push(ObjectOperation {
                command_index,
                operation: OperationType::Consume,
            });
        }
        Ok(())
    }

    /// Record a transfer of an object.
    pub fn record_transfer(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
        to: AccountAddress,
    ) -> Result<(), LifecycleError> {
        self.check_available(object_id, command_index)?;
        if let Some(prov) = self.objects.get_mut(&object_id) {
            prov.state = ObjectState::Transferred;
            prov.history.push(ObjectOperation {
                command_index,
                operation: OperationType::Transfer { to },
            });
        }
        Ok(())
    }

    /// Record deletion of an object.
    pub fn record_delete(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
    ) -> Result<(), LifecycleError> {
        self.check_available(object_id, command_index)?;
        if let Some(prov) = self.objects.get_mut(&object_id) {
            prov.state = ObjectState::Deleted;
            prov.history.push(ObjectOperation {
                command_index,
                operation: OperationType::Delete,
            });
        }
        Ok(())
    }

    /// Check if an object is available for use.
    fn check_available(
        &mut self,
        object_id: ObjectID,
        command_index: usize,
    ) -> Result<(), LifecycleError> {
        match self.objects.get(&object_id) {
            None => {
                let err = LifecycleError {
                    object_id,
                    command_index,
                    message: format!(
                        "Object {} not found in transaction",
                        object_id.to_hex_literal()
                    ),
                    kind: LifecycleErrorKind::ObjectNotFound,
                };
                self.errors.push(err.clone());
                Err(err)
            }
            Some(prov) => match prov.state {
                ObjectState::Available => Ok(()),
                ObjectState::Consumed => {
                    let err = LifecycleError {
                        object_id,
                        command_index,
                        message: format!(
                            "Object {} was consumed at command {} and cannot be used again",
                            object_id.to_hex_literal(),
                            prov.history.last().map(|h| h.command_index).unwrap_or(0)
                        ),
                        kind: LifecycleErrorKind::UseAfterConsume,
                    };
                    self.errors.push(err.clone());
                    Err(err)
                }
                ObjectState::Transferred => {
                    let err = LifecycleError {
                        object_id,
                        command_index,
                        message: format!(
                            "Object {} was transferred at command {} and cannot be used again",
                            object_id.to_hex_literal(),
                            prov.history.last().map(|h| h.command_index).unwrap_or(0)
                        ),
                        kind: LifecycleErrorKind::UseAfterTransfer,
                    };
                    self.errors.push(err.clone());
                    Err(err)
                }
                ObjectState::Deleted => {
                    let err = LifecycleError {
                        object_id,
                        command_index,
                        message: format!(
                            "Object {} was deleted at command {} and cannot be used",
                            object_id.to_hex_literal(),
                            prov.history.last().map(|h| h.command_index).unwrap_or(0)
                        ),
                        kind: LifecycleErrorKind::UseAfterDelete,
                    };
                    self.errors.push(err.clone());
                    Err(err)
                }
                ObjectState::Wrapped => {
                    let err = LifecycleError {
                        object_id,
                        command_index,
                        message: format!(
                            "Object {} was wrapped at command {} and cannot be used directly",
                            object_id.to_hex_literal(),
                            prov.history.last().map(|h| h.command_index).unwrap_or(0)
                        ),
                        kind: LifecycleErrorKind::UseAfterWrap,
                    };
                    self.errors.push(err.clone());
                    Err(err)
                }
                ObjectState::Frozen => Ok(()), // Frozen objects can still be read
            },
        }
    }

    /// Get the provenance of an object.
    pub fn get_provenance(&self, object_id: &ObjectID) -> Option<&ObjectProvenance> {
        self.objects.get(object_id)
    }

    /// Get all tracked objects.
    pub fn all_objects(&self) -> &HashMap<ObjectID, ObjectProvenance> {
        &self.objects
    }

    /// Get all lifecycle errors.
    pub fn errors(&self) -> &[LifecycleError] {
        &self.errors
    }

    /// Check if there were any lifecycle errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Get a summary of object states.
    pub fn summary(&self) -> ObjectLifecycleSummary {
        let mut summary = ObjectLifecycleSummary::default();
        for prov in self.objects.values() {
            match prov.state {
                ObjectState::Available => summary.available += 1,
                ObjectState::Consumed => summary.consumed += 1,
                ObjectState::Transferred => summary.transferred += 1,
                ObjectState::Deleted => summary.deleted += 1,
                ObjectState::Wrapped => summary.wrapped += 1,
                ObjectState::Frozen => summary.frozen += 1,
            }
            match &prov.origin {
                ObjectOrigin::Input { .. } => summary.from_inputs += 1,
                ObjectOrigin::Created { .. } => summary.created += 1,
                ObjectOrigin::Received => summary.received += 1,
                ObjectOrigin::Split { .. } => summary.split += 1,
            }
        }
        summary
    }
}

/// Summary statistics of object lifecycle states.
#[derive(Debug, Clone, Default)]
pub struct ObjectLifecycleSummary {
    /// Objects still available at end of transaction.
    pub available: usize,
    /// Objects that were consumed.
    pub consumed: usize,
    /// Objects that were transferred.
    pub transferred: usize,
    /// Objects that were deleted.
    pub deleted: usize,
    /// Objects that were wrapped.
    pub wrapped: usize,
    /// Objects that were frozen.
    pub frozen: usize,
    /// Objects from transaction inputs.
    pub from_inputs: usize,
    /// Objects created during transaction.
    pub created: usize,
    /// Objects received from previous transactions.
    pub received: usize,
    /// Objects created by splitting.
    pub split: usize,
}

// =============================================================================
// PTB Execution Trace
// =============================================================================

/// Detailed execution trace for a PTB transaction.
///
/// This extends the base ExecutionTrace with PTB-specific information like
/// command-level tracing and argument resolution details.
#[derive(Debug, Clone, Default)]
pub struct PTBExecutionTrace {
    /// Per-command execution traces.
    pub commands: Vec<CommandTrace>,
    /// Total gas used across all commands.
    pub total_gas_used: u64,
    /// Overall execution success.
    pub success: bool,
    /// Error message if execution failed.
    pub error: Option<String>,
    /// Index of the command that failed (if any).
    pub failed_command_index: Option<usize>,
    /// Object lifecycle summary at end of execution.
    pub object_summary: Option<ObjectLifecycleSummary>,
    /// Total execution duration in milliseconds.
    pub duration_ms: Option<u64>,
}

/// Trace for a single PTB command execution.
#[derive(Debug, Clone)]
pub struct CommandTrace {
    /// Command index (0-based).
    pub index: usize,
    /// Type of command (e.g., "MoveCall", "SplitCoins").
    pub command_type: String,
    /// Human-readable description of the command.
    pub description: String,
    /// Whether the command succeeded.
    pub success: bool,
    /// Gas used by this command.
    pub gas_used: u64,
    /// Error message if failed.
    pub error: Option<String>,
    /// Duration in microseconds.
    pub duration_us: Option<u64>,
    /// Number of return values produced.
    pub return_count: usize,
    /// Objects created by this command.
    pub objects_created: Vec<String>,
    /// Objects consumed by this command.
    pub objects_consumed: Vec<String>,
    /// For MoveCall: the function that was called.
    pub function_called: Option<FunctionCallInfo>,
}

/// Information about a function call in a PTB.
#[derive(Debug, Clone)]
pub struct FunctionCallInfo {
    /// Full module path (e.g., "0x2::coin").
    pub module: String,
    /// Function name.
    pub function: String,
    /// Type arguments.
    pub type_args: Vec<String>,
    /// Number of arguments passed.
    pub arg_count: usize,
}

impl PTBExecutionTrace {
    /// Create a new empty trace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a successful command trace.
    pub fn add_success(
        &mut self,
        index: usize,
        command_type: &str,
        description: String,
        gas_used: u64,
        return_count: usize,
    ) {
        self.commands.push(CommandTrace {
            index,
            command_type: command_type.to_string(),
            description,
            success: true,
            gas_used,
            error: None,
            duration_us: None,
            return_count,
            objects_created: Vec::new(),
            objects_consumed: Vec::new(),
            function_called: None,
        });
        self.total_gas_used += gas_used;
    }

    /// Add a failed command trace.
    pub fn add_failure(
        &mut self,
        index: usize,
        command_type: &str,
        description: String,
        error: String,
    ) {
        self.commands.push(CommandTrace {
            index,
            command_type: command_type.to_string(),
            description,
            success: false,
            gas_used: 0,
            error: Some(error.clone()),
            duration_us: None,
            return_count: 0,
            objects_created: Vec::new(),
            objects_consumed: Vec::new(),
            function_called: None,
        });
        self.success = false;
        self.error = Some(error);
        self.failed_command_index = Some(index);
    }

    /// Add function call info to the last command trace.
    pub fn add_function_call(&mut self, info: FunctionCallInfo) {
        if let Some(last) = self.commands.last_mut() {
            last.function_called = Some(info);
        }
    }

    /// Record objects created by the last command.
    pub fn record_created_objects(&mut self, objects: Vec<String>) {
        if let Some(last) = self.commands.last_mut() {
            last.objects_created = objects;
        }
    }

    /// Record objects consumed by the last command.
    pub fn record_consumed_objects(&mut self, objects: Vec<String>) {
        if let Some(last) = self.commands.last_mut() {
            last.objects_consumed = objects;
        }
    }

    /// Mark execution as complete.
    pub fn complete(&mut self, success: bool, duration_ms: Option<u64>) {
        self.success = success;
        self.duration_ms = duration_ms;
    }

    /// Get a summary of the execution.
    pub fn summary(&self) -> PTBTraceSummary {
        PTBTraceSummary {
            total_commands: self.commands.len(),
            successful_commands: self.commands.iter().filter(|c| c.success).count(),
            failed_commands: self.commands.iter().filter(|c| !c.success).count(),
            total_gas_used: self.total_gas_used,
            move_calls: self
                .commands
                .iter()
                .filter(|c| c.command_type == "MoveCall")
                .count(),
            transfers: self
                .commands
                .iter()
                .filter(|c| c.command_type == "TransferObjects")
                .count(),
            splits: self
                .commands
                .iter()
                .filter(|c| c.command_type == "SplitCoins")
                .count(),
            merges: self
                .commands
                .iter()
                .filter(|c| c.command_type == "MergeCoins")
                .count(),
        }
    }
}

/// Summary of a PTB execution trace.
#[derive(Debug, Clone, Default)]
pub struct PTBTraceSummary {
    /// Total number of commands.
    pub total_commands: usize,
    /// Number of successful commands.
    pub successful_commands: usize,
    /// Number of failed commands.
    pub failed_commands: usize,
    /// Total gas used.
    pub total_gas_used: u64,
    /// Number of MoveCall commands.
    pub move_calls: usize,
    /// Number of TransferObjects commands.
    pub transfers: usize,
    /// Number of SplitCoins commands.
    pub splits: usize,
    /// Number of MergeCoins commands.
    pub merges: usize,
}

/// An input value to the PTB.
#[derive(Debug, Clone)]
pub enum InputValue {
    /// A pure BCS-serialized value (primitives, vectors of primitives)
    Pure(Vec<u8>),

    /// An object input (by reference or by value)
    Object(ObjectInput),
}

/// How an object is passed to the PTB.
#[derive(Debug, Clone)]
pub enum ObjectInput {
    /// Object passed by immutable reference
    ImmRef {
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: Option<TypeTag>,
    },

    /// Object passed by mutable reference
    MutRef {
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: Option<TypeTag>,
    },

    /// Object passed by value (ownership transferred)
    Owned {
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: Option<TypeTag>,
    },

    /// Shared object
    Shared {
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: Option<TypeTag>,
    },
}

impl ObjectInput {
    pub fn id(&self) -> &ObjectID {
        match self {
            ObjectInput::ImmRef { id, .. } => id,
            ObjectInput::MutRef { id, .. } => id,
            ObjectInput::Owned { id, .. } => id,
            ObjectInput::Shared { id, .. } => id,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            ObjectInput::ImmRef { bytes, .. } => bytes,
            ObjectInput::MutRef { bytes, .. } => bytes,
            ObjectInput::Owned { bytes, .. } => bytes,
            ObjectInput::Shared { bytes, .. } => bytes,
        }
    }

    pub fn type_tag(&self) -> Option<&TypeTag> {
        match self {
            ObjectInput::ImmRef { type_tag, .. } => type_tag.as_ref(),
            ObjectInput::MutRef { type_tag, .. } => type_tag.as_ref(),
            ObjectInput::Owned { type_tag, .. } => type_tag.as_ref(),
            ObjectInput::Shared { type_tag, .. } => type_tag.as_ref(),
        }
    }
}

impl InputValue {
    /// Convert input to BCS bytes for passing to the VM.
    pub fn to_bcs(&self) -> Result<Vec<u8>> {
        match self {
            InputValue::Pure(bytes) => Ok(bytes.clone()),
            InputValue::Object(obj) => Ok(obj.bytes().to_vec()),
        }
    }
}

/// Result of executing a single command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Command returned no values
    Empty,

    /// Command returned one or more values (BCS-serialized)
    Values(Vec<Vec<u8>>),

    /// Command created objects (for Publish/Upgrade)
    Created(Vec<ObjectID>),
}

impl CommandResult {
    /// Get the primary (first) return value.
    pub fn primary_value(&self) -> Result<Vec<u8>> {
        match self {
            CommandResult::Empty => Err(anyhow!(
                "Command returned Empty result (no values). \
                 The function may return unit type or may not have been executed."
            )),
            CommandResult::Values(vs) if vs.is_empty() => Err(anyhow!(
                "Command returned an empty Values list. \
                 The function may return unit type or all values were filtered out."
            )),
            CommandResult::Values(vs) => Ok(vs[0].clone()),
            CommandResult::Created(ids) => Err(anyhow!(
                "Command returned {} created object IDs, not BCS values. \
                 Use CommandResult::created_ids() to access created objects.",
                ids.len()
            )),
        }
    }

    /// Get a specific return value by index.
    pub fn get(&self, index: usize) -> Result<Vec<u8>> {
        match self {
            CommandResult::Empty => Err(anyhow!(
                "Command returned Empty result; cannot get value at index {}",
                index
            )),
            CommandResult::Values(vs) => vs.get(index).cloned().ok_or_else(|| {
                anyhow!(
                    "Result index {} out of bounds: command returned {} value(s)",
                    index,
                    vs.len()
                )
            }),
            CommandResult::Created(ids) => Err(anyhow!(
                "Command returned {} created object IDs, not indexable values. \
                 Use CommandResult::created_ids() to access them.",
                ids.len()
            )),
        }
    }

    /// Get the number of return values.
    pub fn len(&self) -> usize {
        match self {
            CommandResult::Empty => 0,
            CommandResult::Values(vs) => vs.len(),
            CommandResult::Created(ids) => ids.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Update a specific value in the result (for mutable reference propagation).
    ///
    /// This is used when a Result/NestedResult is passed as a mutable reference
    /// to a subsequent command. The mutation needs to be reflected in the results
    /// so later commands see the updated state.
    ///
    /// Returns true if the update succeeded, false if the index was out of bounds
    /// or the result type doesn't support updates.
    pub fn update_value(&mut self, index: usize, new_bytes: Vec<u8>) -> bool {
        match self {
            CommandResult::Values(vs) => {
                if index < vs.len() {
                    vs[index] = new_bytes;
                    true
                } else {
                    false
                }
            }
            CommandResult::Empty | CommandResult::Created(_) => false,
        }
    }
}

/// Ownership status for tracking object mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// Owned by an address
    Address(AccountAddress),
    /// Shared object
    Shared,
    /// Immutable (frozen)
    Immutable,
}

/// Status of an object after PTB execution.
#[derive(Debug, Clone)]
pub enum ObjectChange {
    /// Object was created
    Created {
        id: ObjectID,
        owner: Owner,
        /// Type of the created object (if known)
        object_type: Option<TypeTag>,
    },
    /// Object was mutated
    Mutated {
        id: ObjectID,
        owner: Owner,
        /// Type of the mutated object (if known)
        object_type: Option<TypeTag>,
    },
    /// Object was deleted
    Deleted {
        id: ObjectID,
        /// Type of the deleted object (if known)
        object_type: Option<TypeTag>,
    },
    /// Object was wrapped (stored inside another object)
    Wrapped {
        id: ObjectID,
        /// Type of the wrapped object (if known)
        object_type: Option<TypeTag>,
    },
    /// Object was unwrapped (extracted from another object)
    Unwrapped {
        id: ObjectID,
        owner: Owner,
        /// Type of the unwrapped object (if known)
        object_type: Option<TypeTag>,
    },
    /// Object was transferred to another address.
    /// This is distinct from Mutated because it enables cross-PTB chaining:
    /// the transferred object can be received in a subsequent PTB.
    Transferred {
        id: ObjectID,
        /// The recipient address
        recipient: AccountAddress,
        /// Type of the transferred object (if known)
        object_type: Option<TypeTag>,
        /// The BCS bytes of the transferred object (for receiving)
        object_bytes: Vec<u8>,
    },
}

/// Effects of executing a PTB.
#[derive(Debug, Clone, Default)]
pub struct TransactionEffects {
    /// Objects that were created
    pub created: Vec<ObjectID>,

    /// Objects that were mutated
    pub mutated: Vec<ObjectID>,

    /// Objects that were deleted
    pub deleted: Vec<ObjectID>,

    /// Objects that were wrapped
    pub wrapped: Vec<ObjectID>,

    /// Objects that were unwrapped
    pub unwrapped: Vec<ObjectID>,

    /// Objects that were transferred to another address.
    /// These can be received in subsequent PTBs via the Receive command.
    pub transferred: Vec<ObjectID>,

    /// Objects that were received from pending_receives.
    /// These should be removed from the SimulationEnvironment's pending_receives.
    pub received: Vec<ObjectID>,

    /// Detailed object changes
    pub object_changes: Vec<ObjectChange>,

    /// Events emitted during execution
    pub events: Vec<EmittedEvent>,

    /// Gas used (always 0 in our unmetered execution)
    pub gas_used: u64,

    /// Whether execution succeeded
    pub success: bool,

    /// Error message if execution failed
    pub error: Option<String>,

    /// Return values from each command (BCS-encoded bytes).
    /// Each entry corresponds to a command in execution order.
    /// Commands that return nothing have an empty Vec.
    pub return_values: Vec<Vec<Vec<u8>>>,

    /// Index of the command that failed (0-based), if execution failed.
    pub failed_command_index: Option<usize>,

    /// Description of the failed command (e.g., "MoveCall 0x2::coin::split").
    pub failed_command_description: Option<String>,

    /// Number of commands that succeeded before the failure.
    pub commands_succeeded: usize,

    /// Mutated object bytes: id -> updated BCS bytes.
    /// Used by SimulationEnvironment to sync state back after PTB execution.
    pub mutated_object_bytes: HashMap<ObjectID, Vec<u8>>,

    /// Created object bytes: id -> BCS bytes.
    /// Used by SimulationEnvironment to populate newly created objects.
    pub created_object_bytes: HashMap<ObjectID, Vec<u8>>,

    /// Dynamic field entries: (parent_id, child_id) -> (type_tag, bytes).
    /// Used to sync Table/Bag state back to SimulationEnvironment.
    pub dynamic_field_entries: HashMap<(ObjectID, ObjectID), (TypeTag, Vec<u8>)>,

    /// Detailed error context for debugging failures.
    /// Populated when a command fails with information about the failure.
    pub error_context: Option<crate::error_context::CommandErrorContext>,

    /// Snapshot of execution state at the time of failure.
    /// Includes all objects loaded, commands that succeeded, etc.
    pub state_at_failure: Option<crate::error_context::ExecutionSnapshot>,
}

impl TransactionEffects {
    pub fn success() -> Self {
        Self {
            success: true,
            ..Default::default()
        }
    }

    pub fn failure(error: String) -> Self {
        Self {
            success: false,
            error: Some(error),
            ..Default::default()
        }
    }

    /// Create a failure at a specific command index.
    pub fn failure_at(
        error: String,
        command_index: usize,
        command_description: String,
        commands_succeeded: usize,
    ) -> Self {
        Self {
            success: false,
            error: Some(error),
            failed_command_index: Some(command_index),
            failed_command_description: Some(command_description),
            commands_succeeded,
            ..Default::default()
        }
    }

    /// Create a failure at a specific command index with full error context.
    pub fn failure_at_with_context(
        error: String,
        command_index: usize,
        command_description: String,
        commands_succeeded: usize,
        error_context: crate::error_context::CommandErrorContext,
        state_at_failure: crate::error_context::ExecutionSnapshot,
    ) -> Self {
        Self {
            success: false,
            error: Some(error),
            failed_command_index: Some(command_index),
            failed_command_description: Some(command_description),
            commands_succeeded,
            error_context: Some(error_context),
            state_at_failure: Some(state_at_failure),
            ..Default::default()
        }
    }
}

/// Executor for Programmable Transaction Blocks.
///
/// Manages inputs, executes commands in sequence, and tracks results
/// for chaining between commands.
pub struct PTBExecutor<'a, 'b> {
    /// Reference to the VM harness for executing Move functions
    vm: &'a mut VMHarness<'b>,

    /// Transaction inputs (pure values and objects)
    inputs: Vec<InputValue>,

    /// Results from each executed command
    results: Vec<CommandResult>,

    /// Objects created during execution (id -> (bytes, type))
    created_objects: HashMap<ObjectID, (Vec<u8>, Option<TypeTag>)>,

    /// Objects that were deleted (id -> type)
    deleted_objects: HashMap<ObjectID, Option<TypeTag>>,

    /// Objects that were mutated (id -> (new_bytes, type))
    /// Stores the updated BCS bytes after mutation for syncing back to environment
    mutated_objects: HashMap<ObjectID, (Vec<u8>, Option<TypeTag>)>,

    /// Counter for generating unique object IDs
    id_counter: u64,

    /// Pre-published packages: (package_id, upgrade_cap_id) pairs
    /// These are populated by SimulationEnvironment before execution
    pre_published: Vec<(ObjectID, ObjectID)>,

    /// Index into pre_published for the next Publish command
    publish_index: usize,

    /// Pre-upgraded packages: (new_package_id, receipt_id) pairs
    /// These are populated by SimulationEnvironment before execution
    pre_upgraded: Vec<(ObjectID, ObjectID)>,

    /// Index into pre_upgraded for the next Upgrade command
    upgrade_index: usize,

    /// Object ownership tracking: id -> Owner
    object_owners: HashMap<ObjectID, Owner>,

    /// Detailed object changes for the effects
    object_changes: Vec<ObjectChange>,

    /// Pending receives: objects transferred from previous transactions.
    /// Used by the Receive command for transaction chaining.
    /// Stores (bytes, optional type) for type validation.
    pending_receives: HashMap<ObjectID, (Vec<u8>, Option<TypeTag>)>,

    /// Transaction sender address
    sender: AccountAddress,

    /// Objects that have been consumed (passed by value and used).
    /// Prevents double-spending of owned objects.
    consumed_objects: HashSet<ObjectID>,

    /// Objects that the sender can transfer (Owned inputs + created objects).
    /// This tracks which objects came in as Owned (transferable by sender).
    transferable_objects: HashSet<ObjectID>,

    /// Accumulated gas used across all commands
    gas_used: u64,

    /// Optional gas budget limit. If set, execution fails when gas_used exceeds this.
    /// If None, no limit is enforced (unlimited gas).
    gas_budget: Option<u64>,

    /// Objects that were wrapped (stored inside another object).
    /// An object is wrapped when it's passed by value to a function and not returned.
    wrapped_objects: HashMap<ObjectID, Option<TypeTag>>,

    /// Objects that were received via the Receive command.
    /// Tracked so SimulationEnvironment can clear them from pending_receives.
    received_objects: Vec<ObjectID>,

    /// Objects that are immutable (cannot be mutated).
    /// If enforce_immutability is true, mutations to these will fail.
    immutable_objects: HashSet<ObjectID>,

    /// Whether to enforce immutability constraints.
    enforce_immutability: bool,

    /// Object lifecycle tracker for provenance and double-use detection.
    lifecycle_tracker: ObjectLifecycleTracker,

    /// Execution trace for debugging and analysis.
    execution_trace: PTBExecutionTrace,

    /// Whether to enable detailed lifecycle tracking.
    enable_lifecycle_tracking: bool,
}

impl<'a, 'b> PTBExecutor<'a, 'b> {
    /// Create a new PTB executor.
    pub fn new(vm: &'a mut VMHarness<'b>) -> Self {
        Self::with_sender(vm, AccountAddress::ZERO)
    }

    /// Create a new PTB executor with a specific sender address.
    pub fn with_sender(vm: &'a mut VMHarness<'b>, sender: AccountAddress) -> Self {
        Self {
            vm,
            inputs: Vec::new(),
            results: Vec::new(),
            created_objects: HashMap::new(),
            deleted_objects: HashMap::new(),
            mutated_objects: HashMap::new(),
            id_counter: 0,
            pre_published: Vec::new(),
            publish_index: 0,
            pre_upgraded: Vec::new(),
            upgrade_index: 0,
            object_owners: HashMap::new(),
            object_changes: Vec::new(),
            pending_receives: HashMap::new(),
            sender,
            gas_used: 0,
            consumed_objects: HashSet::new(),
            transferable_objects: HashSet::new(),
            gas_budget: None,
            wrapped_objects: HashMap::new(),
            received_objects: Vec::new(),
            immutable_objects: HashSet::new(),
            enforce_immutability: false,
            lifecycle_tracker: ObjectLifecycleTracker::new(),
            execution_trace: PTBExecutionTrace::new(),
            enable_lifecycle_tracking: true,
        }
    }

    /// Create a PTB executor with pre-published package info.
    /// Used by SimulationEnvironment to pass package/UpgradeCap IDs.
    pub fn new_with_published(
        vm: &'a mut VMHarness<'b>,
        pre_published: Vec<(ObjectID, ObjectID)>,
    ) -> Self {
        Self::new_with_packages(vm, pre_published, Vec::new())
    }

    /// Create a PTB executor with both pre-published and pre-upgraded package info.
    /// Used by SimulationEnvironment to pass package IDs for Publish and Upgrade commands.
    pub fn new_with_packages(
        vm: &'a mut VMHarness<'b>,
        pre_published: Vec<(ObjectID, ObjectID)>,
        pre_upgraded: Vec<(ObjectID, ObjectID)>,
    ) -> Self {
        Self::new_with_packages_and_sender(vm, pre_published, pre_upgraded, AccountAddress::ZERO)
    }

    /// Create a PTB executor with pre-published/pre-upgraded package info and a sender address.
    /// This is the full constructor used by SimulationEnvironment.
    pub fn new_with_packages_and_sender(
        vm: &'a mut VMHarness<'b>,
        pre_published: Vec<(ObjectID, ObjectID)>,
        pre_upgraded: Vec<(ObjectID, ObjectID)>,
        sender: AccountAddress,
    ) -> Self {
        Self {
            vm,
            inputs: Vec::new(),
            results: Vec::new(),
            created_objects: HashMap::new(),
            deleted_objects: HashMap::new(),
            mutated_objects: HashMap::new(),
            id_counter: 0,
            pre_published,
            publish_index: 0,
            pre_upgraded,
            upgrade_index: 0,
            object_owners: HashMap::new(),
            object_changes: Vec::new(),
            pending_receives: HashMap::new(),
            sender,
            gas_used: 0,
            consumed_objects: HashSet::new(),
            transferable_objects: HashSet::new(),
            gas_budget: None,
            wrapped_objects: HashMap::new(),
            received_objects: Vec::new(),
            immutable_objects: HashSet::new(),
            enforce_immutability: false,
            lifecycle_tracker: ObjectLifecycleTracker::new(),
            execution_trace: PTBExecutionTrace::new(),
            enable_lifecycle_tracking: true,
        }
    }

    /// Set the gas budget for this PTB execution.
    /// If gas usage exceeds this budget, execution will fail with an out-of-gas error.
    /// Pass None to disable gas budget enforcement (unlimited gas).
    pub fn set_gas_budget(&mut self, budget: Option<u64>) {
        self.gas_budget = budget;
    }

    /// Get the current gas budget, if set.
    pub fn gas_budget(&self) -> Option<u64> {
        self.gas_budget
    }

    /// Get the accumulated gas used during execution.
    pub fn gas_used(&self) -> u64 {
        self.gas_used
    }

    /// Get a reference to the execution trace.
    /// This contains detailed information about each command that was executed.
    pub fn execution_trace(&self) -> &PTBExecutionTrace {
        &self.execution_trace
    }

    /// Get a reference to the object lifecycle tracker.
    /// This contains provenance and state information for all objects.
    pub fn lifecycle_tracker(&self) -> &ObjectLifecycleTracker {
        &self.lifecycle_tracker
    }

    /// Get a summary of the execution trace.
    pub fn trace_summary(&self) -> PTBTraceSummary {
        self.execution_trace.summary()
    }

    /// Get a summary of object lifecycle operations.
    pub fn lifecycle_summary(&self) -> ObjectLifecycleSummary {
        self.lifecycle_tracker.summary()
    }

    /// Enable or disable detailed lifecycle tracking.
    /// When disabled, lifecycle_tracker still exists but won't record input objects.
    pub fn set_enable_lifecycle_tracking(&mut self, enable: bool) {
        self.enable_lifecycle_tracking = enable;
    }

    /// Check if lifecycle tracking is enabled.
    pub fn is_lifecycle_tracking_enabled(&self) -> bool {
        self.enable_lifecycle_tracking
    }

    /// Check if the current gas usage exceeds the budget.
    /// Returns an error if over budget, Ok(()) otherwise.
    fn check_gas_budget(&self) -> Result<()> {
        if let Some(budget) = self.gas_budget {
            if self.gas_used > budget {
                return Err(anyhow!(
                    "out of gas: used {} but budget is {} (exceeded by {})",
                    self.gas_used,
                    budget,
                    self.gas_used - budget
                ));
            }
        }
        Ok(())
    }

    /// Enable or disable immutability enforcement.
    /// When enabled, mutations to immutable objects will fail with an error.
    pub fn set_enforce_immutability(&mut self, enforce: bool) {
        self.enforce_immutability = enforce;
    }

    /// Mark an object as immutable.
    /// If enforce_immutability is true, mutations to this object will fail.
    pub fn mark_immutable(&mut self, object_id: ObjectID) {
        self.immutable_objects.insert(object_id);
    }

    /// Check if an object is marked as immutable.
    pub fn is_immutable(&self, object_id: &ObjectID) -> bool {
        self.immutable_objects.contains(object_id)
    }

    /// Check if mutating an object is allowed. Returns an error if the object is immutable
    /// and enforcement is enabled.
    fn check_mutation_allowed(&self, object_id: &ObjectID) -> Result<()> {
        if self.enforce_immutability && self.immutable_objects.contains(object_id) {
            return Err(anyhow!(
                "cannot mutate immutable object {}",
                object_id.to_hex_literal()
            ));
        }
        Ok(())
    }

    /// Mark an object as wrapped (stored inside another object).
    /// This is called when an object is consumed by value but not returned.
    pub fn mark_wrapped(&mut self, object_id: ObjectID, object_type: Option<TypeTag>) {
        self.wrapped_objects.insert(object_id, object_type.clone());
        self.object_changes.push(ObjectChange::Wrapped {
            id: object_id,
            object_type,
        });
    }

    /// Mark an object as unwrapped (extracted from another object).
    pub fn mark_unwrapped(
        &mut self,
        object_id: ObjectID,
        owner: Owner,
        object_type: Option<TypeTag>,
    ) {
        // Remove from wrapped if it was there
        self.wrapped_objects.remove(&object_id);
        self.object_changes.push(ObjectChange::Unwrapped {
            id: object_id,
            owner,
            object_type,
        });
    }

    /// Add a pure value input (BCS-serialized).
    pub fn add_pure_input(&mut self, bytes: Vec<u8>) -> Result<u16> {
        let idx = self.inputs.len();
        if idx > u16::MAX as usize {
            return Err(anyhow!("too many inputs"));
        }
        self.inputs.push(InputValue::Pure(bytes));
        Ok(idx as u16)
    }

    /// Add an object input.
    pub fn add_object_input(&mut self, obj: ObjectInput) -> Result<u16> {
        let idx = self.inputs.len();
        if idx > u16::MAX as usize {
            return Err(anyhow!("too many inputs"));
        }
        // Track Owned objects as transferable by the sender
        if let ObjectInput::Owned { id, .. } = &obj {
            self.transferable_objects.insert(*id);
            self.object_owners.insert(*id, Owner::Address(self.sender));
        }
        self.inputs.push(InputValue::Object(obj));
        Ok(idx as u16)
    }

    /// Add an input value (pure or object).
    /// For object inputs, this tracks ownership for transfer validation.
    pub fn add_input(&mut self, input: InputValue) -> u16 {
        let idx = self.inputs.len();
        // Track Owned objects as transferable by the sender
        if let InputValue::Object(ObjectInput::Owned { id, .. }) = &input {
            self.transferable_objects.insert(*id);
            self.object_owners.insert(*id, Owner::Address(self.sender));
        }
        self.inputs.push(input);
        idx as u16
    }

    /// Update an input's bytes in place (used by MergeCoins).
    fn update_input_bytes(&mut self, index: u16, new_bytes: Vec<u8>) -> Result<()> {
        let input = self
            .inputs
            .get_mut(index as usize)
            .ok_or_else(|| anyhow!("input index {} out of bounds", index))?;
        match input {
            InputValue::Object(obj) => match obj {
                ObjectInput::Owned { bytes, .. } => *bytes = new_bytes,
                ObjectInput::Shared { bytes, .. } => *bytes = new_bytes,
                ObjectInput::ImmRef { bytes, .. } => *bytes = new_bytes,
                ObjectInput::MutRef { bytes, .. } => *bytes = new_bytes,
            },
            InputValue::Pure(bytes) => *bytes = new_bytes,
        }
        Ok(())
    }

    /// Execute a list of commands and return the effects.
    /// This is an alias for `execute` that takes a slice instead of owned Vec.
    pub fn execute_commands(&mut self, commands: &[Command]) -> Result<TransactionEffects> {
        self.execute(commands.to_vec())
    }

    /// Generate a fresh object ID.
    fn fresh_id(&mut self) -> ObjectID {
        let id = self.id_counter;
        self.id_counter += 1;
        // Create a deterministic ID based on counter
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&id.to_le_bytes());
        AccountAddress::new(bytes)
    }

    /// Resolve an argument to its BCS bytes.
    fn resolve_arg(&self, arg: &Argument) -> Result<Vec<u8>> {
        match arg {
            Argument::Input(i) => {
                let input = self
                    .inputs
                    .get(*i as usize)
                    .ok_or_else(|| anyhow!("input index {} out of bounds", i))?;
                input.to_bcs()
            }
            Argument::Result(cmd_idx) => {
                let result =
                    self.results.get(*cmd_idx as usize).ok_or_else(|| {
                        anyhow!(
                        "Result({}): command index {} out of bounds (only {} commands executed)",
                        cmd_idx, cmd_idx, self.results.len()
                    )
                    })?;
                result
                    .primary_value()
                    .map_err(|e| anyhow!("Result({}): {}", cmd_idx, e))
            }
            Argument::NestedResult(cmd_idx, val_idx) => {
                let result = self
                    .results
                    .get(*cmd_idx as usize)
                    .ok_or_else(|| anyhow!(
                        "NestedResult({}, {}): command index {} out of bounds (only {} commands executed)",
                        cmd_idx, val_idx, cmd_idx, self.results.len()
                    ))?;
                result.get(*val_idx as usize).map_err(|e| {
                    anyhow!(
                        "NestedResult({}, {}): {}. Command {} returned {} value(s).",
                        cmd_idx,
                        val_idx,
                        e,
                        cmd_idx,
                        result.len()
                    )
                })
            }
        }
    }

    /// Resolve multiple arguments to BCS bytes.
    fn resolve_args(&self, args: &[Argument]) -> Result<Vec<Vec<u8>>> {
        args.iter().map(|arg| self.resolve_arg(arg)).collect()
    }

    /// Execute a single command.
    fn execute_command(&mut self, cmd: Command) -> Result<CommandResult> {
        match cmd {
            Command::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => self.execute_move_call(package, module, function, type_args, args),

            Command::SplitCoins { coin, amounts } => self.execute_split_coins(coin, amounts),

            Command::MergeCoins {
                destination,
                sources,
            } => self.execute_merge_coins(destination, sources),

            Command::TransferObjects { objects, address } => {
                self.execute_transfer_objects(objects, address)
            }

            Command::MakeMoveVec { type_tag, elements } => {
                self.execute_make_move_vec(type_tag, elements)
            }

            Command::Publish { modules, dep_ids } => self.execute_publish(modules, dep_ids),

            Command::Upgrade {
                modules,
                package,
                ticket,
            } => self.execute_upgrade(modules, package, ticket),

            Command::Receive {
                object_id,
                object_type,
            } => self.execute_receive(&object_id, object_type.as_ref()),
        }
    }

    /// Execute a MoveCall command.
    ///
    /// This method automatically handles TxContext injection for entry functions.
    /// Sui entry functions receive TxContext as an implicit last argument from the runtime.
    /// It also tracks mutable reference outputs to update object state.
    fn execute_move_call(
        &mut self,
        package: AccountAddress,
        module: Identifier,
        function: Identifier,
        type_args: Vec<TypeTag>,
        args: Vec<Argument>,
    ) -> Result<CommandResult> {
        let mut resolved_args = self.resolve_args(&args)?;
        let module_id = ModuleId::new(package, module.clone());

        // Track which arguments map to which object IDs and their original Argument reference.
        // We need the Argument to update input bytes for subsequent commands.
        let arg_to_info: Vec<(Argument, Option<ObjectID>)> = args
            .iter()
            .map(|arg| {
                (
                    *arg,
                    self.get_object_id_and_type_from_arg(arg).map(|(id, _)| id),
                )
            })
            .collect();

        // First attempt: execute as-is
        match self.vm.execute_function_full(
            &module_id,
            function.as_str(),
            type_args.clone(),
            resolved_args.clone(),
        ) {
            Ok(output) => {
                // Track mutations from mutable reference outputs
                self.apply_mutable_ref_outputs(&arg_to_info, &output.mutable_ref_outputs)?;

                // Accumulate gas
                self.gas_used += output.gas_used;

                if output.return_values.is_empty() {
                    Ok(CommandResult::Empty)
                } else {
                    Ok(CommandResult::Values(output.return_values))
                }
            }
            Err(e) => {
                // Check if this is an argument count mismatch - might need TxContext
                let err_msg = e.to_string();
                if err_msg.contains("argument length mismatch")
                    || err_msg.contains("NUMBER_OF_ARGUMENTS_MISMATCH")
                {
                    // Try again with TxContext appended
                    let tx_context_bytes = self.vm.synthesize_tx_context()?;
                    resolved_args.push(tx_context_bytes);

                    match self.vm.execute_function_full(
                        &module_id,
                        function.as_str(),
                        type_args,
                        resolved_args,
                    ) {
                        Ok(output) => {
                            // Track mutations from mutable reference outputs
                            self.apply_mutable_ref_outputs(
                                &arg_to_info,
                                &output.mutable_ref_outputs,
                            )?;

                            // Accumulate gas
                            self.gas_used += output.gas_used;

                            if output.return_values.is_empty() {
                                return Ok(CommandResult::Empty);
                            } else {
                                return Ok(CommandResult::Values(output.return_values));
                            }
                        }
                        Err(e2) => {
                            // TxContext injection didn't help - return the retry error
                            // which is more informative about the actual problem
                            return Err(e2);
                        }
                    }
                }
                Err(e)
            }
        }
    }

    /// Apply mutable reference outputs from a MoveCall to track object mutations.
    /// This maps the VM's argument indices back to object IDs and updates input/result bytes
    /// so subsequent commands see the modified state.
    ///
    /// ## Mutable Reference Propagation
    ///
    /// When a MoveCall mutates an argument passed by mutable reference, the VM returns
    /// the updated bytes in `mutable_ref_outputs`. This function propagates those changes:
    ///
    /// 1. **Input arguments**: Update the input bytes directly
    /// 2. **Result arguments**: Update the stored result so subsequent commands see the mutation
    /// 3. **NestedResult arguments**: Update the specific value in the multi-return result
    fn apply_mutable_ref_outputs(
        &mut self,
        arg_to_info: &[(Argument, Option<ObjectID>)],
        mutable_ref_outputs: &[(u8, Vec<u8>)],
    ) -> Result<()> {
        for (arg_idx, new_bytes) in mutable_ref_outputs {
            let idx = *arg_idx as usize;
            if idx < arg_to_info.len() {
                let (original_arg, maybe_object_id) = &arg_to_info[idx];

                // Record the mutation in our tracking map
                if let Some(object_id) = maybe_object_id {
                    // Check immutability enforcement before allowing mutation
                    self.check_mutation_allowed(object_id)?;

                    // Get the type from the existing tracking if available
                    let existing_type = self
                        .mutated_objects
                        .get(object_id)
                        .and_then(|(_, t)| t.clone());

                    // Record the mutation with updated bytes
                    self.mutated_objects
                        .insert(*object_id, (new_bytes.clone(), existing_type));
                }

                // CRITICAL: Update the stored value in place so subsequent commands
                // see the modified object state.
                match original_arg {
                    Argument::Input(input_idx) => {
                        self.update_input_bytes(*input_idx, new_bytes.clone())?;
                    }
                    Argument::Result(cmd_idx) => {
                        // Update the primary result value from this command
                        if let Some(result) = self.results.get_mut(*cmd_idx as usize) {
                            result.update_value(0, new_bytes.clone());
                        }
                    }
                    Argument::NestedResult(cmd_idx, val_idx) => {
                        // Update the specific nested value
                        if let Some(result) = self.results.get_mut(*cmd_idx as usize) {
                            result.update_value(*val_idx as usize, new_bytes.clone());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Execute a SplitCoins command.
    ///
    /// In a real Sui execution, this would:
    /// 1. Take a Coin<T> and a list of amounts
    /// 2. Create new Coin<T> objects with those amounts
    /// 3. Reduce the original coin's balance
    ///
    /// For our sandbox, we simulate this by:
    /// 1. Parsing the input coin bytes (UID + Balance { value: u64 })
    /// 2. Creating new coin bytes for each amount
    fn execute_split_coins(
        &mut self,
        coin: Argument,
        amounts: Vec<Argument>,
    ) -> Result<CommandResult> {
        let coin_bytes = self.resolve_arg(&coin)?;
        let amount_bytes: Vec<Vec<u8>> = self.resolve_args(&amounts)?;

        // Parse amounts (they should be u64 values)
        let amounts: Vec<u64> = amount_bytes
            .iter()
            .map(|bytes| {
                if bytes.len() != 8 {
                    return Err(anyhow!("amount must be u64 (8 bytes), got {}", bytes.len()));
                }
                // Safe: we just verified length is exactly 8
                let arr: [u8; 8] = bytes[..8].try_into().expect("length checked above");
                Ok(u64::from_le_bytes(arr))
            })
            .collect::<Result<Vec<_>>>()?;

        // Coin structure: { id: UID (32 bytes), balance: Balance<T> { value: u64 } }
        // UID is 32 bytes, then value is 8 bytes
        if coin_bytes.len() < 40 {
            return Err(anyhow!(
                "coin bytes too short: expected at least 40, got {}",
                coin_bytes.len()
            ));
        }

        // Safe: we just verified length >= 40
        let original_value = u64::from_le_bytes(
            coin_bytes[32..40]
                .try_into()
                .expect("slice is exactly 8 bytes"),
        );

        // Check we have enough balance
        let total_split: u64 = amounts.iter().sum();
        if total_split > original_value {
            return Err(anyhow!(
                "insufficient balance: have {}, trying to split {}",
                original_value,
                total_split
            ));
        }

        // Try to get the coin type from the input argument
        let coin_type = self
            .get_object_id_and_type_from_arg(&coin)
            .and_then(|(_, t)| t)
            .or_else(|| {
                // Default to Coin<SUI> if type not known
                Some(well_known::types::sui_coin())
            });

        // Create new coins for each amount
        let mut new_coins = Vec::new();
        for amount in &amounts {
            let new_id = self.fresh_id();
            let mut new_coin_bytes = Vec::with_capacity(40);
            new_coin_bytes.extend_from_slice(new_id.as_ref());
            new_coin_bytes.extend_from_slice(&amount.to_le_bytes());
            self.created_objects
                .insert(new_id, (new_coin_bytes.clone(), coin_type.clone()));
            new_coins.push(new_coin_bytes);
        }

        // Mark original coin as mutated (balance reduced)
        // Calculate new balance and create updated coin bytes
        let new_balance = original_value - total_split;
        let mut updated_coin_bytes = coin_bytes.clone();
        updated_coin_bytes[32..40].copy_from_slice(&new_balance.to_le_bytes());

        if let Some((obj_id, _)) = self.get_object_id_and_type_from_arg(&coin) {
            self.mutated_objects
                .insert(obj_id, (updated_coin_bytes.clone(), coin_type.clone()));
        }

        // Also update the input in place so subsequent commands see the new balance
        if let Argument::Input(idx) = coin {
            self.update_input_bytes(idx, updated_coin_bytes)?;
        }

        // Estimate gas: native call + object mutation + object creation per new coin
        let num_new_coins = new_coins.len() as u64;
        self.gas_used += gas_costs::NATIVE_CALL
            + gas_costs::OBJECT_MUTATE  // original coin mutated
            + num_new_coins * gas_costs::OBJECT_CREATE  // new coins created
            + num_new_coins * 40 * gas_costs::STORAGE_BYTE; // storage for new coins

        Ok(CommandResult::Values(new_coins))
    }

    /// Execute a MergeCoins command.
    ///
    /// Merges multiple source coins into the destination coin.
    /// Source coins are destroyed, destination coin's balance increases.
    /// IMPORTANT: MergeCoins modifies the destination IN PLACE - subsequent
    /// reads of the destination Input will see the merged balance.
    fn execute_merge_coins(
        &mut self,
        destination: Argument,
        sources: Vec<Argument>,
    ) -> Result<CommandResult> {
        let dest_bytes = self.resolve_arg(&destination)?;
        let source_bytes_list = self.resolve_args(&sources)?;

        if dest_bytes.len() < 40 {
            return Err(anyhow!(
                "destination coin bytes too short: expected at least 40, got {}",
                dest_bytes.len()
            ));
        }

        // Safe: we just verified length >= 40
        let dest_value = u64::from_le_bytes(
            dest_bytes[32..40]
                .try_into()
                .expect("slice is exactly 8 bytes"),
        );

        // Sum up all source values
        let mut total_merge: u64 = 0;
        for (i, source_bytes) in source_bytes_list.iter().enumerate() {
            if source_bytes.len() < 40 {
                return Err(anyhow!(
                    "source coin {} bytes too short: expected at least 40, got {}",
                    i,
                    source_bytes.len()
                ));
            }
            // Safe: we just verified length >= 40
            let source_value = u64::from_le_bytes(
                source_bytes[32..40]
                    .try_into()
                    .expect("slice is exactly 8 bytes"),
            );
            total_merge = total_merge
                .checked_add(source_value)
                .ok_or_else(|| anyhow!("merge would overflow"))?;
        }

        // Create new destination with merged balance
        let new_value = dest_value
            .checked_add(total_merge)
            .ok_or_else(|| anyhow!("merge would overflow"))?;

        let mut new_dest_bytes = Vec::with_capacity(40);
        new_dest_bytes.extend_from_slice(&dest_bytes[0..32]); // Keep same UID
        new_dest_bytes.extend_from_slice(&new_value.to_le_bytes());

        // Update the destination input IN PLACE so subsequent commands see the merged balance
        if let Argument::Input(idx) = destination {
            self.update_input_bytes(idx, new_dest_bytes.clone())?;
        }

        // Get coin type for tracking
        let coin_type = self
            .get_object_id_and_type_from_arg(&destination)
            .and_then(|(_, t)| t);

        // Mark destination as mutated with the new bytes
        if let Some((dest_id, _)) = self.get_object_id_and_type_from_arg(&destination) {
            self.mutated_objects
                .insert(dest_id, (new_dest_bytes.clone(), coin_type.clone()));
        }

        // Sources are destroyed (track as deleted)
        for source in &sources {
            // Mark source as deleted with type info
            if let Some((source_id, _)) = self.get_object_id_and_type_from_arg(source) {
                self.deleted_objects.insert(source_id, coin_type.clone());
            }

            if let Argument::Input(idx) = source {
                // Mark source as deleted (set balance to 0)
                let source_bytes = self.resolve_arg(source)?;
                if source_bytes.len() >= 40 {
                    let mut zeroed = source_bytes.clone();
                    zeroed[32..40].fill(0);
                    self.update_input_bytes(*idx, zeroed)?;
                }
            }
        }

        // Estimate gas: native call + object mutation + object deletion per source
        let num_sources = sources.len() as u64;
        self.gas_used += gas_costs::NATIVE_CALL
            + gas_costs::OBJECT_MUTATE  // destination coin mutated
            + num_sources * gas_costs::OBJECT_DELETE; // source coins deleted

        // MergeCoins returns empty (no return value in Sui PTB semantics)
        Ok(CommandResult::Empty)
    }

    /// Execute a TransferObjects command.
    ///
    /// Transfers ownership of objects to the specified address.
    /// Transfer objects to a new owner.
    /// Validates that:
    /// 1. The sender owns or created the objects being transferred
    /// 2. The objects haven't already been consumed
    fn execute_transfer_objects(
        &mut self,
        objects: Vec<Argument>,
        address: Argument,
    ) -> Result<CommandResult> {
        // Resolve the address (should be 32 bytes)
        let addr_bytes = self.resolve_arg(&address)?;
        if addr_bytes.len() != 32 {
            return Err(anyhow!(
                "address must be 32 bytes, got {}",
                addr_bytes.len()
            ));
        }

        let recipient = AccountAddress::from_bytes(&addr_bytes)
            .map_err(|e| anyhow!("Invalid address: {}", e))?;
        let new_owner = Owner::Address(recipient);

        // First pass: validate all objects can be transferred
        let mut objects_to_transfer: Vec<(ObjectID, Option<TypeTag>)> = Vec::new();

        for obj_arg in &objects {
            if let Some((obj_id, obj_type)) = self.get_object_id_and_type_from_arg(obj_arg) {
                // Check if object has already been consumed
                if self.consumed_objects.contains(&obj_id) {
                    return Err(anyhow!(
                        "cannot transfer object {}: already consumed in this transaction",
                        obj_id.to_hex_literal()
                    ));
                }

                // Check if sender can transfer this object
                // Transferable objects are: Owned inputs, created objects in this PTB
                let can_transfer = self.transferable_objects.contains(&obj_id)
                    || self.created_objects.contains_key(&obj_id);

                if !can_transfer {
                    // Check if it's a shared object (can't transfer shared objects)
                    if let Some(input) = self.get_input_for_object_id(&obj_id) {
                        if matches!(input, InputValue::Object(ObjectInput::Shared { .. })) {
                            return Err(anyhow!(
                                "cannot transfer shared object {}",
                                obj_id.to_hex_literal()
                            ));
                        }
                        // ImmRef and MutRef are not transferable (borrowed, not owned)
                        if matches!(
                            input,
                            InputValue::Object(
                                ObjectInput::ImmRef { .. } | ObjectInput::MutRef { .. }
                            )
                        ) {
                            return Err(anyhow!(
                                "cannot transfer borrowed object {}: only owned objects can be transferred",
                                obj_id.to_hex_literal()
                            ));
                        }
                    }
                    return Err(anyhow!(
                        "cannot transfer object {}: sender does not own it",
                        obj_id.to_hex_literal()
                    ));
                }

                objects_to_transfer.push((obj_id, obj_type));
            }
        }

        // Second pass: actually transfer the objects
        for (obj_id, obj_type) in objects_to_transfer {
            // Mark as consumed (can't use again in this PTB)
            self.consumed_objects.insert(obj_id);

            // Remove from transferable (new owner's objects aren't our transferable anymore)
            self.transferable_objects.remove(&obj_id);

            // Update ownership tracking
            self.object_owners.insert(obj_id, new_owner);

            // Get the current bytes for the object (needed for cross-PTB receiving)
            let obj_bytes = self.get_object_bytes(&obj_id).unwrap_or_default();

            // Record the transfer with bytes so it can be received in the next PTB
            // Note: We intentionally do NOT add to mutated_objects because Transferred
            // is a distinct change type that should not be duplicated as Mutated.
            self.object_changes.push(ObjectChange::Transferred {
                id: obj_id,
                recipient,
                object_type: obj_type,
                object_bytes: obj_bytes,
            });
        }

        // Estimate gas: native call + object mutation per transferred object
        let num_objects = objects.len() as u64;
        self.gas_used += gas_costs::NATIVE_CALL + num_objects * gas_costs::OBJECT_MUTATE; // ownership change counts as mutation

        // TransferObjects has no return value
        Ok(CommandResult::Empty)
    }

    /// Get the input value for a given object ID, if it exists.
    fn get_input_for_object_id(&self, object_id: &ObjectID) -> Option<&InputValue> {
        self.inputs.iter().find(|input| {
            if let InputValue::Object(obj) = input {
                obj.id() == object_id
            } else {
                false
            }
        })
    }

    /// Get the current bytes for an object (from inputs, results, or created objects).
    fn get_object_bytes(&self, object_id: &ObjectID) -> Option<Vec<u8>> {
        // Check inputs
        for input in &self.inputs {
            if let InputValue::Object(obj) = input {
                if obj.id() == object_id {
                    return Some(obj.bytes().to_vec());
                }
            }
        }
        // Check created objects
        if let Some((bytes, _)) = self.created_objects.get(object_id) {
            return Some(bytes.clone());
        }
        // Check mutated objects
        if let Some((bytes, _)) = self.mutated_objects.get(object_id) {
            return Some(bytes.clone());
        }
        None
    }

    /// Try to extract an object ID and its type from an Argument.
    fn get_object_id_and_type_from_arg(
        &self,
        arg: &Argument,
    ) -> Option<(ObjectID, Option<TypeTag>)> {
        match arg {
            Argument::Input(idx) => {
                if let Some(InputValue::Object(obj)) = self.inputs.get(*idx as usize) {
                    // Get type from the input if available
                    Some((*obj.id(), obj.type_tag().cloned()))
                } else {
                    None
                }
            }
            Argument::Result(idx) => {
                // Check if this result created an object
                if let Some(CommandResult::Created(ids)) = self.results.get(*idx as usize) {
                    if let Some(id) = ids.first() {
                        // Look up the type from created_objects
                        let obj_type = self.created_objects.get(id).and_then(|(_, t)| t.clone());
                        Some((*id, obj_type))
                    } else {
                        None
                    }
                } else if let Some(CommandResult::Values(vs)) = self.results.get(*idx as usize) {
                    // Try to extract UID from first 32 bytes
                    if let Some(bytes) = vs.first() {
                        if bytes.len() >= 32 {
                            if let Ok(id) = AccountAddress::from_bytes(&bytes[..32]) {
                                // Look up type from created_objects
                                let obj_type =
                                    self.created_objects.get(&id).and_then(|(_, t)| t.clone());
                                Some((id, obj_type))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Argument::NestedResult(cmd_idx, val_idx) => {
                if let Some(CommandResult::Created(ids)) = self.results.get(*cmd_idx as usize) {
                    if let Some(id) = ids.get(*val_idx as usize) {
                        let obj_type = self.created_objects.get(id).and_then(|(_, t)| t.clone());
                        Some((*id, obj_type))
                    } else {
                        None
                    }
                } else if let Some(CommandResult::Values(vs)) = self.results.get(*cmd_idx as usize)
                {
                    if let Some(bytes) = vs.get(*val_idx as usize) {
                        if bytes.len() >= 32 {
                            if let Ok(id) = AccountAddress::from_bytes(&bytes[..32]) {
                                let obj_type =
                                    self.created_objects.get(&id).and_then(|(_, t)| t.clone());
                                Some((id, obj_type))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        }
    }

    /// Execute a MakeMoveVec command.
    ///
    /// Creates a vector from the given elements.
    fn execute_make_move_vec(
        &mut self,
        type_tag: Option<TypeTag>,
        elements: Vec<Argument>,
    ) -> Result<CommandResult> {
        // Validate: if elements is empty, type_tag must be provided
        // (Sui requires knowing the element type to create an empty vector)
        if elements.is_empty() && type_tag.is_none() {
            return Err(anyhow!(
                "MakeMoveVec with no elements requires a type_tag to specify the element type"
            ));
        }

        // For non-empty vectors, validate element sizes are consistent
        // (primitive types should have fixed sizes within the same vector)
        let element_bytes = self.resolve_args(&elements)?;

        if !element_bytes.is_empty() {
            // Check if this looks like a vector of fixed-size primitives
            // Primitive sizes: bool=1, u8=1, u16=2, u32=4, u64=8, u128=16, u256=32, address=32
            let first_len = element_bytes[0].len();
            let is_likely_primitive = matches!(first_len, 1 | 2 | 4 | 8 | 16 | 32);

            if is_likely_primitive {
                for (i, elem) in element_bytes.iter().enumerate() {
                    if elem.len() != first_len {
                        return Err(anyhow!(
                            "MakeMoveVec: element {} has {} bytes but element 0 has {} bytes. \
                             All elements must have the same type.",
                            i,
                            elem.len(),
                            first_len
                        ));
                    }
                }
            }
            // For non-primitive types (structs, nested vectors), sizes can vary
            // because BCS encoding includes length prefixes. We can't easily validate those.
        }

        // BCS vector format: length prefix (ULEB128) followed by elements
        let mut vec_bytes = Vec::new();

        // Write length as ULEB128
        let len = element_bytes.len();
        let mut remaining = len;
        loop {
            let mut byte = (remaining & 0x7F) as u8;
            remaining >>= 7;
            if remaining != 0 {
                byte |= 0x80;
            }
            vec_bytes.push(byte);
            if remaining == 0 {
                break;
            }
        }

        // Append all elements
        for elem in element_bytes {
            vec_bytes.extend(elem);
        }

        // Estimate gas: native call + input bytes + output bytes
        self.gas_used += gas_costs::NATIVE_CALL + (vec_bytes.len() as u64) * gas_costs::OUTPUT_BYTE;

        // Note: type_tag is used by the VM for type checking at the Move level.
        // In our simulation, we trust the caller provides the correct type.
        // The type_tag is stored in Sui's type registry but not in the BCS bytes.
        let _ = type_tag; // Acknowledged: type is used for Move VM type checking

        Ok(CommandResult::Values(vec![vec_bytes]))
    }

    /// Execute a Publish command.
    ///
    /// Execute a Publish command to deploy new modules.
    ///
    /// When executed through SimulationEnvironment.execute_ptb(), modules are
    /// pre-published and this just returns the already-created IDs.
    /// When executed standalone, this returns an error since the resolver
    /// can't be modified mid-execution.
    fn execute_publish(
        &mut self,
        _modules: Vec<Vec<u8>>,
        _dep_ids: Vec<ObjectID>,
    ) -> Result<CommandResult> {
        // Check if we have pre-published info from SimulationEnvironment
        if self.publish_index < self.pre_published.len() {
            let (package_id, upgrade_cap_id) = self.pre_published[self.publish_index];
            self.publish_index += 1;

            // UpgradeCap type: 0x2::package::UpgradeCap
            let upgrade_cap_type = well_known::types::UPGRADE_CAP_TYPE.clone();

            // Mark the UpgradeCap as created with its type
            self.created_objects
                .insert(upgrade_cap_id, (Vec::new(), Some(upgrade_cap_type)));
            // Created objects are transferable by the sender
            self.transferable_objects.insert(upgrade_cap_id);
            self.object_owners
                .insert(upgrade_cap_id, Owner::Address(self.sender));

            // Estimate gas: publishing is expensive - base cost + per-module cost
            // Note: actual gas is computed when modules are loaded in pre_publish_modules
            self.gas_used += gas_costs::NATIVE_CALL * 10  // publish overhead
                + gas_costs::OBJECT_CREATE * 2; // package object + UpgradeCap

            // Return [package_id, upgrade_cap_id]
            Ok(CommandResult::Created(vec![package_id, upgrade_cap_id]))
        } else {
            // No pre-published info available - this happens when PTBExecutor
            // is used directly without SimulationEnvironment
            Err(anyhow!(
                "Publish command requires execution through SimulationEnvironment.execute_ptb(). \
                 Use env.deploy_package() for standalone publishing, or include Publish in a PTB \
                 executed via env.execute_ptb()."
            ))
        }
    }

    /// Execute an Upgrade command.
    ///
    /// Execute an Upgrade command to upgrade an existing package.
    ///
    /// When executed through SimulationEnvironment.execute_ptb(), modules are
    /// pre-upgraded and this just returns the already-created IDs (new package + receipt).
    /// The ticket argument is consumed but not fully validated in simulation.
    fn execute_upgrade(
        &mut self,
        _modules: Vec<Vec<u8>>,
        _package: ObjectID,
        _ticket: Argument,
    ) -> Result<CommandResult> {
        // Check if we have pre-upgraded info from SimulationEnvironment
        if self.upgrade_index < self.pre_upgraded.len() {
            let (new_package_id, receipt_id) = self.pre_upgraded[self.upgrade_index];
            self.upgrade_index += 1;

            // UpgradeReceipt type: 0x2::package::UpgradeReceipt
            let upgrade_receipt_type = well_known::types::UPGRADE_RECEIPT_TYPE.clone();

            // Mark the UpgradeReceipt as created with its type
            self.created_objects
                .insert(receipt_id, (Vec::new(), Some(upgrade_receipt_type)));
            // Created objects are transferable by the sender
            self.transferable_objects.insert(receipt_id);
            self.object_owners
                .insert(receipt_id, Owner::Address(self.sender));

            // The ticket would normally be consumed here
            // In simulation, we don't strictly validate it

            // Estimate gas: upgrade is expensive similar to publish
            self.gas_used += gas_costs::NATIVE_CALL * 10  // upgrade overhead
                + gas_costs::OBJECT_CREATE * 2  // new package object + UpgradeReceipt
                + gas_costs::OBJECT_DELETE; // ticket consumed

            // Return [new_package_id, upgrade_receipt_id]
            Ok(CommandResult::Created(vec![new_package_id, receipt_id]))
        } else {
            // No pre-upgraded info available - this happens when PTBExecutor
            // is used directly without SimulationEnvironment
            Err(anyhow!(
                "Upgrade command requires execution through SimulationEnvironment.execute_ptb(). \
                 The package modules must be pre-processed before PTB execution."
            ))
        }
    }

    /// Execute a Receive command - receive an object sent in a previous transaction.
    /// This enables transaction chaining where objects are passed between PTBs.
    fn execute_receive(
        &mut self,
        object_id: &ObjectID,
        expected_type: Option<&TypeTag>,
    ) -> Result<CommandResult> {
        // Check if we have this object in our pending receives
        let (object_bytes, stored_type) = self.pending_receives
            .remove(object_id)
            .ok_or_else(|| anyhow!(
                "Object {} not found in pending receives. It must be transferred to this transaction first.",
                object_id.to_hex_literal()
            ))?;

        // Validate type if both expected and stored types are available
        if let (Some(expected), Some(stored)) = (expected_type, &stored_type) {
            if expected != stored {
                return Err(anyhow!(
                    "Type mismatch for received object {}: expected {}, but object has type {}",
                    object_id.to_hex_literal(),
                    format_type_tag(expected),
                    format_type_tag(stored)
                ));
            }
        }

        // Use stored type if expected type is not provided
        let actual_type = expected_type.cloned().or(stored_type);

        // Track that this object was received (unwrapped from pending state)
        // Store in created_objects so it can be referenced in subsequent commands
        self.created_objects
            .insert(*object_id, (object_bytes.clone(), actual_type.clone()));
        self.object_owners
            .insert(*object_id, Owner::Address(self.sender));
        // Received objects are transferable by the sender
        self.transferable_objects.insert(*object_id);
        // Track for clearing from SimulationEnvironment's pending_receives
        self.received_objects.push(*object_id);
        self.object_changes.push(ObjectChange::Unwrapped {
            id: *object_id,
            owner: Owner::Address(self.sender),
            object_type: actual_type,
        });

        // Estimate gas: native call + unwrap operation
        self.gas_used += gas_costs::NATIVE_CALL
            + gas_costs::OBJECT_CREATE  // receiving materializes the object
            + (object_bytes.len() as u64) * gas_costs::OUTPUT_BYTE;

        // Return the object bytes as the result
        Ok(CommandResult::Values(vec![object_bytes]))
    }

    /// Add an object to the pending receives queue.
    /// Call this before executing a PTB that will use Receive commands.
    pub fn add_pending_receive(&mut self, object_id: ObjectID, object_bytes: Vec<u8>) {
        self.pending_receives
            .insert(object_id, (object_bytes, None));
    }

    /// Add an object to the pending receives queue with type information.
    /// This enables type validation when the object is received.
    pub fn add_pending_receive_with_type(
        &mut self,
        object_id: ObjectID,
        object_bytes: Vec<u8>,
        type_tag: TypeTag,
    ) {
        self.pending_receives
            .insert(object_id, (object_bytes, Some(type_tag)));
    }

    /// Build a CommandErrorContext for a failed command.
    fn build_error_context(
        &self,
        cmd: &Command,
        cmd_index: usize,
        error_msg: &str,
    ) -> crate::error_context::CommandErrorContext {
        use crate::error_context::{CoinOperationContext, CommandErrorContext};

        let cmd_type = Self::command_type_name(cmd);
        let mut ctx = CommandErrorContext::new(cmd_index, &cmd_type);

        // Set prior successful commands
        ctx.prior_successful_commands = (0..cmd_index).collect();
        ctx.gas_consumed_before_failure = self.gas_used;

        // Build command-specific context
        match cmd {
            Command::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => {
                ctx.function_signature = Some(format!(
                    "{}::{}::{}",
                    package.to_hex_literal(),
                    module,
                    function
                ));
                ctx.type_arguments = type_args.iter().map(|t| format!("{}", t)).collect();

                // Add input object snapshots
                for arg in args {
                    if let Some(snapshot) = self.build_object_snapshot_from_arg(arg) {
                        ctx.input_objects.push(snapshot);
                    }
                }

                // Parse abort info from error message if it's an abort
                if let Some(abort_info) =
                    Self::parse_abort_info(error_msg, module.as_str(), function.as_str())
                {
                    ctx.abort_info = Some(abort_info);
                }
            }
            Command::SplitCoins { coin, amounts } => {
                // Add the source coin snapshot
                if let Some(snapshot) = self.build_object_snapshot_from_arg(coin) {
                    ctx.input_objects.push(snapshot);
                }

                // Build coin operation context
                let source_balance = self.get_coin_balance_from_arg(coin);
                let requested_splits: Option<Vec<u64>> = amounts
                    .iter()
                    .map(|a| {
                        self.resolve_arg(a).ok().and_then(|b| {
                            if b.len() == 8 {
                                Some(u64::from_le_bytes(b[..8].try_into().ok()?))
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                ctx.coin_balances = Some(CoinOperationContext {
                    coin_type: self.get_coin_type_from_arg(coin).unwrap_or_default(),
                    source_balance,
                    requested_splits,
                    destination_balance: None,
                    source_balances: None,
                });
            }
            Command::MergeCoins {
                destination,
                sources,
            } => {
                // Add destination coin snapshot
                if let Some(snapshot) = self.build_object_snapshot_from_arg(destination) {
                    ctx.input_objects.push(snapshot);
                }

                // Add source coin snapshots
                for src in sources {
                    if let Some(snapshot) = self.build_object_snapshot_from_arg(src) {
                        ctx.input_objects.push(snapshot);
                    }
                }

                // Build coin operation context
                let dest_balance = self.get_coin_balance_from_arg(destination);
                let src_balances: Option<Vec<u64>> = Some(
                    sources
                        .iter()
                        .filter_map(|s| self.get_coin_balance_from_arg(s))
                        .collect(),
                );

                ctx.coin_balances = Some(CoinOperationContext {
                    coin_type: self.get_coin_type_from_arg(destination).unwrap_or_default(),
                    source_balance: None,
                    requested_splits: None,
                    destination_balance: dest_balance,
                    source_balances: src_balances,
                });
            }
            Command::TransferObjects { objects, address } => {
                for obj_arg in objects {
                    if let Some(snapshot) = self.build_object_snapshot_from_arg(obj_arg) {
                        ctx.input_objects.push(snapshot);
                    }
                }
                // Add address argument snapshot if it references an object
                if let Some(snapshot) = self.build_object_snapshot_from_arg(address) {
                    ctx.input_objects.push(snapshot);
                }
            }
            _ => {}
        }

        ctx
    }

    /// Build an ObjectSnapshot from an Argument if it references an object.
    fn build_object_snapshot_from_arg(
        &self,
        arg: &Argument,
    ) -> Option<crate::error_context::ObjectSnapshot> {
        use crate::error_context::ObjectSnapshot;

        let (id, type_tag) = self.get_object_id_and_type_from_arg(arg)?;
        let bytes = self.resolve_arg(arg).ok()?;

        // Check if this object was modified in the PTB
        let modified = self.mutated_objects.contains_key(&id);

        // Get owner info
        let owner = self
            .object_owners
            .get(&id)
            .map(|o| match o {
                Owner::Address(addr) => format!("address:{}", addr.to_hex_literal()),
                Owner::Shared => "shared".to_string(),
                Owner::Immutable => "immutable".to_string(),
            })
            .unwrap_or_else(|| "unknown".to_string());

        Some(
            ObjectSnapshot::new(
                id.to_hex_literal(),
                type_tag
                    .map(|t| format!("{}", t))
                    .unwrap_or_else(|| "unknown".to_string()),
                0, // version not tracked at PTB level
                bytes.len(),
                owner,
            )
            .as_modified_if(modified),
        )
    }

    /// Get the coin balance from an argument if it's a Coin.
    fn get_coin_balance_from_arg(&self, arg: &Argument) -> Option<u64> {
        let bytes = self.resolve_arg(arg).ok()?;
        // Coin structure: { id: UID (32 bytes), balance: Balance<T> { value: u64 } }
        if bytes.len() >= 40 {
            let balance_bytes: [u8; 8] = bytes[32..40].try_into().ok()?;
            Some(u64::from_le_bytes(balance_bytes))
        } else {
            None
        }
    }

    /// Get the coin type from an argument if it's a Coin.
    fn get_coin_type_from_arg(&self, arg: &Argument) -> Option<String> {
        let (_, type_tag) = self.get_object_id_and_type_from_arg(arg)?;
        let type_tag = type_tag?;
        // Extract T from Coin<T>
        if let TypeTag::Struct(s) = &type_tag {
            if s.name.as_str() == "Coin" && !s.type_params.is_empty() {
                return Some(format!("{}", s.type_params[0]));
            }
        }
        Some(format!("{}", type_tag))
    }

    /// Parse abort information from an error message.
    fn parse_abort_info(
        error_msg: &str,
        module: &str,
        function: &str,
    ) -> Option<crate::error_context::AbortInfo> {
        use crate::error_context::AbortInfo;

        // Parse abort code from various VM error formats:
        // - VMError { major_status: ABORTED, sub_status: Some(202), ... }
        // - "abort code: 1"
        // - "ABORTED with code 1"
        // - "Move abort: 1"
        // - "MoveAbort(..., 42)"
        let abort_code = if let Some(idx) = error_msg.find("sub_status: Some(") {
            // VMError format: sub_status: Some(202)
            let start = idx + 17;
            error_msg[start..]
                .split(')')
                .next()
                .and_then(|s| s.parse().ok())
        } else if let Some(idx) = error_msg.find("MoveAbort") {
            // MoveAbort(location, code) format - extract the last number
            error_msg[idx..]
                .split(|c: char| !c.is_ascii_digit())
                .rfind(|s: &&str| !s.is_empty())
                .and_then(|s| s.parse().ok())
        } else if let Some(idx) = error_msg.find("abort code:") {
            error_msg[idx + 11..]
                .split_whitespace()
                .next()
                .and_then(|s| s.trim_matches(|c: char| !c.is_numeric()).parse().ok())
        } else if let Some(idx) = error_msg.find("ABORTED with code") {
            error_msg[idx + 17..]
                .split_whitespace()
                .next()
                .and_then(|s| s.trim_matches(|c: char| !c.is_numeric()).parse().ok())
        } else if let Some(idx) = error_msg.find("Move abort:") {
            error_msg[idx + 11..]
                .split_whitespace()
                .next()
                .and_then(|s| s.trim_matches(|c: char| !c.is_numeric()).parse().ok())
        } else if error_msg.contains("abort") || error_msg.contains("ABORT") {
            // Generic fallback: try to find any number after "abort"
            error_msg
                .split_whitespace()
                .filter_map(|s| {
                    s.trim_matches(|c: char| !c.is_numeric())
                        .parse::<u64>()
                        .ok()
                })
                .next()
        } else {
            None
        };

        abort_code.map(|code| {
            let abort_meaning = crate::error_context::get_abort_code_context(code, module);
            AbortInfo {
                module: module.to_string(),
                function: function.to_string(),
                abort_code: code,
                constant_name: None, // Not available from local execution - only from gRPC CleverError
                abort_meaning,
                involved_objects: Vec::new(),
            }
        })
    }

    /// Build an ExecutionSnapshot capturing the state at failure time.
    fn build_execution_snapshot(
        &self,
        successful_cmd_count: usize,
    ) -> crate::error_context::ExecutionSnapshot {
        use crate::error_context::{CommandSummary, ExecutionSnapshot, ObjectSnapshot};

        let mut snapshot = ExecutionSnapshot {
            total_gas_consumed: self.gas_used,
            ..Default::default()
        };

        // Add all loaded input objects
        for (idx, input) in self.inputs.iter().enumerate() {
            if let InputValue::Object(obj_input) = input {
                let bytes = obj_input.bytes();
                let id = obj_input.id();
                let modified = self.mutated_objects.contains_key(id);
                let owner = self
                    .object_owners
                    .get(id)
                    .map(|o| match o {
                        Owner::Address(addr) => format!("address:{}", addr.to_hex_literal()),
                        Owner::Shared => "shared".to_string(),
                        Owner::Immutable => "immutable".to_string(),
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                snapshot.objects.push(
                    ObjectSnapshot::new(
                        id.to_hex_literal(),
                        format!("input_{}", idx), // Type not always available
                        0,
                        bytes.len(),
                        owner,
                    )
                    .as_modified_if(modified),
                );
            }
        }

        // Add created objects
        for (id, (bytes, type_tag)) in &self.created_objects {
            let owner = self
                .object_owners
                .get(id)
                .map(|o| match o {
                    Owner::Address(addr) => format!("address:{}", addr.to_hex_literal()),
                    Owner::Shared => "shared".to_string(),
                    Owner::Immutable => "immutable".to_string(),
                })
                .unwrap_or_else(|| "unknown".to_string());

            snapshot.objects.push(ObjectSnapshot::new(
                id.to_hex_literal(),
                type_tag
                    .as_ref()
                    .map(|t| format!("{}", t))
                    .unwrap_or_else(|| "created".to_string()),
                0,
                bytes.len(),
                owner,
            ));
        }

        // Add successful command summaries from execution trace
        for entry in &self.execution_trace.commands {
            if entry.success && entry.index < successful_cmd_count {
                snapshot.successful_commands.push(CommandSummary {
                    index: entry.index,
                    command_type: entry.command_type.clone(),
                    description: entry.description.clone(),
                    gas_consumed: entry.gas_used,
                    objects_created: entry.objects_created.clone(),
                    objects_mutated: Vec::new(),
                });
            }
        }

        snapshot
    }

    /// Execute all commands in the PTB.
    pub fn execute(&mut self, commands: Vec<Command>) -> Result<TransactionEffects> {
        let start_time = std::time::Instant::now();

        // Validate PTB causality before execution
        let validation = validate_ptb(&commands, self.inputs.len());
        if !validation.valid {
            let error_msgs: Vec<String> = validation
                .errors
                .iter()
                .map(|e| e.message.clone())
                .collect();
            self.execution_trace.add_failure(
                0,
                "validation",
                "PTB validation".to_string(),
                error_msgs.join("; "),
            );
            return Ok(TransactionEffects::failure_at(
                format!("PTB validation failed: {}", error_msgs.join("; ")),
                0,
                "validation".to_string(),
                0,
            ));
        }

        // Register input objects with lifecycle tracker
        if self.enable_lifecycle_tracking {
            for (idx, input) in self.inputs.iter().enumerate() {
                if let InputValue::Object(obj_input) = input {
                    self.lifecycle_tracker
                        .register_input(*obj_input.id(), idx as u16, None);
                }
            }
        }

        // Clear the VM's execution trace and events before starting
        self.vm.clear_trace();
        self.vm.clear_events();

        for (index, cmd) in commands.iter().enumerate() {
            let cmd_description = Self::describe_command(cmd);
            let cmd_type = Self::command_type_name(cmd);

            // Extract function call info for MoveCall commands
            let func_info = if let Command::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } = cmd
            {
                Some(FunctionCallInfo {
                    module: format!("{}::{}", package.to_hex_literal(), module),
                    function: function.to_string(),
                    type_args: type_args.iter().map(|t| format!("{}", t)).collect(),
                    arg_count: args.len(),
                })
            } else {
                None
            };

            match self.execute_command(cmd.clone()) {
                Ok(result) => {
                    let return_count = result.len();
                    self.results.push(result);

                    // Record success in trace
                    self.execution_trace.add_success(
                        index,
                        &cmd_type,
                        cmd_description.clone(),
                        self.gas_used,
                        return_count,
                    );
                    if let Some(info) = func_info {
                        self.execution_trace.add_function_call(info);
                    }

                    // Check gas budget after each successful command
                    if let Err(gas_err) = self.check_gas_budget() {
                        // Build error context for out-of-gas failure
                        let error_context =
                            self.build_error_context(cmd, index, &gas_err.to_string());
                        let state_at_failure = self.build_execution_snapshot(self.results.len());

                        self.execution_trace.add_failure(
                            index,
                            &cmd_type,
                            format!("{} (out of gas)", cmd_description),
                            gas_err.to_string(),
                        );
                        self.execution_trace
                            .complete(false, Some(start_time.elapsed().as_millis() as u64));
                        return Ok(TransactionEffects::failure_at_with_context(
                            gas_err.to_string(),
                            index,
                            format!("{} (out of gas)", cmd_description),
                            self.results.len(),
                            error_context,
                            state_at_failure,
                        ));
                    }
                }
                Err(e) => {
                    // Build error context for command failure
                    let error_context = self.build_error_context(cmd, index, &e.to_string());
                    let state_at_failure = self.build_execution_snapshot(self.results.len());

                    self.execution_trace.add_failure(
                        index,
                        &cmd_type,
                        cmd_description.clone(),
                        e.to_string(),
                    );
                    self.execution_trace
                        .complete(false, Some(start_time.elapsed().as_millis() as u64));
                    return Ok(TransactionEffects::failure_at_with_context(
                        e.to_string(),
                        index,
                        cmd_description,
                        self.results.len(),
                        error_context,
                        state_at_failure,
                    ));
                }
            }
        }

        // Complete trace with success
        self.execution_trace
            .complete(true, Some(start_time.elapsed().as_millis() as u64));
        if self.enable_lifecycle_tracking {
            self.execution_trace.object_summary = Some(self.lifecycle_tracker.summary());
        }

        Ok(self.compute_effects())
    }

    /// Get the command type name for tracing.
    fn command_type_name(cmd: &Command) -> String {
        match cmd {
            Command::MoveCall { .. } => "MoveCall".to_string(),
            Command::SplitCoins { .. } => "SplitCoins".to_string(),
            Command::MergeCoins { .. } => "MergeCoins".to_string(),
            Command::TransferObjects { .. } => "TransferObjects".to_string(),
            Command::MakeMoveVec { .. } => "MakeMoveVec".to_string(),
            Command::Publish { .. } => "Publish".to_string(),
            Command::Upgrade { .. } => "Upgrade".to_string(),
            Command::Receive { .. } => "Receive".to_string(),
        }
    }

    /// Generate a human-readable description of a command.
    fn describe_command(cmd: &Command) -> String {
        match cmd {
            Command::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } => {
                let type_args_str = if type_args.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<{}>",
                        type_args
                            .iter()
                            .map(|t| format!("{}", t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                format!(
                    "MoveCall {}::{}::{}{} ({} args)",
                    package.to_hex_literal(),
                    module,
                    function,
                    type_args_str,
                    args.len()
                )
            }
            Command::SplitCoins { coin, amounts } => {
                format!("SplitCoins (coin: {:?}, {} amounts)", coin, amounts.len())
            }
            Command::MergeCoins {
                destination,
                sources,
            } => {
                format!(
                    "MergeCoins (dest: {:?}, {} sources)",
                    destination,
                    sources.len()
                )
            }
            Command::TransferObjects { objects, address } => {
                format!(
                    "TransferObjects ({} objects to {:?})",
                    objects.len(),
                    address
                )
            }
            Command::MakeMoveVec { type_tag, elements } => {
                let type_str = type_tag
                    .as_ref()
                    .map(|t| format!("{}", t))
                    .unwrap_or_else(|| "unknown".to_string());
                format!("MakeMoveVec<{}> ({} elements)", type_str, elements.len())
            }
            Command::Publish { modules, dep_ids } => {
                format!(
                    "Publish ({} modules, {} deps)",
                    modules.len(),
                    dep_ids.len()
                )
            }
            Command::Upgrade { package, .. } => {
                format!("Upgrade (package {})", package.to_hex_literal())
            }
            Command::Receive {
                object_id,
                object_type,
            } => {
                let type_str = object_type
                    .as_ref()
                    .map(|t| format!("{}", t))
                    .unwrap_or_else(|| "unknown".to_string());
                format!(
                    "Receive {} (type: {})",
                    object_id.to_hex_literal(),
                    type_str
                )
            }
        }
    }

    /// Compute the transaction effects after execution.
    fn compute_effects(&self) -> TransactionEffects {
        let mut effects = TransactionEffects::success();

        // Add created objects with their tracked ownership and type
        for (id, (_bytes, object_type)) in &self.created_objects {
            let owner = self
                .object_owners
                .get(id)
                .copied()
                .unwrap_or(Owner::Address(AccountAddress::ZERO));
            effects.created.push(*id);
            effects.object_changes.push(ObjectChange::Created {
                id: *id,
                owner,
                object_type: object_type.clone(),
            });
        }

        // Add deleted objects with their type
        for (id, object_type) in &self.deleted_objects {
            effects.deleted.push(*id);
            effects.object_changes.push(ObjectChange::Deleted {
                id: *id,
                object_type: object_type.clone(),
            });
        }

        // Add mutated objects with their tracked ownership and type
        for (id, (_bytes, object_type)) in &self.mutated_objects {
            if !self.created_objects.contains_key(id) && !self.deleted_objects.contains_key(id) {
                let owner = self
                    .object_owners
                    .get(id)
                    .copied()
                    .unwrap_or(Owner::Address(AccountAddress::ZERO));
                effects.mutated.push(*id);
                effects.object_changes.push(ObjectChange::Mutated {
                    id: *id,
                    owner,
                    object_type: object_type.clone(),
                });
            }
        }

        // Add wrapped objects from the wrapped_objects tracking
        for (id, object_type) in &self.wrapped_objects {
            if !effects.wrapped.contains(id) {
                effects.wrapped.push(*id);
                effects.object_changes.push(ObjectChange::Wrapped {
                    id: *id,
                    object_type: object_type.clone(),
                });
            }
        }

        // Include any additional object changes tracked during execution
        // Also populate the wrapped/unwrapped/transferred vectors
        for change in &self.object_changes {
            // Avoid duplicates - only add if not already present
            let id = match change {
                ObjectChange::Created { id, .. } => id,
                ObjectChange::Mutated { id, .. } => id,
                ObjectChange::Deleted { id, .. } => id,
                ObjectChange::Wrapped { id, .. } => {
                    // Track wrapped objects
                    if !effects.wrapped.contains(id) {
                        effects.wrapped.push(*id);
                    }
                    id
                }
                ObjectChange::Unwrapped { id, .. } => {
                    // Track unwrapped objects
                    if !effects.unwrapped.contains(id) {
                        effects.unwrapped.push(*id);
                    }
                    id
                }
                ObjectChange::Transferred { id, .. } => {
                    // Track transferred objects
                    if !effects.transferred.contains(id) {
                        effects.transferred.push(*id);
                    }
                    id
                }
            };
            if !effects.object_changes.iter().any(|c| match c {
                ObjectChange::Created { id: cid, .. } => cid == id,
                ObjectChange::Mutated { id: cid, .. } => cid == id,
                ObjectChange::Deleted { id: cid, .. } => cid == id,
                ObjectChange::Wrapped { id: cid, .. } => cid == id,
                ObjectChange::Unwrapped { id: cid, .. } => cid == id,
                ObjectChange::Transferred { id: cid, .. } => cid == id,
            }) {
                effects.object_changes.push(change.clone());
            }
        }

        // Collect events emitted during execution
        effects.events = self.vm.get_events();

        // Capture return values from each command
        effects.return_values = self
            .results
            .iter()
            .map(|result| {
                match result {
                    CommandResult::Empty => vec![],
                    CommandResult::Values(values) => values.clone(),
                    CommandResult::Created(ids) => {
                        // For created objects, return their IDs as BCS-encoded bytes
                        ids.iter().map(|id| id.to_vec()).collect()
                    }
                }
            })
            .collect();

        // Populate mutated object bytes for syncing back to environment
        effects.mutated_object_bytes = self
            .mutated_objects
            .iter()
            .map(|(id, (bytes, _))| (*id, bytes.clone()))
            .collect();

        // Populate created object bytes for syncing back to environment
        effects.created_object_bytes = self
            .created_objects
            .iter()
            .map(|(id, (bytes, _))| (*id, bytes.clone()))
            .collect();

        // Extract dynamic field entries from the VM's shared state.
        // This captures all Table/Bag operations that occurred during MoveCall execution.
        for ((parent_id, child_id), type_tag, bytes) in self.vm.extract_dynamic_fields() {
            effects
                .dynamic_field_entries
                .insert((parent_id, child_id), (type_tag, bytes));
        }

        // Track objects that were received from pending_receives
        effects.received = self.received_objects.clone();

        // Set accumulated gas usage
        effects.gas_used = self.gas_used;

        effects
    }

    /// Get the results of all executed commands.
    pub fn results(&self) -> &[CommandResult] {
        &self.results
    }

    /// Get a specific command result.
    pub fn get_result(&self, index: usize) -> Option<&CommandResult> {
        self.results.get(index)
    }

    /// Get the created objects.
    /// Get the created objects map (id -> (bytes, type)).
    pub fn created_objects(&self) -> &HashMap<ObjectID, (Vec<u8>, Option<TypeTag>)> {
        &self.created_objects
    }

    /// Get created objects bytes only (for backwards compatibility).
    pub fn created_objects_bytes(&self) -> HashMap<ObjectID, Vec<u8>> {
        self.created_objects
            .iter()
            .map(|(id, (bytes, _))| (*id, bytes.clone()))
            .collect()
    }

    /// Get the mutated objects map (id -> (bytes, type)).
    /// Used by SimulationEnvironment to sync state back after PTB execution.
    pub fn mutated_objects(&self) -> &HashMap<ObjectID, (Vec<u8>, Option<TypeTag>)> {
        &self.mutated_objects
    }

    /// Get mutated objects bytes only (for backwards compatibility).
    pub fn mutated_objects_bytes(&self) -> HashMap<ObjectID, Vec<u8>> {
        self.mutated_objects
            .iter()
            .map(|(id, (bytes, _))| (*id, bytes.clone()))
            .collect()
    }
}

/// Builder for constructing PTB commands more ergonomically.
pub struct PTBBuilder {
    inputs: Vec<InputValue>,
    commands: Vec<Command>,
}

impl PTBBuilder {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// Add a pure value input and return its argument reference.
    pub fn pure<T: serde::Serialize>(&mut self, value: &T) -> Result<Argument> {
        let bytes = bcs::to_bytes(value)?;
        let idx = self.inputs.len();
        self.inputs.push(InputValue::Pure(bytes));
        Ok(Argument::Input(idx as u16))
    }

    /// Add raw bytes as a pure input.
    pub fn pure_bytes(&mut self, bytes: Vec<u8>) -> Argument {
        let idx = self.inputs.len();
        self.inputs.push(InputValue::Pure(bytes));
        Argument::Input(idx as u16)
    }

    /// Add an owned object input.
    pub fn object_owned(&mut self, id: ObjectID, bytes: Vec<u8>) -> Argument {
        let idx = self.inputs.len();
        self.inputs.push(InputValue::Object(ObjectInput::Owned {
            id,
            bytes,
            type_tag: None,
        }));
        Argument::Input(idx as u16)
    }

    /// Add an owned object input with type information.
    pub fn object_owned_with_type(
        &mut self,
        id: ObjectID,
        bytes: Vec<u8>,
        type_tag: TypeTag,
    ) -> Argument {
        let idx = self.inputs.len();
        self.inputs.push(InputValue::Object(ObjectInput::Owned {
            id,
            bytes,
            type_tag: Some(type_tag),
        }));
        Argument::Input(idx as u16)
    }

    /// Add any object input (owned, shared, or immutable).
    pub fn add_object_input(&mut self, obj: ObjectInput) -> Argument {
        let idx = self.inputs.len();
        self.inputs.push(InputValue::Object(obj));
        Argument::Input(idx as u16)
    }

    /// Add a MoveCall command and return the result argument.
    pub fn move_call(
        &mut self,
        package: AccountAddress,
        module: &str,
        function: &str,
        type_args: Vec<TypeTag>,
        args: Vec<Argument>,
    ) -> Result<Argument> {
        let cmd_idx = self.commands.len();
        self.commands.push(Command::MoveCall {
            package,
            module: Identifier::new(module)?,
            function: Identifier::new(function)?,
            type_args,
            args,
        });
        Ok(Argument::Result(cmd_idx as u16))
    }

    /// Add a SplitCoins command.
    pub fn split_coins(&mut self, coin: Argument, amounts: Vec<Argument>) -> Argument {
        let cmd_idx = self.commands.len();
        self.commands.push(Command::SplitCoins { coin, amounts });
        Argument::Result(cmd_idx as u16)
    }

    /// Add a MergeCoins command.
    pub fn merge_coins(&mut self, destination: Argument, sources: Vec<Argument>) -> Argument {
        let cmd_idx = self.commands.len();
        self.commands.push(Command::MergeCoins {
            destination,
            sources,
        });
        Argument::Result(cmd_idx as u16)
    }

    /// Add a TransferObjects command.
    pub fn transfer_objects(&mut self, objects: Vec<Argument>, address: Argument) {
        self.commands
            .push(Command::TransferObjects { objects, address });
    }

    /// Add a MakeMoveVec command.
    pub fn make_move_vec(
        &mut self,
        type_tag: Option<TypeTag>,
        elements: Vec<Argument>,
    ) -> Argument {
        let cmd_idx = self.commands.len();
        self.commands
            .push(Command::MakeMoveVec { type_tag, elements });
        Argument::Result(cmd_idx as u16)
    }

    /// Execute the built PTB.
    pub fn execute<'a, 'b>(self, vm: &'a mut VMHarness<'b>) -> Result<TransactionEffects> {
        let mut executor = PTBExecutor::new(vm);

        // Add all inputs
        for input in self.inputs {
            match input {
                InputValue::Pure(bytes) => {
                    executor.add_pure_input(bytes)?;
                }
                InputValue::Object(obj) => {
                    executor.add_object_input(obj)?;
                }
            }
        }

        // Execute commands
        executor.execute(self.commands)
    }

    /// Get the built commands (for inspection).
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Get the inputs (for inspection).
    pub fn inputs(&self) -> &[InputValue] {
        &self.inputs
    }

    /// Consume the builder and return the inputs and commands.
    /// This is useful for executing via SimulationEnvironment.execute_ptb().
    pub fn into_parts(self) -> (Vec<InputValue>, Vec<Command>) {
        (self.inputs, self.commands)
    }
}

impl Default for PTBBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argument_types() {
        let input = Argument::Input(0);
        let result = Argument::Result(1);
        let nested = Argument::NestedResult(2, 3);

        assert_eq!(input, Argument::Input(0));
        assert_eq!(result, Argument::Result(1));
        assert_eq!(nested, Argument::NestedResult(2, 3));
    }

    #[test]
    fn test_command_result_empty() {
        let result = CommandResult::Empty;
        assert!(result.is_empty());
        assert_eq!(result.len(), 0);
        assert!(result.primary_value().is_err());
    }

    #[test]
    fn test_command_result_values() {
        let result = CommandResult::Values(vec![vec![1, 2, 3], vec![4, 5, 6]]);
        assert!(!result.is_empty());
        assert_eq!(result.len(), 2);
        assert_eq!(result.primary_value().unwrap(), vec![1, 2, 3]);
        assert_eq!(result.get(1).unwrap(), vec![4, 5, 6]);
        assert!(result.get(2).is_err());
    }

    #[test]
    fn test_input_value_pure() {
        let input = InputValue::Pure(vec![1, 2, 3]);
        assert_eq!(input.to_bcs().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_transaction_effects() {
        let effects = TransactionEffects::success();
        assert!(effects.success);
        assert!(effects.error.is_none());

        let effects = TransactionEffects::failure("test error".to_string());
        assert!(!effects.success);
        assert_eq!(effects.error, Some("test error".to_string()));
    }

    #[test]
    fn test_ptb_builder_pure() {
        let mut builder = PTBBuilder::new();
        let arg = builder.pure(&100u64).unwrap();
        assert_eq!(arg, Argument::Input(0));

        let arg2 = builder.pure(&"hello").unwrap();
        assert_eq!(arg2, Argument::Input(1));
    }

    #[test]
    fn test_uleb128_encoding() {
        // Test that MakeMoveVec properly encodes vector length
        let mut builder = PTBBuilder::new();
        let elem1 = builder.pure_bytes(vec![1]);
        let elem2 = builder.pure_bytes(vec![2]);
        let _vec_arg = builder.make_move_vec(None, vec![elem1, elem2]);

        // The command should be recorded
        assert_eq!(builder.commands().len(), 1);
    }

    // =========================================================================
    // PTB Causality Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_ptb_valid() {
        // Valid PTB: each command only references previous results
        let commands = vec![
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("foo").unwrap(),
                type_args: vec![],
                args: vec![Argument::Input(0)],
            },
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("bar").unwrap(),
                type_args: vec![],
                args: vec![Argument::Result(0)], // References first command's result
            },
        ];

        let result = validate_ptb(&commands, 1);
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_ptb_forward_reference() {
        // Invalid: command 0 references result 1 which doesn't exist yet
        let commands = vec![
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("foo").unwrap(),
                type_args: vec![],
                args: vec![Argument::Result(1)], // Forward reference!
            },
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("bar").unwrap(),
                type_args: vec![],
                args: vec![Argument::Input(0)],
            },
        ];

        let result = validate_ptb(&commands, 1);
        assert!(!result.valid);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].kind, ValidationErrorKind::ForwardReference);
    }

    #[test]
    fn test_validate_ptb_self_reference() {
        // Invalid: command 0 references its own result
        let commands = vec![Command::MoveCall {
            package: AccountAddress::ZERO,
            module: Identifier::new("test").unwrap(),
            function: Identifier::new("foo").unwrap(),
            type_args: vec![],
            args: vec![Argument::Result(0)], // Self reference!
        }];

        let result = validate_ptb(&commands, 1);
        assert!(!result.valid);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].kind, ValidationErrorKind::SelfReference);
    }

    #[test]
    fn test_validate_ptb_input_out_of_bounds() {
        // Invalid: references Input(5) but only 2 inputs available
        let commands = vec![Command::MoveCall {
            package: AccountAddress::ZERO,
            module: Identifier::new("test").unwrap(),
            function: Identifier::new("foo").unwrap(),
            type_args: vec![],
            args: vec![Argument::Input(5)],
        }];

        let result = validate_ptb(&commands, 2);
        assert!(!result.valid);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].kind, ValidationErrorKind::InputOutOfBounds);
    }

    #[test]
    fn test_validate_ptb_nested_result_forward_reference() {
        // Invalid: NestedResult references future command
        let commands = vec![
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("foo").unwrap(),
                type_args: vec![],
                args: vec![Argument::NestedResult(1, 0)], // Forward reference
            },
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("bar").unwrap(),
                type_args: vec![],
                args: vec![Argument::Input(0)],
            },
        ];

        let result = validate_ptb(&commands, 1);
        assert!(!result.valid);
        assert_eq!(result.errors[0].kind, ValidationErrorKind::ForwardReference);
    }

    #[test]
    fn test_compute_dependency_graph() {
        let commands = vec![
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("a").unwrap(),
                type_args: vec![],
                args: vec![Argument::Input(0)],
            },
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("b").unwrap(),
                type_args: vec![],
                args: vec![Argument::Result(0)],
            },
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("c").unwrap(),
                type_args: vec![],
                args: vec![Argument::Result(0), Argument::Result(1)],
            },
        ];

        let deps = compute_dependency_graph(&commands);
        assert!(deps[&0].is_empty()); // cmd 0 has no dependencies
        assert_eq!(deps[&1], [0].into_iter().collect()); // cmd 1 depends on 0
        assert_eq!(deps[&2], [0, 1].into_iter().collect()); // cmd 2 depends on 0 and 1
    }

    #[test]
    fn test_topological_sort_valid() {
        let commands = vec![
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("a").unwrap(),
                type_args: vec![],
                args: vec![Argument::Input(0)],
            },
            Command::MoveCall {
                package: AccountAddress::ZERO,
                module: Identifier::new("test").unwrap(),
                function: Identifier::new("b").unwrap(),
                type_args: vec![],
                args: vec![Argument::Result(0)],
            },
        ];

        let sorted = topological_sort(&commands);
        assert!(sorted.is_ok());
        // The sort should put dependencies before dependents
        let order = sorted.unwrap();
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn test_extract_arguments_move_call() {
        let cmd = Command::MoveCall {
            package: AccountAddress::ZERO,
            module: Identifier::new("test").unwrap(),
            function: Identifier::new("foo").unwrap(),
            type_args: vec![],
            args: vec![Argument::Input(0), Argument::Result(1)],
        };

        let args = extract_arguments(&cmd);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], Argument::Input(0));
        assert_eq!(args[1], Argument::Result(1));
    }

    #[test]
    fn test_extract_arguments_split_coins() {
        let cmd = Command::SplitCoins {
            coin: Argument::Input(0),
            amounts: vec![Argument::Input(1), Argument::Input(2)],
        };

        let args = extract_arguments(&cmd);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], Argument::Input(0));
        assert_eq!(args[1], Argument::Input(1));
        assert_eq!(args[2], Argument::Input(2));
    }

    #[test]
    fn test_extract_arguments_transfer_objects() {
        let cmd = Command::TransferObjects {
            objects: vec![Argument::Result(0), Argument::Result(1)],
            address: Argument::Input(0),
        };

        let args = extract_arguments(&cmd);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], Argument::Result(0));
        assert_eq!(args[1], Argument::Result(1));
        assert_eq!(args[2], Argument::Input(0));
    }

    // =========================================================================
    // Object Lifecycle Tracking Tests
    // =========================================================================

    #[test]
    fn test_lifecycle_tracker_basic() {
        let mut tracker = ObjectLifecycleTracker::new();
        let obj_id = AccountAddress::from_hex_literal("0x1234").unwrap();

        // Register an input object
        tracker.register_input(obj_id, 0, Some(TypeTag::U64));

        // Should be available
        assert!(tracker.get_provenance(&obj_id).is_some());
        assert_eq!(
            tracker.get_provenance(&obj_id).unwrap().state,
            ObjectState::Available
        );

        // Record a read - should succeed
        assert!(tracker.record_read(obj_id, 0).is_ok());
    }

    #[test]
    fn test_lifecycle_tracker_use_after_consume() {
        let mut tracker = ObjectLifecycleTracker::new();
        let obj_id = AccountAddress::from_hex_literal("0x1234").unwrap();

        tracker.register_input(obj_id, 0, None);

        // Consume the object
        assert!(tracker.record_consume(obj_id, 0).is_ok());
        assert_eq!(
            tracker.get_provenance(&obj_id).unwrap().state,
            ObjectState::Consumed
        );

        // Try to use it again - should fail
        let result = tracker.record_read(obj_id, 1);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            LifecycleErrorKind::UseAfterConsume
        );
    }

    #[test]
    fn test_lifecycle_tracker_use_after_transfer() {
        let mut tracker = ObjectLifecycleTracker::new();
        let obj_id = AccountAddress::from_hex_literal("0x1234").unwrap();
        let recipient = AccountAddress::from_hex_literal("0x5678").unwrap();

        tracker.register_input(obj_id, 0, None);

        // Transfer the object
        assert!(tracker.record_transfer(obj_id, 0, recipient).is_ok());
        assert_eq!(
            tracker.get_provenance(&obj_id).unwrap().state,
            ObjectState::Transferred
        );

        // Try to use it again - should fail
        let result = tracker.record_mutate(obj_id, 1);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            LifecycleErrorKind::UseAfterTransfer
        );
    }

    #[test]
    fn test_lifecycle_tracker_object_not_found() {
        let mut tracker = ObjectLifecycleTracker::new();
        let obj_id = AccountAddress::from_hex_literal("0x1234").unwrap();

        // Try to use an object that was never registered
        let result = tracker.record_read(obj_id, 0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind, LifecycleErrorKind::ObjectNotFound);
    }

    #[test]
    fn test_lifecycle_tracker_summary() {
        let mut tracker = ObjectLifecycleTracker::new();

        // Register some objects with different origins
        tracker.register_input(AccountAddress::from_hex_literal("0x1").unwrap(), 0, None);
        tracker.register_input(AccountAddress::from_hex_literal("0x2").unwrap(), 1, None);
        tracker.register_created(AccountAddress::from_hex_literal("0x3").unwrap(), 0, None);
        tracker.register_received(AccountAddress::from_hex_literal("0x4").unwrap(), None);

        // Consume one, transfer one
        tracker
            .record_consume(AccountAddress::from_hex_literal("0x1").unwrap(), 0)
            .unwrap();
        tracker
            .record_transfer(
                AccountAddress::from_hex_literal("0x2").unwrap(),
                1,
                AccountAddress::ZERO,
            )
            .unwrap();

        let summary = tracker.summary();
        assert_eq!(summary.from_inputs, 2);
        assert_eq!(summary.created, 1);
        assert_eq!(summary.received, 1);
        assert_eq!(summary.consumed, 1);
        assert_eq!(summary.transferred, 1);
        assert_eq!(summary.available, 2); // The created and received ones
    }

    #[test]
    fn test_lifecycle_tracker_history() {
        let mut tracker = ObjectLifecycleTracker::new();
        let obj_id = AccountAddress::from_hex_literal("0x1234").unwrap();

        tracker.register_input(obj_id, 0, None);

        // Perform multiple operations
        tracker.record_read(obj_id, 0).unwrap();
        tracker.record_mutate(obj_id, 1).unwrap();
        tracker.record_read(obj_id, 2).unwrap();

        let prov = tracker.get_provenance(&obj_id).unwrap();
        assert_eq!(prov.history.len(), 3);
        assert_eq!(prov.history[0].operation, OperationType::Read);
        assert_eq!(prov.history[1].operation, OperationType::Mutate);
        assert_eq!(prov.history[2].operation, OperationType::Read);
    }

    // =========================================================================
    // PTB Execution Trace Tests
    // =========================================================================

    #[test]
    fn test_ptb_trace_add_success() {
        let mut trace = PTBExecutionTrace::new();
        trace.add_success(0, "MoveCall", "call foo".to_string(), 100, 1);
        trace.add_success(1, "TransferObjects", "transfer".to_string(), 50, 0);

        assert_eq!(trace.commands.len(), 2);
        assert_eq!(trace.total_gas_used, 150);
        assert!(trace.commands[0].success);
        assert_eq!(trace.commands[0].command_type, "MoveCall");
    }

    #[test]
    fn test_ptb_trace_add_failure() {
        let mut trace = PTBExecutionTrace::new();
        trace.add_success(0, "MoveCall", "call foo".to_string(), 100, 1);
        trace.add_failure(
            1,
            "MoveCall",
            "call bar".to_string(),
            "abort at 42".to_string(),
        );

        assert_eq!(trace.commands.len(), 2);
        assert!(!trace.success);
        assert_eq!(trace.failed_command_index, Some(1));
        assert!(trace.commands[1].error.is_some());
    }

    #[test]
    fn test_ptb_trace_summary() {
        let mut trace = PTBExecutionTrace::new();
        trace.add_success(0, "MoveCall", "call 1".to_string(), 100, 1);
        trace.add_success(1, "MoveCall", "call 2".to_string(), 100, 1);
        trace.add_success(2, "SplitCoins", "split".to_string(), 50, 2);
        trace.add_success(3, "TransferObjects", "transfer".to_string(), 25, 0);

        let summary = trace.summary();
        assert_eq!(summary.total_commands, 4);
        assert_eq!(summary.successful_commands, 4);
        assert_eq!(summary.failed_commands, 0);
        assert_eq!(summary.move_calls, 2);
        assert_eq!(summary.splits, 1);
        assert_eq!(summary.transfers, 1);
        assert_eq!(summary.total_gas_used, 275);
    }

    #[test]
    fn test_ptb_trace_function_call_info() {
        let mut trace = PTBExecutionTrace::new();
        trace.add_success(0, "MoveCall", "call foo".to_string(), 100, 1);
        trace.add_function_call(FunctionCallInfo {
            module: "0x2::coin".to_string(),
            function: "mint".to_string(),
            type_args: vec!["0x2::sui::SUI".to_string()],
            arg_count: 2,
        });

        let cmd = &trace.commands[0];
        assert!(cmd.function_called.is_some());
        let func = cmd.function_called.as_ref().unwrap();
        assert_eq!(func.module, "0x2::coin");
        assert_eq!(func.function, "mint");
        assert_eq!(func.type_args.len(), 1);
    }

    // =========================================================================
    // Error Context Population Tests
    // =========================================================================

    #[test]
    fn test_parse_abort_info_with_abort_code() {
        // Test parsing abort info from various error message formats
        use crate::ptb::PTBExecutor;

        // Format: "abort code: X"
        let info = PTBExecutor::parse_abort_info(
            "execution failed with abort code: 1 in module",
            "coin",
            "split",
        );
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.abort_code, 1);
        assert_eq!(info.module, "coin");
        assert_eq!(info.function, "split");
        assert!(info.abort_meaning.is_some()); // Should match coin module code 1

        // Format: "ABORTED with code X"
        let info = PTBExecutor::parse_abort_info(
            "Move function ABORTED with code 257",
            "vector",
            "borrow",
        );
        assert!(info.is_some());
        assert_eq!(info.unwrap().abort_code, 257);

        // Format: VMError with sub_status (actual VM output format)
        let info = PTBExecutor::parse_abort_info(
            "VMError { major_status: ABORTED, sub_status: Some(202), message: Some(\"0xf825::sq::csst at offset 13\") }",
            "sq",
            "csst",
        );
        assert!(info.is_some());
        assert_eq!(info.unwrap().abort_code, 202);

        // Format: MoveAbort with location and code
        let info = PTBExecutor::parse_abort_info(
            "MoveAbort(MoveLocation { module: 0x2::coin, function: 0, instruction: 5 }, 1)",
            "coin",
            "split",
        );
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.abort_code, 1);
        assert!(info.abort_meaning.is_some()); // coin module code 1 = insufficient balance

        // No abort code present
        let info = PTBExecutor::parse_abort_info("type mismatch error", "test", "func");
        assert!(info.is_none());
    }

    #[test]
    fn test_transaction_effects_failure_with_context() {
        use crate::error_context::{CommandErrorContext, ExecutionSnapshot};

        // Test that failure_at_with_context properly stores the context
        let ctx = CommandErrorContext::new(2, "SplitCoins")
            .with_gas_consumed(1500)
            .with_prior_commands(vec![0, 1]);

        let mut snapshot = ExecutionSnapshot::default();
        snapshot.total_gas_consumed = 1500;

        let effects = TransactionEffects::failure_at_with_context(
            "insufficient balance".to_string(),
            2,
            "SplitCoins (coin: Input(0), 1 amounts)".to_string(),
            2,
            ctx,
            snapshot,
        );

        assert!(!effects.success);
        assert_eq!(effects.failed_command_index, Some(2));
        assert_eq!(effects.commands_succeeded, 2);

        // Verify error_context is populated
        assert!(effects.error_context.is_some());
        let ctx = effects.error_context.unwrap();
        assert_eq!(ctx.command_index, 2);
        assert_eq!(ctx.command_type, "SplitCoins");
        assert_eq!(ctx.gas_consumed_before_failure, 1500);
        assert_eq!(ctx.prior_successful_commands, vec![0, 1]);

        // Verify state_at_failure is populated
        assert!(effects.state_at_failure.is_some());
        let snapshot = effects.state_at_failure.unwrap();
        assert_eq!(snapshot.total_gas_consumed, 1500);
    }

    #[test]
    fn test_transaction_effects_success_no_context() {
        // Verify that success() doesn't have error context
        let effects = TransactionEffects::success();

        assert!(effects.success);
        assert!(effects.error_context.is_none());
        assert!(effects.state_at_failure.is_none());
    }

    #[test]
    fn test_transaction_effects_failure_at_no_context() {
        // Verify that failure_at() (without context) doesn't have error context
        let effects =
            TransactionEffects::failure_at("some error".to_string(), 1, "MoveCall".to_string(), 1);

        assert!(!effects.success);
        assert_eq!(effects.failed_command_index, Some(1));
        assert!(effects.error_context.is_none()); // Old method doesn't populate context
        assert!(effects.state_at_failure.is_none());
    }

    #[test]
    fn test_command_error_context_coin_operation() {
        use crate::error_context::{CoinOperationContext, CommandErrorContext};

        // Test that coin operation context is properly constructed
        let coin_ctx = CoinOperationContext {
            coin_type: "0x2::sui::SUI".to_string(),
            source_balance: Some(100),
            requested_splits: Some(vec![300, 400]),
            destination_balance: None,
            source_balances: None,
        };

        let ctx = CommandErrorContext::new(0, "SplitCoins").with_coin_context(coin_ctx);

        assert!(ctx.coin_balances.is_some());
        let coin = ctx.coin_balances.unwrap();
        assert_eq!(coin.coin_type, "0x2::sui::SUI");
        assert_eq!(coin.source_balance, Some(100));
        assert_eq!(coin.requested_splits, Some(vec![300, 400]));
    }

    #[test]
    fn test_execution_snapshot_structure() {
        use crate::error_context::{CommandSummary, ExecutionSnapshot, ObjectSnapshot};

        let mut snapshot = ExecutionSnapshot::default();

        // Add an object
        snapshot.objects.push(ObjectSnapshot::new(
            "0x123",
            "0x2::coin::Coin<0x2::sui::SUI>",
            42,
            40,
            "address:0xabc",
        ));

        // Add a successful command
        snapshot.successful_commands.push(CommandSummary {
            index: 0,
            command_type: "SplitCoins".to_string(),
            description: "SplitCoins (coin: Input(0), 1 amounts)".to_string(),
            gas_consumed: 500,
            objects_created: vec!["0x456".to_string()],
            objects_mutated: vec!["0x123".to_string()],
        });

        snapshot.total_gas_consumed = 500;

        assert_eq!(snapshot.objects.len(), 1);
        assert_eq!(snapshot.successful_commands.len(), 1);
        assert_eq!(snapshot.successful_commands[0].index, 0);
        assert_eq!(snapshot.total_gas_consumed, 500);
    }
}
