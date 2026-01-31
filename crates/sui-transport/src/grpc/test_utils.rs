//! Test utilities for gRPC types.
//!
//! Provides builders and helpers for constructing test fixtures without
//! repetitive boilerplate.

use super::client::{GrpcArgument, GrpcCommand, GrpcInput, GrpcTransaction};

/// Builder for creating `GrpcTransaction` test fixtures.
///
/// # Example
///
/// ```ignore
/// let tx = GrpcTransactionBuilder::new()
///     .sender("0x1")
///     .with_move_call("0x2", "coin", "value")
///     .build();
/// assert!(tx.is_ptb());
/// ```
#[derive(Default)]
pub struct GrpcTransactionBuilder {
    digest: String,
    sender: String,
    gas_budget: Option<u64>,
    gas_price: Option<u64>,
    checkpoint: Option<u64>,
    timestamp_ms: Option<u64>,
    epoch: Option<u64>,
    inputs: Vec<GrpcInput>,
    commands: Vec<GrpcCommand>,
    status: Option<String>,
}

impl GrpcTransactionBuilder {
    /// Create a new builder with default test values.
    pub fn new() -> Self {
        Self {
            digest: "test_digest".to_string(),
            sender: "0x1".to_string(),
            gas_budget: Some(1000),
            gas_price: Some(1),
            ..Default::default()
        }
    }

    /// Create a builder for a minimal transaction (no defaults).
    pub fn minimal() -> Self {
        Self::default()
    }

    pub fn digest(mut self, d: &str) -> Self {
        self.digest = d.to_string();
        self
    }

    pub fn sender(mut self, s: &str) -> Self {
        self.sender = s.to_string();
        self
    }

    pub fn gas_budget(mut self, budget: Option<u64>) -> Self {
        self.gas_budget = budget;
        self
    }

    pub fn gas_price(mut self, price: Option<u64>) -> Self {
        self.gas_price = price;
        self
    }

    pub fn checkpoint(mut self, cp: u64) -> Self {
        self.checkpoint = Some(cp);
        self
    }

    pub fn epoch(mut self, e: u64) -> Self {
        self.epoch = Some(e);
        self
    }

    pub fn inputs(mut self, inputs: Vec<GrpcInput>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn commands(mut self, commands: Vec<GrpcCommand>) -> Self {
        self.commands = commands;
        self
    }

    /// Add a MoveCall command.
    pub fn with_move_call(mut self, package: &str, module: &str, function: &str) -> Self {
        self.commands.push(GrpcCommand::MoveCall {
            package: package.to_string(),
            module: module.to_string(),
            function: function.to_string(),
            type_arguments: vec![],
            arguments: vec![],
        });
        self
    }

    /// Add a MoveCall command with type arguments and arguments.
    pub fn with_move_call_full(
        mut self,
        package: &str,
        module: &str,
        function: &str,
        type_args: Vec<&str>,
        args: Vec<GrpcArgument>,
    ) -> Self {
        self.commands.push(GrpcCommand::MoveCall {
            package: package.to_string(),
            module: module.to_string(),
            function: function.to_string(),
            type_arguments: type_args.into_iter().map(String::from).collect(),
            arguments: args,
        });
        self
    }

    /// Add a SplitCoins command.
    pub fn with_split_coins(mut self, coin: GrpcArgument, amounts: Vec<GrpcArgument>) -> Self {
        self.commands
            .push(GrpcCommand::SplitCoins { coin, amounts });
        self
    }

    /// Add a TransferObjects command.
    pub fn with_transfer_objects(
        mut self,
        objects: Vec<GrpcArgument>,
        address: GrpcArgument,
    ) -> Self {
        self.commands
            .push(GrpcCommand::TransferObjects { objects, address });
        self
    }

    pub fn build(self) -> GrpcTransaction {
        GrpcTransaction {
            digest: self.digest,
            sender: self.sender,
            gas_budget: self.gas_budget,
            gas_price: self.gas_price,
            checkpoint: self.checkpoint,
            timestamp_ms: self.timestamp_ms,
            epoch: self.epoch,
            inputs: self.inputs,
            commands: self.commands,
            status: self.status,
            objects: vec![],
            execution_error: None,
            unchanged_loaded_runtime_objects: vec![],
            changed_objects: vec![],
            created_objects: vec![],
            unchanged_consensus_objects: vec![],
        }
    }
}

/// Test case for parameterized GrpcOwner tests.
pub struct OwnerTestCase {
    pub name: &'static str,
    pub kind: i32,
    pub address: Option<String>,
    pub version: Option<u64>,
    pub expected_variant: &'static str,
}

/// Test case for parameterized GrpcArgument tests.
pub struct ArgumentTestCase {
    pub name: &'static str,
    pub kind: Option<i32>,
    pub input: Option<u32>,
    pub result: Option<u32>,
    pub subresult: Option<u32>,
    pub expected_variant: &'static str,
}

/// Test case for parameterized GrpcInput tests.
pub struct InputTestCase {
    pub name: &'static str,
    pub kind: Option<i32>,
    pub object_id: Option<String>,
    pub version: Option<u64>,
    pub digest: Option<String>,
    pub pure: Option<Vec<u8>>,
    pub mutable: Option<bool>,
    pub expected_variant: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_defaults() {
        let tx = GrpcTransactionBuilder::new().build();
        assert_eq!(tx.sender, "0x1");
        assert_eq!(tx.gas_budget, Some(1000));
        assert!(tx.commands.is_empty());
    }

    #[test]
    fn test_builder_with_move_call() {
        let tx = GrpcTransactionBuilder::new()
            .with_move_call("0x2", "coin", "value")
            .build();
        assert_eq!(tx.commands.len(), 1);
        assert!(tx.is_ptb());
    }

    #[test]
    fn test_builder_minimal() {
        let tx = GrpcTransactionBuilder::minimal().build();
        assert!(tx.sender.is_empty());
        assert!(tx.gas_budget.is_none());
    }
}
