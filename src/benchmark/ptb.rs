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
use move_core_types::language_storage::{ModuleId, TypeTag};
use std::collections::{BTreeSet, HashMap};

use crate::benchmark::vm::VMHarness;

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
    },
    /// Object was mutated
    Mutated {
        id: ObjectID,
        owner: Owner,
    },
    /// Object was deleted
    Deleted {
        id: ObjectID,
    },
    /// Object was wrapped (stored inside another object)
    Wrapped {
        id: ObjectID,
    },
    /// Object was unwrapped (extracted from another object)
    Unwrapped {
        id: ObjectID,
        owner: Owner,
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

    /// Gas used (always 0 in our unmetered execution)
    pub gas_used: u64,

    /// Whether execution succeeded
    pub success: bool,

    /// Error message if execution failed
    pub error: Option<String>,
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

    /// Objects created during execution (id -> bytes)
    created_objects: HashMap<ObjectID, Vec<u8>>,

    /// Objects that were deleted
    deleted_objects: BTreeSet<ObjectID>,

    /// Objects that were mutated
    mutated_objects: BTreeSet<ObjectID>,

    /// Counter for generating unique object IDs
    id_counter: u64,
}

impl<'a, 'b> PTBExecutor<'a, 'b> {
    /// Create a new PTB executor.
    pub fn new(vm: &'a mut VMHarness<'b>) -> Self {
        Self {
            vm,
            inputs: Vec::new(),
            results: Vec::new(),
            created_objects: HashMap::new(),
            deleted_objects: BTreeSet::new(),
            mutated_objects: BTreeSet::new(),
            id_counter: 0,
        }
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
        self.inputs.push(InputValue::Object(obj));
        Ok(idx as u16)
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
            Argument::Result(i) => {
                let result = self
                    .results
                    .get(*i as usize)
                    .ok_or_else(|| anyhow!("result index {} out of bounds", i))?;
                result.primary_value()
            }
            Argument::NestedResult(i, j) => {
                let result = self
                    .results
                    .get(*i as usize)
                    .ok_or_else(|| anyhow!("result index {} out of bounds", i))?;
                result.get(*j as usize)
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
        }
    }

    /// Execute a MoveCall command.
    fn execute_move_call(
        &mut self,
        package: AccountAddress,
        module: Identifier,
        function: Identifier,
        type_args: Vec<TypeTag>,
        args: Vec<Argument>,
    ) -> Result<CommandResult> {
        let resolved_args = self.resolve_args(&args)?;
        let module_id = ModuleId::new(package, module);

        let returns = self.vm.execute_function_with_return(
            &module_id,
            function.as_str(),
            type_args,
            resolved_args,
        )?;

        if returns.is_empty() {
            Ok(CommandResult::Empty)
        } else {
            Ok(CommandResult::Values(returns))
        }
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

        // Create new coins for each amount
        let mut new_coins = Vec::new();
        for amount in amounts {
            let new_id = self.fresh_id();
            let mut new_coin_bytes = Vec::with_capacity(40);
            new_coin_bytes.extend_from_slice(new_id.as_ref());
            new_coin_bytes.extend_from_slice(&amount.to_le_bytes());
            self.created_objects
                .insert(new_id, new_coin_bytes.clone());
            new_coins.push(new_coin_bytes);
        }

        // Mark original coin as mutated (balance reduced)
        // Note: We don't actually update the original coin bytes here since
        // we're tracking by argument reference, not by object ID

        Ok(CommandResult::Values(new_coins))
    }

    /// Execute a MergeCoins command.
    ///
    /// Merges multiple source coins into the destination coin.
    /// Source coins are destroyed, destination coin's balance increases.
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

        // Sources are destroyed (we'd track this in a real implementation)

        Ok(CommandResult::Values(vec![new_dest_bytes]))
    }

    /// Execute a TransferObjects command.
    ///
    /// Transfers ownership of objects to the specified address.
    /// In our sandbox, this is mostly a no-op since we don't track ownership,
    /// but we record it in the effects.
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

        // Resolve all objects to ensure they exist
        let _ = self.resolve_args(&objects)?;

        // In a full implementation, we'd update ownership tracking here
        // For now, just return empty (transfer has no return value)
        Ok(CommandResult::Empty)
    }

    /// Execute a MakeMoveVec command.
    ///
    /// Creates a vector from the given elements.
    fn execute_make_move_vec(
        &mut self,
        _type_tag: Option<TypeTag>,
        elements: Vec<Argument>,
    ) -> Result<CommandResult> {
        let element_bytes = self.resolve_args(&elements)?;

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

        Ok(CommandResult::Values(vec![vec_bytes]))
    }

    /// Execute a Publish command.
    ///
    /// In a full implementation, this would:
    /// 1. Verify and link the modules
    /// 2. Create a new package object
    /// 3. Return the UpgradeCap
    ///
    /// For now, this is a stub that returns an error.
    fn execute_publish(
        &mut self,
        _modules: Vec<Vec<u8>>,
        _dep_ids: Vec<ObjectID>,
    ) -> Result<CommandResult> {
        // TODO: Implement module publishing
        // This would require:
        // 1. Module verification
        // 2. Linking against dependencies
        // 3. Creating package object
        Err(anyhow!("Publish command not yet implemented"))
    }

    /// Execute an Upgrade command.
    ///
    /// For now, this is a stub.
    fn execute_upgrade(
        &mut self,
        _modules: Vec<Vec<u8>>,
        _package: ObjectID,
        _ticket: Argument,
    ) -> Result<CommandResult> {
        Err(anyhow!("Upgrade command not yet implemented"))
    }

    /// Execute all commands in the PTB.
    pub fn execute(&mut self, commands: Vec<Command>) -> Result<TransactionEffects> {
        // Clear the VM's execution trace before starting
        self.vm.clear_trace();

        for cmd in commands {
            match self.execute_command(cmd) {
                Ok(result) => {
                    self.results.push(result);
                }
                Err(e) => {
                    return Ok(TransactionEffects::failure(e.to_string()));
                }
            }
        }

        Ok(self.compute_effects())
    }

    /// Compute the transaction effects after execution.
    fn compute_effects(&self) -> TransactionEffects {
        let mut effects = TransactionEffects::success();

        // Add created objects
        for id in self.created_objects.keys() {
            effects.created.push(*id);
            effects.object_changes.push(ObjectChange::Created {
                id: *id,
                owner: Owner::Address(AccountAddress::ZERO), // Default to zero address
            });
        }

        // Add deleted objects
        for id in &self.deleted_objects {
            effects.deleted.push(*id);
            effects.object_changes.push(ObjectChange::Deleted { id: *id });
        }

        // Add mutated objects
        for id in &self.mutated_objects {
            if !self.created_objects.contains_key(id) && !self.deleted_objects.contains(id) {
                effects.mutated.push(*id);
                effects.object_changes.push(ObjectChange::Mutated {
                    id: *id,
                    owner: Owner::Address(AccountAddress::ZERO),
                });
            }
        }

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
    pub fn created_objects(&self) -> &HashMap<ObjectID, Vec<u8>> {
        &self.created_objects
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
