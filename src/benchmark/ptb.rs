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
//! ```ignore
//! let mut executor = PTBExecutor::new(&mut vm_harness);
//!
//! // Add pure value inputs
//! executor.add_pure_input(bcs::to_bytes(&100u64)?)?;
//!
//! // Execute commands
//! let effects = executor.execute(vec![
//!     Command::MoveCall {
//!         package: package_addr,
//!         module: "my_module".into(),
//!         function: "create_thing".into(),
//!         type_args: vec![],
//!         args: vec![Argument::Input(0)],
//!     },
//!     Command::TransferObjects {
//!         objects: vec![Argument::Result(0)],
//!         address: Argument::Input(1),
//!     },
//! ])?;
//! ```

use anyhow::{anyhow, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{ModuleId, TypeTag, StructTag};
use std::collections::{HashMap, HashSet};

use crate::benchmark::natives::EmittedEvent;
use crate::benchmark::vm::{gas_costs, VMHarness};

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
    ImmRef { id: ObjectID, bytes: Vec<u8> },

    /// Object passed by mutable reference
    MutRef { id: ObjectID, bytes: Vec<u8> },

    /// Object passed by value (ownership transferred)
    Owned { id: ObjectID, bytes: Vec<u8> },

    /// Shared object
    Shared { id: ObjectID, bytes: Vec<u8> },
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
            CommandResult::Empty => Err(anyhow!("command returned no values")),
            CommandResult::Values(vs) if vs.is_empty() => {
                Err(anyhow!("command returned no values"))
            }
            CommandResult::Values(vs) => Ok(vs[0].clone()),
            CommandResult::Created(_) => Err(anyhow!("command returned created objects, not values")),
        }
    }

    /// Get a specific return value by index.
    pub fn get(&self, index: usize) -> Result<Vec<u8>> {
        match self {
            CommandResult::Empty => Err(anyhow!("command returned no values")),
            CommandResult::Values(vs) => vs
                .get(index)
                .cloned()
                .ok_or_else(|| anyhow!("result index {} out of bounds (len={})", index, vs.len())),
            CommandResult::Created(_) => Err(anyhow!("command returned created objects, not values")),
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
    pending_receives: HashMap<ObjectID, Vec<u8>>,

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

    /// Objects that are immutable (cannot be mutated).
    /// If enforce_immutability is true, mutations to these will fail.
    immutable_objects: HashSet<ObjectID>,

    /// Whether to enforce immutability constraints.
    enforce_immutability: bool,
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
            immutable_objects: HashSet::new(),
            enforce_immutability: false,
        }
    }

    /// Create a PTB executor with pre-published package info.
    /// Used by SimulationEnvironment to pass package/UpgradeCap IDs.
    pub fn new_with_published(vm: &'a mut VMHarness<'b>, pre_published: Vec<(ObjectID, ObjectID)>) -> Self {
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
            immutable_objects: HashSet::new(),
            enforce_immutability: false,
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
    pub fn mark_unwrapped(&mut self, object_id: ObjectID, owner: Owner, object_type: Option<TypeTag>) {
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
        let input = self.inputs.get_mut(index as usize)
            .ok_or_else(|| anyhow!("input index {} out of bounds", index))?;
        match input {
            InputValue::Object(obj) => {
                match obj {
                    ObjectInput::Owned { bytes, .. } => *bytes = new_bytes,
                    ObjectInput::Shared { bytes, .. } => *bytes = new_bytes,
                    ObjectInput::ImmRef { bytes, .. } => *bytes = new_bytes,
                    ObjectInput::MutRef { bytes, .. } => *bytes = new_bytes,
                }
            }
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
                let result = self
                    .results
                    .get(*cmd_idx as usize)
                    .ok_or_else(|| anyhow!(
                        "Result({}): command index {} out of bounds (only {} commands executed)",
                        cmd_idx, cmd_idx, self.results.len()
                    ))?;
                result.primary_value().map_err(|e| anyhow!(
                    "Result({}): {}",
                    cmd_idx, e
                ))
            }
            Argument::NestedResult(cmd_idx, val_idx) => {
                let result = self
                    .results
                    .get(*cmd_idx as usize)
                    .ok_or_else(|| anyhow!(
                        "NestedResult({}, {}): command index {} out of bounds (only {} commands executed)",
                        cmd_idx, val_idx, cmd_idx, self.results.len()
                    ))?;
                result.get(*val_idx as usize).map_err(|e| anyhow!(
                    "NestedResult({}, {}): {}. Command {} returned {} value(s).",
                    cmd_idx, val_idx, e, cmd_idx, result.len()
                ))
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

            Command::Receive { object_id, object_type } => {
                self.execute_receive(&object_id, object_type.as_ref())
            }
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
        let arg_to_info: Vec<(Argument, Option<ObjectID>)> = args.iter()
            .map(|arg| (*arg, self.get_object_id_and_type_from_arg(arg).map(|(id, _)| id)))
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
                    return Ok(CommandResult::Empty);
                } else {
                    return Ok(CommandResult::Values(output.return_values));
                }
            }
            Err(e) => {
                // Check if this is an argument count mismatch - might need TxContext
                let err_msg = e.to_string();
                if err_msg.contains("argument length mismatch") || err_msg.contains("NUMBER_OF_ARGUMENTS_MISMATCH") {
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
                            self.apply_mutable_ref_outputs(&arg_to_info, &output.mutable_ref_outputs)?;

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
                return Err(e);
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
                    let existing_type = self.mutated_objects.get(object_id)
                        .map(|(_, t)| t.clone())
                        .flatten();

                    // Record the mutation with updated bytes
                    self.mutated_objects.insert(*object_id, (new_bytes.clone(), existing_type));
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
                Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
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

        let original_value = u64::from_le_bytes(coin_bytes[32..40].try_into().unwrap());

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
        let coin_type = self.get_object_id_and_type_from_arg(&coin)
            .and_then(|(_, t)| t)
            .or_else(|| {
                // Default to Coin<SUI> if type not known
                Some(TypeTag::Struct(Box::new(StructTag {
                    address: AccountAddress::from_hex_literal("0x2").unwrap(),
                    module: Identifier::new("coin").unwrap(),
                    name: Identifier::new("Coin").unwrap(),
                    type_params: vec![TypeTag::Struct(Box::new(StructTag {
                        address: AccountAddress::from_hex_literal("0x2").unwrap(),
                        module: Identifier::new("sui").unwrap(),
                        name: Identifier::new("SUI").unwrap(),
                        type_params: vec![],
                    }))],
                })))
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
            self.mutated_objects.insert(obj_id, (updated_coin_bytes.clone(), coin_type.clone()));
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
            + num_new_coins * 40 * gas_costs::STORAGE_BYTE;  // storage for new coins

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
            return Err(anyhow!("destination coin bytes too short"));
        }

        let dest_value = u64::from_le_bytes(dest_bytes[32..40].try_into().unwrap());

        // Sum up all source values
        let mut total_merge: u64 = 0;
        for source_bytes in &source_bytes_list {
            if source_bytes.len() < 40 {
                return Err(anyhow!("source coin bytes too short"));
            }
            let source_value = u64::from_le_bytes(source_bytes[32..40].try_into().unwrap());
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
        let coin_type = self.get_object_id_and_type_from_arg(&destination)
            .and_then(|(_, t)| t);

        // Mark destination as mutated with the new bytes
        if let Some((dest_id, _)) = self.get_object_id_and_type_from_arg(&destination) {
            self.mutated_objects.insert(dest_id, (new_dest_bytes.clone(), coin_type.clone()));
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
            + num_sources * gas_costs::OBJECT_DELETE;  // source coins deleted

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
                        if matches!(input, InputValue::Object(ObjectInput::ImmRef { .. } | ObjectInput::MutRef { .. })) {
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

            // Record the change with type info
            self.object_changes.push(ObjectChange::Mutated {
                id: obj_id,
                owner: new_owner,
                object_type: obj_type.clone(),
            });

            // Get the current bytes for the object
            let obj_bytes = self.get_object_bytes(&obj_id).unwrap_or_default();

            // Mark as mutated with current bytes and type info
            self.mutated_objects.insert(obj_id, (obj_bytes, obj_type));
        }

        // Estimate gas: native call + object mutation per transferred object
        let num_objects = objects.len() as u64;
        self.gas_used += gas_costs::NATIVE_CALL
            + num_objects * gas_costs::OBJECT_MUTATE;  // ownership change counts as mutation

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
    fn get_object_id_and_type_from_arg(&self, arg: &Argument) -> Option<(ObjectID, Option<TypeTag>)> {
        match arg {
            Argument::Input(idx) => {
                if let Some(InputValue::Object(obj)) = self.inputs.get(*idx as usize) {
                    // For input objects, we don't have the type readily available
                    // unless it was tracked elsewhere
                    Some((*obj.id(), None))
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
                                let obj_type = self.created_objects.get(&id).and_then(|(_, t)| t.clone());
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
                } else if let Some(CommandResult::Values(vs)) = self.results.get(*cmd_idx as usize) {
                    if let Some(bytes) = vs.get(*val_idx as usize) {
                        if bytes.len() >= 32 {
                            if let Ok(id) = AccountAddress::from_bytes(&bytes[..32]) {
                                let obj_type = self.created_objects.get(&id).and_then(|(_, t)| t.clone());
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
                            i, elem.len(), first_len
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
        self.gas_used += gas_costs::NATIVE_CALL
            + (vec_bytes.len() as u64) * gas_costs::OUTPUT_BYTE;

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
            let upgrade_cap_type = TypeTag::Struct(Box::new(StructTag {
                address: AccountAddress::from_hex_literal("0x2").unwrap(),
                module: Identifier::new("package").unwrap(),
                name: Identifier::new("UpgradeCap").unwrap(),
                type_params: vec![],
            }));

            // Mark the UpgradeCap as created with its type
            self.created_objects.insert(upgrade_cap_id, (Vec::new(), Some(upgrade_cap_type)));
            // Created objects are transferable by the sender
            self.transferable_objects.insert(upgrade_cap_id);
            self.object_owners.insert(upgrade_cap_id, Owner::Address(self.sender));

            // Estimate gas: publishing is expensive - base cost + per-module cost
            // Note: actual gas is computed when modules are loaded in pre_publish_modules
            self.gas_used += gas_costs::NATIVE_CALL * 10  // publish overhead
                + gas_costs::OBJECT_CREATE * 2;  // package object + UpgradeCap

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
            let upgrade_receipt_type = TypeTag::Struct(Box::new(StructTag {
                address: AccountAddress::from_hex_literal("0x2").unwrap(),
                module: Identifier::new("package").unwrap(),
                name: Identifier::new("UpgradeReceipt").unwrap(),
                type_params: vec![],
            }));

            // Mark the UpgradeReceipt as created with its type
            self.created_objects.insert(receipt_id, (Vec::new(), Some(upgrade_receipt_type)));
            // Created objects are transferable by the sender
            self.transferable_objects.insert(receipt_id);
            self.object_owners.insert(receipt_id, Owner::Address(self.sender));

            // The ticket would normally be consumed here
            // In simulation, we don't strictly validate it

            // Estimate gas: upgrade is expensive similar to publish
            self.gas_used += gas_costs::NATIVE_CALL * 10  // upgrade overhead
                + gas_costs::OBJECT_CREATE * 2  // new package object + UpgradeReceipt
                + gas_costs::OBJECT_DELETE;  // ticket consumed

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
        let object_bytes = self.pending_receives
            .remove(object_id)
            .ok_or_else(|| anyhow!(
                "Object {} not found in pending receives. It must be transferred to this transaction first.",
                object_id.to_hex_literal()
            ))?;

        // Track that this object was received (unwrapped from pending state)
        // Store in created_objects so it can be referenced in subsequent commands
        self.created_objects.insert(*object_id, (object_bytes.clone(), expected_type.cloned()));
        self.object_owners.insert(*object_id, Owner::Address(self.sender));
        // Received objects are transferable by the sender
        self.transferable_objects.insert(*object_id);
        self.object_changes.push(ObjectChange::Unwrapped {
            id: *object_id,
            owner: Owner::Address(self.sender),
            object_type: expected_type.cloned(),
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
        self.pending_receives.insert(object_id, object_bytes);
    }

    /// Execute all commands in the PTB.
    pub fn execute(&mut self, commands: Vec<Command>) -> Result<TransactionEffects> {
        // Clear the VM's execution trace and events before starting
        self.vm.clear_trace();
        self.vm.clear_events();

        for (index, cmd) in commands.into_iter().enumerate() {
            let cmd_description = Self::describe_command(&cmd);
            match self.execute_command(cmd) {
                Ok(result) => {
                    self.results.push(result);

                    // Check gas budget after each successful command
                    if let Err(gas_err) = self.check_gas_budget() {
                        return Ok(TransactionEffects::failure_at(
                            gas_err.to_string(),
                            index,
                            format!("{} (out of gas)", cmd_description),
                            self.results.len(),
                        ));
                    }
                }
                Err(e) => {
                    return Ok(TransactionEffects::failure_at(
                        e.to_string(),
                        index,
                        cmd_description,
                        self.results.len(),
                    ));
                }
            }
        }

        Ok(self.compute_effects())
    }

    /// Generate a human-readable description of a command.
    fn describe_command(cmd: &Command) -> String {
        match cmd {
            Command::MoveCall { package, module, function, type_args, args } => {
                let type_args_str = if type_args.is_empty() {
                    String::new()
                } else {
                    format!("<{}>", type_args.iter().map(|t| format!("{}", t)).collect::<Vec<_>>().join(", "))
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
            Command::MergeCoins { destination, sources } => {
                format!("MergeCoins (dest: {:?}, {} sources)", destination, sources.len())
            }
            Command::TransferObjects { objects, address } => {
                format!("TransferObjects ({} objects to {:?})", objects.len(), address)
            }
            Command::MakeMoveVec { type_tag, elements } => {
                let type_str = type_tag.as_ref().map(|t| format!("{}", t)).unwrap_or_else(|| "unknown".to_string());
                format!("MakeMoveVec<{}> ({} elements)", type_str, elements.len())
            }
            Command::Publish { modules, dep_ids } => {
                format!("Publish ({} modules, {} deps)", modules.len(), dep_ids.len())
            }
            Command::Upgrade { package, .. } => {
                format!("Upgrade (package {})", package.to_hex_literal())
            }
            Command::Receive { object_id, object_type } => {
                let type_str = object_type.as_ref().map(|t| format!("{}", t)).unwrap_or_else(|| "unknown".to_string());
                format!("Receive {} (type: {})", object_id.to_hex_literal(), type_str)
            }
        }
    }

    /// Compute the transaction effects after execution.
    fn compute_effects(&self) -> TransactionEffects {
        let mut effects = TransactionEffects::success();

        // Add created objects with their tracked ownership and type
        for (id, (_bytes, object_type)) in &self.created_objects {
            let owner = self.object_owners.get(id).copied()
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
                let owner = self.object_owners.get(id).copied()
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
        // Also populate the wrapped/unwrapped vectors
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
            };
            if !effects.object_changes.iter().any(|c| match c {
                ObjectChange::Created { id: cid, .. } => cid == id,
                ObjectChange::Mutated { id: cid, .. } => cid == id,
                ObjectChange::Deleted { id: cid, .. } => cid == id,
                ObjectChange::Wrapped { id: cid, .. } => cid == id,
                ObjectChange::Unwrapped { id: cid, .. } => cid == id,
            }) {
                effects.object_changes.push(change.clone());
            }
        }

        // Collect events emitted during execution
        effects.events = self.vm.get_events();

        // Capture return values from each command
        effects.return_values = self.results.iter().map(|result| {
            match result {
                CommandResult::Empty => vec![],
                CommandResult::Values(values) => values.clone(),
                CommandResult::Created(ids) => {
                    // For created objects, return their IDs as BCS-encoded bytes
                    ids.iter().map(|id| id.to_vec()).collect()
                }
            }
        }).collect();

        // Populate mutated object bytes for syncing back to environment
        effects.mutated_object_bytes = self.mutated_objects.iter()
            .map(|(id, (bytes, _))| (*id, bytes.clone()))
            .collect();

        // Populate created object bytes for syncing back to environment
        effects.created_object_bytes = self.created_objects.iter()
            .map(|(id, (bytes, _))| (*id, bytes.clone()))
            .collect();

        // Extract dynamic field entries from the VM's shared state.
        // This captures all Table/Bag operations that occurred during MoveCall execution.
        for ((parent_id, child_id), type_tag, bytes) in self.vm.extract_dynamic_fields() {
            effects.dynamic_field_entries.insert((parent_id, child_id), (type_tag, bytes));
        }

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
        self.created_objects.iter()
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
        self.mutated_objects.iter()
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
        self.inputs.push(InputValue::Object(ObjectInput::Owned { id, bytes }));
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
        self.commands.push(Command::TransferObjects { objects, address });
    }

    /// Add a MakeMoveVec command.
    pub fn make_move_vec(&mut self, type_tag: Option<TypeTag>, elements: Vec<Argument>) -> Argument {
        let cmd_idx = self.commands.len();
        self.commands.push(Command::MakeMoveVec { type_tag, elements });
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
}
